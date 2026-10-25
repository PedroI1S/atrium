use std::collections::{BTreeMap, HashMap, HashSet};

// Pure normalizer mapping `agy` stream-json records and hook bridge inputs
// into `atrium_protocol::agent::AgentEvent`s.

use atrium_agents::{AdapterInput, GateDecision, Normalizer, ToolCheck};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageKind, PlanEntry, PlanStatus, Provider, Question,
    QuestionOption, QuotaWindow, RequestOutcome, SessionCommand, ToolKind, ToolStatus, TurnOutcome,
    Usage,
};
use serde_json::{Map, Value};

/// Classifies an `agy` tool name into Atrium's coarse [`ToolKind`].
pub fn classify_tool(name: &str) -> ToolKind {
    if name == "call_mcp_tool" || name.starts_with("mcp__") || name.starts_with("mcp_") {
        return ToolKind::Mcp;
    }
    match name {
        "view_file"
        | "read_file"
        | "list_dir"
        | "list_resources"
        | "read_resource"
        | "list_permissions"
        | "list_plugin_accounts"
        | "list_browser_pages"
        | "read_browser_page"
        | "browser_get_dom"
        | "browser_get_network_request"
        | "browser_list_network_requests"
        | "capture_browser_console_logs"
        | "capture_browser_screenshot"
        | "command_status"
        | "manage_inbox" => ToolKind::Read,
        "write_to_file"
        | "write_file"
        | "create_file"
        | "edit_file"
        | "replace_file_content"
        | "multi_replace_file_content"
        | "sed_file"
        | "notebook_edit"
        | "generate_image" => ToolKind::Edit,
        "delete_knowledge" | "delete_file" | "rm_file" => ToolKind::Delete,
        "move_file" | "rename_file" => ToolKind::Move,
        "find_by_name" | "grep_search" | "search_marketplace" => ToolKind::Search,
        "run_command"
        | "send_command_input"
        | "notebook_execution"
        | "run_workflow"
        | "execute_browser_javascript" => ToolKind::Execute,
        "search_web" | "read_url_content" | "open_browser_url" => ToolKind::Fetch,
        "think" => ToolKind::Think,
        _ => ToolKind::Other,
    }
}

/// Extracts a concise human-readable summary from an `agy` tool call's parameters.
pub fn extract_tool_title(name: &str, params: &Value) -> Option<String> {
    let obj = params.as_object()?;
    let keys: &[&str] = match name {
        "run_command" => &["CommandLine", "command", "toolSummary"],
        "call_mcp_tool" => &["tool_name", "name", "server_name"],
        _ => &[
            "Path",
            "TargetFile",
            "DirectoryPath",
            "SearchPath",
            "CommandLine",
            "command",
            "Query",
            "query",
            "Pattern",
            "Url",
            "url",
            "Prompt",
            "path",
            "file_path",
        ],
    };
    for key in keys {
        if let Some(s) = obj.get(*key).and_then(Value::as_str)
            && !s.is_empty()
        {
            return Some(s.to_owned());
        }
    }
    None
}

/// Produces a canonical JSON string with all object keys sorted recursively,
/// independent of `serde_json`'s `preserve_order` feature.
pub fn canonical_json(value: &Value) -> String {
    canonicalize_value(value).to_string()
}

pub fn canonicalize_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: BTreeMap<&String, Value> = map
                .iter()
                .map(|(k, v)| (k, canonicalize_value(v)))
                .collect();
            let mut out = Map::with_capacity(sorted.len());
            for (k, v) in sorted {
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_value).collect()),
        other => other.clone(),
    }
}

#[derive(Debug, Clone)]
struct OpenMessage {
    step_index: u64,
    message_id: String,
    kind: MessageKind,
    text: String,
}

#[derive(Debug, Clone)]
struct OpenToolCall {
    step_index: u64,
    call_id: String,
    name: String,
    denied_reason: Option<String>,
}

#[derive(Debug, Clone)]
enum PendingRequestKind {
    Approval { call_id: Option<String> },
    Question,
}

#[derive(Debug, Clone)]
struct PendingRequest {
    request_id: String,
    kind: PendingRequestKind,
}

#[derive(Debug, Clone)]
struct PendingGateDenial {
    check: ToolCheck,
    reason: String,
    matched_step: bool,
}

/// Stateful, pure normalizer for `agy` sessions.
#[derive(Debug, Default)]
pub struct AgyNormalizer {
    session_started: bool,
    emitted_fatal_error: bool,
    startup_stderr: Vec<String>,
    in_turn: bool,
    current_turn: u32,
    turn_interrupted: bool,
    turn_usage: Option<Usage>,
    turn_emitted_assistant_message: bool,
    open_messages: Vec<OpenMessage>,
    open_tool_calls: Vec<OpenToolCall>,
    used_call_ids: HashSet<String>,
    used_message_ids: HashSet<String>,
    pending_requests: Vec<PendingRequest>,
    last_hook_request_id: Option<String>,
    next_request_seq: u64,
    next_synthetic_call_seq: u64,
    pending_gate_denials: Vec<PendingGateDenial>,
    turn_denied_tools: Vec<String>,
}

