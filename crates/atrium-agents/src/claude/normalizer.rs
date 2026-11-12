//! `ClaudeNormalizer`: Claude Code stream-json + control protocol → [`AgentEvent`].
//!
//! Pure: no I/O, no clock. Everything it knows comes through
//! [`AdapterInput`], so live sessions and fixture replays produce the same
//! events. Lifecycle decisions (see `docs/spikes/M0-01-claude-adapter.md`):
//!
//! - `SessionStarted` is held until the first real model response, so a bad
//!   model id is a pre-session failure (`Error { fatal }` then `SessionEnded`,
//!   diagnostics in `Error::details`, no `Unmapped` before `SessionStarted`);
//! - the turn's user message follows `TurnStarted`;
//! - `AskUserQuestion` is a virtual call: only `QuestionAsked`;
//! - approvals and questions carry the gate's ticket ids, never Claude's own.

use std::collections::{HashMap, HashSet, VecDeque};

use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageBody, MessageKind, PlanEntry, PlanStatus, Provider,
    Question, QuestionOption, QuotaWindow, RequestOutcome, SessionCommand, ToolKind, ToolStatus,
    TurnOutcome, Usage,
};
use atrium_protocol::limits::{
    MAX_DIAGNOSTIC_LINE_BYTES, MAX_DIAGNOSTIC_LINES, MAX_INLINE_MESSAGE_BYTES,
    MAX_PARTIAL_ARGS_BYTES, MAX_PROVIDER_RECORD_BYTES, MAX_TOOL_OUTPUT_BYTES,
    MAX_UNMAPPED_RAW_BYTES,
};
use atrium_protocol::policy::{EnforcementDescriptor, EnforcementLevel};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::tools::{gate_key, tool_kind, tool_title};
use crate::{AdapterInput, ApprovalTicket, GateDecision, MAX_QUEUED_MESSAGES, Normalizer};

/// The synthetic model name Claude Code puts on records it writes itself
/// (API errors, "no response" notices).
const SYNTHETIC_MODEL: &str = "<synthetic>";
const ASK_USER_QUESTION: &str = "AskUserQuestion";
const TODO_WRITE: &str = "TodoWrite";

/// `AdapterInput::Deadline::what` values the normalizer understands.
pub const DEADLINE_RECORD_TOO_LARGE: &str = "record_too_large";
pub const DEADLINE_INTERRUPT: &str = "interrupt";

#[derive(Debug)]
struct OpenCall {
    id: String,
}

/// Tool calls that are not shown as tool calls: the question dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Virtual {
    Question,
}

#[derive(Debug)]
struct CanUse {
    question: bool,
    /// Matches a gate decision: the `tool_use_id`, or the tool name and input
    /// when the request carries no id.
    key: String,
    tool_use_id: Option<String>,
    name: String,
    input: Value,
    title: Option<String>,
    reason: Option<String>,
    emitted: bool,
}

#[derive(Debug)]
enum Pending {
    Approval {
        call_id: Option<String>,
        native: String,
    },
    Question {
        native: String,
    },
}

impl Pending {
    fn native(&self) -> &str {
        match self {
            Pending::Approval { native, .. } | Pending::Question { native } => native,
        }
    }
}

#[derive(Debug, Clone, Default)]
struct MsgUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
}

/// Session-cumulative totals from `result.modelUsage` (every model, subagents
/// included).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Totals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    cost: f64,
}

#[derive(Debug)]
struct InitInfo {
    session_id: String,
    model: Option<String>,
    tools: Vec<String>,
}

#[derive(Debug)]
struct OpenMsg {
    id: String,
    kind: MessageKind,
    text: String,
    deltas: u64,
}

#[derive(Debug, Default)]
pub struct ClaudeNormalizer {
    init: Option<InitInfo>,
    established: bool,
    ended: bool,
    turn: u32,
    turn_open: bool,
    turn_start_emitted: bool,
    ever_had_turn: bool,
    interrupt_requested: bool,
    shutdown_requested: bool,
    error_emitted: bool,
    /// The failure of a session that never got established: reported with the
    /// stderr tail when the process exits.
    pre_error: Option<String>,
    pre_session_failed: bool,
    /// Why the adapter ended the session itself (a deadline).
    end_reason: Option<String>,

    /// The user message of the open turn, until it has been emitted.
    user_text: Option<String>,

    enforcement: Option<EnforcementDescriptor>,

    block_counts: HashMap<String, usize>,
    open_msgs: Vec<OpenMsg>,

    calls: Vec<OpenCall>,
    seen_calls: HashSet<String>,
    done_calls: HashSet<String>,
    virtual_calls: HashMap<String, Virtual>,
    denied_early: HashMap<String, String>,

    /// Open approvals and questions, by ticket id.
    pending: Vec<(String, Pending)>,
    /// `can_use_tool` requests, by Claude's own request id.
    can_use: HashMap<String, CanUse>,
    /// Gate keys answered `Ask` (with the ticket) / `Allow` (see [`gate_key`]).
    asked: HashMap<String, ApprovalTicket>,
    allowed: HashSet<String>,

    /// User messages accepted and not yet written to the provider.
    waiting: usize,
    rejected: bool,

    msg_usage: HashMap<String, MsgUsage>,
    /// Context size at the last main-thread API call of the turn.
    last_context: Option<u64>,
    prev_totals: Totals,
    last_quota: Option<Vec<QuotaWindow>>,
    last_reset_ms: Option<u64>,
    rate_limited: bool,
    /// A `rejected` rate-limit record was seen; the refusal itself is emitted
    /// with the message that follows, or at the end of the turn.
    limit_pending: Option<Option<u64>>,
    /// Events produced before `SessionStarted`, released right after it (or
    /// turned into diagnostics when the session never starts).
    held: Vec<AgentEvent>,

    stderr_tail: VecDeque<String>,
}

