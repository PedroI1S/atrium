use std::collections::{BTreeMap, BTreeSet, VecDeque};

use agent_client_protocol::schema::v1::{
    ContentBlock, CreateElicitationRequest, CreateTerminalRequest, ElicitationMode,
    ElicitationPropertySchema, ElicitationScope, LoadSessionRequest, LoadSessionResponse,
    MultiSelectItems, NewSessionRequest, NewSessionResponse, PlanEntryStatus, PromptResponse,
    ReadTextFileRequest, RequestPermissionRequest, ResumeSessionRequest, ResumeSessionResponse,
    SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigOptionValue,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest, SetSessionModeRequest,
    StopReason, ToolCall, ToolCallContent, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
    ToolKind as AcpToolKind, WriteTextFileRequest,
};
use atrium_agents::{AdapterInput, GateDecision, MAX_QUEUED_MESSAGES, Normalizer, ToolCheck};
use atrium_protocol::agent::{
    AgentEvent, ApprovalDecision, MessageKind, PlanEntry, PlanStatus, Provider, Question,
    QuestionOption, RequestOutcome, SessionCommand, ToolKind, ToolStatus, TurnOutcome, Usage,
};
use serde_json::{Value, json};

use crate::model::{ModelVia, parse_model_state};

#[derive(Debug, Clone)]
enum OutboundReq {
    Initialize,
    SessionNew {
        model: Option<String>,
    },
    SessionLoad {
        session_id: String,
        model: Option<String>,
    },
    SessionResume {
        session_id: String,
        model: Option<String>,
    },
    SetMode,
    SetConfigOption,
    /// `session/set_config_option` (model) or `session/set_model` sent while
    /// the session start waits for the model to be confirmed.
    SetModel {
        model: String,
    },
    Prompt {
        turn: u32,
    },
}

#[derive(Debug, Clone)]
struct OpenMessage {
    message_id: String,
    kind: MessageKind,
    text: String,
}

#[derive(Debug, Clone)]
struct OpenToolCall {
    call_id: String,
    name: String,
    kind: ToolKind,
    title: Option<String>,
    input: Value,
    synthesized: bool,
    denied_reason: Option<String>,
}

#[derive(Debug, Clone)]
struct PendingGateableRpc {
    rpc_id: String,
    call_id: String,
}

/// A session the agent created, whose start is held back until the requested
/// model is confirmed (see [`AcpNormalizer`]).
#[derive(Debug, Clone)]
struct DeferredStart {
    native_session_id: String,
    requested_model: String,
    via: Option<ModelVia>,
    tools: Vec<String>,
}

#[derive(Debug, Clone)]
struct PendingApproval {
    request_id: String,
    call_id: Option<String>,
}

/// Pure ACP-to-[`AgentEvent`] state machine.
#[derive(Debug)]
pub struct AcpNormalizer {
    session_started: bool,
    session_ended: bool,
    fatal_error_emitted: bool,
    last_error_reason: Option<String>,
    stderr_lines: Vec<String>,
    configured_model: Option<String>,
    /// Set between the `session/new` response and the model confirmation.
    deferred_start: Option<DeferredStart>,
    /// True between a `session/load` / `session/resume` request and its
    /// response. The agent replays the old conversation as `session/update`s in
    /// that window. The event contract has no history event and the session is
    /// not established yet (`SessionStarted` must be first), so replayed updates
    /// are not turned into events; they are counted and reported in one
    /// `Unmapped { record_type: "session/load:history_replay" }` right after
    /// `SessionStarted`. The raw lines stay in the fixture.
    loading_history: bool,
    history_replay: Vec<String>,
    /// `session/update`s that arrive before the session is established are held
    /// and replayed right after `SessionStarted` (dropped if it never starts:
    /// a pre-session failure begins with its `Error`).
    pre_start_updates: Vec<(Value, Value)>,

    pending_client_reqs: BTreeMap<String, OutboundReq>,
    pending_gateable_rpcs: VecDeque<PendingGateableRpc>,
    rpc_to_call_id: BTreeMap<String, String>,
    terminal_to_call_id: BTreeMap<String, String>,

    turn_counter: u32,
    active_turn: Option<u32>,
    cancel_requested: bool,
    queued_user_messages: usize,
    turn_usage: Option<Usage>,

    open_message: Option<OpenMessage>,
    next_message_seq: u64,

    open_tool_calls: BTreeMap<String, OpenToolCall>,
    open_tool_order: Vec<String>,
    closed_tool_calls: BTreeSet<String>,

    pending_approvals: BTreeMap<String, PendingApproval>,
    pending_approval_order: Vec<String>,
    pending_questions: BTreeSet<String>,
    pending_question_order: Vec<String>,
    next_request_seq: u64,
}