impl AgyNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_turn_started(&mut self, out: &mut Vec<AgentEvent>) {
        if self.session_started && !self.in_turn {
            self.current_turn += 1;
            self.in_turn = true;
            self.turn_interrupted = false;
            self.turn_usage = None;
            self.turn_emitted_assistant_message = false;
            self.pending_gate_denials.clear();
            self.turn_denied_tools.clear();
            out.push(AgentEvent::TurnStarted {
                turn: self.current_turn,
            });
        }
    }

    fn alloc_call_id(&mut self, step_index: u64) -> String {
        let base = format!("step-{step_index}");
        if self.used_call_ids.insert(base.clone()) {
            return base;
        }
        let mut suffix = 2u32;
        loop {
            let candidate = format!("step-{step_index}-{suffix}");
            if self.used_call_ids.insert(candidate.clone()) {
                return candidate;
            }
            suffix += 1;
        }
    }

    fn alloc_synthetic_call_id(&mut self) -> String {
        self.next_synthetic_call_seq += 1;
        let id = format!("call-denied-{}", self.next_synthetic_call_seq);
        self.used_call_ids.insert(id.clone());
        id
    }

    fn alloc_message_id(&mut self, step_index: u64, kind: MessageKind) -> String {
        let base = match kind {
            MessageKind::Assistant => format!("msg-{step_index}"),
            MessageKind::Reasoning => format!("msg-{step_index}-reasoning"),
        };
        if self.used_message_ids.insert(base.clone()) {
            return base;
        }
        let mut suffix = 2u32;
        loop {
            let candidate = format!("{base}-{suffix}");
            if self.used_message_ids.insert(candidate.clone()) {
                return candidate;
            }
            suffix += 1;
        }
    }

    fn alloc_request_id(&mut self) -> String {
        if let Some(id) = self.last_hook_request_id.take() {
            return id;
        }
        self.next_request_seq += 1;
        format!("req-{}", self.next_request_seq)
    }

    fn current_open_call_id(&self, tool_name: &str) -> Option<String> {
        self.open_tool_calls
            .iter()
            .rev()
            .find(|c| c.name == tool_name)
            .or_else(|| self.open_tool_calls.last())
            .map(|c| c.call_id.clone())
    }

    fn handle_stdout_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            if !self.session_started {
                self.startup_stderr.push(trimmed.to_owned());
                return Vec::new();
            }
            return vec![AgentEvent::Unmapped {
                record_type: "non_json_stdout".into(),
                raw: Value::String(trimmed.to_owned()),
            }];
        };

        let event_type = val
            .get("event")
            .or_else(|| val.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");

        match event_type {
            "init" => self.handle_init(&val),
            "step_update" => self.handle_step_update(&val),
            "result" => self.handle_result(&val),
            "error" => self.handle_error_record(&val),
            "quota" | "quota_update" => self.handle_quota_record(&val),
            "rate_limit" | "rate_limited" => self.handle_rate_limit_record(&val),
            other => {
                if !self.session_started
                    && let Some(msg) = val
                        .get("message")
                        .or_else(|| val.get("error").and_then(|e| e.get("message")))
                        .and_then(Value::as_str)
                {
                    self.emitted_fatal_error = true;
                    return vec![AgentEvent::Error {
                        message: msg.to_owned(),
                        fatal: true,
                    }];
                }
                vec![AgentEvent::Unmapped {
                    record_type: other.to_owned(),
                    raw: val,
                }]
            }
        }
    }

    fn handle_init(&mut self, val: &Value) -> Vec<AgentEvent> {
        if self.session_started {
            return Vec::new();
        }
        let init_obj = val.get("init").unwrap_or(val);
        let Some(conversation_id) = val
            .get("conversation_id")
            .or_else(|| init_obj.get("conversation_id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
        else {
            self.emitted_fatal_error = true;
            return vec![AgentEvent::Error {
                message: "agy init record missing conversation_id".into(),
                fatal: true,
            }];
        };
        let model = init_obj
            .get("model")
            .or_else(|| val.get("model"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let tools = init_obj
            .get("tools")
            .or_else(|| val.get("tools"))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        self.session_started = true;
        vec![AgentEvent::SessionStarted {
            provider: Provider::Agy,
            native_session_id: conversation_id.to_owned(),
            model,
            tools,
        }]
    }

    fn handle_step_update(&mut self, val: &Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        let Some(step) = val.get("step_update") else {
            out.push(AgentEvent::Unmapped {
                record_type: "step_update".into(),
                raw: val.clone(),
            });
            return out;
        };

        let step_index = step.get("step_index").and_then(Value::as_u64).unwrap_or(0);
        let state = step.get("state").and_then(Value::as_str).unwrap_or("");
        let step_type = step.get("step_type").and_then(Value::as_str).unwrap_or("");

        match step_type {
            "user_input" => {
                self.ensure_turn_started(&mut out);
            }
            "agent_response" | "thinking" | "reasoning" => {
                self.ensure_turn_started(&mut out);
                self.handle_agent_response_step(step, step_index, state, step_type, &mut out);
            }
            "tool" => {
                self.ensure_turn_started(&mut out);
                self.handle_tool_step(step, step_index, state, &mut out);
            }
            "plan" => {
                self.ensure_turn_started(&mut out);
                if let Some(entries) = parse_plan_entries(step) {
                    out.push(AgentEvent::PlanUpdated { entries });
                }
            }
            other => {
                out.push(AgentEvent::Unmapped {
                    record_type: format!("step_update:{other}"),
                    raw: val.clone(),
                });
            }
        }
        out
    }

    fn handle_agent_response_step(
        &mut self,
        step: &Value,
        step_index: u64,
        state: &str,
        step_type: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let reasoning_delta = step
            .get("thinking_delta")
            .or_else(|| step.get("reasoning_delta"))
            .and_then(Value::as_str)
            .or_else(|| {
                if step_type == "thinking" || step_type == "reasoning" {
                    step.get("text_delta").and_then(Value::as_str)
                } else {
                    None
                }
            });

        if let Some(delta) = reasoning_delta
            && !delta.is_empty()
        {
            let msg_id = self.append_message_delta(step_index, MessageKind::Reasoning, delta);
            out.push(AgentEvent::MessageDelta {
                message_id: msg_id,
                kind: MessageKind::Reasoning,
                text: delta.to_owned(),
            });
        }

        if step_type != "thinking"
            && step_type != "reasoning"
            && let Some(delta) = step.get("text_delta").and_then(Value::as_str)
            && !delta.is_empty()
        {
            let msg_id = self.append_message_delta(step_index, MessageKind::Assistant, delta);
            out.push(AgentEvent::MessageDelta {
                message_id: msg_id,
                kind: MessageKind::Assistant,
                text: delta.to_owned(),
            });
        }

        let is_terminal = matches!(state, "DONE" | "ERROR" | "CANCELLED");
        if is_terminal {
            if let Some(msg) = self.take_open_message(step_index, MessageKind::Reasoning) {
                out.push(AgentEvent::MessageCompleted {
                    message_id: msg.message_id,
                    kind: MessageKind::Reasoning,
                    text: msg.text,
                });
            }
            if let Some(msg) = self.take_open_message(step_index, MessageKind::Assistant) {
                self.turn_emitted_assistant_message = true;
                out.push(AgentEvent::MessageCompleted {
                    message_id: msg.message_id,
                    kind: MessageKind::Assistant,
                    text: msg.text,
                });
            } else if let Some(full_text) = step.get("text").and_then(Value::as_str)
                && !full_text.is_empty()
            {
                let message_id = self.alloc_message_id(step_index, MessageKind::Assistant);
                self.turn_emitted_assistant_message = true;
                out.push(AgentEvent::MessageCompleted {
                    message_id,
                    kind: MessageKind::Assistant,
                    text: full_text.to_owned(),
                });
            }
        }

        if let Some(usage_val) = step.get("usage")
            && let Some(step_usage) = parse_usage(usage_val)
        {
            accumulate_usage(&mut self.turn_usage, &step_usage);
        }
    }

    fn append_message_delta(&mut self, step_index: u64, kind: MessageKind, delta: &str) -> String {
        if let Some(existing) = self
            .open_messages
            .iter_mut()
            .find(|m| m.step_index == step_index && m.kind == kind)
        {
            existing.text.push_str(delta);
            return existing.message_id.clone();
        }
        let message_id = self.alloc_message_id(step_index, kind);
        self.open_messages.push(OpenMessage {
            step_index,
            message_id: message_id.clone(),
            kind,
            text: delta.to_owned(),
        });
        message_id
    }

    fn take_open_message(&mut self, step_index: u64, kind: MessageKind) -> Option<OpenMessage> {
        let pos = self
            .open_messages
            .iter()
            .position(|m| m.step_index == step_index && m.kind == kind)?;
        Some(self.open_messages.remove(pos))
    }

    fn handle_tool_step(
        &mut self,
        step: &Value,
        step_index: u64,
        state: &str,
        out: &mut Vec<AgentEvent>,
    ) {
        let tool_info = step.get("tool_info").unwrap_or(&Value::Null);
        let tool_name = step
            .get("tool_name")
            .or_else(|| tool_info.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let params = tool_info
            .get("parameters")
            .or_else(|| tool_info.get("args"))
            .or_else(|| step.get("parameters"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let kind = classify_tool(tool_name);
        let title = extract_tool_title(tool_name, &params);

        match state {
            "ACTIVE" => {
                if let Some(existing) = self
                    .open_tool_calls
                    .iter()
                    .find(|c| c.step_index == step_index)
                {
                    let call_id = existing.call_id.clone();
                    let output_delta = tool_info
                        .get("output_delta")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    let output = tool_info
                        .get("output")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned);
                    out.push(AgentEvent::ToolCallUpdated {
                        call_id,
                        title,
                        kind: Some(kind),
                        input: Some(params),
                        output_delta,
                        output,
                    });
                } else {
                    let call_id = self.alloc_call_id(step_index);
                    let denied_reason = self.take_matching_gate_denial(tool_name, &call_id);
                    self.open_tool_calls.push(OpenToolCall {
                        step_index,
                        call_id: call_id.clone(),
                        name: tool_name.to_owned(),
                        denied_reason,
                    });
                    out.push(AgentEvent::ToolCallStarted {
                        call_id: call_id.clone(),
                        name: tool_name.to_owned(),
                        kind,
                        title,
                        input: params.clone(),
                    });

                    // `ask_question` is NOT surfaced from the step: the `QuestionAsked` must carry
                    // the id of the hook request that is blocked waiting for the answer
                    // (`AnswerQuestion` is routed back to the bridge by that id), and the step
                    // can arrive before or after the hook request. `handle_hook_request`
                    // emits it, exactly once.
                }
            }
            "DONE" | "ERROR" | "CANCELLED" | "INTERRUPTED" => {
                let open_call = if let Some(pos) = self
                    .open_tool_calls
                    .iter()
                    .position(|c| c.step_index == step_index)
                {
                    self.open_tool_calls.remove(pos)
                } else {
                    let call_id = self.alloc_call_id(step_index);
                    let denied_reason = self.take_matching_gate_denial(tool_name, &call_id);
                    out.push(AgentEvent::ToolCallStarted {
                        call_id: call_id.clone(),
                        name: tool_name.to_owned(),
                        kind,
                        title,
                        input: params,
                    });
                    OpenToolCall {
                        step_index,
                        call_id,
                        name: tool_name.to_owned(),
                        denied_reason,
                    }
                };

                let output = tool_info
                    .get("output")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                let error_msg = extract_tool_error(tool_info, step);

                let status = match state {
                    "DONE" => ToolStatus::Succeeded,
                    "CANCELLED" | "INTERRUPTED" => ToolStatus::Cancelled,
                    _ => {
                        let gate_denied = open_call.denied_reason.is_some()
                            || self
                                .take_matching_gate_denial(tool_name, &open_call.call_id)
                                .is_some();
                        let msg_indicates_denial =
                            error_msg.as_deref().is_some_and(is_denial_error_message);
                        if gate_denied || msg_indicates_denial {
                            self.turn_denied_tools.push(open_call.name.clone());
                            ToolStatus::Denied
                        } else {
                            ToolStatus::Failed
                        }
                    }
                };

                let final_error = if status == ToolStatus::Denied {
                    error_msg.or(open_call.denied_reason)
                } else {
                    error_msg
                };

                out.push(AgentEvent::ToolCallFinished {
                    call_id: open_call.call_id,
                    status,
                    output,
                    error: final_error,
                });
            }
            _ => {}
        }
    }

    fn take_matching_gate_denial(&mut self, tool_name: &str, call_id: &str) -> Option<String> {
        if let Some(d) = self.pending_gate_denials.iter_mut().find(|d| {
            !d.matched_step
                && (d.check.call_id.as_deref() == Some(call_id) || d.check.name == tool_name)
        }) {
            d.matched_step = true;
            return Some(d.reason.clone());
        }
        None
    }

    fn handle_result(&mut self, val: &Value) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        self.ensure_turn_started(&mut out);

        let res = val.get("result").unwrap_or(val);
        let status_str = res
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("SUCCESS");

        // 1. Close any still-open messages first.
        let open_msgs: Vec<OpenMessage> = std::mem::take(&mut self.open_messages);
        for msg in open_msgs {
            if msg.kind == MessageKind::Assistant {
                self.turn_emitted_assistant_message = true;
            }
            out.push(AgentEvent::MessageCompleted {
                message_id: msg.message_id,
                kind: msg.kind,
                text: msg.text,
            });
        }

        // 2. Process `denied_actions` from `result`.
        if let Some(denied_arr) = res.get("denied_actions").and_then(Value::as_array) {
            for item in denied_arr {
                self.handle_denied_action_item(item, &mut out);
            }
        }

        // 3. Emit synthetic denied tool calls for any GateDecision::Deny that never produced a step_update.
        let unmatched_denials: Vec<PendingGateDenial> = self
            .pending_gate_denials
            .drain(..)
            .filter(|d| !d.matched_step)
            .collect();
        for denial in unmatched_denials {
            if let Some(pos) = self.open_tool_calls.iter().position(|c| {
                denial.check.call_id.as_deref() == Some(c.call_id.as_str())
                    || c.name == denial.check.name
            }) {
                let open = self.open_tool_calls.remove(pos);
                out.push(AgentEvent::ToolCallFinished {
                    call_id: open.call_id,
                    status: ToolStatus::Denied,
                    output: None,
                    error: Some(denial.reason),
                });
            } else {
                let call_id = denial
                    .check
                    .call_id
                    .unwrap_or_else(|| self.alloc_synthetic_call_id());
                self.used_call_ids.insert(call_id.clone());
                out.push(AgentEvent::ToolCallStarted {
                    call_id: call_id.clone(),
                    name: denial.check.name,
                    kind: denial.check.kind,
                    title: denial.check.title,
                    input: denial.check.input,
                });
                out.push(AgentEvent::ToolCallFinished {
                    call_id,
                    status: ToolStatus::Denied,
                    output: None,
                    error: Some(denial.reason),
                });
            }
        }

        // 4. Close any remaining open tool calls.
        let remaining_calls: Vec<OpenToolCall> = std::mem::take(&mut self.open_tool_calls);
        for call in remaining_calls {
            let status = if call.denied_reason.is_some() {
                ToolStatus::Denied
            } else if self.turn_interrupted
                || matches!(status_str, "INTERRUPTED" | "CANCELLED" | "ABORTED")
            {
                ToolStatus::Cancelled
            } else if matches!(status_str, "ERROR" | "FAILED") {
                ToolStatus::Failed
            } else {
                ToolStatus::Cancelled
            };
            out.push(AgentEvent::ToolCallFinished {
                call_id: call.call_id,
                status,
                output: None,
                error: call.denied_reason,
            });
        }

        // 5. Close any pending approvals / questions.
        let pending_reqs: Vec<PendingRequest> = std::mem::take(&mut self.pending_requests);
        for req in pending_reqs {
            out.push(AgentEvent::RequestResolved {
                request_id: req.request_id,
                outcome: RequestOutcome::Cancelled,
            });
        }

        // 6. If the result carries a whole response and no assistant message was streamed in step_updates, emit MessageCompleted.
        if !self.turn_emitted_assistant_message
            && let Some(resp_text) = res.get("response").and_then(Value::as_str)
            && !resp_text.is_empty()
        {
            let message_id =
                self.alloc_message_id(u64::from(self.current_turn), MessageKind::Assistant);
            self.turn_emitted_assistant_message = true;
            out.push(AgentEvent::MessageCompleted {
                message_id,
                kind: MessageKind::Assistant,
                text: resp_text.to_owned(),
            });
        }

        // 7. Usage (running total for turn, emitted right before TurnCompleted).
        let final_usage = res
            .get("usage")
            .and_then(parse_usage)
            .or_else(|| self.turn_usage.take());
        if let Some(usage) = &final_usage {
            out.push(AgentEvent::Usage {
                usage: usage.clone(),
            });
        }

        // 8. Check if result reports rate limiting.
        if status_str == "RATE_LIMITED"
            || res
                .get("error")
                .and_then(|e| e.get("type"))
                .and_then(Value::as_str)
                == Some("RATE_LIMITED")
        {
            let msg = res
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            let resets_at_ms = res
                .get("error")
                .and_then(|e| e.get("resets_at_ms"))
                .and_then(Value::as_u64);
            out.push(AgentEvent::RateLimited {
                resets_at_ms,
                message: msg,
            });
        }

        // 9. Emit TurnCompleted.
        let outcome = if self.turn_interrupted {
            TurnOutcome::Interrupted
        } else {
            match status_str {
                "SUCCESS" | "COMPLETED" | "DONE" => TurnOutcome::Completed,
                "INTERRUPTED" | "CANCELLED" | "ABORTED" => TurnOutcome::Interrupted,
                "LIMIT_REACHED" | "MAX_TURNS" | "MAX_TOKENS" => TurnOutcome::LimitReached,
                _ => TurnOutcome::Failed,
            }
        };

        out.push(AgentEvent::TurnCompleted {
            turn: self.current_turn,
            outcome,
            usage: final_usage,
        });
        self.in_turn = false;
        self.turn_interrupted = false;
        self.turn_usage = None;
        out
    }

    fn handle_denied_action_item(&mut self, item: &Value, out: &mut Vec<AgentEvent>) {
        let tool_name = item
            .get("tool_name")
            .or_else(|| item.get("name"))
            .or_else(|| item.get("tool"))
            .and_then(Value::as_str)
            .or_else(|| item.as_str())
            .unwrap_or("unknown");
        let reason = item
            .get("reason")
            .or_else(|| item.get("message"))
            .or_else(|| item.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("action denied by provider")
            .to_owned();
        let step_idx = item.get("step_index").and_then(Value::as_u64);

        // Check if an open tool call matches this denied action.
        if let Some(pos) = self
            .open_tool_calls
            .iter()
            .position(|c| step_idx.is_some_and(|idx| c.step_index == idx) || c.name == tool_name)
        {
            let open = self.open_tool_calls.remove(pos);
            self.turn_denied_tools.push(open.name);
            out.push(AgentEvent::ToolCallFinished {
                call_id: open.call_id,
                status: ToolStatus::Denied,
                output: None,
                error: Some(reason),
            });
            return;
        }

        // Check if a step_update already finished this tool as Denied in the current turn.
        if let Some(pos) = self.turn_denied_tools.iter().position(|n| n == tool_name) {
            self.turn_denied_tools.remove(pos);
            return;
        }

        // Otherwise emit ToolCallStarted + ToolCallFinished { status: Denied }.
        let call_id = match step_idx {
            Some(idx) => self.alloc_call_id(idx),
            None => self.alloc_synthetic_call_id(),
        };
        let params = item
            .get("parameters")
            .or_else(|| item.get("args"))
            .or_else(|| item.get("input"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let kind = classify_tool(tool_name);
        let title = extract_tool_title(tool_name, &params);

        out.push(AgentEvent::ToolCallStarted {
            call_id: call_id.clone(),
            name: tool_name.to_owned(),
            kind,
            title,
            input: params,
        });
        out.push(AgentEvent::ToolCallFinished {
            call_id,
            status: ToolStatus::Denied,
            output: None,
            error: Some(reason),
        });
    }

    fn handle_error_record(&mut self, val: &Value) -> Vec<AgentEvent> {
        let err_obj = val.get("error").unwrap_or(val);
        let message = err_obj
            .get("message")
            .or_else(|| val.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("unknown agy error")
            .to_owned();
        let code_or_type = err_obj
            .get("type")
            .or_else(|| err_obj.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if code_or_type == "RATE_LIMITED" || message.to_lowercase().contains("rate limit") {
            let resets_at_ms = err_obj.get("resets_at_ms").and_then(Value::as_u64);
            return vec![AgentEvent::RateLimited {
                resets_at_ms,
                message: Some(message),
            }];
        }
        let fatal = if !self.session_started {
            true
        } else {
            err_obj
                .get("fatal")
                .or_else(|| val.get("fatal"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        if fatal {
            self.emitted_fatal_error = true;
        }
        vec![AgentEvent::Error { message, fatal }]
    }

    fn handle_quota_record(&mut self, val: &Value) -> Vec<AgentEvent> {
        let windows = val
            .get("windows")
            .or_else(|| val.get("quota").and_then(|q| q.get("windows")))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|w| {
                        let name = w.get("name")?.as_str()?.to_owned();
                        let used_percent = w.get("used_percent").and_then(Value::as_f64);
                        let resets_at_ms = w.get("resets_at_ms").and_then(Value::as_u64);
                        Some(QuotaWindow {
                            name,
                            used_percent,
                            resets_at_ms,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        vec![AgentEvent::QuotaUpdated { windows }]
    }

    fn handle_rate_limit_record(&mut self, val: &Value) -> Vec<AgentEvent> {
        let rl = val.get("rate_limit").unwrap_or(val);
        let resets_at_ms = rl.get("resets_at_ms").and_then(Value::as_u64);
        let message = rl
            .get("message")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        vec![AgentEvent::RateLimited {
            resets_at_ms,
            message,
        }]
    }

    fn handle_hook_request(&mut self, id: &str, payload: &Value) -> Vec<AgentEvent> {
        self.last_hook_request_id = Some(id.to_owned());
        let call = payload.get("toolCall").unwrap_or(payload);
        let name = call.get("name").and_then(Value::as_str).unwrap_or("");
        if name == "ask_question" {
            let args = call.get("args").unwrap_or(&Value::Null);
            if let Some(questions) = parse_questions(args) {
                let request_id = self.alloc_request_id();
                self.pending_requests.push(PendingRequest {
                    request_id: request_id.clone(),
                    kind: PendingRequestKind::Question,
                });
                return vec![AgentEvent::QuestionAsked {
                    request_id,
                    questions,
                }];
            }
        }
        Vec::new()
    }

    fn handle_hook_response(&mut self, _id: &str, payload: &Value) -> Vec<AgentEvent> {
        if payload.get("decision").and_then(Value::as_str) == Some("deny") {
            let reason = payload
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("denied by hook")
                .to_owned();
            if let Some(last_open) = self.open_tool_calls.last_mut()
                && last_open.denied_reason.is_none()
            {
                last_open.denied_reason = Some(reason);
            }
        }
        Vec::new()
    }

    fn handle_gate(&mut self, check: &ToolCheck, decision: &GateDecision) -> Vec<AgentEvent> {
        match decision {
            GateDecision::Allow => {
                self.last_hook_request_id = None;
                Vec::new()
            }
            GateDecision::Deny { reason } => {
                self.last_hook_request_id = None;
                let mut matched_step = false;
                if let Some(open) = self.open_tool_calls.iter_mut().rev().find(|c| {
                    check.call_id.as_deref() == Some(c.call_id.as_str()) || c.name == check.name
                }) {
                    open.denied_reason = Some(reason.clone());
                    matched_step = true;
                }
                self.pending_gate_denials.push(PendingGateDenial {
                    check: check.clone(),
                    reason: reason.clone(),
                    matched_step,
                });
                Vec::new()
            }
            GateDecision::Ask => {
                let request_id = self.alloc_request_id();
                let call_id = check
                    .call_id
                    .clone()
                    .or_else(|| self.current_open_call_id(&check.name));
                self.pending_requests.push(PendingRequest {
                    request_id: request_id.clone(),
                    kind: PendingRequestKind::Approval {
                        call_id: call_id.clone(),
                    },
                });
                vec![AgentEvent::ApprovalRequested {
                    request_id,
                    call_id,
                    name: check.name.clone(),
                    kind: check.kind,
                    title: check.title.clone(),
                    input: check.input.clone(),
                    reason: None,
                }]
            }
        }
    }

    fn handle_command(&mut self, command: &SessionCommand) -> Vec<AgentEvent> {
        match command {
            SessionCommand::SendUserMessage { .. } => Vec::new(),
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => {
                let Some(pos) = self
                    .pending_requests
                    .iter()
                    .position(|r| r.request_id == *request_id)
                else {
                    return vec![AgentEvent::Error {
                        message: format!(
                            "unknown or already resolved approval request: {request_id}"
                        ),
                        fatal: false,
                    }];
                };
                if !matches!(
                    self.pending_requests[pos].kind,
                    PendingRequestKind::Approval { .. }
                ) {
                    return vec![AgentEvent::Error {
                        message: format!("request {request_id} is not an approval request"),
                        fatal: false,
                    }];
                }
                let removed = self.pending_requests.remove(pos);
                let PendingRequestKind::Approval { call_id } = removed.kind else {
                    unreachable!()
                };
                let outcome = match decision {
                    ApprovalDecision::Allow => RequestOutcome::Approved,
                    ApprovalDecision::Deny { message } => {
                        let reason = message
                            .clone()
                            .unwrap_or_else(|| "denied by user".to_owned());
                        if let Some(open) = self
                            .open_tool_calls
                            .iter_mut()
                            .rev()
                            .find(|c| call_id.as_deref() == Some(c.call_id.as_str()))
                        {
                            open.denied_reason = Some(reason);
                        } else if let Some(open) = self.open_tool_calls.last_mut() {
                            open.denied_reason = Some(reason);
                        }
                        RequestOutcome::Denied
                    }
                };
                vec![AgentEvent::RequestResolved {
                    request_id: request_id.clone(),
                    outcome,
                }]
            }
            SessionCommand::AnswerQuestion { request_id, .. } => {
                let Some(pos) = self
                    .pending_requests
                    .iter()
                    .position(|r| r.request_id == *request_id)
                else {
                    return vec![AgentEvent::Error {
                        message: format!(
                            "unknown or already resolved question request: {request_id}"
                        ),
                        fatal: false,
                    }];
                };
                if !matches!(
                    self.pending_requests[pos].kind,
                    PendingRequestKind::Question
                ) {
                    return vec![AgentEvent::Error {
                        message: format!("request {request_id} is not a question request"),
                        fatal: false,
                    }];
                }
                self.pending_requests.remove(pos);
                vec![AgentEvent::RequestResolved {
                    request_id: request_id.clone(),
                    outcome: RequestOutcome::Answered,
                }]
            }
            SessionCommand::Interrupt => {
                if self.in_turn {
                    self.turn_interrupted = true;
                }
                Vec::new()
            }
            SessionCommand::Shutdown => Vec::new(),
        }
    }

    fn handle_exit(&mut self, code: Option<i32>) -> Vec<AgentEvent> {
        let mut out = Vec::new();

        if !self.session_started {
            if !self.emitted_fatal_error {
                let message = if !self.startup_stderr.is_empty() {
                    self.startup_stderr.join("\n")
                } else {
                    format!("agy process exited before session started (exit code: {code:?})")
                };
                self.emitted_fatal_error = true;
                out.push(AgentEvent::Error {
                    message,
                    fatal: true,
                });
            }
            out.push(AgentEvent::SessionEnded {
                exit_code: code,
                reason: Some("startup failed".into()),
            });
            return out;
        }

        // Closing order mandated by `atrium_protocol::agent`:
        // 1. Open messages -> MessageCompleted (text so far)
        let open_msgs: Vec<OpenMessage> = std::mem::take(&mut self.open_messages);
        for msg in open_msgs {
            out.push(AgentEvent::MessageCompleted {
                message_id: msg.message_id,
                kind: msg.kind,
                text: msg.text,
            });
        }

        // 2. Open tool calls -> ToolCallFinished { status: cancelled }
        let open_calls: Vec<OpenToolCall> = std::mem::take(&mut self.open_tool_calls);
        for call in open_calls {
            out.push(AgentEvent::ToolCallFinished {
                call_id: call.call_id,
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
            });
        }

        // 3. Pending approvals and questions -> RequestResolved { outcome: cancelled }
        let pending_reqs: Vec<PendingRequest> = std::mem::take(&mut self.pending_requests);
        for req in pending_reqs {
            out.push(AgentEvent::RequestResolved {
                request_id: req.request_id,
                outcome: RequestOutcome::Cancelled,
            });
        }

        // 4. An open turn -> TurnCompleted { outcome: interrupted | failed }
        if self.in_turn {
            let outcome = if self.turn_interrupted {
                TurnOutcome::Interrupted
            } else {
                TurnOutcome::Failed
            };
            let usage = self.turn_usage.take();
            if let Some(u) = &usage {
                out.push(AgentEvent::Usage { usage: u.clone() });
            }
            out.push(AgentEvent::TurnCompleted {
                turn: self.current_turn,
                outcome,
                usage,
            });
            self.in_turn = false;
        }

        // 5. SessionEnded
        out.push(AgentEvent::SessionEnded {
            exit_code: code,
            reason: None,
        });
        out
    }
}

impl Normalizer for AgyNormalizer {
    fn on_input(&mut self, input: &AdapterInput) -> Vec<AgentEvent> {
        match input {
            AdapterInput::Stdin { .. } => Vec::new(),
            AdapterInput::Stdout { line } => self.handle_stdout_line(line),
            AdapterInput::Stderr { line } => {
                let trimmed = line.trim();
                if !self.session_started && !trimmed.is_empty() {
                    self.startup_stderr.push(trimmed.to_owned());
                }
                Vec::new()
            }
            AdapterInput::HookRequest { id, payload } => self.handle_hook_request(id, payload),
            AdapterInput::HookResponse { id, payload } => self.handle_hook_response(id, payload),
            AdapterInput::Gate { check, decision } => self.handle_gate(check, decision),
            AdapterInput::Command { command } => self.handle_command(command),
            AdapterInput::Exit { code } => self.handle_exit(*code),
        }
    }
}

fn parse_usage(val: &Value) -> Option<Usage> {
    let obj = val.as_object()?;
    let input_tokens = obj.get("input_tokens").and_then(Value::as_u64);
    let output_tokens = obj.get("output_tokens").and_then(Value::as_u64);
    let cache_read_tokens = obj.get("cache_read_tokens").and_then(Value::as_u64);
    let cache_write_tokens = obj.get("cache_write_tokens").and_then(Value::as_u64);
    let reasoning_tokens = obj
        .get("thinking_tokens")
        .or_else(|| obj.get("reasoning_tokens"))
        .and_then(Value::as_u64);
    let cost_usd = obj.get("cost_usd").and_then(Value::as_f64);

    if input_tokens.is_none()
        && output_tokens.is_none()
        && cache_read_tokens.is_none()
        && cache_write_tokens.is_none()
        && reasoning_tokens.is_none()
        && cost_usd.is_none()
    {
        return None;
    }

    Some(Usage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        reasoning_tokens,
        cost_usd,
    })
}

fn accumulate_usage(accum: &mut Option<Usage>, step: &Usage) {
    let target = accum.get_or_insert_with(Usage::default);
    add_opt(&mut target.input_tokens, step.input_tokens);
    add_opt(&mut target.output_tokens, step.output_tokens);
    add_opt(&mut target.cache_read_tokens, step.cache_read_tokens);
    add_opt(&mut target.cache_write_tokens, step.cache_write_tokens);
    add_opt(&mut target.reasoning_tokens, step.reasoning_tokens);
    if let Some(cost) = step.cost_usd {
        *target.cost_usd.get_or_insert(0.0) += cost;
    }
}

fn add_opt(dst: &mut Option<u64>, src: Option<u64>) {
    if let Some(v) = src {
        *dst = Some(dst.unwrap_or(0) + v);
    }
}

fn extract_tool_error(tool_info: &Value, step: &Value) -> Option<String> {
    if let Some(err) = tool_info.get("error").or_else(|| step.get("error")) {
        if let Some(s) = err.as_str() {
            return Some(s.to_owned());
        }
        if let Some(msg) = err.get("message").and_then(Value::as_str) {
            return Some(msg.to_owned());
        }
    }
    None
}

fn is_denial_error_message(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("denied by pre-tool hook")
        || lower.contains("tool call denied")
        || lower.contains("permission denied")
        || lower.contains("denied by policy")
        || lower.contains("denied by user")
        || lower.contains("action denied")
}

/// Parses `ask_question` arguments into Atrium's structured [`Question`]s.
pub fn parse_questions(params: &Value) -> Option<Vec<Question>> {
    let arr = params.get("questions")?.as_array()?;
    if arr.is_empty() {
        return None;
    }
    let mut questions = Vec::with_capacity(arr.len());
    for (idx, q_val) in arr.iter().enumerate() {
        let prompt = q_val
            .get("question")
            .or_else(|| q_val.get("prompt"))
            .and_then(Value::as_str)?
            .to_owned();
        let id = q_val
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("q-{idx}"));
        let multi_select = q_val
            .get("is_multi_select")
            .or_else(|| q_val.get("multi_select"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let allow_text = q_val
            .get("allow_text")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let options = q_val
            .get("options")
            .and_then(Value::as_array)
            .map(|opts| {
                opts.iter()
                    .enumerate()
                    .filter_map(|(opt_idx, opt_val)| {
                        if let Some(raw_label) = opt_val.as_str() {
                            let (label, description) =
                                if let Some(rest) = raw_label.strip_prefix("(Recommended) ") {
                                    (rest.to_owned(), Some("Recommended".to_owned()))
                                } else {
                                    (raw_label.to_owned(), None)
                                };
                            Some(QuestionOption {
                                id: format!("opt-{opt_idx}"),
                                label,
                                description,
                            })
                        } else if let Some(obj) = opt_val.as_object() {
                            let opt_id = obj
                                .get("id")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned)
                                .unwrap_or_else(|| format!("opt-{opt_idx}"));
                            let label = obj.get("label")?.as_str()?.to_owned();
                            let description = obj
                                .get("description")
                                .and_then(Value::as_str)
                                .map(ToOwned::to_owned);
                            Some(QuestionOption {
                                id: opt_id,
                                label,
                                description,
                            })
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        questions.push(Question {
            id,
            prompt,
            options,
            multi_select,
            allow_text,
        });
    }
    Some(questions)
}

fn parse_plan_entries(step: &Value) -> Option<Vec<PlanEntry>> {
    let arr = step.get("entries")?.as_array()?;
    let mut entries = Vec::with_capacity(arr.len());
    for item in arr {
        let content = item.get("content")?.as_str()?.to_owned();
        let status = match item
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
        {
            "in_progress" | "ACTIVE" => PlanStatus::InProgress,
            "completed" | "DONE" => PlanStatus::Completed,
            _ => PlanStatus::Pending,
        };
        entries.push(PlanEntry { content, status });
    }
    Some(entries)
}

/// Helper to build a HashMap of question options for formatting answers back to the provider.
pub fn format_question_answers(
    questions: &[Question],
    answers: &[atrium_protocol::agent::QuestionAnswer],
) -> String {
    let q_map: HashMap<&str, &Question> = questions.iter().map(|q| (q.id.as_str(), q)).collect();
    let mut parts = Vec::with_capacity(answers.len());
    for ans in answers {
        let mut selected_labels = Vec::new();
        if let Some(q) = q_map.get(ans.question_id.as_str()) {
            for sel_id in &ans.selected {
                if let Some(opt) = q.options.iter().find(|o| &o.id == sel_id) {
                    selected_labels.push(opt.label.clone());
                } else {
                    selected_labels.push(sel_id.clone());
                }
            }
        } else {
            selected_labels.extend(ans.selected.iter().cloned());
        }
        if let Some(text) = &ans.text
            && !text.is_empty()
        {
            selected_labels.push(text.clone());
        }
        parts.push(format!(
            "{}: {}",
            ans.question_id,
            selected_labels.join(", ")
        ));
    }
    parts.join("; ")
}