impl ClaudeNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// A user message was written and the turn has not produced its `result`
    /// yet. The adapter uses this to decide whether to queue.
    pub fn turn_in_flight(&self) -> bool {
        self.turn_open
    }

    /// The current turn number (0 before the first). The adapter stamps it on
    /// every `ToolCheck`.
    pub fn turn_number(&self) -> u32 {
        self.turn
    }

    /// The first turn failed before the session was established (bad model,
    /// auth error...). The process is not going to be useful.
    pub fn pre_session_failed(&self) -> bool {
        self.pre_session_failed
    }

    /// The last `SendUserMessage` was refused because too many were waiting
    /// (the normalizer emitted the `Error`). Reading clears the flag.
    pub fn take_rejected(&mut self) -> bool {
        std::mem::take(&mut self.rejected)
    }

    pub fn native_session_id(&self) -> Option<&str> {
        self.init.as_ref().map(|i| i.session_id.as_str())
    }

    /// The adapter must end the session now (a fatal condition was fed).
    pub fn wants_end(&self) -> bool {
        self.end_reason.is_some() && !self.ended
    }

    /// `SessionEnded` was emitted.
    pub fn has_ended(&self) -> bool {
        self.ended
    }

    pub fn is_established(&self) -> bool {
        self.established
    }

    // ---- inputs -------------------------------------------------------

    fn stdin(&mut self, line: &str, out: &mut Vec<AgentEvent>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if v.get("type").and_then(Value::as_str) == Some("user") {
            self.waiting = self.waiting.saturating_sub(1);
            self.user_text = Some(user_text_of(&v));
            self.begin_turn(out);
        }
    }

    fn begin_turn(&mut self, out: &mut Vec<AgentEvent>) {
        self.turn += 1;
        self.turn_open = true;
        self.ever_had_turn = true;
        self.turn_start_emitted = false;
        self.rate_limited = false;
        self.limit_pending = None;
        self.error_emitted = false;
        self.msg_usage.clear();
        self.last_context = None;
        self.interrupt_requested = false;
        if self.established {
            self.start_turn_events(out);
        }
    }

    /// `TurnStarted`, then the turn's user message.
    fn start_turn_events(&mut self, out: &mut Vec<AgentEvent>) {
        self.turn_start_emitted = true;
        out.push(AgentEvent::TurnStarted { turn: self.turn });
        if let Some(text) = self.user_text.take()
            && !text.is_empty()
        {
            out.extend(whole_message(
                format!("user:{}", self.turn),
                MessageKind::User,
                text,
            ));
        }
    }

    /// Emits `SessionStarted` (and the held `TurnStarted`) once the provider
    /// has answered for real.
    fn establish(&mut self, out: &mut Vec<AgentEvent>) {
        if self.established {
            return;
        }
        let Some(init) = &self.init else {
            return;
        };
        self.established = true;
        let enforcement = self
            .enforcement
            .clone()
            .unwrap_or_else(unknown_enforcement);
        out.push(AgentEvent::SessionStarted {
            provider: Provider::Claude,
            native_session_id: init.session_id.clone(),
            model: init.model.clone(),
            tools: init.tools.clone(),
            enforcement,
        });
        if self.turn_open && !self.turn_start_emitted {
            self.start_turn_events(out);
        }
        out.append(&mut self.held);
    }

    /// A record arrived outside any turn the client started (a background
    /// agent finishing, for example): open a turn so messages stay inside one.
    fn ensure_turn(&mut self, out: &mut Vec<AgentEvent>) {
        if !self.turn_open && self.established {
            self.turn += 1;
            self.turn_open = true;
            self.turn_start_emitted = true;
            self.rate_limited = false;
            self.error_emitted = false;
            self.msg_usage.clear();
            self.last_context = None;
            out.push(AgentEvent::TurnStarted { turn: self.turn });
        }
    }

    fn stdout(&mut self, line: &str, out: &mut Vec<AgentEvent>) {
        if line.len() > MAX_PROVIDER_RECORD_BYTES {
            self.fatal(
                format!("a provider record exceeds {MAX_PROVIDER_RECORD_BYTES} bytes"),
                out,
            );
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            out.push(AgentEvent::Unmapped {
                record_type: "unparseable".into(),
                raw: capped_raw(Value::String(line.to_owned())),
            });
            self.hold_unmapped(0, out);
            return;
        };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        let before = out.len();
        self.dispatch(ty, &v, out);
        self.hold_unmapped(before, out);
    }

    /// Before `SessionStarted` nothing but the session's own events may be
    /// emitted: notes about records we don't map wait for it (or become
    /// diagnostics of a failure).
    fn hold_unmapped(&mut self, from: usize, out: &mut Vec<AgentEvent>) {
        if self.established {
            return;
        }
        let mut i = from;
        while i < out.len() {
            if matches!(out[i], AgentEvent::Unmapped { .. }) {
                self.held.push(out.remove(i));
            } else {
                i += 1;
            }
        }
    }

    fn dispatch(&mut self, ty: &str, v: &Value, out: &mut Vec<AgentEvent>) {
        match ty {
            "system" => self.system(v, out),
            "stream_event" => self.stream_event(v, out),
            "assistant" => self.assistant(v, out),
            "user" => self.user(v, out),
            "result" => self.result(v, out),
            "rate_limit_event" => self.rate_limit(v, out),
            "control_request" => self.control_request(v, out),
            "control_cancel_request" => self.control_cancel(v, out),
            // Replies to our own control requests (initialize, interrupt).
            "control_response" | "keep_alive" => {}
            other => out.push(unmapped(other, v)),
        }
    }

    fn system(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let sub = v.get("subtype").and_then(Value::as_str).unwrap_or("");
        match sub {
            "init" => {
                if self.established {
                    return;
                }
                self.init = Some(InitInfo {
                    session_id: str_of(v, "session_id").unwrap_or_default(),
                    model: str_of(v, "model"),
                    tools: v
                        .get("tools")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| t.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
            // Progress noise with no Atrium equivalent. Everything else under
            // `system` stays visible as Unmapped.
            "status" | "thinking_tokens" | "post_turn_summary" | "task_summary" => {}
            // Subagent progress: the parent call's title follows the subagent.
            "task_progress" => {
                let call = str_of(v, "tool_use_id");
                let title = str_of(v, "description");
                if let (Some(call_id), Some(title)) = (call, title)
                    && self.calls.iter().any(|c| c.id == call_id)
                {
                    out.push(AgentEvent::ToolCallUpdated {
                        call_id,
                        title: Some(title),
                        kind: None,
                        input: None,
                        output_delta: None,
                        output: None,
                    });
                }
            }
            other => out.push(unmapped(&format!("system/{other}"), v)),
        }
    }

    fn stream_event(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let Some(e) = v.get("event") else {
            out.push(unmapped("stream_event", v));
            return;
        };
        let ety = e.get("type").and_then(Value::as_str).unwrap_or("");
        let api_id = str_of(v, "api_message_id").unwrap_or_default();
        match ety {
            "message_start" => {
                let model = e.pointer("/message/model").and_then(Value::as_str);
                if model != Some(SYNTHETIC_MODEL) {
                    self.establish(out);
                    self.ensure_turn(out);
                }
            }
            "content_block_delta" => {
                let index = e.get("index").and_then(Value::as_u64).unwrap_or(0);
                let d = e.get("delta").cloned().unwrap_or(Value::Null);
                let (kind, text) = match d.get("type").and_then(Value::as_str) {
                    Some("text_delta") => (MessageKind::Assistant, str_of(&d, "text")),
                    Some("thinking_delta") => (MessageKind::Reasoning, str_of(&d, "thinking")),
                    // Structural: tool arguments are taken whole from the
                    // assistant record; signatures and citations have no event.
                    _ => return,
                };
                let Some(text) = text.filter(|t| !t.is_empty()) else {
                    return;
                };
                let id = format!("{api_id}:{index}");
                match self.open_msgs.iter_mut().find(|m| m.id == id) {
                    Some(m) => {
                        m.text.push_str(&text);
                        m.deltas += 1;
                    }
                    None => self.open_msgs.push(OpenMsg {
                        id: id.clone(),
                        kind,
                        text: text.clone(),
                        deltas: 1,
                    }),
                }
                out.push(AgentEvent::MessageDelta {
                    message_id: id,
                    kind,
                    text,
                });
            }
            "message_delta" => {
                if let Some(u) = e.get("usage") {
                    self.ensure_turn(out);
                    let mu = msg_usage(u);
                    if v.get("parent_tool_use_id").is_none_or(Value::is_null) {
                        self.last_context = Some(mu.input + mu.cache_read + mu.cache_write + mu.output);
                    }
                    self.msg_usage.insert(api_id, mu);
                    out.push(AgentEvent::Usage {
                        usage: self.turn_usage(),
                    });
                }
            }
            "content_block_start" | "content_block_stop" | "message_stop" => {}
            other => out.push(unmapped(&format!("stream_event/{other}"), v)),
        }
    }

    fn turn_usage(&self) -> Usage {
        let mut total = MsgUsage::default();
        for u in self.msg_usage.values() {
            total.input += u.input;
            total.output += u.output;
            total.cache_read += u.cache_read;
            total.cache_write += u.cache_write;
            total.reasoning += u.reasoning;
        }
        Usage {
            input_tokens: Some(total.input),
            output_tokens: Some(total.output),
            cache_read_tokens: Some(total.cache_read),
            cache_write_tokens: Some(total.cache_write),
            reasoning_tokens: Some(total.reasoning),
            cost_usd: None,
        }
    }

    fn assistant(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let Some(msg) = v.get("message") else {
            out.push(unmapped("assistant", v));
            return;
        };
        if msg.get("model").and_then(Value::as_str) == Some(SYNTHETIC_MODEL) {
            self.synthetic(v, out);
            return;
        }
        self.establish(out);
        self.ensure_turn(out);
        let mid = str_of(msg, "id").unwrap_or_default();
        let blocks = msg
            .get("content")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for block in blocks {
            let n = self.block_counts.entry(mid.clone()).or_insert(0);
            let id = format!("{mid}:{n}");
            *n += 1;
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = str_of(&block, "text").unwrap_or_default();
                    self.complete_message(id, MessageKind::Assistant, text, out);
                }
                Some("thinking") => {
                    let text = str_of(&block, "thinking").unwrap_or_default();
                    self.complete_message(id, MessageKind::Reasoning, text, out);
                }
                Some("tool_use") => self.tool_use(&block, out),
                // Opaque (encrypted) reasoning: nothing to show.
                Some("redacted_thinking") => {}
                Some(other) => out.push(unmapped(&format!("assistant/{other}"), &block)),
                None => out.push(unmapped("assistant/block", &block)),
            }
        }
    }

    fn complete_message(
        &mut self,
        id: String,
        kind: MessageKind,
        record_text: String,
        out: &mut Vec<AgentEvent>,
    ) {
        let streamed = self
            .open_msgs
            .iter()
            .position(|m| m.id == id)
            .map(|i| self.open_msgs.remove(i));
        match streamed {
            Some(m) => out.push(streamed_completion(m)),
            None if record_text.is_empty() => {}
            None => out.extend(whole_message(id, kind, record_text)),
        }
    }

    fn tool_use(&mut self, block: &Value, out: &mut Vec<AgentEvent>) {
        let (Some(id), Some(name)) = (str_of(block, "id"), str_of(block, "name")) else {
            out.push(unmapped("assistant/tool_use", block));
            return;
        };
        if !self.seen_calls.insert(id.clone()) {
            return;
        }
        let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
        if name == ASK_USER_QUESTION {
            self.virtual_calls.insert(id, Virtual::Question);
            return;
        }
        // Arguments past the limit: the call is shown and failed, and the
        // adapter's hook already denied it without asking the gate.
        if value_size(&input) > MAX_PARTIAL_ARGS_BYTES {
            out.push(AgentEvent::ToolCallStarted {
                call_id: id.clone(),
                name: name.clone(),
                kind: tool_kind(&name),
                title: None,
                input: json!({}),
            });
            self.calls.push(OpenCall { id: id.clone() });
            self.finish(
                &id,
                ToolStatus::Failed,
                None,
                Some(format!(
                    "tool arguments exceed {MAX_PARTIAL_ARGS_BYTES} bytes"
                )),
                out,
            );
            return;
        }
        let plan = (name == TODO_WRITE).then(|| plan_entries(&input));
        out.push(AgentEvent::ToolCallStarted {
            call_id: id.clone(),
            name: name.clone(),
            kind: tool_kind(&name),
            title: tool_title(&name, &input),
            input,
        });
        self.calls.push(OpenCall { id: id.clone() });
        if let Some(entries) = plan {
            out.push(AgentEvent::PlanUpdated { entries });
        }
        if let Some(reason) = self.denied_early.remove(&id) {
            self.finish(&id, ToolStatus::Denied, None, Some(reason), out);
        }
    }

    fn finish(
        &mut self,
        call_id: &str,
        status: ToolStatus,
        output: Option<String>,
        error: Option<String>,
        out: &mut Vec<AgentEvent>,
    ) {
        if let Some(pos) = self.calls.iter().position(|c| c.id == call_id) {
            self.calls.remove(pos);
            self.done_calls.insert(call_id.to_owned());
            let (output, out_cut) = cap_output(output);
            let (error, err_cut) = cap_output(error);
            out.push(AgentEvent::ToolCallFinished {
                call_id: call_id.to_owned(),
                status,
                output,
                error,
                truncated_bytes: out_cut + err_cut,
            });
        }
    }

    fn user(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let content = v.pointer("/message/content");
        let Some(items) = content.and_then(Value::as_array) else {
            out.push(unmapped("user", v));
            return;
        };
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("tool_result") => self.tool_result(item, out),
                Some("text") => {
                    let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                    let from_subagent = v.get("parent_tool_use_id").is_some_and(|p| !p.is_null());
                    // The CLI's own marker for an interrupted turn, and the
                    // prompt a subagent starts with (already in the Agent
                    // call's input).
                    if !from_subagent && !text.starts_with("[Request interrupted by user") {
                        out.push(unmapped("user/text", item));
                    }
                }
                _ => out.push(unmapped("user", item)),
            }
        }
    }

    fn tool_result(&mut self, item: &Value, out: &mut Vec<AgentEvent>) {
        let Some(id) = str_of(item, "tool_use_id") else {
            out.push(unmapped("user/tool_result", item));
            return;
        };
        if self.done_calls.contains(&id) || self.virtual_calls.contains_key(&id) {
            return;
        }
        let is_error = item
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let text = result_text(item.get("content"));
        if !self.calls.iter().any(|c| c.id == id) {
            out.push(unmapped("user/tool_result", item));
            return;
        }
        if !is_error {
            self.finish(&id, ToolStatus::Succeeded, Some(text), None, out);
        } else if self.interrupt_requested || self.shutdown_requested {
            // Interrupted or shut down while the call was undecided or running.
            self.finish(&id, ToolStatus::Cancelled, None, Some(text), out);
        } else {
            self.finish(&id, ToolStatus::Failed, None, Some(text), out);
        }
    }

    fn synthetic(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let text = v
            .pointer("/message/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        match v.get("error").and_then(Value::as_str) {
            Some("rate_limit") => {
                // A limit is the provider talking, not a startup failure: the
                // session exists.
                self.establish(out);
                self.ensure_turn(out);
                let resets = self.limit_pending.take().flatten().or(self.last_reset_ms);
                self.rate_limited = true;
                out.push(AgentEvent::RateLimited {
                    resets_at_ms: resets,
                    message: Some(text),
                });
            }
            Some(_) => {
                self.error_emitted = true;
                if self.established {
                    out.push(err(text, false));
                } else {
                    // The failure is reported, with its diagnostics, when the
                    // process exits.
                    self.pre_error.get_or_insert(text);
                }
            }
            None => out.push(unmapped("assistant/synthetic", v)),
        }
    }

    fn result(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        if let Some(resets) = self.limit_pending.take()
            && !self.rate_limited
        {
            // A rejection with no refusal message after it.
            self.establish(out);
            self.ensure_turn(out);
            self.rate_limited = true;
            out.push(AgentEvent::RateLimited {
                resets_at_ms: resets,
                message: None,
            });
        }
        let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        if !self.established {
            if is_error {
                // Pre-session failure: the process exits next, and the Error
                // is emitted with it.
                if self.pre_error.is_none() {
                    self.pre_error = Some(
                        str_of(v, "result")
                            .unwrap_or_else(|| "claude failed before the session started".into()),
                    );
                }
                self.pre_session_failed = true;
                self.turn_open = false;
                return;
            }
            self.establish(out);
        }
        self.ensure_turn(out);
        self.close_messages(out);
        self.cancel_open_calls(out);
        self.cancel_pending(out);

        let terminal = str_of(v, "terminal_reason").unwrap_or_default();
        let subtype = str_of(v, "subtype").unwrap_or_default();
        let outcome = if self.interrupt_requested {
            TurnOutcome::Interrupted
        } else if subtype == "error_max_turns" || terminal == "max_turns" {
            TurnOutcome::LimitReached
        } else if is_error || subtype.starts_with("error") {
            TurnOutcome::Failed
        } else {
            TurnOutcome::Completed
        };
        if outcome == TurnOutcome::Failed && !self.error_emitted && !self.rate_limited {
            out.push(err(
                str_of(v, "result").unwrap_or_else(|| format!("turn failed ({terminal})")),
                false,
            ));
        }
        if let Some(used) = self.last_context.take() {
            out.push(AgentEvent::ContextUpdated {
                used_tokens: used,
                window_tokens: self.context_window(v),
            });
        }
        let usage = result_usage(v, &mut self.prev_totals);
        if let Some(u) = &usage {
            out.push(AgentEvent::Usage { usage: u.clone() });
        }
        out.push(AgentEvent::TurnCompleted {
            turn: self.turn,
            outcome,
            usage,
        });
        self.turn_open = false;
        self.interrupt_requested = false;
    }

    /// The context window of the session's model, from `modelUsage`.
    fn context_window(&self, v: &Value) -> Option<u64> {
        let models = v.get("modelUsage").and_then(Value::as_object)?;
        let main = self.init.as_ref().and_then(|i| i.model.as_deref());
        let entry = main
            .and_then(|m| models.get(m))
            .or_else(|| (models.len() == 1).then(|| models.values().next()).flatten())?;
        entry.get("contextWindow").and_then(Value::as_u64)
    }

    fn rate_limit(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let Some(info) = v.get("rate_limit_info") else {
            out.push(unmapped("rate_limit_event", v));
            return;
        };
        let mut windows = Vec::new();
        if let Some(w) = info.get("unifiedWindows").and_then(Value::as_object) {
            for (key, val) in w {
                windows.push(QuotaWindow {
                    name: window_name(key),
                    // `utilization` is the used fraction of the window (0..1).
                    used_percent: val
                        .get("utilization")
                        .and_then(Value::as_f64)
                        .map(|f| f * 100.0),
                    resets_at_ms: val
                        .get("resetsAt")
                        .and_then(Value::as_u64)
                        .map(|s| s * 1000),
                });
            }
        } else if let Some(t) = info.get("rateLimitType").and_then(Value::as_str) {
            windows.push(QuotaWindow {
                name: window_name(t),
                used_percent: info
                    .get("utilization")
                    .and_then(Value::as_f64)
                    .map(|f| f * 100.0),
                resets_at_ms: info
                    .get("resetsAt")
                    .and_then(Value::as_u64)
                    .map(|s| s * 1000),
            });
        }
        let resets = info
            .get("resetsAt")
            .and_then(Value::as_u64)
            .map(|s| s * 1000);
        if resets.is_some() {
            self.last_reset_ms = resets;
        }
        let rejected = info.get("status").and_then(Value::as_str) == Some("rejected");
        if rejected {
            // A refusal is the provider answering: the session exists.
            self.establish(out);
            self.ensure_turn(out);
        }
        if !windows.is_empty() && self.last_quota.as_ref() != Some(&windows) {
            self.last_quota = Some(windows.clone());
            let ev = AgentEvent::QuotaUpdated { windows };
            if self.established {
                out.push(ev);
            } else {
                self.held.push(ev);
            }
        }
        if rejected && !self.rate_limited {
            self.limit_pending = Some(resets);
        }
    }

    fn control_request(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let sub = v.pointer("/request/subtype").and_then(Value::as_str);
        match sub {
            // Answered by the adapter through the gate; the outcome reaches us
            // as an `AdapterInput::Gate`.
            Some("hook_callback") => {}
            Some("can_use_tool") => self.can_use_tool(v, out),
            _ => out.push(unmapped("control_request", v)),
        }
    }

    fn can_use_tool(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let (Some(rid), Some(r)) = (str_of(v, "request_id"), v.get("request")) else {
            out.push(unmapped("control_request/can_use_tool", v));
            return;
        };
        let name = str_of(r, "tool_name").unwrap_or_default();
        let input = r.get("input").cloned().unwrap_or(Value::Null);
        let tool_use_id = str_of(r, "tool_use_id");
        let key = gate_key(tool_use_id.as_deref(), &name, &input);
        let reason = str_of(r, "decision_reason").filter(|s| !s.is_empty());
        let title = tool_title(&name, &input).or_else(|| str_of(r, "description"));
        let question = name == ASK_USER_QUESTION;
        self.can_use.insert(
            rid.clone(),
            CanUse {
                question,
                key: key.clone(),
                tool_use_id,
                name,
                input,
                title,
                reason,
                emitted: false,
            },
        );
        // A question waits for its ticket (`QuestionTicket`); a tool for the
        // gate's `Ask`.
        if !question && let Some(ticket) = self.asked.remove(&key) {
            self.emit_approval(&rid, ticket, out);
        }
    }

    fn emit_approval(&mut self, rid: &str, ticket: ApprovalTicket, out: &mut Vec<AgentEvent>) {
        let Some(c) = self.can_use.get_mut(rid) else {
            return;
        };
        if c.emitted {
            return;
        }
        c.emitted = true;
        self.ensure_turn(out);
        let Some(c) = self.can_use.get(rid) else {
            return;
        };
        // The approval must refer to a call the client has seen (a subagent's
        // call can ask before its tool_use record arrives).
        let (name, input, title) = (c.name.clone(), c.input.clone(), c.title.clone());
        if let Some(id) = c.tool_use_id.clone()
            && self.seen_calls.insert(id.clone())
        {
            out.push(AgentEvent::ToolCallStarted {
                call_id: id.clone(),
                name: name.clone(),
                kind: tool_kind(&name),
                title: title.clone(),
                input: input.clone(),
            });
            self.calls.push(OpenCall { id });
        }
        let Some(c) = self.can_use.get(rid) else {
            return;
        };
        out.push(AgentEvent::ApprovalRequested {
            request_id: ticket.request_id.clone(),
            call_id: c.tool_use_id.clone(),
            name: c.name.clone(),
            kind: tool_kind(&c.name),
            title: c.title.clone(),
            input: c.input.clone(),
            reason: c.reason.clone(),
            rememberable: ticket.rememberable,
        });
        let call_id = c.tool_use_id.clone();
        self.pending.push((
            ticket.request_id,
            Pending::Approval {
                call_id,
                native: rid.to_owned(),
            },
        ));
    }

    fn question_ticket(
        &mut self,
        native: &str,
        ticket: Option<&ApprovalTicket>,
        out: &mut Vec<AgentEvent>,
    ) {
        let Some(c) = self.can_use.get_mut(native) else {
            return;
        };
        if !c.question || c.emitted {
            return;
        }
        let Some(ticket) = ticket else {
            // Declined: the adapter tells the provider; nothing is shown.
            self.can_use.remove(native);
            return;
        };
        c.emitted = true;
        let questions = questions_of(&c.input);
        self.ensure_turn(out);
        out.push(AgentEvent::QuestionAsked {
            request_id: ticket.request_id.clone(),
            questions,
        });
        self.pending.push((
            ticket.request_id.clone(),
            Pending::Question {
                native: native.to_owned(),
            },
        ));
    }

    fn control_cancel(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let Some(rid) = str_of(v, "request_id") else {
            out.push(unmapped("control_cancel_request", v));
            return;
        };
        self.can_use.remove(&rid);
        if let Some(pos) = self.pending.iter().position(|p| p.1.native() == rid) {
            let (ticket_id, _) = self.pending.remove(pos);
            out.push(AgentEvent::RequestResolved {
                request_id: ticket_id,
                outcome: RequestOutcome::Cancelled,
            });
        }
    }

    fn gate(&mut self, check: &crate::ToolCheck, decision: &GateDecision, out: &mut Vec<AgentEvent>) {
        let key = gate_key(check.call_id.as_deref(), &check.name, &check.input);
        let waiting: Vec<String> = self
            .can_use
            .iter()
            .filter(|(_, c)| c.key == key && !c.emitted && !c.question)
            .map(|(rid, _)| rid.clone())
            .collect();
        match decision {
            GateDecision::Allow => {
                self.allowed.insert(key);
            }
            GateDecision::Deny { reason } => {
                // A can_use_tool request waiting on this decision is dead.
                self.can_use.retain(|_, c| c.key != key);
                if let Some(id) = &check.call_id {
                    self.deny_call(id, reason, out);
                }
            }
            GateDecision::Ask { ticket } => {
                let mut waiting = waiting.into_iter();
                match waiting.next() {
                    Some(rid) => self.emit_approval(&rid, ticket.clone(), out),
                    None => {
                        self.asked.insert(key, ticket.clone());
                    }
                }
            }
        }
    }

    fn deny_call(&mut self, id: &str, reason: &str, out: &mut Vec<AgentEvent>) {
        if self.virtual_calls.contains_key(id) || self.done_calls.contains(id) {
            return;
        }
        if self.calls.iter().any(|c| c.id == id) {
            self.finish(id, ToolStatus::Denied, None, Some(reason.to_owned()), out);
        } else if !self.seen_calls.contains(id) {
            self.denied_early.insert(id.to_owned(), reason.to_owned());
        }
    }

    fn command(&mut self, command: &SessionCommand, out: &mut Vec<AgentEvent>) {
        match command {
            // The adapter holds the texts; the count and the limit live here so
            // a replay rejects the same messages.
            SessionCommand::SendUserMessage { .. } => {
                let busy = self.turn_open || self.waiting > 0;
                if busy && self.waiting >= MAX_QUEUED_MESSAGES {
                    self.rejected = true;
                    out.push(err(
                        format!(
                            "message rejected: {MAX_QUEUED_MESSAGES} messages are already waiting"
                        ),
                        false,
                    ));
                } else {
                    self.waiting += 1;
                }
            }
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => match self.take_pending(request_id, true) {
                Some(Pending::Approval { call_id, native }) => {
                    self.can_use.remove(&native);
                    let (outcome, deny_msg) = match decision {
                        ApprovalDecision::Allow => (RequestOutcome::Approved, None),
                        ApprovalDecision::Deny { message } => (
                            RequestOutcome::Denied,
                            Some(
                                message
                                    .clone()
                                    .unwrap_or_else(|| "Denied by the user".into()),
                            ),
                        ),
                    };
                    out.push(AgentEvent::RequestResolved {
                        request_id: request_id.clone(),
                        outcome,
                    });
                    if let (Some(msg), Some(call)) = (deny_msg, call_id) {
                        self.deny_call(&call, &msg, out);
                    }
                }
                _ => out.push(unknown_request(request_id)),
            },
            SessionCommand::AnswerQuestion { request_id, .. } => {
                match self.take_pending(request_id, false) {
                    Some(Pending::Question { native }) => {
                        self.can_use.remove(&native);
                        out.push(AgentEvent::RequestResolved {
                            request_id: request_id.clone(),
                            outcome: RequestOutcome::Answered,
                        });
                    }
                    _ => out.push(unknown_request(request_id)),
                }
            }
            SessionCommand::Interrupt => {
                if self.turn_open {
                    self.interrupt_requested = true;
                }
            }
            SessionCommand::Shutdown => self.shutdown_requested = true,
        }
    }

    /// Removes and returns a pending request of the wanted kind.
    fn take_pending(&mut self, rid: &str, approval: bool) -> Option<Pending> {
        let pos = self
            .pending
            .iter()
            .position(|p| p.0 == rid && matches!(p.1, Pending::Approval { .. }) == approval)?;
        Some(self.pending.remove(pos).1)
    }

    fn close_messages(&mut self, out: &mut Vec<AgentEvent>) {
        for m in std::mem::take(&mut self.open_msgs) {
            out.push(streamed_completion(m));
        }
    }

    fn cancel_open_calls(&mut self, out: &mut Vec<AgentEvent>) {
        for c in std::mem::take(&mut self.calls) {
            self.done_calls.insert(c.id.clone());
            out.push(AgentEvent::ToolCallFinished {
                call_id: c.id,
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
                truncated_bytes: 0,
            });
        }
    }

    /// A fatal problem the adapter detected: the session ends with it.
    fn fatal(&mut self, message: String, out: &mut Vec<AgentEvent>) {
        if self.established {
            if self.end_reason.is_none() {
                out.push(err(message.clone(), true));
            }
        } else {
            self.pre_error.get_or_insert(message.clone());
            self.pre_session_failed = true;
        }
        self.error_emitted = true;
        self.end_reason.get_or_insert(message);
    }

    fn deadline(&mut self, what: &str, out: &mut Vec<AgentEvent>) {
        match what {
            DEADLINE_RECORD_TOO_LARGE => self.fatal(
                format!("a provider record exceeds {MAX_PROVIDER_RECORD_BYTES} bytes"),
                out,
            ),
            DEADLINE_INTERRUPT => {
                // The CLI ignored the interrupt: the adapter ends the session
                // and the daemon resumes it in a new incarnation.
                self.end_reason
                    .get_or_insert("the interrupt was not honoured".to_owned());
            }
            _ => {}
        }
    }

    fn exit(&mut self, code: Option<i32>, out: &mut Vec<AgentEvent>) {
        if self.ended {
            return;
        }
        let after_interrupt = self.interrupt_requested && !self.shutdown_requested;
        let requested = after_interrupt || self.shutdown_requested || self.end_reason.is_some();

        if !self.established {
            if after_interrupt && self.init.is_some() {
                // The turn was cut off before the model answered.
                self.establish(out);
            } else {
                if self.pre_error.is_some()
                    || self.ever_had_turn
                    || !self.shutdown_requested
                {
                    let message = self.pre_error.clone().unwrap_or_else(|| {
                        format!("claude exited{} before the session started", exit_text(code))
                    });
                    let details = self.diagnostics();
                    out.push(AgentEvent::Error {
                        message,
                        fatal: true,
                        details,
                    });
                }
                self.held.clear();
                self.finish_session(code, out);
                return;
            }
        }

        if self.turn_open && !requested {
            let details = self.diagnostics();
            out.push(AgentEvent::Error {
                message: format!("claude exited{} during a turn", exit_text(code)),
                fatal: true,
                details,
            });
        }
        self.close_messages(out);
        self.cancel_open_calls(out);
        self.cancel_pending(out);
        if self.turn_open {
            out.push(AgentEvent::TurnCompleted {
                turn: self.turn,
                outcome: if requested {
                    TurnOutcome::Interrupted
                } else {
                    TurnOutcome::Failed
                },
                usage: None,
            });
            self.turn_open = false;
        }
        self.finish_session(code, out);
    }

    fn cancel_pending(&mut self, out: &mut Vec<AgentEvent>) {
        for (rid, _) in std::mem::take(&mut self.pending) {
            out.push(AgentEvent::RequestResolved {
                request_id: rid,
                outcome: RequestOutcome::Cancelled,
            });
        }
        self.can_use.clear();
        self.asked.clear();
    }

    fn finish_session(&mut self, code: Option<i32>, out: &mut Vec<AgentEvent>) {
        self.cancel_pending(out);
        self.ended = true;
        let after_interrupt = self.interrupt_requested;
        let reason = if self.pre_session_failed || self.pre_error.is_some() {
            Some("provider error".to_owned())
        } else if let Some(r) = &self.end_reason {
            Some(r.clone())
        } else if self.shutdown_requested {
            Some("shutdown".to_owned())
        } else if after_interrupt {
            Some("claude exited after an interrupt".to_owned())
        } else {
            code.map(|c| format!("claude exited with code {c}"))
        };
        out.push(AgentEvent::SessionEnded {
            exit_code: code,
            reason,
        });
    }

    /// Stderr tail and the records that never made it into a session, for
    /// `Error::details`.
    fn diagnostics(&self) -> Vec<String> {
        let mut lines: Vec<String> = self.stderr_tail.iter().cloned().collect();
        for ev in &self.held {
            if let AgentEvent::Unmapped { record_type, raw } = ev {
                lines.push(diag_line(&format!("{record_type}: {raw}")));
            }
        }
        if lines.len() > MAX_DIAGNOSTIC_LINES {
            lines.drain(..lines.len() - MAX_DIAGNOSTIC_LINES);
        }
        lines
    }
}

