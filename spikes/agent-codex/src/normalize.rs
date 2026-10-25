use std::collections::{BTreeMap, BTreeSet};

use atrium_agents::{AdapterInput, GateDecision, Normalizer, ToolCheck};
use atrium_protocol::agent::*;
use serde_json::{Value, json};

#[derive(Default)]
pub struct CodexNormalizer {
    pub(crate) native_id: Option<String>,
    pub(crate) turn_id: Option<String>,
    turn: u32,
    messages: BTreeMap<String, (MessageKind, String)>,
    completed: BTreeSet<String>,
    pub(crate) tools: BTreeMap<String, Value>,
    requests: BTreeMap<String, Pending>,
    rpc: BTreeMap<String, String>,
    usage: Option<Usage>,
    total: Value,
    baseline: Value,
    ended: bool,
    interrupted: bool,
    denied: BTreeSet<String>,
    model: Option<String>,
    prelude: Vec<AgentEvent>,
    starting: bool,
    queued: usize,
    resumed: bool,
    usage_baseline_known: bool,
    errors: BTreeSet<String>,
    quota_reset: Option<u64>,
}

struct Pending {
    check: Option<ToolCheck>,
    questions: Vec<Question>,
    exposed: bool,
    outcome: Option<RequestOutcome>,
}

pub(crate) fn id(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}
fn s(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or_default().to_owned()
}
fn opt(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(str::to_owned)
}
fn unmapped(kind: &str, raw: &Value) -> AgentEvent {
    AgentEvent::Unmapped {
        record_type: kind.into(),
        raw: raw.clone(),
    }
}

