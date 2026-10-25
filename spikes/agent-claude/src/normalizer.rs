//! `ClaudeNormalizer`: Claude Code stream-json + control protocol → [`AgentEvent`].
//!
//! Pure: no I/O, no clock. Everything it knows comes through
//! [`AdapterInput`], so live sessions and fixture replays produce the same
//! events. See the spike report for the lifecycle decisions (deferred
//! `SessionStarted`, interrupt-induced exits, virtual tool calls).

use std::collections::{HashMap, HashSet, VecDeque};

use atrium_agents::{AdapterInput, GateDecision, Normalizer, ToolCheck};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageKind, PlanEntry, PlanStatus, Provider, Question,
    QuestionOption, QuotaWindow, RequestOutcome, SessionCommand, ToolKind, ToolStatus, TurnOutcome,
    Usage,
};
use serde_json::{Value, json};

/// The synthetic model name Claude Code puts on records it writes itself
/// (API errors, "no response" notices).
const SYNTHETIC_MODEL: &str = "<synthetic>";
const ASK_USER_QUESTION: &str = "AskUserQuestion";
const TODO_WRITE: &str = "TodoWrite";

/// Maps Claude Code's tool name to Atrium's coarse [`ToolKind`].
pub fn tool_kind(name: &str) -> ToolKind {
    if name.starts_with("mcp__") {
        return ToolKind::Mcp;
    }
    match name {
        "Read" => ToolKind::Read,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ToolKind::Edit,
        "Glob" | "Grep" | "LS" | "ToolSearch" => ToolKind::Search,
        "Bash" | "BashOutput" | "KillShell" | "KillBash" | "Monitor" | "PowerShell" => {
            ToolKind::Execute
        }
        "WebFetch" | "WebSearch" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

/// A short human-readable summary of a tool call: the command line, the file
/// path, the URL...
pub fn tool_title(name: &str, input: &Value) -> Option<String> {
    let s = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    match name {
        "Bash" | "PowerShell" => s("command"),
        "Read" | "Edit" | "MultiEdit" | "Write" => s("file_path"),
        "NotebookEdit" => s("notebook_path"),
        "Glob" | "Grep" => s("pattern"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        "Task" | "Agent" => s("description"),
        "Skill" => s("skill"),
        "ToolSearch" => s("query"),
        _ if name.starts_with("mcp__") => Some(name.trim_start_matches("mcp__").replace("__", ".")),
        _ => None,
    }
}

/// The [`ToolCheck`] for a `PreToolUse` hook callback or a `can_use_tool`
/// request.
pub fn tool_check(call_id: Option<&str>, name: &str, input: &Value) -> ToolCheck {
    ToolCheck {
        call_id: call_id.map(str::to_owned),
        name: name.to_owned(),
        kind: tool_kind(name),
        title: tool_title(name, input),
        input: input.clone(),
    }
}

#[derive(Debug)]
struct OpenCall {
    id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Virtual {
    Question,
    Todo,
}

#[derive(Debug)]
struct CanUse {
    tool_use_id: Option<String>,
    name: String,
    input: Value,
    title: Option<String>,
    reason: Option<String>,
    emitted: bool,
}

#[derive(Debug)]
enum Pending {
    Approval { call_id: Option<String> },
    Question,
}

#[derive(Debug, Clone, Default)]
struct MsgUsage {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
}

#[derive(Debug)]
struct InitInfo {
    session_id: String,
    model: Option<String>,
    tools: Vec<String>,
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
    /// The result record for the open turn already closed it.
    pre_session_failed: bool,

    block_counts: HashMap<String, usize>,
    /// Open streamed messages, in order: (message_id, kind, text so far).
    open_msgs: Vec<(String, MessageKind, String)>,

    calls: Vec<OpenCall>,
    seen_calls: HashSet<String>,
    done_calls: HashSet<String>,
    virtual_calls: HashMap<String, Virtual>,
    denied_early: HashMap<String, String>,

    pending: Vec<(String, Pending)>,
    can_use: HashMap<String, CanUse>,
    asked: HashSet<String>,

    msg_usage: HashMap<String, MsgUsage>,
    prev_cost: f64,
    last_quota: Option<Vec<QuotaWindow>>,
    last_reset_ms: Option<u64>,
    rate_limited: bool,

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

    /// The first turn failed before the session was established (bad model,
    /// auth error...). The process is not going to be useful.
    pub fn pre_session_failed(&self) -> bool {
        self.pre_session_failed
    }

    pub fn native_session_id(&self) -> Option<&str> {
        self.init.as_ref().map(|i| i.session_id.as_str())
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
            self.begin_turn(out);
        }
    }

    fn begin_turn(&mut self, out: &mut Vec<AgentEvent>) {
        self.turn += 1;
        self.turn_open = true;
        self.ever_had_turn = true;
        self.turn_start_emitted = false;
        self.rate_limited = false;
        self.error_emitted = false;
        self.msg_usage.clear();
        self.interrupt_requested = false;
        if self.established {
            self.turn_start_emitted = true;
            out.push(AgentEvent::TurnStarted { turn: self.turn });
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
        out.push(AgentEvent::SessionStarted {
            provider: Provider::Claude,
            native_session_id: init.session_id.clone(),
            model: init.model.clone(),
            tools: init.tools.clone(),
        });
        if self.turn_open && !self.turn_start_emitted {
            self.turn_start_emitted = true;
            out.push(AgentEvent::TurnStarted { turn: self.turn });
        }
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
            out.push(AgentEvent::TurnStarted { turn: self.turn });
        }
    }

    fn stdout(&mut self, line: &str, out: &mut Vec<AgentEvent>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            out.push(AgentEvent::Unmapped {
                record_type: "unparseable".into(),
                raw: Value::String(line.to_owned()),
            });
            return;
        };
        let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
        match ty {
            "system" => self.system(&v, out),
            "stream_event" => self.stream_event(&v, out),
            "assistant" => self.assistant(&v, out),
            "user" => self.user(&v, out),
            "result" => self.result(&v, out),
            "rate_limit_event" => self.rate_limit(&v, out),
            "control_request" => self.control_request(&v, out),
            "control_cancel_request" => self.control_cancel(&v, out),
            // Replies to our own control requests (initialize, interrupt).
            "control_response" | "keep_alive" => {}
            other => out.push(unmapped(other, &v)),
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
                match self.open_msgs.iter_mut().find(|m| m.0 == id) {
                    Some(m) => m.2.push_str(&text),
                    None => self.open_msgs.push((id.clone(), kind, text.clone())),
                }
                out.push(AgentEvent::MessageDelta {
                    message_id: id,
                    kind,
                    text,
                });
            }
            "message_delta" => {
                if v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
                    return;
                }
                if let Some(u) = e.get("usage") {
                    let mu = msg_usage(u);
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
        }
        Usage {
            input_tokens: Some(total.input),
            output_tokens: Some(total.output),
            cache_read_tokens: Some(total.cache_read),
            cache_write_tokens: Some(total.cache_write),
            reasoning_tokens: None,
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
            .position(|m| m.0 == id)
            .map(|i| self.open_msgs.remove(i).2);
        let text = streamed.unwrap_or(record_text);
        if text.is_empty() {
            return;
        }
        out.push(AgentEvent::MessageCompleted {
            message_id: id,
            kind,
            text,
        });
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
        if name == TODO_WRITE {
            self.virtual_calls.insert(id, Virtual::Todo);
            out.push(AgentEvent::PlanUpdated {
                entries: plan_entries(&input),
            });
            return;
        }
        out.push(AgentEvent::ToolCallStarted {
            call_id: id.clone(),
            name: name.clone(),
            kind: tool_kind(&name),
            title: tool_title(&name, &input),
            input,
        });
        self.calls.push(OpenCall { id: id.clone() });
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
            out.push(AgentEvent::ToolCallFinished {
                call_id: call_id.to_owned(),
                status,
                output,
                error,
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
                    // The CLI's own marker for an interrupted turn.
                    if !text.starts_with("[Request interrupted by user") {
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
                self.ensure_turn(out);
                self.rate_limited = true;
                out.push(AgentEvent::RateLimited {
                    resets_at_ms: self.last_reset_ms,
                    message: Some(text),
                });
            }
            Some(_) => {
                let fatal = !self.established;
                self.error_emitted = true;
                out.push(AgentEvent::Error {
                    message: text,
                    fatal,
                });
            }
            None => out.push(unmapped("assistant/synthetic", v)),
        }
    }

    fn result(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
        if !self.established {
            if is_error {
                // Pre-session failure: the Error came with the synthetic
                // record; the process exits next.
                if !self.error_emitted {
                    self.error_emitted = true;
                    out.push(AgentEvent::Error {
                        message: str_of(v, "result")
                            .unwrap_or_else(|| "claude failed before the session started".into()),
                        fatal: true,
                    });
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
            out.push(AgentEvent::Error {
                message: str_of(v, "result").unwrap_or_else(|| format!("turn failed ({terminal})")),
                fatal: false,
            });
        }
        let usage = result_usage(v, &mut self.prev_cost);
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
        if !windows.is_empty() && self.last_quota.as_ref() != Some(&windows) {
            self.last_quota = Some(windows.clone());
            out.push(AgentEvent::QuotaUpdated { windows });
        }
        if info.get("status").and_then(Value::as_str) == Some("rejected") && !self.rate_limited {
            self.rate_limited = true;
            out.push(AgentEvent::RateLimited {
                resets_at_ms: resets,
                message: None,
            });
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
        if name == ASK_USER_QUESTION {
            self.ensure_turn(out);
            out.push(AgentEvent::QuestionAsked {
                request_id: rid.clone(),
                questions: questions_of(&input),
            });
            self.pending.push((rid, Pending::Question));
            return;
        }
        let reason = str_of(r, "decision_reason").filter(|s| !s.is_empty());
        let title = tool_title(&name, &input).or_else(|| str_of(r, "description"));
        self.can_use.insert(
            rid.clone(),
            CanUse {
                tool_use_id: tool_use_id.clone(),
                name,
                input,
                title,
                reason,
                emitted: false,
            },
        );
        if tool_use_id.is_some_and(|t| self.asked.contains(&t)) {
            self.emit_approval(&rid, out);
        }
    }

    fn emit_approval(&mut self, rid: &str, out: &mut Vec<AgentEvent>) {
        let Some(c) = self.can_use.get_mut(rid) else {
            return;
        };
        if c.emitted {
            return;
        }
        c.emitted = true;
        out.push(AgentEvent::ApprovalRequested {
            request_id: rid.to_owned(),
            call_id: c.tool_use_id.clone(),
            name: c.name.clone(),
            kind: tool_kind(&c.name),
            title: c.title.clone(),
            input: c.input.clone(),
            reason: c.reason.clone(),
        });
        let call_id = c.tool_use_id.clone();
        self.pending
            .push((rid.to_owned(), Pending::Approval { call_id }));
    }

    fn control_cancel(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let Some(rid) = str_of(v, "request_id") else {
            out.push(unmapped("control_cancel_request", v));
            return;
        };
        self.can_use.remove(&rid);
        if let Some(pos) = self.pending.iter().position(|p| p.0 == rid) {
            self.pending.remove(pos);
            out.push(AgentEvent::RequestResolved {
                request_id: rid,
                outcome: RequestOutcome::Cancelled,
            });
        }
    }

    fn gate(&mut self, check: &ToolCheck, decision: &GateDecision, out: &mut Vec<AgentEvent>) {
        let Some(id) = check.call_id.clone() else {
            return;
        };
        match decision {
            GateDecision::Allow => {}
            GateDecision::Deny { reason } => {
                // A can_use_tool request waiting on this decision is dead.
                self.can_use
                    .retain(|_, c| c.tool_use_id.as_deref() != Some(id.as_str()));
                self.deny_call(&id, reason, out);
            }
            GateDecision::Ask => {
                self.asked.insert(id.clone());
                let rid = self
                    .can_use
                    .iter()
                    .find(|(_, c)| c.tool_use_id.as_deref() == Some(id.as_str()) && !c.emitted)
                    .map(|(k, _)| k.clone());
                if let Some(rid) = rid {
                    self.emit_approval(&rid, out);
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
            // The adapter owns the queue of waiting messages.
            SessionCommand::SendUserMessage { .. } => {}
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => match self.take_pending(request_id, true) {
                Some(Pending::Approval { call_id }) => {
                    self.can_use.remove(request_id);
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
                    Some(_) => out.push(AgentEvent::RequestResolved {
                        request_id: request_id.clone(),
                        outcome: RequestOutcome::Answered,
                    }),
                    None => out.push(unknown_request(request_id)),
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
        for (id, kind, text) in std::mem::take(&mut self.open_msgs) {
            out.push(AgentEvent::MessageCompleted {
                message_id: id,
                kind,
                text,
            });
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
            });
        }
    }

    fn exit(&mut self, code: Option<i32>, out: &mut Vec<AgentEvent>) {
        if self.ended {
            return;
        }
        let after_interrupt = self.interrupt_requested && !self.shutdown_requested;
        let requested = after_interrupt || self.shutdown_requested;

        if !self.established {
            if after_interrupt && self.init.is_some() {
                // The turn was cut off before the model answered.
                self.establish(out);
            } else {
                let tail = self.stderr_summary();
                if !self.error_emitted && (self.ever_had_turn || !self.shutdown_requested) {
                    out.push(AgentEvent::Error {
                        message: format!(
                            "claude exited{} before the session started{tail}",
                            exit_text(code)
                        ),
                        fatal: true,
                    });
                }
                self.finish_session(code, out);
                return;
            }
        }

        self.close_messages(out);
        self.cancel_open_calls(out);
        self.cancel_pending(out);
        if self.turn_open {
            if !requested {
                out.push(AgentEvent::Error {
                    message: format!(
                        "claude exited{} during a turn{}",
                        exit_text(code),
                        self.stderr_summary()
                    ),
                    fatal: true,
                });
            }
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
        if after_interrupt {
            // Claude Code leaves after an interrupt; the session lives on and
            // the adapter resumes it with the next message.
            self.interrupt_requested = false;
            return;
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
    }

    fn finish_session(&mut self, code: Option<i32>, out: &mut Vec<AgentEvent>) {
        self.cancel_pending(out);
        self.ended = true;
        let reason = if self.pre_session_failed || (self.error_emitted && !self.established) {
            Some("provider error".to_owned())
        } else if self.shutdown_requested {
            Some("shutdown".to_owned())
        } else {
            code.map(|c| format!("claude exited with code {c}"))
        };
        out.push(AgentEvent::SessionEnded {
            exit_code: code,
            reason,
        });
    }

    fn stderr_summary(&self) -> String {
        if self.stderr_tail.is_empty() {
            return String::new();
        }
        let joined: Vec<&str> = self.stderr_tail.iter().map(String::as_str).collect();
        format!(": {}", joined.join(" | "))
    }
}

impl Normalizer for ClaudeNormalizer {
    fn on_input(&mut self, input: &AdapterInput) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        match input {
            AdapterInput::Stdin { line } => self.stdin(line, &mut out),
            AdapterInput::Stdout { line } => self.stdout(line, &mut out),
            AdapterInput::Stderr { line } => {
                if self.stderr_tail.len() >= 3 {
                    self.stderr_tail.pop_front();
                }
                self.stderr_tail.push_back(line.clone());
            }
            AdapterInput::HookRequest { .. } | AdapterInput::HookResponse { .. } => {}
            AdapterInput::Gate { check, decision } => self.gate(check, decision, &mut out),
            AdapterInput::Command { command } => self.command(command, &mut out),
            AdapterInput::Exit { code } => self.exit(*code, &mut out),
        }
        out
    }
}

fn exit_text(code: Option<i32>) -> String {
    code.map(|c| format!(" with code {c}")).unwrap_or_default()
}

fn unknown_request(rid: &str) -> AgentEvent {
    AgentEvent::Error {
        message: format!("unknown or already resolved request: {rid}"),
        fatal: false,
    }
}

fn unmapped(record_type: &str, raw: &Value) -> AgentEvent {
    AgentEvent::Unmapped {
        record_type: record_type.to_owned(),
        raw: raw.clone(),
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
    }
}

/// `result.usage` is this turn's tally; `total_cost_usd` is the session's
/// running total, so the turn's cost is the difference.
fn result_usage(v: &Value, prev_cost: &mut f64) -> Option<Usage> {
    let u = v.get("usage")?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64);
    let total = v.get("total_cost_usd").and_then(Value::as_f64);
    let cost = total.map(|t| {
        let c = (t - *prev_cost).max(0.0);
        *prev_cost = t;
        c
    });
    Some(Usage {
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_read_tokens: n("cache_read_input_tokens"),
        cache_write_tokens: n("cache_creation_input_tokens"),
        reasoning_tokens: u
            .pointer("/output_tokens_details/thinking_tokens")
            .and_then(Value::as_u64),
        cost_usd: cost,
    })
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