impl Normalizer for ClaudeNormalizer {
    fn on_input(&mut self, input: &AdapterInput) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match input {
            AdapterInput::Stdin { line } => self.stdin(line, &mut out),
            AdapterInput::Stdout { line } => self.stdout(line, &mut out),
            AdapterInput::Stderr { line } => {
                if self.stderr_tail.len() >= MAX_DIAGNOSTIC_LINES {
                    self.stderr_tail.pop_front();
                }
                self.stderr_tail.push_back(diag_line(line));
            }
            AdapterInput::Gate { check, decision } => self.gate(check, decision, &mut out),
            AdapterInput::Command { command } => self.command(command, &mut out),
            AdapterInput::Redeem {
                check, decision, ..
            } => {
                // The user allowed it, but the gate refused at the moment of
                // the effect: the call does not run.
                if let GateDecision::Deny { reason } = decision
                    && let Some(id) = &check.call_id
                {
                    self.deny_call(id, reason, &mut out);
                }
            }
            AdapterInput::QuestionTicket { id, ticket } => {
                self.question_ticket(id, ticket.as_ref(), &mut out)
            }
            AdapterInput::Enforcement { descriptor } => {
                if self.established && self.enforcement.as_ref() != Some(descriptor) {
                    out.push(AgentEvent::EnforcementChanged {
                        enforcement: descriptor.clone(),
                    });
                }
                self.enforcement = Some(descriptor.clone());
            }
            AdapterInput::Deadline { what } => self.deadline(what, &mut out),
            AdapterInput::Exit { code } => self.exit(*code, &mut out),
            AdapterInput::HookRequest { .. }
            | AdapterInput::HookResponse { .. }
            | AdapterInput::HookDisconnected { .. }
            | AdapterInput::WriteFailed { .. } => {}
        }
        out
    }
}