pub(crate) fn tool_kind(item: &Value) -> ToolKind {
    match item["type"].as_str().unwrap_or_default() {
        "commandExecution" => {
            let actions = item["commandActions"].as_array();
            if actions.is_some_and(|a| !a.is_empty() && a.iter().all(|a| a["type"] == "read")) {
                ToolKind::Read
            } else if actions.is_some_and(|a| {
                !a.is_empty()
                    && a.iter()
                        .all(|a| matches!(a["type"].as_str(), Some("search" | "listFiles")))
            }) {
                ToolKind::Search
            } else {
                ToolKind::Execute
            }
        }
        "fileChange" => ToolKind::Edit,
        "mcpToolCall" => ToolKind::Mcp,
        "webSearch" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

impl CodexNormalizer {
    // Reserve channel slots for cleanup, including future Ask results for
    // already received gate requests. This lets shutdown close the lifecycle
    // without retaining an event delivery task when the receiver never reads.
    pub(crate) fn closing_slots(&self) -> usize {
        1 + self.messages.len()
            + self.tools.len()
            + self.requests.len() * 2
            + usize::from(self.turn_id.is_some())
    }
    pub(crate) fn check(&self, method: &str, p: &Value) -> ToolCheck {
        if method == "mcpServer/elicitation/request" {
            return ToolCheck {
                call_id: opt(p, "itemId"),
                name: s(p, "serverName"),
                kind: ToolKind::Mcp,
                title: opt(p, "message"),
                input: p.clone(),
            };
        }
        let item = self.tools.get(&s(p, "itemId")).cloned().unwrap_or_else(|| json!({"type": if method.contains("fileChange") {"fileChange"} else {"commandExecution"}}));
        let mut input = p.clone();
        input["item"] = item.clone();
        ToolCheck {
            call_id: opt(p, "itemId"),
            name: s(&item, "type"),
            kind: tool_kind(&item),
            title: opt(p, "command").or_else(|| opt(&item, "command")),
            input,
        }
    }
    pub(crate) fn pending_mcp(&self, request_id: &str) -> bool {
        self.requests.get(request_id).is_some_and(|r| {
            r.outcome.is_none() && r.check.as_ref().is_some_and(|c| c.kind == ToolKind::Mcp)
        })
    }
    pub(crate) fn approval_result(&self, request_id: &str, action: &str) -> Value {
        if self
            .request_check(request_id)
            .is_some_and(|c| c.kind == ToolKind::Mcp)
        {
            // Elicitation acceptance is consent, not fabricated form input.
            json!({"action":action,"content":null})
        } else {
            json!({"decision":action})
        }
    }
    pub(crate) fn request_check(&self, request_id: &str) -> Option<ToolCheck> {
        self.requests.get(request_id).and_then(|r| r.check.clone())
    }
    pub(crate) fn is_pending(&self, request_id: &str, question: bool) -> bool {
        self.requests
            .get(request_id)
            .is_some_and(|r| r.exposed && r.outcome.is_none() && (r.check.is_none() == question))
    }
    pub(crate) fn answer_labels(
        &self,
        request_id: &str,
        answers: &[QuestionAnswer],
    ) -> Option<Value> {
        let r = self.requests.get(request_id)?;
        if answers.len() != r.questions.len() {
            return None;
        }
        let mut result = serde_json::Map::new();
        for a in answers {
            let q = r.questions.iter().find(|q| q.id == a.question_id)?;
            if result.contains_key(&q.id)
                || (!q.multi_select && a.selected.len() > 1)
                || (a.text.is_some() && !q.allow_text)
            {
                return None;
            }
            let mut labels = Vec::new();
            for selected in &a.selected {
                labels.push(q.options.iter().find(|o| &o.id == selected)?.label.clone());
            }
            if let Some(text) = &a.text {
                labels.push(text.clone());
            }
            if labels.is_empty() {
                return None;
            }
            result.insert(q.id.clone(), json!({"answers": labels}));
        }
        Some(json!({"answers": result}))
    }
    fn started(&mut self, thread: &Value, model: Option<String>, out: &mut Vec<AgentEvent>) {
        if self.native_id.is_none()
            && let Some(native) = thread["id"].as_str()
        {
            self.native_id = Some(native.into());
            out.push(AgentEvent::SessionStarted {
                provider: Provider::Codex,
                native_session_id: native.into(),
                model: model.or_else(|| self.model.clone()),
                tools: vec![],
            });
        }
    }
    fn complete_message(
        &mut self,
        key: &str,
        fallback: Option<(MessageKind, String)>,
        out: &mut Vec<AgentEvent>,
    ) {
        if self.completed.contains(key) {
            return;
        }
        if let Some((kind, text)) = self.messages.remove(key).or(fallback) {
            out.push(AgentEvent::MessageCompleted {
                message_id: key.into(),
                kind,
                text,
            });
            self.completed.insert(key.into());
        }
    }
    fn close(&mut self, outcome: TurnOutcome, out: &mut Vec<AgentEvent>) {
        for key in self.messages.keys().cloned().collect::<Vec<_>>() {
            self.complete_message(&key, None, out);
        }
        for (call_id, _) in std::mem::take(&mut self.tools) {
            out.push(AgentEvent::ToolCallFinished {
                call_id,
                status: ToolStatus::Cancelled,
                output: None,
                error: None,
            });
        }
        for (request_id, r) in std::mem::take(&mut self.requests) {
            if r.exposed {
                out.push(AgentEvent::RequestResolved {
                    request_id,
                    outcome: r.outcome.unwrap_or(RequestOutcome::Cancelled),
                });
            }
        }
        if self.turn_id.take().is_some() {
            out.push(AgentEvent::TurnCompleted {
                turn: self.turn,
                outcome,
                usage: self.usage.take(),
            });
        }
    }
    fn record(&mut self, v: &Value, out: &mut Vec<AgentEvent>) {
        let method = v["method"].as_str().unwrap_or_default();
        let p = &v["params"];
        if method.is_empty() {
            let request = self.rpc.remove(&id(&v["id"]));
            if let Some(error) = v.get("error") {
                out.push(AgentEvent::Error {
                    message: error.to_string(),
                    fatal: self.native_id.is_none(),
                });
            } else if matches!(request.as_deref(), Some("thread/start" | "thread/resume")) {
                self.started(&v["result"]["thread"], opt(&v["result"], "model"), out);
            } else if request.is_none() {
                out.push(unmapped("response", v));
            }
            return;
        }
        match method {
            "thread/started" => self.started(&p["thread"], opt(p, "model"), out),
            "turn/started" => {
                if self.turn_id.is_none() {
                    self.turn += 1;
                    self.errors.clear();
                    self.turn_id = opt(&p["turn"], "id");
                    self.usage = None;
                    self.baseline = self.total.clone();
                    self.usage_baseline_known = !self.resumed || !self.total.is_null();
                    self.interrupted = false;
                    self.starting = false;
                    out.push(AgentEvent::TurnStarted { turn: self.turn });
                }
            }
            "turn/completed" => {
                if let Some(error) = p["turn"].get("error").filter(|e| !e.is_null()) {
                    self.error(error, out);
                }
                let outcome = match p["turn"]["status"].as_str() {
                    Some("completed") => TurnOutcome::Completed,
                    Some("interrupted") => TurnOutcome::Interrupted,
                    _ => TurnOutcome::Failed,
                };
                self.close(outcome, out);
                if self.queued > 0 {
                    self.queued -= 1;
                    self.starting = true;
                }
            }
            "item/agentMessage/delta"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/textDelta" => {
                let kind = if method.contains("reasoning") {
                    MessageKind::Reasoning
                } else {
                    MessageKind::Assistant
                };
                // Summary and raw reasoning have distinct ids to avoid mixing streams.
                let suffix = if method.contains("summary") {
                    ":summary"
                } else if kind == MessageKind::Reasoning {
                    ":reasoning"
                } else {
                    ""
                };
                let key = format!("{}{suffix}", s(p, "itemId"));
                let text = s(p, "delta");
                self.messages
                    .entry(key.clone())
                    .or_insert((kind, String::new()))
                    .1
                    .push_str(&text);
                out.push(AgentEvent::MessageDelta {
                    message_id: key,
                    kind,
                    text,
                });
            }
            "item/started" | "item/completed" => {
                self.item(&p["item"], method == "item/completed", out)
            }
            "item/commandExecution/outputDelta" => out.push(AgentEvent::ToolCallUpdated {
                call_id: s(p, "itemId"),
                title: None,
                kind: None,
                input: None,
                output_delta: opt(p, "delta"),
                output: None,
            }),
            "item/fileChange/outputDelta" => out.push(AgentEvent::ToolCallUpdated {
                call_id: s(p, "itemId"),
                title: None,
                kind: None,
                input: None,
                output_delta: opt(p, "delta"),
                output: None,
            }),
            "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "mcpServer/elicitation/request" => {
                let check = self.check(method, p);
                self.requests.insert(
                    id(&v["id"]),
                    Pending {
                        check: Some(check),
                        questions: vec![],
                        exposed: false,
                        outcome: None,
                    },
                );
            }
            "item/tool/requestUserInput" => {
                let request_id = id(&v["id"]);
                let questions = p["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|q| Question {
                        id: s(q, "id"),
                        prompt: s(q, "question"),
                        multi_select: false,
                        allow_text: q["isOther"].as_bool().unwrap_or(false)
                            || !q["options"].is_array(),
                        options: q["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .enumerate()
                            .map(|(i, o)| QuestionOption {
                                id: i.to_string(),
                                label: s(o, "label"),
                                description: opt(o, "description"),
                            })
                            .collect(),
                    })
                    .collect::<Vec<_>>();
                self.requests.insert(
                    request_id.clone(),
                    Pending {
                        check: None,
                        questions: questions.clone(),
                        exposed: true,
                        outcome: None,
                    },
                );
                out.push(AgentEvent::QuestionAsked {
                    request_id,
                    questions,
                });
            }
            "serverRequest/resolved" => {
                let request_id = id(&p["requestId"]);
                if let Some(r) = self.requests.remove(&request_id)
                    && r.exposed
                {
                    out.push(AgentEvent::RequestResolved {
                        request_id,
                        outcome: r.outcome.unwrap_or(RequestOutcome::Cancelled),
                    });
                }
            }
            "turn/plan/updated" => out.push(AgentEvent::PlanUpdated {
                entries: p["plan"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|e| PlanEntry {
                        content: s(e, "step"),
                        status: match e["status"].as_str() {
                            Some("inProgress") => PlanStatus::InProgress,
                            Some("completed") => PlanStatus::Completed,
                            _ => PlanStatus::Pending,
                        },
                    })
                    .collect(),
            }),
            "thread/tokenUsage/updated" => {
                let t = &p["tokenUsage"]["total"];
                if self.turn_id.is_none() {
                    self.total = t.clone();
                    return;
                }
                // Resumed threads do not necessarily send the historical total.
                // Preserve the raw record rather than attribute old tokens to
                // the new turn when there is no subtraction baseline.
                if !self.usage_baseline_known {
                    self.total = t.clone();
                    out.push(unmapped(
                        "thread/tokenUsage/updated:missing_resume_baseline",
                        v,
                    ));
                    return;
                }
                let count = |k: &str| {
                    t[k].as_u64()
                        .map(|v| v.saturating_sub(self.baseline[k].as_u64().unwrap_or(0)))
                };
                let usage = Usage {
                    input_tokens: count("inputTokens"),
                    output_tokens: count("outputTokens"),
                    cache_read_tokens: count("cachedInputTokens"),
                    cache_write_tokens: count("cacheWriteInputTokens"),
                    reasoning_tokens: count("reasoningOutputTokens"),
                    cost_usd: None,
                };
                self.total = t.clone();
                self.usage = Some(usage.clone());
                out.push(AgentEvent::Usage { usage });
            }
            "account/rateLimits/updated" => {
                let r = &p["rateLimits"];
                let windows = ["primary", "secondary"]
                    .into_iter()
                    .filter_map(|name| {
                        let w = &r[name];
                        if w.is_null() {
                            return None;
                        }
                        Some(QuotaWindow {
                            name: name.into(),
                            used_percent: w["usedPercent"].as_f64(),
                            resets_at_ms: w["resetsAt"].as_u64().map(|x| x.saturating_mul(1000)),
                        })
                    })
                    .collect();
                let windows: Vec<QuotaWindow> = windows;
                self.quota_reset = windows
                    .iter()
                    .filter(|w| w.used_percent.is_some_and(|p| p >= 100.0))
                    .filter_map(|w| w.resets_at_ms)
                    .max()
                    .or_else(|| windows.iter().filter_map(|w| w.resets_at_ms).min());
                out.push(AgentEvent::QuotaUpdated { windows });
            }
            "error" => {
                if p["willRetry"] != true {
                    self.error(&p["error"], out);
                }
            }
            _ => out.push(unmapped(method, v)),
        }
    }
    fn error(&mut self, error: &Value, out: &mut Vec<AgentEvent>) {
        if !self.errors.insert(error.to_string()) {
            return;
        }
        let info = &error["codexErrorInfo"];
        if info == "usageLimitExceeded"
            || info == "rateLimitExceeded"
            || info["httpConnectionFailed"]["httpStatusCode"] == 429
        {
            out.push(AgentEvent::RateLimited {
                resets_at_ms: self.quota_reset,
                message: opt(error, "message"),
            });
        } else {
            out.push(AgentEvent::Error {
                message: opt(error, "message").unwrap_or_else(|| error.to_string()),
                fatal: false,
            });
        }
    }
    fn item(&mut self, item: &Value, done: bool, out: &mut Vec<AgentEvent>) {
        let key = s(item, "id");
        let typ = s(item, "type");
        if typ == "agentMessage" {
            if done {
                self.complete_message(&key, Some((MessageKind::Assistant, s(item, "text"))), out);
            }
            return;
        }
        if typ == "reasoning" {
            if done {
                for (field, suffix) in [("summary", ":summary"), ("content", ":reasoning")] {
                    let k = format!("{key}{suffix}");
                    let text = item[field].as_array().map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("\n")
                    });
                    self.complete_message(
                        &k,
                        text.filter(|s| !s.is_empty())
                            .map(|t| (MessageKind::Reasoning, t)),
                        out,
                    );
                }
            }
            return;
        }
        if typ == "userMessage" {
            return;
        }
        if !matches!(
            typ.as_str(),
            "commandExecution"
                | "fileChange"
                | "mcpToolCall"
                | "webSearch"
                | "dynamicToolCall"
                | "collabAgentToolCall"
        ) {
            out.push(unmapped(&format!("item/{typ}"), item));
            return;
        }
        if !self.tools.contains_key(&key) {
            out.push(AgentEvent::ToolCallStarted {
                call_id: key.clone(),
                name: typ,
                kind: tool_kind(item),
                title: opt(item, "command").or_else(|| opt(item, "tool")),
                input: item.clone(),
            });
        } else if done {
            out.push(AgentEvent::ToolCallUpdated {
                call_id: key.clone(),
                title: None,
                kind: Some(tool_kind(item)),
                input: Some(item.clone()),
                output_delta: None,
                output: opt(item, "aggregatedOutput"),
            });
        }
        if done {
            let denied = self.denied.remove(&key)
                || self.requests.values().any(|r| {
                    r.check
                        .as_ref()
                        .is_some_and(|c| c.call_id.as_deref() == Some(&key))
                        && r.outcome == Some(RequestOutcome::Denied)
                });
            let status = if denied || item["status"] == "declined" {
                ToolStatus::Denied
            } else if item["status"] == "failed" {
                ToolStatus::Failed
            } else {
                ToolStatus::Succeeded
            };
            out.push(AgentEvent::ToolCallFinished {
                call_id: key.clone(),
                status,
                output: opt(item, "aggregatedOutput").or_else(|| {
                    item.get("result")
                        .filter(|r| !r.is_null())
                        .map(Value::to_string)
                }),
                error: item
                    .get("error")
                    .filter(|e| !e.is_null())
                    .map(Value::to_string),
            });
            self.tools.remove(&key);
        } else {
            self.tools.insert(key, item.clone());
        }
    }
}

