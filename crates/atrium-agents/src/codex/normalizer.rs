#![doc = "Pure, synchronous state machine translating Codex app-server AdapterInputs into M1 rev-3 AgentEvents."]

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
use atrium_protocol::policy::{ClassCoverage, Coverage, EnforcementDescriptor, ToolClass};
use serde_json::{Value, json};

use crate::{
    AdapterInput, ApprovalTicket, GateDecision, MAX_QUEUED_MESSAGES, Normalizer, ToolCheck,
};

use super::adapter::TESTED_VERSIONS;
use super::settings::sha256_hex;
use super::tools;

/// Deadline emitted by the adapter when a provider line exceeds the cap.
pub const DEADLINE_RECORD_TOO_LARGE: &str = "record_too_large";
/// Deadline emitted by the adapter when an interrupt goes unhonoured.
pub const DEADLINE_INTERRUPT: &str = "interrupt";

/// Default Codex coverage vector shared between `probe`, `start`, and offline
/// fixture normalization.
pub fn coverage() -> Vec<ClassCoverage> {
    vec![
        ClassCoverage {
            class: ToolClass::Read,
            coverage: Coverage::ProviderGated,
            evidence: Some("fixtures/agents/codex/gate-deny".into()),
        },
        ClassCoverage {
            class: ToolClass::Edit,
            coverage: Coverage::Unknown,
            evidence: None,
        },
        ClassCoverage {
            class: ToolClass::Delete,
            coverage: Coverage::Unknown,
            evidence: None,
        },
        ClassCoverage {
            class: ToolClass::Move,
            coverage: Coverage::Unknown,
            evidence: None,
        },
        ClassCoverage {
            class: ToolClass::Search,
            coverage: Coverage::Unknown,
            evidence: None,
        },
        ClassCoverage {
            class: ToolClass::Execute,
            coverage: Coverage::ProviderGated,
            evidence: Some("fixtures/agents/codex/gate-deny-execute".into()),
        },
        ClassCoverage {
            class: ToolClass::Fetch,
            coverage: Coverage::NotIntercepted,
            evidence: None,
        },
        ClassCoverage {
            class: ToolClass::Mcp,
            coverage: Coverage::Unknown,
            evidence: None,
        },
        ClassCoverage {
            class: ToolClass::Subagent,
            coverage: Coverage::NotIntercepted,
            evidence: None,
        },
        ClassCoverage {
            class: ToolClass::ProviderSpecific,
            coverage: Coverage::Unknown,
            evidence: None,
        },
    ]
}

fn default_enforcement(cli_version: Option<String>) -> EnforcementDescriptor {
    let tested = cli_version
        .as_deref()
        .is_some_and(|v| TESTED_VERSIONS.contains(&v));
    EnforcementDescriptor::evaluate(
        Provider::Codex,
        cli_version,
        tested,
        coverage(),
        Vec::new(),
        Vec::new(),
    )
}

#[derive(Debug, Clone)]
struct OpenMsg {
    id: String,
    kind: MessageKind,
    text: String,
    deltas: u64,
}

#[derive(Debug, Clone)]
struct OpenCall {
    id: String,
    bytes_emitted: usize,
    truncated_bytes: u64,
}

#[derive(Debug, Clone)]
struct ServerReq {
    item_id: Option<String>,
    name: String,
    kind: ToolKind,
    title: Option<String>,
    input: Value,
    reason: Option<String>,
    key: String,
    emitted: bool,
}

#[derive(Debug, Clone)]
struct QuestionReq {
    questions: Vec<Question>,
    emitted: bool,
}

#[derive(Debug, Clone)]
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
            Self::Approval { native, .. } | Self::Question { native } => native,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Totals {
    input_tokens: u64,
    cached_input_tokens: u64,
    output_tokens: u64,
}

#[derive(Debug, Clone)]
struct InitInfo {
    thread_id: String,
    model: Option<String>,
    cli_version: Option<String>,
}

/// Pure, synchronous state machine mapping Codex app-server inputs to Atrium
/// `AgentEvent`s on the M1 rev-3 contract.
#[derive(Debug, Default)]
pub struct CodexNormalizer {
    cli_version: Option<String>,
    init: Option<InitInfo>,
    established: bool,
    first_enforcement: Option<EnforcementDescriptor>,
    enforcement: Option<EnforcementDescriptor>,

    turn: u32,
    turn_open: bool,
    turn_start_emitted: bool,
    ever_had_turn: bool,
    provider_turn_id: Option<String>,
    user_text: Option<String>,
    waiting: usize,
    rejected: bool,

    interrupt_requested: bool,
    shutdown_requested: bool,
    pre_session_failed: bool,
    pre_error: Option<String>,
    error_emitted: bool,
    end_reason: Option<String>,
    ended: bool,

    items: HashMap<String, Value>,
    open_msgs: Vec<OpenMsg>,
    done_msgs: HashSet<String>,

    calls: Vec<OpenCall>,
    seen_calls: HashSet<String>,
    done_calls: HashSet<String>,
    denied_early: HashMap<String, String>,

    server_reqs: HashMap<String, ServerReq>,
    question_reqs: HashMap<String, QuestionReq>,
    asked: HashMap<String, VecDeque<ApprovalTicket>>,
    pending: Vec<(String, Pending)>,

    prev_totals: Totals,
    turn_totals: Option<Totals>,
    turn_context: Option<(u64, u64)>,
    rate_limited: bool,
    last_reset_ms: Option<u64>,
    last_quota: Option<Vec<QuotaWindow>>,

    held: Vec<AgentEvent>,
    stderr_tail: VecDeque<String>,
}