/// What the descriptor says when nothing recorded one (a hand-made input
/// stream): never Full.
fn unknown_enforcement() -> EnforcementDescriptor {
    let d = EnforcementDescriptor::evaluate(Provider::Claude, None, false, vec![], vec![], vec![]);
    debug_assert_eq!(d.level, EnforcementLevel::Partial);
    d
}

fn exit_text(code: Option<i32>) -> String {
    code.map(|c| format!(" with code {c}")).unwrap_or_default()
}

fn err(message: String, fatal: bool) -> AgentEvent {
    AgentEvent::Error {
        message,
        fatal,
        details: vec![],
    }
}

fn unknown_request(rid: &str) -> AgentEvent {
    err(
        format!("unknown or already resolved request: {rid}"),
        false,
    )
}

fn unmapped(record_type: &str, raw: &Value) -> AgentEvent {
    AgentEvent::Unmapped {
        record_type: record_type.to_owned(),
        raw: capped_raw(raw.clone()),
    }
}

/// `Unmapped.raw` is cut to the limit (a JSON string of the truncated text).
fn capped_raw(raw: Value) -> Value {
    let text = raw.to_string();
    if text.len() <= MAX_UNMAPPED_RAW_BYTES {
        return raw;
    }
    Value::String(cut(&text, MAX_UNMAPPED_RAW_BYTES).to_owned())
}