impl Normalizer for CodexNormalizer {
    fn on_input(&mut self, input: &AdapterInput) -> Vec<AgentEvent> {
        let mut out = vec![];
        if self.ended {
            return out;
        }
        match input {
            AdapterInput::Stdout { line } => match serde_json::from_str(line) {
                Ok(v) => self.record(&v, &mut out),
                Err(_) => out.push(unmapped("invalid_json", &json!(line))),
            },
            AdapterInput::Stdin { line } => {
                if let Ok(v) = serde_json::from_str::<Value>(line) {
                    if v.get("method").is_some() && v.get("id").is_some() {
                        self.rpc.insert(id(&v["id"]), s(&v, "method"));
                        if matches!(v["method"].as_str(), Some("thread/start" | "thread/resume")) {
                            self.model = opt(&v["params"], "model");
                            self.resumed = v["method"] == "thread/resume";
                        }
                    } else if let Some(r) = self.requests.get_mut(&id(&v["id"])) {
                        if v["result"]["decision"] == "decline"
                            && let Some(call_id) = r.check.as_ref().and_then(|c| c.call_id.clone())
                        {
                            self.denied.insert(call_id);
                        }
                        r.outcome = Some(if r.check.is_none() {
                            RequestOutcome::Answered
                        } else if v["result"]["action"] == "cancel" {
                            RequestOutcome::Cancelled
                        } else if v["result"]["decision"] == "accept"
                            || v["result"]["action"] == "accept"
                        {
                            RequestOutcome::Approved
                        } else {
                            RequestOutcome::Denied
                        });
                    }
                }
            }
            AdapterInput::Gate { check, decision } => {
                if let Some((request_id, r)) = self.requests.iter_mut().find(|(_, r)| {
                    r.check.as_ref() == Some(check) && !r.exposed && r.outcome.is_none()
                }) {
                    match decision {
                        GateDecision::Ask => {
                            r.exposed = true;
                            out.push(AgentEvent::ApprovalRequested {
                                request_id: request_id.clone(),
                                call_id: check.call_id.clone(),
                                name: check.name.clone(),
                                kind: check.kind,
                                title: check.title.clone(),
                                input: check.input.clone(),
                                reason: opt(&check.input, "reason")
                                    .or_else(|| opt(&check.input, "message")),
                            });
                        }
                        GateDecision::Allow => r.outcome = Some(RequestOutcome::Approved),
                        GateDecision::Deny { .. } => {
                            r.outcome = Some(RequestOutcome::Denied);
                            if let Some(call_id) = &check.call_id {
                                self.denied.insert(call_id.clone());
                            }
                        }
                    }
                }
            }
            AdapterInput::Command { command } => match command {
                SessionCommand::Interrupt => {
                    self.interrupted = true;
                    self.queued = 0;
                }
                SessionCommand::SendUserMessage { .. } => {
                    if self.starting || self.turn_id.is_some() {
                        if self.queued < atrium_agents::MAX_QUEUED_MESSAGES {
                            self.queued += 1;
                        } else {
                            out.push(AgentEvent::Error {
                                message: "user message queue full".into(),
                                fatal: false,
                            });
                        }
                    } else {
                        self.starting = true;
                    }
                }
                SessionCommand::ResolveApproval { request_id, .. }
                    if !self.is_pending(request_id, false) =>
                {
                    out.push(AgentEvent::Error {
                        message: format!("unknown or resolved approval {request_id}"),
                        fatal: false,
                    })
                }
                SessionCommand::AnswerQuestion {
                    request_id,
                    answers,
                } if !self.is_pending(request_id, true)
                    || self.answer_labels(request_id, answers).is_none() =>
                {
                    out.push(AgentEvent::Error {
                        message: format!("invalid question answer {request_id}"),
                        fatal: false,
                    })
                }
                SessionCommand::ResolveApproval {
                    request_id,
                    decision,
                } => {
                    let outcome = if matches!(decision, ApprovalDecision::Allow) {
                        RequestOutcome::Approved
                    } else {
                        RequestOutcome::Denied
                    };
                    let r = self
                        .requests
                        .get_mut(request_id)
                        .expect("validated approval");
                    r.outcome = Some(outcome);
                    if outcome == RequestOutcome::Denied
                        && let Some(call) = r.check.as_ref().and_then(|c| c.call_id.clone())
                    {
                        self.denied.insert(call);
                    }
                }
                SessionCommand::AnswerQuestion { request_id, .. } => {
                    self.requests
                        .get_mut(request_id)
                        .expect("validated question")
                        .outcome = Some(RequestOutcome::Answered)
                }
                _ => {}
            },
            AdapterInput::Exit { code } => {
                if self.native_id.is_none()
                    && !self
                        .prelude
                        .iter()
                        .any(|e| matches!(e, AgentEvent::Error { fatal: true, .. }))
                {
                    out.push(AgentEvent::Error {
                        message: "provider exited before session establishment".into(),
                        fatal: true,
                    });
                }
                self.close(
                    if self.interrupted {
                        TurnOutcome::Interrupted
                    } else {
                        TurnOutcome::Failed
                    },
                    &mut out,
                );
                out.push(AgentEvent::SessionEnded {
                    exit_code: *code,
                    reason: None,
                });
                self.ended = true;
            }
            AdapterInput::Stderr { line } => out.push(unmapped("stderr", &json!(line))),
            _ => out.push(unmapped(
                "adapter_input",
                &serde_json::to_value(input).expect("input serializes"),
            )),
        }
        if self.native_id.is_none() && !matches!(input, AdapterInput::Exit { .. }) {
            self.prelude.extend(out);
            return vec![];
        }
        if !self.prelude.is_empty() {
            let prelude = std::mem::take(&mut self.prelude);
            if let Some(pos) = out
                .iter()
                .position(|e| matches!(e, AgentEvent::SessionStarted { .. }))
            {
                out.splice(pos + 1..pos + 1, prelude);
            } else {
                let mut all = prelude;
                all.extend(out);
                out = all;
                if let Some(pos) = out
                    .iter()
                    .position(|e| matches!(e, AgentEvent::Error { fatal: true, .. }))
                {
                    let fatal = out.remove(pos);
                    out.insert(0, fatal);
                }
            }
        }
        out
    }
}