impl Default for AcpNormalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpNormalizer {
    pub fn new() -> Self {
        Self {
            session_started: false,
            session_ended: false,
            fatal_error_emitted: false,
            last_error_reason: None,
            stderr_lines: Vec::new(),
            configured_model: None,
            deferred_start: None,
            loading_history: false,
            history_replay: Vec::new(),
            pre_start_updates: Vec::new(),
            pending_client_reqs: BTreeMap::new(),
            pending_gateable_rpcs: VecDeque::new(),
            rpc_to_call_id: BTreeMap::new(),
            terminal_to_call_id: BTreeMap::new(),
            turn_counter: 0,
            active_turn: None,
            cancel_requested: false,
            queued_user_messages: 0,
            turn_usage: None,
            open_message: None,
            next_message_seq: 0,
            open_tool_calls: BTreeMap::new(),
            open_tool_order: Vec::new(),
            closed_tool_calls: BTreeSet::new(),
            pending_approvals: BTreeMap::new(),
            pending_approval_order: Vec::new(),
            pending_questions: BTreeSet::new(),
            pending_question_order: Vec::new(),
            next_request_seq: 0,
        }
    }

    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.configured_model = model;
        self
    }

    /// True once `SessionStarted` has been emitted.
    pub fn session_started(&self) -> bool {
        self.session_started
    }

    pub fn call_id_for_rpc(&self, rpc_id: &str) -> Option<String> {
        self.rpc_to_call_id.get(rpc_id).cloned()
    }

    fn flush_open_message(&mut self, out: &mut Vec<AgentEvent>) {
        if let Some(msg) = self.open_message.take() {
            out.push(AgentEvent::MessageCompleted {
                message_id: msg.message_id,
                kind: msg.kind,
                text: msg.text,
            });
        }
    }

    fn push_message_delta(
        &mut self,
        explicit_id: Option<String>,
        kind: MessageKind,
        delta: String,
        out: &mut Vec<AgentEvent>,
    ) {
        if delta.is_empty() {
            return;
        }
        let need_new = match &self.open_message {
            None => true,
            Some(open) => {
                open.kind != kind
                    || explicit_id
                        .as_ref()
                        .is_some_and(|id| id != &open.message_id)
            }
        };
        if need_new {
            self.flush_open_message(out);
            let message_id = explicit_id.unwrap_or_else(|| {
                self.next_message_seq += 1;
                match kind {
                    MessageKind::Assistant => format!("msg-{}", self.next_message_seq),
                    MessageKind::Reasoning => format!("thought-{}", self.next_message_seq),
                }
            });
            self.open_message = Some(OpenMessage {
                message_id,
                kind,
                text: String::new(),
            });
        }
        if let Some(open) = &mut self.open_message {
            open.text.push_str(&delta);
            out.push(AgentEvent::MessageDelta {
                message_id: open.message_id.clone(),
                kind: open.kind,
                text: delta,
            });
        }
    }

    fn start_tool_call(&mut self, call: OpenToolCall, out: &mut Vec<AgentEvent>) {
        if self.closed_tool_calls.contains(&call.call_id)
            || self.open_tool_calls.contains_key(&call.call_id)
        {
            return;
        }
        self.flush_open_message(out);
        out.push(AgentEvent::ToolCallStarted {
            call_id: call.call_id.clone(),
            name: call.name.clone(),
            kind: call.kind,
            title: call.title.clone(),
            input: call.input.clone(),
        });
        self.open_tool_order.push(call.call_id.clone());
        self.open_tool_calls.insert(call.call_id.clone(), call);
    }

    fn finish_tool_call(
        &mut self,
        call_id: &str,
        status: ToolStatus,
        output: Option<String>,
        error: Option<String>,
        out: &mut Vec<AgentEvent>,
    ) {
        if self.closed_tool_calls.contains(call_id) {
            return;
        }
        if let Some(open) = self.open_tool_calls.remove(call_id) {
            self.open_tool_order.retain(|id| id != call_id);
            self.closed_tool_calls.insert(call_id.to_owned());
            let (final_status, final_output, final_error) = if let Some(reason) = open.denied_reason
            {
                (ToolStatus::Denied, None, Some(reason))
            } else {
                (status, output, error)
            };
            out.push(AgentEvent::ToolCallFinished {
                call_id: call_id.to_owned(),
                status: final_status,
                output: final_output,
                error: final_error,
            });
        }
    }

    fn handle_stdin(&mut self, line: &str, out: &mut Vec<AgentEvent>) {
        let Ok(val) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let Some(obj) = val.as_object() else {
            return;
        };

        if let Some(method) = obj.get("method").and_then(Value::as_str) {
            let rpc_id = obj.get("id").and_then(rpc_id_to_string);
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "initialize" => {
                    if let Some(id) = rpc_id {
                        self.pending_client_reqs.insert(id, OutboundReq::Initialize);
                    }
                }
                "session/new" => {
                    let model = extract_meta_model(&params).or_else(|| {
                        serde_json::from_value::<NewSessionRequest>(params.clone())
                            .ok()
                            .and_then(|r| {
                                r.meta
                                    .as_ref()
                                    .and_then(|m| m.get("model"))
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            })
                    });
                    if model.is_some() {
                        self.configured_model = model.clone();
                    }
                    if let Some(id) = rpc_id {
                        self.pending_client_reqs
                            .insert(id, OutboundReq::SessionNew { model });
                    }
                }
                "session/load" => {
                    self.loading_history = true;
                    let model = extract_meta_model(&params);
                    if model.is_some() {
                        self.configured_model = model.clone();
                    }
                    let session_id = serde_json::from_value::<LoadSessionRequest>(params.clone())
                        .map(|r| r.session_id.0.to_string())
                        .unwrap_or_else(|_| {
                            params
                                .get("sessionId")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned()
                        });
                    if let Some(id) = rpc_id {
                        self.pending_client_reqs
                            .insert(id, OutboundReq::SessionLoad { session_id, model });
                    }
                }
                "session/resume" => {
                    self.loading_history = true;
                    let model = extract_meta_model(&params);
                    if model.is_some() {
                        self.configured_model = model.clone();
                    }
                    let session_id = serde_json::from_value::<ResumeSessionRequest>(params.clone())
                        .map(|r| r.session_id.0.to_string())
                        .unwrap_or_else(|_| {
                            params
                                .get("sessionId")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned()
                        });
                    if let Some(id) = rpc_id {
                        self.pending_client_reqs
                            .insert(id, OutboundReq::SessionResume { session_id, model });
                    }
                }
                "session/set_mode" => {
                    let _ = serde_json::from_value::<SetSessionModeRequest>(params.clone());
                    if let Some(id) = rpc_id {
                        self.pending_client_reqs.insert(id, OutboundReq::SetMode);
                    }
                }
                "session/set_config_option" => {
                    if let Ok(req) =
                        serde_json::from_value::<SetSessionConfigOptionRequest>(params.clone())
                    {
                        let config_id = req.config_id.0.to_string();
                        let value = match req.value {
                            SessionConfigOptionValue::ValueId { value: v, .. } => v.0.to_string(),
                            SessionConfigOptionValue::Boolean { value: b, .. } => b.to_string(),
                            _ => String::new(),
                        };
                        let selects_deferred_model =
                            self.deferred_start.as_ref().is_some_and(|d| match &d.via {
                                Some(ModelVia::ConfigOption { config_id: id }) => *id == config_id,
                                _ => config_id == "model",
                            });
                        if let Some(id) = rpc_id {
                            let req = if selects_deferred_model {
                                OutboundReq::SetModel {
                                    model: value.clone(),
                                }
                            } else {
                                OutboundReq::SetConfigOption
                            };
                            self.pending_client_reqs.insert(id, req);
                        }
                        if config_id == "model" && !value.is_empty() {
                            self.configured_model = Some(value);
                        }
                    }
                }
                "session/set_model" => {
                    let model = params
                        .get("modelId")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    if let Some(id) = rpc_id {
                        self.pending_client_reqs
                            .insert(id, OutboundReq::SetModel { model });
                    }
                }
                "session/prompt" => {
                    self.queued_user_messages = self.queued_user_messages.saturating_sub(1);
                    let turn = if let Some(active) = self.active_turn {
                        active
                    } else {
                        self.turn_counter += 1;
                        let t = self.turn_counter;
                        self.active_turn = Some(t);
                        self.cancel_requested = false;
                        self.turn_usage = None;
                        out.push(AgentEvent::TurnStarted { turn: t });
                        t
                    };
                    if let Some(id) = rpc_id {
                        self.pending_client_reqs
                            .insert(id, OutboundReq::Prompt { turn });
                    }
                }
                "session/cancel" => {
                    self.cancel_requested = true;
                }
                _ => {}
            }
            return;
        }

        let Some(rpc_id) = obj.get("id").and_then(rpc_id_to_string) else {
            return;
        };
        if let Some(call_id) = self.rpc_to_call_id.get(&rpc_id).cloned() {
            let is_synthesized = self
                .open_tool_calls
                .get(&call_id)
                .is_some_and(|c| c.synthesized);
            if is_synthesized {
                if let Some(err) = obj.get("error") {
                    let msg = err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("client operation failed")
                        .to_owned();
                    self.finish_tool_call(&call_id, ToolStatus::Failed, None, Some(msg), out);
                } else if let Some(res) = obj.get("result") {
                    if let Some(term_id) = res.get("terminalId").and_then(Value::as_str) {
                        self.terminal_to_call_id.insert(term_id.to_owned(), call_id);
                    } else if let Some(exit_status) = res.get("exitStatus") {
                        let code = exit_status
                            .get("exitCode")
                            .and_then(Value::as_u64)
                            .unwrap_or(0);
                        if code == 0 {
                            self.finish_tool_call(&call_id, ToolStatus::Succeeded, None, None, out);
                        } else {
                            self.finish_tool_call(
                                &call_id,
                                ToolStatus::Failed,
                                None,
                                Some(format!("process exited with code {code}")),
                                out,
                            );
                        }
                    } else {
                        let output = res
                            .get("content")
                            .and_then(Value::as_str)
                            .map(str::to_owned);
                        self.finish_tool_call(&call_id, ToolStatus::Succeeded, output, None, out);
                    }
                }
            }
        }
    }

    fn handle_stdout(&mut self, line: &str, out: &mut Vec<AgentEvent>) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return;
        }
        let Ok(val) = serde_json::from_str::<Value>(trimmed) else {
            out.push(AgentEvent::Unmapped {
                record_type: "stdout:non_json".into(),
                raw: Value::String(line.to_owned()),
            });
            return;
        };
        let Some(obj) = val.as_object() else {
            out.push(AgentEvent::Unmapped {
                record_type: "stdout:non_object".into(),
                raw: val,
            });
            return;
        };

        if let Some(method) = obj.get("method").and_then(Value::as_str) {
            let rpc_id = obj.get("id").and_then(rpc_id_to_string);
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            match method {
                "session/update" => self.handle_session_update(params, &val, out),
                "session/request_permission" => {
                    self.handle_request_permission(rpc_id, params, &val, out);
                }
                "fs/read_text_file" => {
                    self.handle_fs_read_request(rpc_id, params, out);
                }
                "fs/write_text_file" => {
                    self.handle_fs_write_request(rpc_id, params, out);
                }
                "terminal/create" => {
                    self.handle_terminal_create_request(rpc_id, params, out);
                }
                "terminal/output"
                | "terminal/wait_for_exit"
                | "terminal/kill"
                | "terminal/release" => {
                    if let (Some(id), Some(term_id)) = (
                        rpc_id,
                        params
                            .get("terminalId")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    ) {
                        if let Some(call_id) = self.terminal_to_call_id.get(&term_id).cloned() {
                            if method == "terminal/wait_for_exit" {
                                self.rpc_to_call_id.insert(id, call_id);
                            } else if method == "terminal/release" {
                                self.finish_tool_call(
                                    &call_id,
                                    ToolStatus::Succeeded,
                                    None,
                                    None,
                                    out,
                                );
                            }
                        }
                    }
                }
                "elicitation/create" => {
                    self.handle_elicitation_create(rpc_id, params, &val, out);
                }
                _ => {
                    out.push(AgentEvent::Unmapped {
                        record_type: format!("rpc:{method}"),
                        raw: val,
                    });
                }
            }
            return;
        }

        if let Some(rpc_id) = obj.get("id").and_then(rpc_id_to_string) {
            let Some(req) = self.pending_client_reqs.remove(&rpc_id) else {
                out.push(AgentEvent::Unmapped {
                    record_type: "rpc_response:unknown_id".into(),
                    raw: val,
                });
                return;
            };
            if matches!(
                req,
                OutboundReq::SessionLoad { .. } | OutboundReq::SessionResume { .. }
            ) {
                self.loading_history = false;
            }

            if let Some(err) = obj.get("error") {
                let message = err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("ACP JSON-RPC error")
                    .to_owned();
                self.last_error_reason = Some(message.clone());
                match req {
                    OutboundReq::Initialize
                    | OutboundReq::SessionNew { .. }
                    | OutboundReq::SessionLoad { .. }
                    | OutboundReq::SessionResume { .. } => {
                        let fatal = !self.session_started;
                        if fatal {
                            self.fatal_error_emitted = true;
                        }
                        out.push(AgentEvent::Error { message, fatal });
                    }
                    OutboundReq::SetMode
                    | OutboundReq::SetConfigOption
                    | OutboundReq::SetModel { .. } => {
                        if matches!(req, OutboundReq::SetModel { .. }) {
                            self.deferred_start = None;
                        }
                        let fatal = !self.session_started;
                        if fatal {
                            self.fatal_error_emitted = true;
                        }
                        out.push(AgentEvent::Error { message, fatal });
                    }
                    OutboundReq::Prompt { turn } => {
                        self.close_turn_on_error(turn, message, out);
                    }
                }
                return;
            }

            let result = obj.get("result").cloned().unwrap_or(Value::Null);
            match req {
                OutboundReq::Initialize => {}
                OutboundReq::SessionNew { model } => {
                    let (native_session_id, resp_model) =
                        match serde_json::from_value::<NewSessionResponse>(result.clone()) {
                            Ok(resp) => (
                                resp.session_id.0.to_string(),
                                extract_model_from_options(resp.config_options.as_deref())
                                    .or_else(|| model.clone())
                                    .or_else(|| self.configured_model.clone()),
                            ),
                            Err(_) => {
                                let sid = result
                                    .get("sessionId")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned();
                                (sid, model.clone().or_else(|| self.configured_model.clone()))
                            }
                        };
                    self.start_or_defer(native_session_id, model, resp_model, &result, out);
                }
                OutboundReq::SessionLoad { session_id, model } => {
                    let resp_model = serde_json::from_value::<LoadSessionResponse>(result.clone())
                        .ok()
                        .and_then(|r| extract_model_from_options(r.config_options.as_deref()))
                        .or_else(|| model.clone())
                        .or_else(|| self.configured_model.clone());
                    self.start_or_defer(session_id, model, resp_model, &result, out);
                }
                OutboundReq::SessionResume { session_id, model } => {
                    let resp_model =
                        serde_json::from_value::<ResumeSessionResponse>(result.clone())
                            .ok()
                            .and_then(|r| extract_model_from_options(r.config_options.as_deref()))
                            .or_else(|| model.clone())
                            .or_else(|| self.configured_model.clone());
                    self.start_or_defer(session_id, model, resp_model, &result, out);
                }
                OutboundReq::SetModel { model } => {
                    if let Some(d) = self.deferred_start.take() {
                        // The set_config_option response lists the options again;
                        // when it does, it must show the model we asked for.
                        let reported = parse_model_state(&result);
                        if let Some(state) = reported.filter(|s| !s.confirms(&model)) {
                            let message = format!(
                                "the agent did not select model {model} (it reports {})",
                                state.current
                            );
                            self.last_error_reason = Some(message.clone());
                            self.fatal_error_emitted = true;
                            out.push(AgentEvent::Error {
                                message,
                                fatal: true,
                            });
                        } else {
                            self.session_started = true;
                            self.configured_model = Some(d.requested_model.clone());
                            out.push(AgentEvent::SessionStarted {
                                provider: Provider::Acp,
                                native_session_id: d.native_session_id,
                                model: Some(d.requested_model),
                                tools: d.tools,
                            });
                            self.after_session_started(out);
                        }
                    }
                }
                OutboundReq::SetMode | OutboundReq::SetConfigOption => {}
                OutboundReq::Prompt { turn } => {
                    self.handle_prompt_response(turn, result, out);
                }
            }
            return;
        }

        out.push(AgentEvent::Unmapped {
            record_type: "stdout:unknown_json".into(),
            raw: val,
        });
    }

    /// Emits `SessionStarted`, unless a model was requested and the agent does
    /// not confirm it yet. In that case the start is held back: the adapter
    /// selects the model next, and the session starts when that succeeds. If
    /// it fails, or the agent exits first, the session never started
    /// (pre-session failure), which is what a bad model id must produce.
    fn start_or_defer(
        &mut self,
        native_session_id: String,
        requested_model: Option<String>,
        resp_model: Option<String>,
        result: &Value,
        out: &mut Vec<AgentEvent>,
    ) {
        if native_session_id.is_empty() || self.session_started || self.deferred_start.is_some() {
            return;
        }
        if let Some(requested) = requested_model {
            let state = parse_model_state(result);
            if !state.as_ref().is_some_and(|s| s.confirms(&requested)) {
                self.deferred_start = Some(DeferredStart {
                    native_session_id,
                    requested_model: requested,
                    via: state.map(|s| s.via),
                    tools: extract_tools(result),
                });
                return;
            }
        }
        self.session_started = true;
        self.configured_model = resp_model.clone();
        out.push(AgentEvent::SessionStarted {
            provider: Provider::Acp,
            native_session_id,
            model: resp_model,
            tools: extract_tools(result),
        });
        self.after_session_started(out);
    }

    /// Right after `SessionStarted`: report replayed history, then the updates
    /// that arrived while the session was still being established.
    fn after_session_started(&mut self, out: &mut Vec<AgentEvent>) {
        self.emit_history_note(out);
        for (params, raw) in std::mem::take(&mut self.pre_start_updates) {
            self.handle_session_update(params, &raw, out);
        }
    }

    /// Reports the replayed history that was not turned into events.
    fn emit_history_note(&mut self, out: &mut Vec<AgentEvent>) {
        if self.history_replay.is_empty() {
            return;
        }
        let mut counts: BTreeMap<String, u64> = BTreeMap::new();
        for tag in self.history_replay.drain(..) {
            *counts.entry(tag).or_default() += 1;
        }
        out.push(AgentEvent::Unmapped {
            record_type: "session/load:history_replay".into(),
            raw: json!({ "updates": counts }),
        });
    }

    fn handle_session_update(
        &mut self,
        mut params: Value,
        raw_msg: &Value,
        out: &mut Vec<AgentEvent>,
    ) {
        if self.loading_history {
            let tag = params
                .get("update")
                .and_then(|u| u.get("sessionUpdate"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            self.history_replay.push(tag.to_owned());
            return;
        }
        if !self.session_started {
            self.pre_start_updates.push((params, raw_msg.clone()));
            return;
        }
        hoist_tool_name(&mut params);
        let Ok(notif) = serde_json::from_value::<SessionNotification>(params.clone()) else {
            let tag = params
                .get("update")
                .and_then(|u| u.get("sessionUpdate"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            out.push(AgentEvent::Unmapped {
                record_type: format!("session/update:{tag}"),
                raw: raw_msg.clone(),
            });
            return;
        };

        match notif.update {
            SessionUpdate::AgentMessageChunk(chunk) => {
                if let Some(text) = extract_content_text(&chunk.content) {
                    let msg_id = chunk.message_id.map(|m| m.0.to_string());
                    self.push_message_delta(msg_id, MessageKind::Assistant, text, out);
                }
            }
            SessionUpdate::AgentThoughtChunk(chunk) => {
                if let Some(text) = extract_content_text(&chunk.content) {
                    let msg_id = chunk.message_id.map(|m| m.0.to_string());
                    self.push_message_delta(msg_id, MessageKind::Reasoning, text, out);
                }
            }
            SessionUpdate::UserMessageChunk(_) => {}
            SessionUpdate::ToolCall(tc) => {
                self.handle_tool_call(tc, out);
            }
            SessionUpdate::ToolCallUpdate(tcu) => {
                self.handle_tool_call_update(tcu, out);
            }
            SessionUpdate::Plan(plan) => {
                let entries = plan
                    .entries
                    .into_iter()
                    .map(|e| PlanEntry {
                        content: e.content,
                        status: match e.status {
                            PlanEntryStatus::Pending => PlanStatus::Pending,
                            PlanEntryStatus::InProgress => PlanStatus::InProgress,
                            PlanEntryStatus::Completed => PlanStatus::Completed,
                            _ => PlanStatus::Pending,
                        },
                    })
                    .collect();
                out.push(AgentEvent::PlanUpdated { entries });
            }
            SessionUpdate::UsageUpdate(u) => {
                let mut usage = self.turn_usage.clone().unwrap_or_default();
                if usage.input_tokens.is_none() {
                    usage.input_tokens = Some(u.used);
                }
                if let Some(cost) = u.cost {
                    usage.cost_usd = Some(cost.amount);
                }
                self.turn_usage = Some(usage.clone());
                out.push(AgentEvent::Usage { usage });
            }
            SessionUpdate::AvailableCommandsUpdate(_) => {
                out.push(AgentEvent::Unmapped {
                    record_type: "session/update:available_commands_update".into(),
                    raw: raw_msg.clone(),
                });
            }
            SessionUpdate::CurrentModeUpdate(_) => {
                out.push(AgentEvent::Unmapped {
                    record_type: "session/update:current_mode_update".into(),
                    raw: raw_msg.clone(),
                });
            }
            SessionUpdate::ConfigOptionUpdate(cfg) => {
                if let Some(m) = extract_model_from_options(Some(&cfg.config_options)) {
                    self.configured_model = Some(m);
                }
                out.push(AgentEvent::Unmapped {
                    record_type: "session/update:config_option_update".into(),
                    raw: raw_msg.clone(),
                });
            }
            SessionUpdate::SessionInfoUpdate(_) => {
                out.push(AgentEvent::Unmapped {
                    record_type: "session/update:session_info_update".into(),
                    raw: raw_msg.clone(),
                });
            }
            _ => {
                let tag = params
                    .get("update")
                    .and_then(|u| u.get("sessionUpdate"))
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                out.push(AgentEvent::Unmapped {
                    record_type: format!("session/update:{tag}"),
                    raw: raw_msg.clone(),
                });
            }
        }
    }

    fn handle_tool_call(&mut self, tc: ToolCall, out: &mut Vec<AgentEvent>) {
        let call_id = tc.tool_call_id.0.to_string();
        if self.closed_tool_calls.contains(&call_id) {
            return;
        }
        let kind = map_tool_kind(tc.kind, tc.meta.as_ref(), &tc.title);
        let name = extract_tool_name(tc.meta.as_ref(), tc.kind, &tc.title);
        let title = non_empty_string(&tc.title);
        let input = normalize_tool_input(tc.raw_input.as_ref(), &tc.locations);

        if !self.open_tool_calls.contains_key(&call_id) {
            self.start_tool_call(
                OpenToolCall {
                    call_id: call_id.clone(),
                    name,
                    kind,
                    title,
                    input,
                    synthesized: false,
                    denied_reason: None,
                },
                out,
            );
        }

        match tc.status {
            ToolCallStatus::Completed => {
                let output = extract_tool_output(Some(&tc.content), tc.raw_output.as_ref());
                self.finish_tool_call(&call_id, ToolStatus::Succeeded, output, None, out);
            }
            ToolCallStatus::Failed => {
                let status = if self.cancel_requested {
                    ToolStatus::Cancelled
                } else {
                    ToolStatus::Failed
                };
                let error = if status == ToolStatus::Cancelled {
                    None
                } else {
                    extract_tool_output(Some(&tc.content), tc.raw_output.as_ref())
                        .or_else(|| Some("tool call failed".into()))
                };
                self.finish_tool_call(&call_id, status, None, error, out);
            }
            ToolCallStatus::Pending | ToolCallStatus::InProgress | _ => {}
        }
    }

    fn handle_tool_call_update(&mut self, tcu: ToolCallUpdate, out: &mut Vec<AgentEvent>) {
        let call_id = tcu.tool_call_id.0.to_string();
        if self.closed_tool_calls.contains(&call_id) {
            return;
        }

        let fields = tcu.fields;
        if !self.open_tool_calls.contains_key(&call_id) {
            let acp_kind = fields.kind.unwrap_or(AcpToolKind::Other);
            let title_str = fields.title.as_deref().unwrap_or_default();
            let kind = map_tool_kind(acp_kind, tcu.meta.as_ref(), title_str);
            let name = extract_tool_name(tcu.meta.as_ref(), acp_kind, title_str);
            let title = fields.title.as_deref().and_then(non_empty_string);
            let locations = fields.locations.clone().unwrap_or_default();
            let input = normalize_tool_input(fields.raw_input.as_ref(), &locations);
            self.start_tool_call(
                OpenToolCall {
                    call_id: call_id.clone(),
                    name,
                    kind,
                    title,
                    input,
                    synthesized: false,
                    denied_reason: None,
                },
                out,
            );
        }

        let new_title = fields.title.as_deref().and_then(non_empty_string);
        let new_kind = fields
            .kind
            .map(|k| map_tool_kind(k, tcu.meta.as_ref(), fields.title.as_deref().unwrap_or("")));
        let new_input = fields.raw_input.as_ref().and_then(parse_complete_json);
        let output = extract_tool_output(fields.content.as_deref(), fields.raw_output.as_ref());

        if let Some(open) = self.open_tool_calls.get_mut(&call_id) {
            if let Some(t) = &new_title {
                open.title = Some(t.clone());
            }
            if let Some(k) = new_kind {
                open.kind = k;
            }
            if let Some(i) = &new_input {
                open.input = i.clone();
            }
        }

        match fields.status {
            Some(ToolCallStatus::Completed) => {
                if new_title.is_some() || new_kind.is_some() || new_input.is_some() {
                    out.push(AgentEvent::ToolCallUpdated {
                        call_id: call_id.clone(),
                        title: new_title,
                        kind: new_kind,
                        input: new_input,
                        output_delta: None,
                        output: None,
                    });
                }
                self.finish_tool_call(&call_id, ToolStatus::Succeeded, output, None, out);
            }
            Some(ToolCallStatus::Failed) => {
                let status = if self.cancel_requested {
                    ToolStatus::Cancelled
                } else {
                    ToolStatus::Failed
                };
                let error = if status == ToolStatus::Cancelled {
                    None
                } else {
                    output.or_else(|| Some("tool call failed".into()))
                };
                self.finish_tool_call(&call_id, status, None, error, out);
            }
            _ => {
                if new_title.is_some()
                    || new_kind.is_some()
                    || new_input.is_some()
                    || output.is_some()
                {
                    out.push(AgentEvent::ToolCallUpdated {
                        call_id,
                        title: new_title,
                        kind: new_kind,
                        input: new_input,
                        output_delta: None,
                        output,
                    });
                }
            }
        }
    }

    fn handle_request_permission(
        &mut self,
        rpc_id: Option<String>,
        mut params: Value,
        raw_msg: &Value,
        out: &mut Vec<AgentEvent>,
    ) {
        hoist_tool_name(&mut params);
        let Ok(req) = serde_json::from_value::<RequestPermissionRequest>(params) else {
            out.push(AgentEvent::Unmapped {
                record_type: "session/request_permission:invalid".into(),
                raw: raw_msg.clone(),
            });
            return;
        };
        let call_id = req.tool_call.tool_call_id.0.to_string();
        if !self.open_tool_calls.contains_key(&call_id)
            && !self.closed_tool_calls.contains(&call_id)
        {
            let fields = &req.tool_call.fields;
            let acp_kind = fields.kind.unwrap_or(AcpToolKind::Other);
            let title_str = fields.title.as_deref().unwrap_or_default();
            let kind = map_tool_kind(acp_kind, req.tool_call.meta.as_ref(), title_str);
            let name = extract_tool_name(req.tool_call.meta.as_ref(), acp_kind, title_str);
            let title = fields.title.as_deref().and_then(non_empty_string);
            let locations = fields.locations.clone().unwrap_or_default();
            let input = normalize_tool_input(fields.raw_input.as_ref(), &locations);
            self.start_tool_call(
                OpenToolCall {
                    call_id: call_id.clone(),
                    name,
                    kind,
                    title,
                    input,
                    synthesized: false,
                    denied_reason: None,
                },
                out,
            );
        } else if let Some(open) = self.open_tool_calls.get_mut(&call_id) {
            // OpenCode announces a call with an empty input and only sends the
            // real input (the command, the path) with the permission request.
            // Surface it, so the user and the log see what is being asked.
            let fields = &req.tool_call.fields;
            let locations = fields.locations.clone().unwrap_or_default();
            let input = normalize_tool_input(fields.raw_input.as_ref(), &locations);
            let title = fields.title.as_deref().and_then(non_empty_string);
            let new_input = (input != open.input && input != json!({})).then_some(input);
            let new_title = title.filter(|t| open.title.as_deref() != Some(t.as_str()));
            if new_input.is_some() || new_title.is_some() {
                if let Some(i) = &new_input {
                    open.input = i.clone();
                }
                if let Some(t) = &new_title {
                    open.title = Some(t.clone());
                }
                out.push(AgentEvent::ToolCallUpdated {
                    call_id: call_id.clone(),
                    title: new_title,
                    kind: None,
                    input: new_input,
                    output_delta: None,
                    output: None,
                });
            }
        }

        if let Some(id) = rpc_id {
            self.rpc_to_call_id.insert(id.clone(), call_id.clone());
            self.pending_gateable_rpcs.push_back(PendingGateableRpc {
                rpc_id: id,
                call_id,
            });
        }
    }

    fn handle_fs_read_request(
        &mut self,
        rpc_id: Option<String>,
        params: Value,
        out: &mut Vec<AgentEvent>,
    ) {
        let path_str = serde_json::from_value::<ReadTextFileRequest>(params.clone())
            .map(|r| r.path.display().to_string())
            .unwrap_or_else(|_| {
                params
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            });
        let rpc_id_str = rpc_id.unwrap_or_else(|| {
            self.next_request_seq += 1;
            format!("fs-read-{}", self.next_request_seq)
        });
        let call_id = self.find_or_synthesize_client_tool_call(
            &rpc_id_str,
            &params,
            "fs/read_text_file",
            ToolKind::Read,
            Some(format!("Read {path_str}")),
            json!({ "path": path_str }),
            out,
        );
        self.rpc_to_call_id
            .insert(rpc_id_str.clone(), call_id.clone());
        self.pending_gateable_rpcs.push_back(PendingGateableRpc {
            rpc_id: rpc_id_str,
            call_id,
        });
    }

    fn handle_fs_write_request(
        &mut self,
        rpc_id: Option<String>,
        params: Value,
        out: &mut Vec<AgentEvent>,
    ) {
        let path_str = serde_json::from_value::<WriteTextFileRequest>(params.clone())
            .map(|r| r.path.display().to_string())
            .unwrap_or_else(|_| {
                params
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            });
        let rpc_id_str = rpc_id.unwrap_or_else(|| {
            self.next_request_seq += 1;
            format!("fs-write-{}", self.next_request_seq)
        });
        let call_id = self.find_or_synthesize_client_tool_call(
            &rpc_id_str,
            &params,
            "fs/write_text_file",
            ToolKind::Edit,
            Some(format!("Write {path_str}")),
            json!({ "path": path_str }),
            out,
        );
        self.rpc_to_call_id
            .insert(rpc_id_str.clone(), call_id.clone());
        self.pending_gateable_rpcs.push_back(PendingGateableRpc {
            rpc_id: rpc_id_str,
            call_id,
        });
    }

    fn handle_terminal_create_request(
        &mut self,
        rpc_id: Option<String>,
        params: Value,
        out: &mut Vec<AgentEvent>,
    ) {
        let (cmd, args) = match serde_json::from_value::<CreateTerminalRequest>(params.clone()) {
            Ok(r) => (r.command, r.args),
            Err(_) => {
                let c = params
                    .get("command")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let a = params
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                (c, a)
            }
        };
        let title_str = if args.is_empty() {
            cmd.clone()
        } else {
            format!("{} {}", cmd, args.join(" "))
        };
        let rpc_id_str = rpc_id.unwrap_or_else(|| {
            self.next_request_seq += 1;
            format!("term-{}", self.next_request_seq)
        });
        let call_id = self.find_or_synthesize_client_tool_call(
            &rpc_id_str,
            &params,
            "terminal/create",
            ToolKind::Execute,
            non_empty_string(&title_str),
            json!({ "command": cmd, "args": args }),
            out,
        );
        self.rpc_to_call_id
            .insert(rpc_id_str.clone(), call_id.clone());
        self.pending_gateable_rpcs.push_back(PendingGateableRpc {
            rpc_id: rpc_id_str,
            call_id,
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn find_or_synthesize_client_tool_call(
        &mut self,
        rpc_id: &str,
        params: &Value,
        default_name: &str,
        kind: ToolKind,
        title: Option<String>,
        input: Value,
        out: &mut Vec<AgentEvent>,
    ) -> String {
        if let Some(meta_call_id) = params
            .get("_meta")
            .and_then(|m| {
                m.get("toolCallId").or_else(|| {
                    m.get("claudeCode")
                        .and_then(|c| c.get("toolUseId").or_else(|| c.get("toolCallId")))
                })
            })
            .and_then(Value::as_str)
        {
            if self.open_tool_calls.contains_key(meta_call_id) {
                return meta_call_id.to_owned();
            }
            let call_id = meta_call_id.to_owned();
            self.start_tool_call(
                OpenToolCall {
                    call_id: call_id.clone(),
                    name: default_name.to_owned(),
                    kind,
                    title,
                    input,
                    synthesized: true,
                    denied_reason: None,
                },
                out,
            );
            return call_id;
        }

        if let Some(existing_id) = self
            .open_tool_order
            .iter()
            .rev()
            .find(|id| {
                self.open_tool_calls
                    .get(*id)
                    .is_some_and(|c| !c.synthesized && c.kind == kind)
            })
            .cloned()
        {
            return existing_id;
        }

        let call_id = format!("call-{rpc_id}");
        self.start_tool_call(
            OpenToolCall {
                call_id: call_id.clone(),
                name: default_name.to_owned(),
                kind,
                title,
                input,
                synthesized: true,
                denied_reason: None,
            },
            out,
        );
        call_id
    }

    fn handle_elicitation_create(
        &mut self,
        rpc_id: Option<String>,
        params: Value,
        raw_msg: &Value,
        out: &mut Vec<AgentEvent>,
    ) {
        let Ok(req) = serde_json::from_value::<CreateElicitationRequest>(params) else {
            out.push(AgentEvent::Unmapped {
                record_type: "elicitation/create:invalid".into(),
                raw: raw_msg.clone(),
            });
            return;
        };

        let request_id = rpc_id.unwrap_or_else(|| {
            self.next_request_seq += 1;
            format!("q-{}", self.next_request_seq)
        });

        let questions = extract_questions(&req);
        self.flush_open_message(out);
        self.pending_questions.insert(request_id.clone());
        self.pending_question_order.push(request_id.clone());
        out.push(AgentEvent::QuestionAsked {
            request_id,
            questions,
        });
    }

    fn handle_gate(
        &mut self,
        check: &ToolCheck,
        decision: &GateDecision,
        out: &mut Vec<AgentEvent>,
    ) {
        let matched_rpc = if let Some(cid) = &check.call_id {
            if let Some(pos) = self
                .pending_gateable_rpcs
                .iter()
                .position(|r| &r.call_id == cid)
            {
                self.pending_gateable_rpcs.remove(pos)
            } else {
                self.pending_gateable_rpcs.pop_front()
            }
        } else {
            self.pending_gateable_rpcs.pop_front()
        };

        let call_id = check
            .call_id
            .clone()
            .or_else(|| matched_rpc.as_ref().map(|r| r.call_id.clone()));

        if let Some(cid) = &call_id {
            if !self.open_tool_calls.contains_key(cid) && !self.closed_tool_calls.contains(cid) {
                self.start_tool_call(
                    OpenToolCall {
                        call_id: cid.clone(),
                        name: check.name.clone(),
                        kind: check.kind,
                        title: check.title.clone(),
                        input: check.input.clone(),
                        synthesized: false,
                        denied_reason: None,
                    },
                    out,
                );
            }
        }

        match decision {
            GateDecision::Allow => {}
            GateDecision::Deny { reason } => {
                if let Some(cid) = call_id {
                    if let Some(open) = self.open_tool_calls.get_mut(&cid) {
                        open.denied_reason = Some(reason.clone());
                    }
                    self.finish_tool_call(
                        &cid,
                        ToolStatus::Denied,
                        None,
                        Some(reason.clone()),
                        out,
                    );
                }
            }
            GateDecision::Ask => {
                let request_id = matched_rpc
                    .map(|r| r.rpc_id)
                    .or_else(|| call_id.clone())
                    .unwrap_or_else(|| {
                        self.next_request_seq += 1;
                        format!("req-{}", self.next_request_seq)
                    });
                self.pending_approvals.insert(
                    request_id.clone(),
                    PendingApproval {
                        request_id: request_id.clone(),
                        call_id: call_id.clone(),
                    },
                );
                self.pending_approval_order.push(request_id.clone());
                out.push(AgentEvent::ApprovalRequested {
                    request_id,
                    call_id,
                    name: check.name.clone(),
                    kind: check.kind,
                    title: check.title.clone(),
                    input: check.input.clone(),
                    reason: None,
                });
            }
        }
    }

    fn handle_command(&mut self, command: &SessionCommand, out: &mut Vec<AgentEvent>) {
        match command {
            SessionCommand::SendUserMessage { .. } => {
                if self.active_turn.is_some() {
                    if self.queued_user_messages >= MAX_QUEUED_MESSAGES {
                        out.push(AgentEvent::Error {
                            message: format!(
                                "message queue full (max {MAX_QUEUED_MESSAGES} queued messages)"
                            ),
                            fatal: false,
                        });
                    } else {
                        self.queued_user_messages += 1;
                    }
                }
            }
            SessionCommand::ResolveApproval {
                request_id,
                decision,
            } => {
                let Some(pending) = self.pending_approvals.remove(request_id) else {
                    out.push(AgentEvent::Error {
                        message: format!(
                            "unknown or already-resolved approval request: {request_id}"
                        ),
                        fatal: false,
                    });
                    return;
                };
                self.pending_approval_order.retain(|id| id != request_id);
                match decision {
                    ApprovalDecision::Allow => {
                        out.push(AgentEvent::RequestResolved {
                            request_id: pending.request_id,
                            outcome: RequestOutcome::Approved,
                        });
                    }
                    ApprovalDecision::Deny { message } => {
                        out.push(AgentEvent::RequestResolved {
                            request_id: pending.request_id,
                            outcome: RequestOutcome::Denied,
                        });
                        if let Some(cid) = pending.call_id {
                            let reason = message
                                .clone()
                                .unwrap_or_else(|| "denied by user".to_owned());
                            if let Some(open) = self.open_tool_calls.get_mut(&cid) {
                                open.denied_reason = Some(reason.clone());
                            }
                            self.finish_tool_call(
                                &cid,
                                ToolStatus::Denied,
                                None,
                                Some(reason),
                                out,
                            );
                        }
                    }
                }
            }
            SessionCommand::AnswerQuestion { request_id, .. } => {
                if !self.pending_questions.remove(request_id) {
                    out.push(AgentEvent::Error {
                        message: format!(
                            "unknown or already-resolved question request: {request_id}"
                        ),
                        fatal: false,
                    });
                    return;
                }
                self.pending_question_order.retain(|id| id != request_id);
                out.push(AgentEvent::RequestResolved {
                    request_id: request_id.clone(),
                    outcome: RequestOutcome::Answered,
                });
            }
            SessionCommand::Interrupt => {
                if self.active_turn.is_some() {
                    self.cancel_requested = true;
                }
            }
            SessionCommand::Shutdown => {
                if self.active_turn.is_some() {
                    self.cancel_requested = true;
                }
            }
        }
    }

    fn handle_prompt_response(&mut self, turn: u32, result: Value, out: &mut Vec<AgentEvent>) {
        self.flush_open_message(out);

        let (stop_reason, resp_usage) =
            match serde_json::from_value::<PromptResponse>(result.clone()) {
                Ok(resp) => (resp.stop_reason, resp.usage),
                Err(_) => {
                    let sr = match result.get("stopReason").and_then(Value::as_str) {
                        Some("cancelled") => StopReason::Cancelled,
                        Some("max_tokens") => StopReason::MaxTokens,
                        Some("max_turn_requests") => StopReason::MaxTurnRequests,
                        Some("refusal") => StopReason::Refusal,
                        _ => StopReason::EndTurn,
                    };
                    (sr, None)
                }
            };

        let outcome = match stop_reason {
            StopReason::Cancelled => TurnOutcome::Interrupted,
            StopReason::MaxTokens | StopReason::MaxTurnRequests => TurnOutcome::LimitReached,
            StopReason::Refusal => TurnOutcome::Failed,
            StopReason::EndTurn | _ => {
                if self.cancel_requested {
                    TurnOutcome::Interrupted
                } else {
                    TurnOutcome::Completed
                }
            }
        };

        let open_ids = std::mem::take(&mut self.open_tool_order);
        for cid in open_ids {
            if let Some(open) = self.open_tool_calls.remove(&cid) {
                self.closed_tool_calls.insert(cid.clone());
                let (status, error) = if let Some(reason) = open.denied_reason {
                    (ToolStatus::Denied, Some(reason))
                } else if outcome == TurnOutcome::Interrupted {
                    (ToolStatus::Cancelled, None)
                } else if open.synthesized {
                    (ToolStatus::Succeeded, None)
                } else {
                    (ToolStatus::Cancelled, None)
                };
                out.push(AgentEvent::ToolCallFinished {
                    call_id: cid,
                    status,
                    output: None,
                    error,
                });
            }
        }

        let approval_ids = std::mem::take(&mut self.pending_approval_order);
        for req_id in approval_ids {
            if self.pending_approvals.remove(&req_id).is_some() {
                out.push(AgentEvent::RequestResolved {
                    request_id: req_id,
                    outcome: RequestOutcome::Cancelled,
                });
            }
        }

        let question_ids = std::mem::take(&mut self.pending_question_order);
        for req_id in question_ids {
            if self.pending_questions.remove(&req_id) {
                out.push(AgentEvent::RequestResolved {
                    request_id: req_id,
                    outcome: RequestOutcome::Cancelled,
                });
            }
        }

        let mut final_usage = self.turn_usage.take();
        if let Some(u) = resp_usage {
            let prev_cost = final_usage.as_ref().and_then(|p| p.cost_usd);
            final_usage = Some(Usage {
                input_tokens: Some(u.input_tokens),
                output_tokens: Some(u.output_tokens),
                cache_read_tokens: u.cached_read_tokens,
                cache_write_tokens: u.cached_write_tokens,
                reasoning_tokens: u.thought_tokens,
                cost_usd: prev_cost,
            });
        }

        self.active_turn = None;
        self.cancel_requested = false;
        out.push(AgentEvent::TurnCompleted {
            turn,
            outcome,
            usage: final_usage,
        });
    }

    fn close_turn_on_error(&mut self, turn: u32, message: String, out: &mut Vec<AgentEvent>) {
        self.flush_open_message(out);

        let open_ids = std::mem::take(&mut self.open_tool_order);
        for cid in open_ids {
            if let Some(open) = self.open_tool_calls.remove(&cid) {
                self.closed_tool_calls.insert(cid.clone());
                let (status, error) = if let Some(reason) = open.denied_reason {
                    (ToolStatus::Denied, Some(reason))
                } else {
                    (ToolStatus::Cancelled, None)
                };
                out.push(AgentEvent::ToolCallFinished {
                    call_id: cid,
                    status,
                    output: None,
                    error,
                });
            }
        }

        let approval_ids = std::mem::take(&mut self.pending_approval_order);
        for req_id in approval_ids {
            if self.pending_approvals.remove(&req_id).is_some() {
                out.push(AgentEvent::RequestResolved {
                    request_id: req_id,
                    outcome: RequestOutcome::Cancelled,
                });
            }
        }

        let question_ids = std::mem::take(&mut self.pending_question_order);
        for req_id in question_ids {
            if self.pending_questions.remove(&req_id) {
                out.push(AgentEvent::RequestResolved {
                    request_id: req_id,
                    outcome: RequestOutcome::Cancelled,
                });
            }
        }

        let outcome = if self.cancel_requested {
            TurnOutcome::Interrupted
        } else {
            out.push(AgentEvent::Error {
                message,
                fatal: false,
            });
            TurnOutcome::Failed
        };

        let usage = self.turn_usage.take();
        self.active_turn = None;
        self.cancel_requested = false;
        out.push(AgentEvent::TurnCompleted {
            turn,
            outcome,
            usage,
        });
    }

    fn handle_exit(&mut self, code: Option<i32>, out: &mut Vec<AgentEvent>) {
        if self.session_ended {
            return;
        }
        self.session_ended = true;

        if !self.session_started {
            let reason = self
                .last_error_reason
                .clone()
                .or_else(|| (!self.stderr_lines.is_empty()).then(|| self.stderr_lines.join("\n")))
                .unwrap_or_else(|| match &self.deferred_start {
                    Some(d) => format!(
                        "ACP process exited before confirming model {} (code: {code:?})",
                        d.requested_model
                    ),
                    None => format!("ACP process exited before session started (code: {code:?})"),
                });
            if !self.fatal_error_emitted {
                self.fatal_error_emitted = true;
                out.push(AgentEvent::Error {
                    message: reason.clone(),
                    fatal: true,
                });
            }
            out.push(AgentEvent::SessionEnded {
                exit_code: code,
                reason: Some(reason),
            });
            return;
        }

        self.flush_open_message(out);

        let open_ids = std::mem::take(&mut self.open_tool_order);
        for cid in open_ids {
            if let Some(open) = self.open_tool_calls.remove(&cid) {
                self.closed_tool_calls.insert(cid.clone());
                let (status, error) = if let Some(reason) = open.denied_reason {
                    (ToolStatus::Denied, Some(reason))
                } else {
                    (ToolStatus::Cancelled, None)
                };
                out.push(AgentEvent::ToolCallFinished {
                    call_id: cid,
                    status,
                    output: None,
                    error,
                });
            }
        }

        let approval_ids = std::mem::take(&mut self.pending_approval_order);
        for req_id in approval_ids {
            if self.pending_approvals.remove(&req_id).is_some() {
                out.push(AgentEvent::RequestResolved {
                    request_id: req_id,
                    outcome: RequestOutcome::Cancelled,
                });
            }
        }

        let question_ids = std::mem::take(&mut self.pending_question_order);
        for req_id in question_ids {
            if self.pending_questions.remove(&req_id) {
                out.push(AgentEvent::RequestResolved {
                    request_id: req_id,
                    outcome: RequestOutcome::Cancelled,
                });
            }
        }

        if let Some(turn) = self.active_turn.take() {
            let outcome = if self.cancel_requested || code == Some(0) {
                TurnOutcome::Interrupted
            } else {
                TurnOutcome::Failed
            };
            out.push(AgentEvent::TurnCompleted {
                turn,
                outcome,
                usage: self.turn_usage.take(),
            });
        }

        let reason = if code.unwrap_or(0) != 0 {
            self.last_error_reason
                .clone()
                .or_else(|| (!self.stderr_lines.is_empty()).then(|| self.stderr_lines.join("\n")))
        } else {
            None
        };
        out.push(AgentEvent::SessionEnded {
            exit_code: code,
            reason,
        });
    }
}

impl Normalizer for AcpNormalizer {
    fn on_input(&mut self, input: &AdapterInput) -> Vec<AgentEvent> {
        if self.session_ended {
            return Vec::new();
        }
        let mut out = Vec::new();
        match input {
            AdapterInput::Stdin { line } => self.handle_stdin(line, &mut out),
            AdapterInput::Stdout { line } => self.handle_stdout(line, &mut out),
            AdapterInput::Stderr { line } => {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    self.stderr_lines.push(trimmed.to_owned());
                }
            }
            AdapterInput::HookRequest { .. } | AdapterInput::HookResponse { .. } => {}
            AdapterInput::Gate { check, decision } => {
                self.handle_gate(check, decision, &mut out);
            }
            AdapterInput::Command { command } => self.handle_command(command, &mut out),
            AdapterInput::Exit { code } => self.handle_exit(*code, &mut out),
        }
        out
    }
}

pub(crate) fn rpc_id_to_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn non_empty_string(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(s.to_owned())
    }
}

fn extract_meta_model(params: &Value) -> Option<String> {
    params
        .get("_meta")
        .and_then(|m| m.get("model"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn extract_model_from_options(options: Option<&[SessionConfigOption]>) -> Option<String> {
    let options = options?;
    for opt in options {
        let is_model = opt.id.0.as_ref() == "model"
            || matches!(opt.category, Some(SessionConfigOptionCategory::Model));
        if is_model {
            if let SessionConfigKind::Select(sel) = &opt.kind {
                return Some(sel.current_value.0.to_string());
            }
        }
    }
    None
}

fn extract_tools(result: &Value) -> Vec<String> {
    result
        .get("_meta")
        .and_then(|m| m.get("tools"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn extract_content_text(block: &ContentBlock) -> Option<String> {
    match block {
        ContentBlock::Text(t) => Some(t.text.clone()),
        ContentBlock::ResourceLink(r) => Some(r.uri.clone()),
        _ => None,
    }
}

pub(crate) fn map_tool_kind(
    kind: AcpToolKind,
    meta: Option<&serde_json::Map<String, Value>>,
    title: &str,
) -> ToolKind {
    if let Some(m) = meta {
        if m.get("mcp").and_then(Value::as_bool).unwrap_or(false)
            || m.get("toolName")
                .and_then(Value::as_str)
                .is_some_and(|n| n.starts_with("mcp__") || n.starts_with("mcp_"))
        {
            return ToolKind::Mcp;
        }
    }
    if title.starts_with("mcp__") || title.starts_with("mcp:") {
        return ToolKind::Mcp;
    }
    match kind {
        AcpToolKind::Read => ToolKind::Read,
        AcpToolKind::Edit => ToolKind::Edit,
        AcpToolKind::Delete => ToolKind::Delete,
        AcpToolKind::Move => ToolKind::Move,
        AcpToolKind::Search => ToolKind::Search,
        AcpToolKind::Execute => ToolKind::Execute,
        AcpToolKind::Think => ToolKind::Think,
        AcpToolKind::Fetch => ToolKind::Fetch,
        AcpToolKind::SwitchMode | AcpToolKind::Other | _ => ToolKind::Other,
    }
}

/// OpenCode puts the tool's name in a top-level `name` field of `tool_call`,
/// `tool_call_update` and the `toolCall` of `session/request_permission`
/// (live, v2.1.3), which the ACP schema does not have. Move it to
/// `_meta.toolName`, where [`extract_tool_name`] looks, so the gate and the
/// events see `shell` rather than a name guessed from the kind.
pub(crate) fn hoist_tool_name(params: &mut Value) {
    let target = if params.get("update").is_some() {
        params.get_mut("update")
    } else {
        params.get_mut("toolCall")
    };
    let Some(Value::Object(obj)) = target else {
        return;
    };
    let Some(name) = obj
        .get("name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
    else {
        return;
    };
    let meta = obj
        .entry("_meta")
        .or_insert_with(|| Value::Object(Default::default()));
    if let Value::Object(meta) = meta {
        meta.entry("toolName").or_insert(Value::String(name));
    }
}

pub(crate) fn extract_tool_name(
    meta: Option<&serde_json::Map<String, Value>>,
    kind: AcpToolKind,
    title: &str,
) -> String {
    if let Some(m) = meta {
        if let Some(name) = m
            .get("toolName")
            .or_else(|| m.get("name"))
            .or_else(|| {
                m.get("claudeCode")
                    .and_then(|c| c.get("toolName").or_else(|| c.get("name")))
            })
            .and_then(Value::as_str)
        {
            if !name.is_empty() {
                return name.to_owned();
            }
        }
    }
    match kind {
        AcpToolKind::Read => "read".into(),
        AcpToolKind::Edit => "edit".into(),
        AcpToolKind::Delete => "delete".into(),
        AcpToolKind::Move => "move".into(),
        AcpToolKind::Search => "search".into(),
        AcpToolKind::Execute => "execute".into(),
        AcpToolKind::Think => "think".into(),
        AcpToolKind::Fetch => "fetch".into(),
        AcpToolKind::SwitchMode => "switch_mode".into(),
        AcpToolKind::Other | _ => {
            if let Some(first_word) = title.split_whitespace().next() {
                if !first_word.is_empty() {
                    return first_word.to_lowercase();
                }
            }
            "other".into()
        }
    }
}

pub(crate) fn normalize_tool_input(
    raw_input: Option<&Value>,
    locations: &[ToolCallLocation],
) -> Value {
    if let Some(parsed) = raw_input.and_then(parse_complete_json) {
        if !parsed.is_null() {
            return parsed;
        }
    }
    if !locations.is_empty() {
        if locations.len() == 1 {
            return json!({ "path": locations[0].path });
        }
        let paths: Vec<_> = locations.iter().map(|l| &l.path).collect();
        return json!({ "paths": paths });
    }
    json!({})
}

fn parse_complete_json(val: &Value) -> Option<Value> {
    match val {
        Value::Null => None,
        Value::String(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                serde_json::from_str::<Value>(trimmed).ok()
            }
        }
        other => Some(other.clone()),
    }
}

fn extract_tool_output(
    content: Option<&[ToolCallContent]>,
    raw_output: Option<&Value>,
) -> Option<String> {
    if let Some(items) = content {
        let mut parts = Vec::new();
        for item in items {
            match item {
                ToolCallContent::Content(c) => {
                    if let Some(text) = extract_content_text(&c.content) {
                        if !text.is_empty() {
                            parts.push(text);
                        }
                    }
                }
                ToolCallContent::Diff(d) => {
                    parts.push(format!("diff {}", d.path.display()));
                }
                ToolCallContent::Terminal(t) => {
                    parts.push(format!("terminal:{}", t.terminal_id.0));
                }
                _ => {}
            }
        }
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    match raw_output {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Null) | None => None,
        Some(v) => Some(v.to_string()),
    }
}

pub(crate) fn extract_questions(req: &CreateElicitationRequest) -> Vec<Question> {
    match &req.mode {
        ElicitationMode::Form(form) => {
            if form.requested_schema.properties.is_empty() {
                return vec![Question {
                    id: "question".into(),
                    prompt: req.message.clone(),
                    options: Vec::new(),
                    multi_select: false,
                    allow_text: true,
                }];
            }
            let mut questions = Vec::new();
            for (prop_id, prop_schema) in &form.requested_schema.properties {
                match prop_schema {
                    ElicitationPropertySchema::String(s) => {
                        let prompt = s
                            .description
                            .as_deref()
                            .filter(|d| !d.is_empty())
                            .or_else(|| s.title.as_deref().filter(|t| !t.is_empty()))
                            .unwrap_or(&req.message)
                            .to_owned();
                        if let Some(one_of) = &s.one_of {
                            let options = one_of
                                .iter()
                                .map(|opt| QuestionOption {
                                    id: opt.value.clone(),
                                    label: opt.title.clone(),
                                    description: opt.description.clone(),
                                })
                                .collect();
                            questions.push(Question {
                                id: prop_id.clone(),
                                prompt,
                                options,
                                multi_select: false,
                                allow_text: false,
                            });
                        } else if let Some(enum_values) = &s.enum_values {
                            let options = enum_values
                                .iter()
                                .map(|val| QuestionOption {
                                    id: val.clone(),
                                    label: val.clone(),
                                    description: None,
                                })
                                .collect();
                            questions.push(Question {
                                id: prop_id.clone(),
                                prompt,
                                options,
                                multi_select: false,
                                allow_text: false,
                            });
                        } else {
                            questions.push(Question {
                                id: prop_id.clone(),
                                prompt,
                                options: Vec::new(),
                                multi_select: false,
                                allow_text: true,
                            });
                        }
                    }
                    ElicitationPropertySchema::Array(arr) => {
                        let prompt = arr
                            .description
                            .as_deref()
                            .filter(|d| !d.is_empty())
                            .or_else(|| arr.title.as_deref().filter(|t| !t.is_empty()))
                            .unwrap_or(&req.message)
                            .to_owned();
                        let options = match &arr.items {
                            MultiSelectItems::String(e) => e
                                .values
                                .iter()
                                .map(|val| QuestionOption {
                                    id: val.clone(),
                                    label: val.clone(),
                                    description: None,
                                })
                                .collect(),
                            MultiSelectItems::Titled(a) => a
                                .options
                                .iter()
                                .map(|opt| QuestionOption {
                                    id: opt.value.clone(),
                                    label: opt.title.clone(),
                                    description: opt.description.clone(),
                                })
                                .collect(),
                            _ => Vec::new(),
                        };
                        questions.push(Question {
                            id: prop_id.clone(),
                            prompt,
                            options,
                            multi_select: true,
                            allow_text: false,
                        });
                    }
                    ElicitationPropertySchema::Boolean(b) => {
                        let prompt = b
                            .description
                            .as_deref()
                            .filter(|d| !d.is_empty())
                            .or_else(|| b.title.as_deref().filter(|t| !t.is_empty()))
                            .unwrap_or(&req.message)
                            .to_owned();
                        questions.push(Question {
                            id: prop_id.clone(),
                            prompt,
                            options: vec![
                                QuestionOption {
                                    id: "true".into(),
                                    label: "Yes".into(),
                                    description: None,
                                },
                                QuestionOption {
                                    id: "false".into(),
                                    label: "No".into(),
                                    description: None,
                                },
                            ],
                            multi_select: false,
                            allow_text: false,
                        });
                    }
                    ElicitationPropertySchema::Integer(i) => {
                        let prompt = i
                            .description
                            .as_deref()
                            .filter(|d| !d.is_empty())
                            .or_else(|| i.title.as_deref().filter(|t| !t.is_empty()))
                            .unwrap_or(&req.message)
                            .to_owned();
                        questions.push(Question {
                            id: prop_id.clone(),
                            prompt,
                            options: Vec::new(),
                            multi_select: false,
                            allow_text: true,
                        });
                    }
                    ElicitationPropertySchema::Number(n) => {
                        let prompt = n
                            .description
                            .as_deref()
                            .filter(|d| !d.is_empty())
                            .or_else(|| n.title.as_deref().filter(|t| !t.is_empty()))
                            .unwrap_or(&req.message)
                            .to_owned();
                        questions.push(Question {
                            id: prop_id.clone(),
                            prompt,
                            options: Vec::new(),
                            multi_select: false,
                            allow_text: true,
                        });
                    }
                    _ => {
                        questions.push(Question {
                            id: prop_id.clone(),
                            prompt: req.message.clone(),
                            options: Vec::new(),
                            multi_select: false,
                            allow_text: true,
                        });
                    }
                }
            }
            questions
        }
        ElicitationMode::Url(url_mode) => {
            let id = match &url_mode.scope {
                ElicitationScope::Session(_) | ElicitationScope::Request(_) | _ => {
                    url_mode.elicitation_id.0.to_string()
                }
            };
            vec![Question {
                id,
                prompt: format!("{} ({})", req.message, url_mode.url),
                options: Vec::new(),
                multi_select: false,
                allow_text: true,
            }]
        }
        _ => vec![Question {
            id: "question".into(),
            prompt: req.message.clone(),
            options: Vec::new(),
            multi_select: false,
            allow_text: true,
        }],
    }
}