/// The longest prefix of `text` of at most `max` bytes, on a char boundary.
fn cut(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// One diagnostic line: control characters dropped, cut to the limit.
fn diag_line(line: &str) -> String {
    let clean: String = line
        .chars()
        .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
        .collect();
    cut(&clean, MAX_DIAGNOSTIC_LINE_BYTES).to_owned()
}

fn value_size(v: &Value) -> usize {
    v.to_string().len()
}

/// Output kept per call; returns the text and the bytes dropped.
fn cap_output(text: Option<String>) -> (Option<String>, u64) {
    match text {
        Some(t) if t.len() > MAX_TOOL_OUTPUT_BYTES => {
            let kept = cut(&t, MAX_TOOL_OUTPUT_BYTES).to_owned();
            let dropped = (t.len() - kept.len()) as u64;
            (Some(kept), dropped)
        }
        other => (other, 0),
    }
}

fn sha256_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn streamed_completion(m: OpenMsg) -> AgentEvent {
    AgentEvent::MessageCompleted {
        message_id: m.id,
        kind: m.kind,
        body: MessageBody::Streamed {
            chars: m.text.chars().count() as u64,
            deltas: m.deltas,
            sha256: sha256_hex(&m.text),
        },
    }
}

/// A completion-only message: `Inline` up to the limit, otherwise split into
/// deltas on character boundaries and `Streamed`.
fn whole_message(id: String, kind: MessageKind, text: String) -> Vec<AgentEvent> {
    if text.len() <= MAX_INLINE_MESSAGE_BYTES {
        return vec![AgentEvent::MessageCompleted {
            message_id: id,
            kind,
            body: MessageBody::Inline { text },
        }];
    }
    let mut events = Vec::new();
    let mut rest = text.as_str();
    let mut deltas = 0;
    while !rest.is_empty() {
        let piece = cut(rest, MAX_INLINE_MESSAGE_BYTES);
        deltas += 1;
        events.push(AgentEvent::MessageDelta {
            message_id: id.clone(),
            kind,
            text: piece.to_owned(),
        });
        rest = &rest[piece.len()..];
    }
    events.push(AgentEvent::MessageCompleted {
        message_id: id,
        kind,
        body: MessageBody::Streamed {
            chars: text.chars().count() as u64,
            deltas,
            sha256: sha256_hex(&text),
        },
    });
    events
}

/// The text of a `user` stdin record (`content` is a string or text blocks).
fn user_text_of(v: &Value) -> String {
    match v.pointer("/message/content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn window_name(key: &str) -> String {
    match key {
        "five_hour" => "5h".into(),
        "seven_day" => "weekly".into(),
        "seven_day_opus" => "weekly_opus".into(),
        "seven_day_sonnet" => "weekly_sonnet".into(),
        "seven_day_overage_included" => "weekly_overage_included".into(),
        other => other.to_owned(),
    }
}

fn msg_usage(u: &Value) -> MsgUsage {
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    MsgUsage {
        input: n("input_tokens"),
        output: n("output_tokens"),
        cache_read: n("cache_read_input_tokens"),
        cache_write: n("cache_creation_input_tokens"),
        reasoning: u
            .pointer("/output_tokens_details/thinking_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

/// The turn's usage. `result.usage` leaves subagents out, while
/// `result.modelUsage` and `total_cost_usd` are session totals over every
/// model and subagent: the turn is the difference from the previous totals. A
/// total that went down means a new process (after a resume), so counting
/// restarts from zero.
fn result_usage(v: &Value, prev: &mut Totals) -> Option<Usage> {
    let models = v
        .get("modelUsage")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty());
    let Some(models) = models else {
        // No per-model totals (a failed request): use the turn's own usage.
        let u = v.get("usage")?;
        let n = |k: &str| u.get(k).and_then(Value::as_u64);
        let total = v.get("total_cost_usd").and_then(Value::as_f64);
        let cost = total.map(|t| {
            if t < prev.cost {
                prev.cost = 0.0;
            }
            let c = t - prev.cost;
            prev.cost = t;
            c
        });
        return Some(Usage {
            input_tokens: n("input_tokens"),
            output_tokens: n("output_tokens"),
            cache_read_tokens: n("cache_read_input_tokens"),
            cache_write_tokens: n("cache_creation_input_tokens"),
            reasoning_tokens: u
                .pointer("/output_tokens_details/thinking_tokens")
                .and_then(Value::as_u64),
            cost_usd: cost,
        });
    };
    let mut now = Totals::default();
    for m in models.values() {
        let n = |k: &str| m.get(k).and_then(Value::as_u64).unwrap_or(0);
        now.input += n("inputTokens");
        now.output += n("outputTokens");
        now.cache_read += n("cacheReadInputTokens");
        now.cache_write += n("cacheCreationInputTokens");
        now.reasoning += n("thinkingTokens");
    }
    now.cost = v
        .get("total_cost_usd")
        .and_then(Value::as_f64)
        .unwrap_or_else(|| {
            models
                .values()
                .filter_map(|m| m.get("costUSD").and_then(Value::as_f64))
                .sum()
        });
    if now.input < prev.input || now.output < prev.output || now.cost < prev.cost {
        *prev = Totals::default();
    }
    let usage = Usage {
        input_tokens: Some(now.input - prev.input),
        output_tokens: Some(now.output - prev.output),
        cache_read_tokens: Some(now.cache_read.saturating_sub(prev.cache_read)),
        cache_write_tokens: Some(now.cache_write.saturating_sub(prev.cache_write)),
        reasoning_tokens: Some(now.reasoning.saturating_sub(prev.reasoning)),
        cost_usd: Some((now.cost - prev.cost).max(0.0)),
    };
    *prev = now;
    Some(usage)
}

fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| match i.get("text").and_then(Value::as_str) {
                Some(t) => t.to_owned(),
                None => i.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) if !other.is_null() => other.to_string(),
        _ => String::new(),
    }
}

fn plan_entries(input: &Value) -> Vec<PlanEntry> {
    input
        .get("todos")
        .and_then(Value::as_array)
        .map(|todos| {
            todos
                .iter()
                .map(|t| PlanEntry {
                    content: str_of(t, "content").unwrap_or_default(),
                    status: match t.get("status").and_then(Value::as_str) {
                        Some("in_progress") => PlanStatus::InProgress,
                        Some("completed") => PlanStatus::Completed,
                        _ => PlanStatus::Pending,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Stable ids: questions and options are numbered in the order Claude sent
/// them (`q0`, `o0`...), because Claude identifies answers by text.
pub fn questions_of(input: &Value) -> Vec<Question> {
    input
        .get("questions")
        .and_then(Value::as_array)
        .map(|qs| {
            qs.iter()
                .enumerate()
                .map(|(qi, q)| Question {
                    id: format!("q{qi}"),
                    prompt: str_of(q, "question").unwrap_or_default(),
                    options: q
                        .get("options")
                        .and_then(Value::as_array)
                        .map(|os| {
                            os.iter()
                                .enumerate()
                                .map(|(oi, o)| QuestionOption {
                                    id: format!("o{oi}"),
                                    label: str_of(o, "label").unwrap_or_default(),
                                    description: str_of(o, "description").filter(|d| !d.is_empty()),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    multi_select: q
                        .get("multiSelect")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    // Claude's question dialog always has an "Other" text box.
                    allow_text: true,
                })
                .collect()
        })
        .unwrap_or_default()
}