impl CodexNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn turn_in_flight(&self) -> bool {
        self.turn_open
    }

    pub fn turn_number(&self) -> u32 {
        self.turn
    }

    pub fn pre_session_failed(&self) -> bool {
        self.pre_session_failed
    }

    pub fn take_rejected(&mut self) -> bool {
        std::mem::take(&mut self.rejected)
    }

    pub fn native_session_id(&self) -> Option<&str> {
        self.init.as_ref().map(|i| i.thread_id.as_str())
    }

    pub fn wants_end(&self) -> bool {
        self.end_reason.is_some() && !self.ended
    }

    pub fn has_ended(&self) -> bool {
        self.ended
    }

    pub fn is_established(&self) -> bool {
        self.established
    }

    /// Returns the most recently seen item payload for `item_id` (used by the
    /// adapter to enrich `requestApproval` payloads).
    pub fn cached_item(&self, item_id: &str) -> Option<&Value> {
        self.items.get(item_id)
    }

    // ---- inputs ----

    fn stdin(&mut self, line: &str, out: &mut Vec<AgentEvent>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return;
        };
        if v.get("method").and_then(Value::as_str) == Some("turn/start") {
            self.waiting = self.waiting.saturating_sub(1);
            let text = v
                .pointer("/params/input")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| {
                            if p.get("type").and_then(Value::as_str) == Some("text") {
                                p.get("text").and_then(Value::as_str)
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default();
            if !text.is_empty() {
                self.user_text = Some(text);
            }
            self.begin_turn(out);
        }
    }

    fn stdout(&mut self, line: &str, out: &mut Vec<AgentEvent>) {
        if line.trim().is_empty() {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            self.emit_or_hold(
                AgentEvent::Unmapped {
                    record_type: "invalid_json".into(),
                    raw: json!({"line": diag_line(line)}),
                },
                out,
            );
            return;
        };

        // Server requests (both id and method present) vs notifications (method only) vs responses.
        if let Some(method) = v.get("method").and_then(Value::as_str) {
            if let Some(id_val) = v.get("id")
                && !id_val.is_null()
            {
                self.server_request(id_val, method, &v["params"], &v, out);
            } else {
                self.notification(method, &v["params"], &v, out);
            }
            return;
        }

        if let Some(id_val) = v.get("id") {
            self.response(id_val, &v, out);
        }
    }

    fn response(&mut self, id_val: &Value, v: &Value, out: &mut Vec<AgentEvent>) {
        let id = id_key(id_val);
        if let Some(err_obj) = v.get("error").filter(|e| !e.is_null()) {
            let message = err_obj
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("codex JSON-RPC error")
                .to_owned();
            if !self.established {
                self.pre_error.get_or_insert(message.clone());
                self.pre_session_failed = true;
                self.turn_open = false;
                return;
            }
            self.error_emitted = true;
            out.push(AgentEvent::Error {
                message: message.clone(),
                fatal: false,
                details: Vec::new(),
            });
            if id.starts_with("atrium-turn-") && self.turn_open {
                self.close_turn(TurnOutcome::Failed, out);
            }
            return;
        }

        let Some(result) = v.get("result") else {
            return;
        };

        // `initialize` result yields CLI version (`userAgent`), not SessionStarted yet.
        if id == "atrium-init" || result.get("userAgent").is_some() {
            if let Some(ua) = result.get("userAgent").and_then(Value::as_str)
                && let Some(ver) = parse_user_agent_version(ua)
            {
                self.cli_version = Some(ver);
            }
            return;
        }

        // `thread/start` / `thread/resume` response establishes the session.
        if let Some(thread) = result.get("thread") {
            let thread_id = thread
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let model = result
                .get("model")
                .and_then(Value::as_str)
                .or_else(|| thread.get("model").and_then(Value::as_str))
                .map(str::to_owned);
            let cli_ver = thread
                .get("cliVersion")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| self.cli_version.clone());
            self.cli_version = cli_ver.clone();
            if self.init.is_none() {
                self.init = Some(InitInfo {
                    thread_id,
                    model: model.clone(),
                    cli_version: cli_ver,
                });
            }
            self.establish(out);
            return;
        }

        // `turn/start` response gives provider turn id if not set yet.
        if let Some(tid) = result.pointer("/turn/id").and_then(Value::as_str)
            && self.turn_open
            && self.provider_turn_id.is_none()
        {
            self.provider_turn_id = Some(tid.to_owned());
        }
    }

    fn notification(
        &mut self,
        method: &str,
        params: &Value,
        raw: &Value,
        out: &mut Vec<AgentEvent>,
    ) {
        match method {
            "thread/started" => {
                if self.init.is_none()
                    && let Some(thread) = params.get("thread")
                {
                    let thread_id = thread
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    let model = thread
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let cli_ver = thread
                        .get("cliVersion")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| self.cli_version.clone());
                    self.init = Some(InitInfo {
                        thread_id,
                        model,
                        cli_version: cli_ver,
                    });
                }
                self.establish(out);
            }
            "turn/started" => {
                self.establish(out);
                let tid = params
                    .pointer("/turn/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if self.turn_open {
                    if let Some(id) = tid
                        && self.provider_turn_id.is_none()
                    {
                        self.provider_turn_id = Some(id);
                    }
                } else {
                    self.begin_turn(out);
                    self.provider_turn_id = tid;
                }
            }
            "turn/completed" => {
                self.turn_completed(params, out);
            }
            "item/started" => self.item_started(params, out),
            "item/completed" => self.item_completed(params, out),
            "item/agentMessage/delta" => {
                let Some(id) = str_of(params, "itemId") else {
                    return;
                };
                let delta = params
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !delta.is_empty() {
                    self.stream_delta(&id, MessageKind::Assistant, delta, out);
                }
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                let Some(id) = str_of(params, "itemId") else {
                    return;
                };
                let delta = params
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !delta.is_empty() {
                    self.stream_delta(&id, MessageKind::Reasoning, delta, out);
                }
            }
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
                let Some(id) = str_of(params, "itemId") else {
                    return;
                };
                let delta = params
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !delta.is_empty() {
                    self.tool_output_delta(&id, delta, out);
                }
            }
            "item/mcpToolCall/progress" => {
                let Some(id) = str_of(params, "itemId") else {
                    return;
                };
                let msg = params
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !msg.is_empty() {
                    self.tool_output_delta(&id, &format!("{msg}\n"), out);
                }
            }
            "serverRequest/resolved" => {
                if let Some(req_val) = params.get("requestId") {
                    self.server_request_resolved(req_val, out);
                }
            }
            "turn/plan/updated" => self.plan_updated(params, out),
            "thread/tokenUsage/updated" => self.token_usage_updated(params),
            "account/rateLimits/updated" => self.rate_limits_updated(params, out),
            "error" => self.error_notification(params, out),
            // Known Codex app-server lifecycle/housekeeping notifications that carry no M1 event.
            "thread/status/changed"
            | "thread/readState/changed"
            | "thread/goal/cleared"
            | "thread/name/updated"
            | "thread/archived"
            | "thread/unarchived"
            | "thread/closed"
            | "thread/compacted"
            | "account/updated"
            | "account/login/completed"
            | "remoteControl/status/changed"
            | "mcpServer/startupStatus/updated"
            | "mcpServer/oauthLogin/completed"
            | "turn/moderationMetadata"
            | "turn/diff/updated"
            | "item/reasoning/summaryPartAdded"
            | "item/commandExecution/terminalInteraction"
            | "deprecationNotice"
            | "configWarning"
            | "skills/changed" => {}
            _ => {
                self.emit_or_hold(
                    AgentEvent::Unmapped {
                        record_type: method.to_owned(),
                        raw: capped_raw(raw),
                    },
                    out,
                );
            }
        }
    }

    fn server_request(
        &mut self,
        id_val: &Value,
        method: &str,
        params: &Value,
        raw: &Value,
        out: &mut Vec<AgentEvent>,
    ) {
        let native = id_key(id_val);
        if method == "item/tool/requestUserInput" {
            let questions = parse_questions(params.get("questions"));
            self.question_reqs.insert(
                native,
                QuestionReq {
                    questions,
                    emitted: false,
                },
            );
            return;
        }

        let Some((name, item_id, mut payload, reason)) = parse_approval_request(method, params)
        else {
            self.emit_or_hold(
                AgentEvent::Unmapped {
                    record_type: method.to_owned(),
                    raw: capped_raw(raw),
                },
                out,
            );
            return;
        };

        if let Some(id) = item_id.as_deref()
            && let Some(item) = self.items.get(id)
            && !payload.get("item").is_some_and(|i| !i.is_null())
        {
            payload["item"] = item.clone();
        }

        if value_size(&payload) > MAX_PARTIAL_ARGS_BYTES {
            if let Some(id) = item_id {
                self.oversize_call(&id, &name, &payload, out);
            }
            return;
        }

        let kind = tools::tool_kind(&payload, &name);
        let title = tools::tool_title(&payload, &name);
        let key = gate_key(item_id.as_deref(), &name, &payload);

        self.server_reqs.insert(
            native.clone(),
            ServerReq {
                item_id,
                name,
                kind,
                title,
                input: payload,
                reason,
                key: key.clone(),
                emitted: false,
            },
        );

        if let Some(ticket) = self.take_asked(&key) {
            self.emit_approval(&native, ticket, out);
        }
    }

    fn gate(&mut self, check: &ToolCheck, decision: &GateDecision, out: &mut Vec<AgentEvent>) {
        let keys = candidate_keys(check);
        match decision {
            GateDecision::Allow => {}
            GateDecision::Deny { reason } => {
                self.server_reqs
                    .retain(|_, r| !r.emitted && !matches_req(r, check, &keys));
                if let Some(id) = &check.call_id {
                    self.deny_call(id, reason, out);
                }
            }
            GateDecision::Ask { ticket } => {
                let matching_native = self
                    .server_reqs
                    .iter()
                    .find(|(_, r)| !r.emitted && matches_req(r, check, &keys))
                    .map(|(native, _)| native.clone());
                match matching_native {
                    Some(native) => self.emit_approval(&native, ticket.clone(), out),
                    None => {
                        let key = gate_key(check.call_id.as_deref(), &check.name, &check.input);
                        self.asked.entry(key).or_default().push_back(ticket.clone());
                    }
                }
            }
        }
    }

    fn question_ticket(
        &mut self,
        native: &str,
        ticket: Option<&ApprovalTicket>,
        out: &mut Vec<AgentEvent>,
    ) {
        let Some(req) = self.question_reqs.get_mut(native) else {
            return;
        };
        if req.emitted {
            return;
        }
        let Some(ticket) = ticket else {
            self.question_reqs.remove(native);
            return;
        };
        req.emitted = true;
        let questions = req.questions.clone();
        self.establish(out);
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

    fn emit_approval(&mut self, native: &str, ticket: ApprovalTicket, out: &mut Vec<AgentEvent>) {
        let Some(req) = self.server_reqs.get_mut(native) else {
            return;
        };
        if req.emitted {
            return;
        }
        req.emitted = true;
        let req = req.clone();
        self.establish(out);
        self.ensure_turn(out);

        if let Some(id) = req.item_id.clone()
            && self.seen_calls.insert(id.clone())
        {
            out.push(AgentEvent::ToolCallStarted {
                call_id: id.clone(),
                name: req.name.clone(),
                kind: req.kind,
                title: req.title.clone(),
                input: req.input.clone(),
            });
            self.calls.push(OpenCall {
                id,
                bytes_emitted: 0,
                truncated_bytes: 0,
            });
        }

        out.push(AgentEvent::ApprovalRequested {
            request_id: ticket.request_id.clone(),
            call_id: req.item_id.clone(),
            name: req.name,
            kind: req.kind,
            title: req.title,
            input: req.input,
            reason: req.reason,
            rememberable: ticket.rememberable,
        });
        self.pending.push((
            ticket.request_id,
            Pending::Approval {
                call_id: req.item_id,
                native: native.to_owned(),
            },
        ));
    }

    fn take_asked(&mut self, key: &str) -> Option<ApprovalTicket> {
        let queue = self.asked.get_mut(key)?;
        let ticket = queue.pop_front();
        if queue.is_empty() {
            self.asked.remove(key);
        }
        ticket
    }

    fn server_request_resolved(&mut self, req_val: &Value, out: &mut Vec<AgentEvent>) {
        let native = id_key(req_val);
        self.server_reqs.remove(&native);
        self.question_reqs.remove(&native);

        let Some(pos) = self.pending.iter().position(|(_, p)| p.native() == native) else {
            return;
        };
        let (ticket_id, pending) = self.pending.remove(pos);

        if !self.established {
            return;
        }

        // Preserve closing order (`MessagesCompleted -> ToolCallsCancelled -> RequestsCancelled`)
        // when the provider resolves/cancels an open request due to an interrupt or turn stop.
        if self.interrupt_requested {
            self.close_messages(out);
            self.cancel_open_calls(out);
        } else if let Pending::Approval {
            call_id: Some(call_id),
            ..
        } = &pending
            && let Some(call_pos) = self.calls.iter().position(|c| &c.id == call_id)
        {
            let call = self.calls.remove(call_pos);
            self.done_calls.insert(call.id.clone());
            out.push(AgentEvent::ToolCallFinished {
                call_id: call.id,
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
                truncated_bytes: call.truncated_bytes,
            });
        }

        out.push(AgentEvent::RequestResolved {
            request_id: ticket_id,
            outcome: RequestOutcome::Cancelled,
        });
    }

    fn item_started(&mut self, params: &Value, out: &mut Vec<AgentEvent>) {
        let Some(item) = params.get("item") else {
            return;
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            return;
        };
        let id = str_of(item, "id").unwrap_or_default();
        if !id.is_empty() {
            self.items.insert(id.clone(), item.clone());
        }

        match item_type {
            "userMessage" | "reasoning" => {}
            "agentMessage" => {
                if !id.is_empty() && !self.done_msgs.contains(&id) {
                    self.establish(out);
                    self.ensure_turn(out);
                    if !self.open_msgs.iter().any(|m| m.id == id) {
                        self.open_msgs.push(OpenMsg {
                            id,
                            kind: MessageKind::Assistant,
                            text: String::new(),
                            deltas: 0,
                        });
                    }
                }
            }
            _ => {
                if id.is_empty() || !self.seen_calls.insert(id.clone()) {
                    return;
                }
                self.establish(out);
                self.ensure_turn(out);
                let kind = tools::tool_kind(item, item_type);
                let title = tools::tool_title(item, item_type);
                out.push(AgentEvent::ToolCallStarted {
                    call_id: id.clone(),
                    name: item_type.to_owned(),
                    kind,
                    title,
                    input: item.clone(),
                });
                if let Some(reason) = self.denied_early.remove(&id) {
                    self.done_calls.insert(id.clone());
                    out.push(AgentEvent::ToolCallFinished {
                        call_id: id,
                        status: ToolStatus::Denied,
                        output: None,
                        error: Some(reason),
                        truncated_bytes: 0,
                    });
                } else {
                    self.calls.push(OpenCall {
                        id,
                        bytes_emitted: 0,
                        truncated_bytes: 0,
                    });
                }
            }
        }
    }

    fn item_completed(&mut self, params: &Value, out: &mut Vec<AgentEvent>) {
        let Some(item) = params.get("item") else {
            return;
        };
        let Some(item_type) = item.get("type").and_then(Value::as_str) else {
            return;
        };
        let id = str_of(item, "id").unwrap_or_default();

        match item_type {
            "userMessage" => {}
            "agentMessage" => {
                if id.is_empty() || !self.done_msgs.insert(id.clone()) {
                    return;
                }
                self.establish(out);
                self.ensure_turn(out);
                if let Some(pos) = self.open_msgs.iter().position(|m| m.id == id) {
                    let msg = self.open_msgs.remove(pos);
                    if msg.deltas > 0 {
                        out.push(streamed_completion(msg));
                        return;
                    }
                }
                let text = str_of(item, "text").unwrap_or_default();
                out.extend(whole_message(id, MessageKind::Assistant, text));
            }
            "reasoning" => {
                if id.is_empty() || !self.done_msgs.insert(id.clone()) {
                    return;
                }
                if let Some(pos) = self.open_msgs.iter().position(|m| m.id == id) {
                    let msg = self.open_msgs.remove(pos);
                    self.establish(out);
                    self.ensure_turn(out);
                    if msg.deltas > 0 {
                        out.push(streamed_completion(msg));
                    } else {
                        let text = reasoning_text(item);
                        if !text.is_empty() {
                            out.extend(whole_message(id, MessageKind::Reasoning, text));
                        }
                    }
                    return;
                }
                let text = reasoning_text(item);
                if !text.is_empty() {
                    self.establish(out);
                    self.ensure_turn(out);
                    out.extend(whole_message(id, MessageKind::Reasoning, text));
                }
            }
            _ => {
                self.items.remove(&id);
                self.server_reqs
                    .retain(|_, r| r.item_id.as_deref() != Some(id.as_str()));
                if id.is_empty() || self.done_calls.contains(&id) {
                    return;
                }
                self.establish(out);
                self.ensure_turn(out);
                if self.seen_calls.insert(id.clone()) {
                    let kind = tools::tool_kind(item, item_type);
                    let title = tools::tool_title(item, item_type);
                    out.push(AgentEvent::ToolCallStarted {
                        call_id: id.clone(),
                        name: item_type.to_owned(),
                        kind,
                        title,
                        input: item.clone(),
                    });
                }

                let call = if let Some(pos) = self.calls.iter().position(|c| c.id == id) {
                    self.calls.remove(pos)
                } else {
                    OpenCall {
                        id: id.clone(),
                        bytes_emitted: 0,
                        truncated_bytes: 0,
                    }
                };

                let status = match item.get("status").and_then(Value::as_str) {
                    Some("failed") => ToolStatus::Failed,
                    Some("declined") => ToolStatus::Denied,
                    Some("cancelled" | "interrupted") => ToolStatus::Cancelled,
                    _ => ToolStatus::Succeeded,
                };
                let raw_output = extract_tool_output(item, item_type);
                let raw_error = extract_tool_error(item);
                let remaining = MAX_TOOL_OUTPUT_BYTES.saturating_sub(call.bytes_emitted);
                let (output, dropped_out) = match raw_output {
                    Some(s) if !s.is_empty() => {
                        let (kept, dropped) = cap_output(s, remaining);
                        (Some(kept), dropped)
                    }
                    _ => (None, 0),
                };
                let remaining_err =
                    remaining.saturating_sub(output.as_ref().map_or(0, String::len));
                let (error, dropped_err) = match raw_error {
                    Some(s) if !s.is_empty() => {
                        let (kept, dropped) = cap_output(s, remaining_err);
                        (Some(kept), dropped)
                    }
                    _ => (None, 0),
                };

                self.done_calls.insert(id.clone());
                out.push(AgentEvent::ToolCallFinished {
                    call_id: id,
                    status,
                    output,
                    error,
                    truncated_bytes: call.truncated_bytes + dropped_out + dropped_err,
                });
            }
        }
    }

    fn stream_delta(
        &mut self,
        id: &str,
        kind: MessageKind,
        delta: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        if self.done_msgs.contains(id) {
            return;
        }
        self.establish(out);
        self.ensure_turn(out);
        if !self.open_msgs.iter().any(|m| m.id == id) {
            self.open_msgs.push(OpenMsg {
                id: id.to_owned(),
                kind,
                text: String::new(),
                deltas: 0,
            });
        }
        for chunk in split_chunks(delta, MAX_INLINE_MESSAGE_BYTES) {
            if let Some(msg) = self.open_msgs.iter_mut().find(|m| m.id == id) {
                msg.text.push_str(chunk);
                msg.deltas += 1;
            }
            out.push(AgentEvent::MessageDelta {
                message_id: id.to_owned(),
                kind,
                text: chunk.to_owned(),
            });
        }
    }

    fn tool_output_delta(&mut self, id: &str, delta: &str, out: &mut Vec<AgentEvent>) {
        if self.done_calls.contains(id) {
            return;
        }
        let Some(call) = self.calls.iter_mut().find(|c| c.id == id) else {
            return;
        };
        let remaining = MAX_TOOL_OUTPUT_BYTES.saturating_sub(call.bytes_emitted);
        if remaining == 0 {
            call.truncated_bytes += delta.len() as u64;
            return;
        }
        let (kept, dropped) = cap_output(delta.to_owned(), remaining);
        call.bytes_emitted += kept.len();
        call.truncated_bytes += dropped;
        if !kept.is_empty() {
            out.push(AgentEvent::ToolCallUpdated {
                call_id: id.to_owned(),
                title: None,
                kind: None,
                input: None,
                output_delta: Some(kept),
                output: None,
            });
        }
    }

    fn oversize_call(&mut self, id: &str, name: &str, input: &Value, out: &mut Vec<AgentEvent>) {
        if self.done_calls.contains(id) {
            return;
        }
        let msg = format!("denied: tool arguments exceed {MAX_PARTIAL_ARGS_BYTES} bytes");
        self.establish(out);
        self.ensure_turn(out);
        if self.seen_calls.insert(id.to_owned()) {
            out.push(AgentEvent::ToolCallStarted {
                call_id: id.to_owned(),
                name: name.to_owned(),
                kind: tools::tool_kind(input, name),
                title: tools::tool_title(input, name),
                input: Value::Null,
            });
        } else if let Some(pos) = self.calls.iter().position(|c| c.id == id) {
            self.calls.remove(pos);
        }
        self.done_calls.insert(id.to_owned());
        out.push(AgentEvent::ToolCallFinished {
            call_id: id.to_owned(),
            status: ToolStatus::Denied,
            output: None,
            error: Some(msg),
            truncated_bytes: 0,
        });
    }

    fn deny_call(&mut self, id: &str, reason: &str, out: &mut Vec<AgentEvent>) {
        if self.done_calls.contains(id) {
            return;
        }
        if let Some(pos) = self.calls.iter().position(|c| c.id == id) {
            let call = self.calls.remove(pos);
            self.done_calls.insert(id.to_owned());
            out.push(AgentEvent::ToolCallFinished {
                call_id: id.to_owned(),
                status: ToolStatus::Denied,
                output: None,
                error: Some(reason.to_owned()),
                truncated_bytes: call.truncated_bytes,
            });
        } else if !self.seen_calls.contains(id) {
            self.denied_early.insert(id.to_owned(), reason.to_owned());
        }
    }

    fn plan_updated(&mut self, params: &Value, out: &mut Vec<AgentEvent>) {
        let Some(arr) = params.get("plan").and_then(Value::as_array) else {
            return;
        };
        let entries: Vec<PlanEntry> = arr
            .iter()
            .filter_map(|e| {
                let step = e.get("step").and_then(Value::as_str)?.to_owned();
                let status = match e.get("status").and_then(Value::as_str) {
                    Some("completed") => PlanStatus::Completed,
                    Some("inProgress" | "in_progress") => PlanStatus::InProgress,
                    _ => PlanStatus::Pending,
                };
                Some(PlanEntry {
                    content: step,
                    status,
                })
            })
            .collect();
        self.establish(out);
        self.ensure_turn(out);
        out.push(AgentEvent::PlanUpdated { entries });
    }

    fn token_usage_updated(&mut self, params: &Value) {
        let Some(tu) = params.get("tokenUsage") else {
            return;
        };
        let totals = Totals {
            input_tokens: tu
                .pointer("/total/inputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cached_input_tokens: tu
                .pointer("/total/cachedInputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            output_tokens: tu
                .pointer("/total/outputTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        };
        let msg_turn = params.get("turnId").and_then(Value::as_str);
        let for_other_turn = msg_turn.is_some_and(|tid| {
            self.provider_turn_id
                .as_deref()
                .is_some_and(|active| active != tid)
        });
        if !self.turn_open || for_other_turn {
            // Historical token snapshot (e.g. on thread/resume): seed baseline so Turn 1
            // only counts delta tokens.
            self.prev_totals = totals;
            return;
        }
        self.turn_totals = Some(totals);
        if let (Some(used), Some(window)) = (
            tu.pointer("/last/totalTokens").and_then(Value::as_u64),
            tu.get("modelContextWindow").and_then(Value::as_u64),
        ) {
            self.turn_context = Some((used, window));
        }
    }

    fn rate_limits_updated(&mut self, params: &Value, out: &mut Vec<AgentEvent>) {
        let Some(rl) = params.get("rateLimits") else {
            return;
        };
        let mut windows = Vec::new();
        for (name, key) in [("5h", "primary"), ("7d", "secondary")] {
            if let Some(w) = rl.get(key).filter(|v| v.is_object())
                && let Some(pct) = w.get("usedPercent").and_then(Value::as_f64)
            {
                let resets_at_ms = w
                    .get("resetsAt")
                    .and_then(Value::as_u64)
                    .map(|s| s.saturating_mul(1000));
                if let Some(r) = resets_at_ms {
                    self.last_reset_ms = Some(r);
                }
                windows.push(QuotaWindow {
                    name: name.to_owned(),
                    used_percent: Some(pct),
                    resets_at_ms,
                });
            }
        }
        if !windows.is_empty() && self.last_quota.as_ref() != Some(&windows) {
            self.last_quota = Some(windows.clone());
            self.emit_or_hold(AgentEvent::QuotaUpdated { windows }, out);
        }
    }

    fn error_notification(&mut self, params: &Value, out: &mut Vec<AgentEvent>) {
        let will_retry = params
            .get("willRetry")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if will_retry {
            return;
        }
        let message = params
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap_or("provider error")
            .to_owned();
        let err_info = params.pointer("/error/codexErrorInfo");
        let is_rate_limit = err_info
            .and_then(Value::as_str)
            .is_some_and(|s| s == "usageLimitExceeded")
            || message.to_lowercase().contains("usage limit")
            || message.to_lowercase().contains("rate limit");

        if !self.established {
            self.pre_error.get_or_insert(message);
            self.pre_session_failed = true;
            self.turn_open = false;
            return;
        }
        self.establish(out);
        self.ensure_turn(out);
        if is_rate_limit {
            self.rate_limited = true;
            out.push(AgentEvent::RateLimited {
                resets_at_ms: self.last_reset_ms,
                message: Some(message.clone()),
            });
        }
        self.error_emitted = true;
        out.push(AgentEvent::Error {
            message,
            fatal: false,
            details: Vec::new(),
        });
    }

    fn turn_completed(&mut self, params: &Value, out: &mut Vec<AgentEvent>) {
        let turn_obj = &params["turn"];
        let status = turn_obj
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("completed");
        if !self.established {
            if status == "failed" {
                let msg = turn_obj
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("codex turn failed before the session started")
                    .to_owned();
                self.pre_error.get_or_insert(msg);
                self.pre_session_failed = true;
                self.turn_open = false;
                return;
            }
            self.establish(out);
        }
        self.ensure_turn(out);

        let outcome = if status == "interrupted" || self.interrupt_requested {
            TurnOutcome::Interrupted
        } else if status == "failed" {
            if !self.error_emitted && !self.rate_limited {
                let msg = turn_obj
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("turn failed")
                    .to_owned();
                out.push(AgentEvent::Error {
                    message: msg,
                    fatal: false,
                    details: Vec::new(),
                });
            }
            TurnOutcome::Failed
        } else {
            TurnOutcome::Completed
        };

        self.close_turn(outcome, out);
    }

    fn close_turn(&mut self, outcome: TurnOutcome, out: &mut Vec<AgentEvent>) {
        self.close_messages(out);
        self.cancel_open_calls(out);
        self.cancel_pending(out);

        if let Some((used, window)) = self.turn_context.take() {
            out.push(AgentEvent::ContextUpdated {
                used_tokens: used,
                window_tokens: Some(window),
            });
        }
        let usage = self.consume_turn_usage();
        if let Some(u) = &usage {
            out.push(AgentEvent::Usage { usage: u.clone() });
        }
        out.push(AgentEvent::TurnCompleted {
            turn: self.turn,
            outcome,
            usage,
        });
        self.turn_open = false;
        self.provider_turn_id = None;
        self.interrupt_requested = false;
        self.rate_limited = false;
        self.error_emitted = false;
    }

    fn consume_turn_usage(&mut self) -> Option<Usage> {
        let totals = self.turn_totals.take()?;
        let input_tokens = totals
            .input_tokens
            .saturating_sub(self.prev_totals.input_tokens);
        let cache_read_tokens = totals
            .cached_input_tokens
            .saturating_sub(self.prev_totals.cached_input_tokens);
        let output_tokens = totals
            .output_tokens
            .saturating_sub(self.prev_totals.output_tokens);
        self.prev_totals = totals;
        Some(Usage {
            input_tokens: Some(input_tokens),
            output_tokens: Some(output_tokens),
            cache_read_tokens: Some(cache_read_tokens),
            cache_write_tokens: None,
            reasoning_tokens: None,
            cost_usd: None,
        })
    }

    fn begin_turn(&mut self, out: &mut Vec<AgentEvent>) {
        self.turn += 1;
        self.turn_open = true;
        self.ever_had_turn = true;
        self.turn_start_emitted = false;
        self.provider_turn_id = None;
        self.turn_totals = None;
        self.turn_context = None;
        self.rate_limited = false;
        self.error_emitted = false;
        self.interrupt_requested = false;
        if self.established {
            self.start_turn_events(out);
        }
    }

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

    fn establish(&mut self, out: &mut Vec<AgentEvent>) {
        if self.established {
            return;
        }
        let init = self.init.get_or_insert_with(|| InitInfo {
            thread_id: String::new(),
            model: None,
            cli_version: self.cli_version.clone(),
        });
        let ver = init
            .cli_version
            .clone()
            .or_else(|| self.cli_version.clone());
        let enforcement = self
            .first_enforcement
            .clone()
            .unwrap_or_else(|| default_enforcement(ver));
        let changed = self
            .enforcement
            .clone()
            .filter(|current| *current != enforcement);

        self.established = true;
        out.push(AgentEvent::SessionStarted {
            provider: Provider::Codex,
            native_session_id: init.thread_id.clone(),
            model: init.model.clone(),
            tools: Vec::new(),
            enforcement,
        });
        if let Some(enforcement) = changed {
            out.push(AgentEvent::EnforcementChanged { enforcement });
        }
        if self.turn_open && !self.turn_start_emitted {
            self.start_turn_events(out);
        } else if !self.turn_open && self.held.iter().any(needs_turn) {
            self.ensure_turn(out);
        }
        out.append(&mut self.held);
    }

    fn ensure_turn(&mut self, out: &mut Vec<AgentEvent>) {
        if !self.turn_open && self.established {
            self.turn += 1;
            self.turn_open = true;
            self.ever_had_turn = true;
            self.turn_start_emitted = true;
            self.provider_turn_id = None;
            self.turn_totals = None;
            self.turn_context = None;
            self.rate_limited = false;
            self.error_emitted = false;
            self.start_turn_events(out);
        }
    }

    fn emit_or_hold(&mut self, event: AgentEvent, out: &mut Vec<AgentEvent>) {
        if self.established {
            out.push(event);
        } else {
            self.held.push(event);
            if self.held.len() > MAX_DIAGNOSTIC_LINES {
                let extra = self.held.len() - MAX_DIAGNOSTIC_LINES;
                self.held.drain(..extra);
            }
        }
    }

    fn command(&mut self, command: &SessionCommand, out: &mut Vec<AgentEvent>) {
        match command {
            SessionCommand::SendUserMessage { text } => {
                let busy = self.turn_open || self.waiting > 0;
                if busy && self.waiting >= MAX_QUEUED_MESSAGES {
                    self.rejected = true;
                    out.push(AgentEvent::Error {
                        message: format!(
                            "message rejected: {MAX_QUEUED_MESSAGES} messages are already waiting"
                        ),
                        fatal: false,
                        details: Vec::new(),
                    });
                } else {
                    self.waiting += 1;
                    if !self.turn_open && self.user_text.is_none() {
                        self.user_text = Some(text.clone());
                    }
                }
            }
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => match self.take_pending(request_id, true) {
                Some(Pending::Approval { call_id, native }) => {
                    self.server_reqs.remove(&native);
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
                _ => out.push(AgentEvent::Error {
                    message: format!("unknown request: {request_id}"),
                    fatal: false,
                    details: Vec::new(),
                }),
            },
            SessionCommand::AnswerQuestion { request_id, .. } => {
                match self.take_pending(request_id, false) {
                    Some(Pending::Question { native }) => {
                        self.question_reqs.remove(&native);
                        out.push(AgentEvent::RequestResolved {
                            request_id: request_id.clone(),
                            outcome: RequestOutcome::Answered,
                        });
                    }
                    _ => out.push(AgentEvent::Error {
                        message: format!("unknown request: {request_id}"),
                        fatal: false,
                        details: Vec::new(),
                    }),
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

    fn take_pending(&mut self, rid: &str, approval: bool) -> Option<Pending> {
        let pos = self
            .pending
            .iter()
            .position(|p| p.0 == rid && matches!(p.1, Pending::Approval { .. }) == approval)?;
        Some(self.pending.remove(pos).1)
    }

    fn close_messages(&mut self, out: &mut Vec<AgentEvent>) {
        for m in std::mem::take(&mut self.open_msgs) {
            self.done_msgs.insert(m.id.clone());
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
                truncated_bytes: c.truncated_bytes,
            });
        }
    }

    fn cancel_pending(&mut self, out: &mut Vec<AgentEvent>) {
        for (rid, _) in std::mem::take(&mut self.pending) {
            if self.established {
                out.push(AgentEvent::RequestResolved {
                    request_id: rid,
                    outcome: RequestOutcome::Cancelled,
                });
            }
        }
        self.server_reqs.clear();
        self.question_reqs.clear();
        self.asked.clear();
    }

    fn fatal(&mut self, message: String, out: &mut Vec<AgentEvent>) {
        if self.established {
            if self.end_reason.is_none() {
                out.push(AgentEvent::Error {
                    message: message.clone(),
                    fatal: true,
                    details: Vec::new(),
                });
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
                self.establish(out);
            } else {
                let message = self.pre_error.clone().unwrap_or_else(|| {
                    format!("codex exited{} before the session started", exit_text(code))
                });
                let details = self.diagnostics();
                out.push(AgentEvent::Error {
                    message,
                    fatal: true,
                    details,
                });
                self.held.clear();
                self.finish_session(code, out);
                return;
            }
        }

        if self.turn_open && !requested {
            let details = self.diagnostics();
            out.push(AgentEvent::Error {
                message: format!("codex exited{} during a turn", exit_text(code)),
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

    fn finish_session(&mut self, code: Option<i32>, out: &mut Vec<AgentEvent>) {
        self.cancel_pending(out);
        self.ended = true;
        let reason = if self.pre_session_failed || self.pre_error.is_some() {
            Some("provider error".to_owned())
        } else if let Some(r) = &self.end_reason {
            Some(r.clone())
        } else if self.shutdown_requested {
            Some("shutdown".to_owned())
        } else if self.interrupt_requested {
            Some("codex exited after an interrupt".to_owned())
        } else {
            code.map(|c| format!("codex exited with code {c}"))
        };
        out.push(AgentEvent::SessionEnded {
            exit_code: code,
            reason,
        });
    }

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

impl Normalizer for CodexNormalizer {
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
                if let GateDecision::Deny { reason } = decision
                    && let Some(id) = &check.call_id
                {
                    self.deny_call(id, reason, &mut out);
                }
            }
            AdapterInput::QuestionTicket { id, ticket } => {
                self.question_ticket(id, ticket.as_ref(), &mut out);
            }
            AdapterInput::Enforcement { descriptor } => {
                if self.established && self.enforcement.as_ref() != Some(descriptor) {
                    out.push(AgentEvent::EnforcementChanged {
                        enforcement: descriptor.clone(),
                    });
                }
                self.first_enforcement
                    .get_or_insert_with(|| descriptor.clone());
                self.enforcement = Some(descriptor.clone());
            }
            AdapterInput::WriteFailed { error } => {
                self.fatal(format!("writing to codex failed: {error}"), &mut out);
            }
            AdapterInput::Deadline { what } => self.deadline(what, &mut out),
            AdapterInput::Exit { code } => self.exit(*code, &mut out),
            AdapterInput::HookRequest { .. }
            | AdapterInput::HookResponse { .. }
            | AdapterInput::HookDisconnected { .. } => {}
        }
        out
    }
}

// ---- helpers ----

fn whole_message(id: String, kind: MessageKind, text: String) -> Vec<AgentEvent> {
    if text.len() <= MAX_INLINE_MESSAGE_BYTES {
        return vec![AgentEvent::MessageCompleted {
            message_id: id,
            kind,
            body: MessageBody::Inline { text },
        }];
    }
    let mut events = Vec::new();
    let mut deltas = 0u64;
    for chunk in split_chunks(&text, MAX_INLINE_MESSAGE_BYTES) {
        deltas += 1;
        events.push(AgentEvent::MessageDelta {
            message_id: id.clone(),
            kind,
            text: chunk.to_owned(),
        });
    }
    let body = MessageBody::Streamed {
        chars: text.chars().count() as u64,
        deltas,
        sha256: sha256_hex(text.as_bytes()),
    };
    events.push(AgentEvent::MessageCompleted {
        message_id: id,
        kind,
        body,
    });
    events
}

fn streamed_completion(msg: OpenMsg) -> AgentEvent {
    let body = MessageBody::Streamed {
        chars: msg.text.chars().count() as u64,
        deltas: msg.deltas,
        sha256: sha256_hex(msg.text.as_bytes()),
    };
    AgentEvent::MessageCompleted {
        message_id: msg.id,
        kind: msg.kind,
        body,
    }
}

fn split_chunks(text: &str, max_bytes: usize) -> Vec<&str> {
    if text.is_empty() || text.len() <= max_bytes {
        return vec![text];
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + max_bytes).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            // Single character exceeds max_bytes (impossible with 64 KiB cap, defensive).
            end = text.len();
        }
        out.push(&text[start..end]);
        start = end;
    }
    out
}

fn cap_output(s: String, max_bytes: usize) -> (String, u64) {
    if s.len() <= max_bytes {
        return (s, 0);
    }
    let mut end = max_bytes;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = (s.len() - end) as u64;
    (s[..end].to_owned(), dropped)
}

fn capped_raw(v: &Value) -> Value {
    let s = v.to_string();
    if s.len() <= MAX_UNMAPPED_RAW_BYTES {
        return v.clone();
    }
    let (prefix, _) = cap_output(s, MAX_UNMAPPED_RAW_BYTES);
    json!({"truncated": prefix, "originalBytes": v.to_string().len()})
}

fn diag_line(s: &str) -> String {
    let clean = s.replace(['\r', '\n'], " ");
    let (kept, _) = cap_output(clean, MAX_DIAGNOSTIC_LINE_BYTES);
    kept
}

fn id_key(v: &Value) -> String {
    match v.as_str() {
        Some(s) => s.to_owned(),
        None => v.to_string(),
    }
}

fn str_of(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn parse_user_agent_version(ua: &str) -> Option<String> {
    let first = ua.split_whitespace().next()?;
    let after_slash = first.split_once('/').map_or(first, |(_, v)| v);
    let cleaned = after_slash.trim_start_matches('v');
    let parts: Vec<_> = cleaned.split('.').collect();
    if parts.len() >= 2
        && parts
            .iter()
            .take(2)
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        Some(cleaned.to_owned())
    } else {
        None
    }
}

fn value_size(v: &Value) -> usize {
    serde_json::to_string(v).map(|s| s.len()).unwrap_or(0)
}

fn gate_key(call_id: Option<&str>, name: &str, input: &Value) -> String {
    match call_id {
        Some(id) if !id.is_empty() => format!("id:{id}:{name}"),
        _ => format!("anon:{name}:{}", sha256_hex(input.to_string().as_bytes())),
    }
}

fn candidate_keys(check: &ToolCheck) -> Vec<String> {
    let mut keys = Vec::new();
    keys.push(gate_key(
        check.call_id.as_deref(),
        &check.name,
        &check.input,
    ));
    if let Some(id) = &check.call_id
        && !id.is_empty()
    {
        keys.push(format!("call:{id}"));
    }
    keys
}

fn matches_req(req: &ServerReq, check: &ToolCheck, keys: &[String]) -> bool {
    if let (Some(req_id), Some(check_id)) = (&req.item_id, &check.call_id)
        && !req_id.is_empty()
        && req_id == check_id
    {
        return true;
    }
    keys.iter().any(|k| k == &req.key)
}

fn parse_approval_request(
    method: &str,
    params: &Value,
) -> Option<(String, Option<String>, Value, Option<String>)> {
    let (name, id_key) = match method {
        "item/commandExecution/requestApproval" => ("commandExecution", "itemId"),
        "item/fileChange/requestApproval" => ("fileChange", "itemId"),
        "item/permissions/requestApproval" => ("item/permissions/requestApproval", "itemId"),
        "mcpServer/elicitation/request" => ("mcpServer/elicitation/request", "elicitationId"),
        "execCommandApproval" => ("execCommandApproval", "callId"),
        "applyPatchApproval" => ("applyPatchApproval", "callId"),
        _ => return None,
    };
    let item_id = str_of(params, id_key).or_else(|| str_of(params, "itemId"));
    let reason = str_of(params, "reason").or_else(|| str_of(params, "message"));
    Some((name.to_owned(), item_id, params.clone(), reason))
}

fn parse_questions(val: Option<&Value>) -> Vec<Question> {
    let Some(arr) = val.and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|q| {
            let id = q.get("id").and_then(Value::as_str)?.to_owned();
            let prompt = q
                .get("question")
                .or_else(|| q.get("prompt"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let options = q
                .get("options")
                .and_then(Value::as_array)
                .map(|opts| {
                    opts.iter()
                        .enumerate()
                        .filter_map(|(opt_idx, o)| {
                            let label = o.get("label").and_then(Value::as_str)?.to_owned();
                            let opt_id = o
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                                .unwrap_or_else(|| opt_idx.to_string());
                            Some(QuestionOption {
                                id: opt_id,
                                label,
                                description: o
                                    .get("description")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            let multi_select = q
                .get("multiSelect")
                .or_else(|| q.get("multi_select"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let allow_text = q
                .get("isOther")
                .or_else(|| q.get("allowOther"))
                .or_else(|| q.get("allowText"))
                .or_else(|| q.get("allow_text"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(Question {
                id,
                prompt,
                options,
                multi_select,
                allow_text,
            })
        })
        .collect()
}

fn reasoning_text(item: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["summary", "content"] {
        if let Some(arr) = item.get(key).and_then(Value::as_array) {
            for entry in arr {
                if let Some(s) = entry.as_str()
                    && !s.is_empty()
                {
                    parts.push(s);
                } else if let Some(s) = entry.get("text").and_then(Value::as_str)
                    && !s.is_empty()
                {
                    parts.push(s);
                }
            }
        }
    }
    parts.join("\n")
}

fn extract_tool_output(item: &Value, item_type: &str) -> Option<String> {
    match item_type {
        "commandExecution" | "execCommandApproval" => {
            str_of(item, "aggregatedOutput").or_else(|| str_of(item, "formattedOutput"))
        }
        "mcpToolCall" => {
            if let Some(res) = item.get("result").filter(|v| !v.is_null()) {
                if let Some(s) = res.as_str() {
                    return Some(s.to_owned());
                }
                return Some(res.to_string());
            }
            None
        }
        "dynamicToolCall" => item
            .get("contentItems")
            .filter(|v| !v.is_null())
            .map(|v| v.to_string()),
        _ => None,
    }
}

fn extract_tool_error(item: &Value) -> Option<String> {
    if let Some(err) = item.get("error").filter(|e| !e.is_null()) {
        if let Some(s) = err.as_str()
            && !s.is_empty()
        {
            return Some(s.to_owned());
        }
        if let Some(s) = err.get("message").and_then(Value::as_str)
            && !s.is_empty()
        {
            return Some(s.to_owned());
        }
        return Some(err.to_string());
    }
    None
}

fn needs_turn(ev: &AgentEvent) -> bool {
    matches!(
        ev,
        AgentEvent::QuotaUpdated { .. }
            | AgentEvent::PlanUpdated { .. }
            | AgentEvent::ContextUpdated { .. }
            | AgentEvent::Usage { .. }
            | AgentEvent::RateLimited { .. }
    )
}

fn exit_text(code: Option<i32>) -> String {
    match code {
        Some(c) => format!(" with code {c}"),
        None => " from a signal".into(),
    }
}
