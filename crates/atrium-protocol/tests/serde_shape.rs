use atrium_protocol::agent::*;
use atrium_protocol::policy::*;
use atrium_protocol::thread::*;
use atrium_protocol::wire::*;
use serde_json::json;

fn round_trip<T>(value: &T, expected: serde_json::Value)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    assert_eq!(serde_json::to_value(value).unwrap(), expected);
    assert_eq!(&serde_json::from_value::<T>(expected).unwrap(), value);
}

#[test]
fn agent_providers_round_trip() {
    round_trip(&Provider::Claude, json!("claude"));
    round_trip(&Provider::Codex, json!("codex"));
    round_trip(&Provider::Agy, json!("agy"));
    round_trip(&Provider::Acp, json!("acp"));
}

#[test]
fn agent_message_kinds_and_bodies() {
    round_trip(&MessageKind::User, json!("user"));
    round_trip(&MessageKind::Assistant, json!("assistant"));
    round_trip(&MessageKind::Reasoning, json!("reasoning"));

    round_trip(
        &MessageBody::Inline {
            text: "hello world".into(),
        },
        json!({"type": "inline", "text": "hello world"}),
    );
    round_trip(
        &MessageBody::Streamed {
            chars: 11,
            deltas: 2,
            sha256: "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9".into(),
        },
        json!({
            "type": "streamed",
            "chars": 11,
            "deltas": 2,
            "sha256": "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        }),
    );
}

#[test]
fn agent_tool_kinds_and_statuses() {
    round_trip(&ToolKind::Read, json!("read"));
    round_trip(&ToolKind::Edit, json!("edit"));
    round_trip(&ToolKind::Delete, json!("delete"));
    round_trip(&ToolKind::Move, json!("move"));
    round_trip(&ToolKind::Search, json!("search"));
    round_trip(&ToolKind::Execute, json!("execute"));
    round_trip(&ToolKind::Fetch, json!("fetch"));
    round_trip(&ToolKind::Think, json!("think"));
    round_trip(&ToolKind::Mcp, json!("mcp"));
    round_trip(&ToolKind::Other, json!("other"));

    round_trip(&ToolStatus::Succeeded, json!("succeeded"));
    round_trip(&ToolStatus::Failed, json!("failed"));
    round_trip(&ToolStatus::Denied, json!("denied"));
    round_trip(&ToolStatus::Cancelled, json!("cancelled"));
}

#[test]
fn agent_outcomes() {
    round_trip(&TurnOutcome::Completed, json!("completed"));
    round_trip(&TurnOutcome::Interrupted, json!("interrupted"));
    round_trip(&TurnOutcome::Failed, json!("failed"));
    round_trip(&TurnOutcome::LimitReached, json!("limit_reached"));

    round_trip(&RequestOutcome::Approved, json!("approved"));
    round_trip(&RequestOutcome::Denied, json!("denied"));
    round_trip(&RequestOutcome::Answered, json!("answered"));
    round_trip(&RequestOutcome::Cancelled, json!("cancelled"));
    round_trip(&RequestOutcome::Expired, json!("expired"));

    round_trip(&PlanStatus::Pending, json!("pending"));
    round_trip(&PlanStatus::InProgress, json!("in_progress"));
    round_trip(&PlanStatus::Completed, json!("completed"));
}

#[test]
fn agent_all_event_variants() {
    let dummy_enforcement = EnforcementDescriptor {
        level: EnforcementLevel::Full,
        reasons: vec![],
        provider: Provider::Claude,
        cli_version: Some("1.0.0".into()),
        version_tested: true,
        coverage: vec![ClassCoverage {
            class: ToolClass::Execute,
            coverage: Coverage::Intercepted,
            evidence: Some("fixture/test".into()),
        }],
        config_sources: vec![ConfigSource {
            kind: ConfigSourceKind::UserSettings,
            path: "~/config.json".into(),
            sha256: Some("abcd".into()),
            inspected: true,
        }],
    };

    round_trip(
        &AgentEvent::SessionStarted {
            provider: Provider::Claude,
            native_session_id: "s1".into(),
            model: Some("claude-3-7-sonnet".into()),
            tools: vec!["Bash".into()],
            enforcement: dummy_enforcement.clone(),
        },
        json!({
            "type": "session_started",
            "provider": "claude",
            "native_session_id": "s1",
            "model": "claude-3-7-sonnet",
            "tools": ["Bash"],
            "enforcement": {
                "level": "full",
                "reasons": [],
                "provider": "claude",
                "cli_version": "1.0.0",
                "version_tested": true,
                "coverage": [{
                    "class": "execute",
                    "coverage": "intercepted",
                    "evidence": "fixture/test"
                }],
                "config_sources": [{
                    "kind": "user_settings",
                    "path": "~/config.json",
                    "sha256": "abcd",
                    "inspected": true
                }]
            }
        }),
    );

    round_trip(
        &AgentEvent::EnforcementChanged {
            enforcement: dummy_enforcement.clone(),
        },
        json!({
            "type": "enforcement_changed",
            "enforcement": {
                "level": "full",
                "reasons": [],
                "provider": "claude",
                "cli_version": "1.0.0",
                "version_tested": true,
                "coverage": [{
                    "class": "execute",
                    "coverage": "intercepted",
                    "evidence": "fixture/test"
                }],
                "config_sources": [{
                    "kind": "user_settings",
                    "path": "~/config.json",
                    "sha256": "abcd",
                    "inspected": true
                }]
            }
        }),
    );

    round_trip(
        &AgentEvent::TurnStarted { turn: 2 },
        json!({"type": "turn_started", "turn": 2}),
    );

    round_trip(
        &AgentEvent::MessageDelta {
            message_id: "m1".into(),
            kind: MessageKind::Assistant,
            text: "hello".into(),
        },
        json!({
            "type": "message_delta",
            "message_id": "m1",
            "kind": "assistant",
            "text": "hello"
        }),
    );

    round_trip(
        &AgentEvent::MessageCompleted {
            message_id: "m1".into(),
            kind: MessageKind::User,
            body: MessageBody::Inline { text: "hi".into() },
        },
        json!({
            "type": "message_completed",
            "message_id": "m1",
            "kind": "user",
            "body": {"type": "inline", "text": "hi"}
        }),
    );

    round_trip(
        &AgentEvent::ToolCallStarted {
            call_id: "c1".into(),
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: Some("ls".into()),
            input: json!({"cmd": "ls"}),
        },
        json!({
            "type": "tool_call_started",
            "call_id": "c1",
            "name": "Bash",
            "kind": "execute",
            "title": "ls",
            "input": {"cmd": "ls"}
        }),
    );

    round_trip(
        &AgentEvent::ToolCallUpdated {
            call_id: "c1".into(),
            title: Some("ls -la".into()),
            kind: Some(ToolKind::Execute),
            input: Some(json!({"cmd": "ls -la"})),
            output_delta: Some("total 0\n".into()),
            output: Some("total 0\n".into()),
        },
        json!({
            "type": "tool_call_updated",
            "call_id": "c1",
            "title": "ls -la",
            "kind": "execute",
            "input": {"cmd": "ls -la"},
            "output_delta": "total 0\n",
            "output": "total 0\n"
        }),
    );

    round_trip(
        &AgentEvent::ToolCallFinished {
            call_id: "c1".into(),
            status: ToolStatus::Succeeded,
            output: Some("done".into()),
            error: None,
            truncated_bytes: 100,
        },
        json!({
            "type": "tool_call_finished",
            "call_id": "c1",
            "status": "succeeded",
            "output": "done",
            "error": null,
            "truncated_bytes": 100
        }),
    );

    round_trip(
        &AgentEvent::ApprovalRequested {
            request_id: "req1".into(),
            call_id: Some("c1".into()),
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: Some("rm -rf target".into()),
            input: json!({"command": "rm -rf target"}),
            reason: Some("delete files".into()),
            rememberable: true,
        },
        json!({
            "type": "approval_requested",
            "request_id": "req1",
            "call_id": "c1",
            "name": "Bash",
            "kind": "execute",
            "title": "rm -rf target",
            "input": {"command": "rm -rf target"},
            "reason": "delete files",
            "rememberable": true
        }),
    );

    round_trip(
        &AgentEvent::QuestionAsked {
            request_id: "q1".into(),
            questions: vec![Question {
                id: "q_color".into(),
                prompt: "Favorite color?".into(),
                options: vec![QuestionOption {
                    id: "opt_blue".into(),
                    label: "Blue".into(),
                    description: Some("Sky color".into()),
                }],
                multi_select: false,
                allow_text: true,
            }],
        },
        json!({
            "type": "question_asked",
            "request_id": "q1",
            "questions": [{
                "id": "q_color",
                "prompt": "Favorite color?",
                "options": [{
                    "id": "opt_blue",
                    "label": "Blue",
                    "description": "Sky color"
                }],
                "multi_select": false,
                "allow_text": true
            }]
        }),
    );

    round_trip(
        &AgentEvent::RequestResolved {
            request_id: "req1".into(),
            outcome: RequestOutcome::Approved,
        },
        json!({
            "type": "request_resolved",
            "request_id": "req1",
            "outcome": "approved"
        }),
    );

    round_trip(
        &AgentEvent::PlanUpdated {
            entries: vec![PlanEntry {
                content: "Step 1".into(),
                status: PlanStatus::InProgress,
            }],
        },
        json!({
            "type": "plan_updated",
            "entries": [{"content": "Step 1", "status": "in_progress"}]
        }),
    );

    round_trip(
        &AgentEvent::Usage {
            usage: Usage {
                input_tokens: Some(100),
                output_tokens: Some(50),
                cache_read_tokens: Some(10),
                cache_write_tokens: Some(5),
                reasoning_tokens: Some(20),
                cost_usd: Some(0.005),
            },
        },
        json!({
            "type": "usage",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_tokens": 10,
                "cache_write_tokens": 5,
                "reasoning_tokens": 20,
                "cost_usd": 0.005
            }
        }),
    );

    round_trip(
        &AgentEvent::ContextUpdated {
            used_tokens: 4000,
            window_tokens: Some(128000),
        },
        json!({
            "type": "context_updated",
            "used_tokens": 4000,
            "window_tokens": 128000
        }),
    );

    round_trip(
        &AgentEvent::QuotaUpdated {
            windows: vec![QuotaWindow {
                name: "5h".into(),
                used_percent: Some(25.5),
                resets_at_ms: Some(1700000000),
            }],
        },
        json!({
            "type": "quota_updated",
            "windows": [{
                "name": "5h",
                "used_percent": 25.5,
                "resets_at_ms": 1700000000
            }]
        }),
    );

    round_trip(
        &AgentEvent::RateLimited {
            resets_at_ms: Some(1700003600),
            message: Some("Hit hourly limit".into()),
        },
        json!({
            "type": "rate_limited",
            "resets_at_ms": 1700003600,
            "message": "Hit hourly limit"
        }),
    );

    round_trip(
        &AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Completed,
            usage: None,
        },
        json!({
            "type": "turn_completed",
            "turn": 1,
            "outcome": "completed",
            "usage": null
        }),
    );

    round_trip(
        &AgentEvent::Error {
            message: "failure".into(),
            fatal: true,
            details: vec!["trace line 1".into(), "trace line 2".into()],
        },
        json!({
            "type": "error",
            "message": "failure",
            "fatal": true,
            "details": ["trace line 1", "trace line 2"]
        }),
    );

    round_trip(
        &AgentEvent::Unmapped {
            record_type: "custom_log".into(),
            raw: json!({"raw_key": "val"}),
        },
        json!({
            "type": "unmapped",
            "record_type": "custom_log",
            "raw": {"raw_key": "val"}
        }),
    );

    round_trip(
        &AgentEvent::SessionEnded {
            exit_code: Some(0),
            reason: Some("normal exit".into()),
        },
        json!({
            "type": "session_ended",
            "exit_code": 0,
            "reason": "normal exit"
        }),
    );
}

#[test]
fn agent_envelope_round_trip() {
    round_trip(
        &AgentEventEnvelope {
            incarnation: 1,
            seq: 42,
            at_ms: 1700000000123,
            event: AgentEvent::TurnStarted { turn: 1 },
        },
        json!({
            "incarnation": 1,
            "seq": 42,
            "at_ms": 1700000000123u64,
            "event": {"type": "turn_started", "turn": 1}
        }),
    );
}

#[test]
fn agent_session_commands() {
    round_trip(
        &SessionCommand::SendUserMessage {
            text: "do work".into(),
        },
        json!({"type": "send_user_message", "text": "do work"}),
    );

    round_trip(
        &SessionCommand::ResolveApproval {
            request_id: "req1".into(),
            decision: ApprovalDecision::Allow,
        },
        json!({
            "type": "resolve_approval",
            "request_id": "req1",
            "decision": {"type": "allow"}
        }),
    );

    round_trip(
        &SessionCommand::ResolveApproval {
            request_id: "req2".into(),
            decision: ApprovalDecision::Deny {
                message: Some("denied".into()),
            },
        },
        json!({
            "type": "resolve_approval",
            "request_id": "req2",
            "decision": {"type": "deny", "message": "denied"}
        }),
    );

    round_trip(
        &SessionCommand::AnswerQuestion {
            request_id: "q1".into(),
            answers: vec![QuestionAnswer {
                question_id: "scope".into(),
                selected: vec!["a".into()],
                text: Some("custom answer".into()),
            }],
        },
        json!({
            "type": "answer_question",
            "request_id": "q1",
            "answers": [{
                "question_id": "scope",
                "selected": ["a"],
                "text": "custom answer"
            }]
        }),
    );

    round_trip(&SessionCommand::Interrupt, json!({"type": "interrupt"}));
    round_trip(&SessionCommand::Shutdown, json!({"type": "shutdown"}));
}

#[test]
fn policy_enums_and_descriptors() {
    round_trip(&EnforcementLevel::Full, json!("full"));
    round_trip(&EnforcementLevel::Partial, json!("partial"));

    round_trip(&ToolClass::Read, json!("read"));
    round_trip(&ToolClass::Edit, json!("edit"));
    round_trip(&ToolClass::Delete, json!("delete"));
    round_trip(&ToolClass::Move, json!("move"));
    round_trip(&ToolClass::Search, json!("search"));
    round_trip(&ToolClass::Execute, json!("execute"));
    round_trip(&ToolClass::Fetch, json!("fetch"));
    round_trip(&ToolClass::Mcp, json!("mcp"));
    round_trip(&ToolClass::Subagent, json!("subagent"));
    round_trip(&ToolClass::ProviderSpecific, json!("provider_specific"));

    round_trip(&Coverage::Intercepted, json!("intercepted"));
    round_trip(&Coverage::ProviderGated, json!("provider_gated"));
    round_trip(&Coverage::NotIntercepted, json!("not_intercepted"));
    round_trip(&Coverage::Unknown, json!("unknown"));
    round_trip(&Coverage::NotApplicable, json!("not_applicable"));

    round_trip(&ConfigSourceKind::UserSettings, json!("user_settings"));
    round_trip(
        &ConfigSourceKind::ProjectSettings,
        json!("project_settings"),
    );
    round_trip(&ConfigSourceKind::LocalSettings, json!("local_settings"));
    round_trip(&ConfigSourceKind::ManagedPolicy, json!("managed_policy"));
    round_trip(
        &ConfigSourceKind::ExplicitSettings,
        json!("explicit_settings"),
    );
    round_trip(&ConfigSourceKind::Plugin, json!("plugin"));
    round_trip(&ConfigSourceKind::Hooks, json!("hooks"));
    round_trip(&ConfigSourceKind::Other, json!("other"));

    round_trip(
        &ReasonCode::ClassNotIntercepted,
        json!("class_not_intercepted"),
    );
    round_trip(&ReasonCode::CoverageUnknown, json!("coverage_unknown"));
    round_trip(&ReasonCode::UntestedVersion, json!("untested_version"));
    round_trip(&ReasonCode::UninspectedConfig, json!("uninspected_config"));
    round_trip(&ReasonCode::ExternalHooks, json!("external_hooks"));
    round_trip(&ReasonCode::GateUnreachable, json!("gate_unreachable"));

    round_trip(
        &EnforcementReason {
            code: ReasonCode::ExternalHooks,
            message: "Hooks detected".into(),
        },
        json!({"code": "external_hooks", "message": "Hooks detected"}),
    );

    round_trip(&Preset::Home, json!("home"));
    round_trip(&Preset::Careful, json!("careful"));
    round_trip(&Preset::Normal, json!("normal"));
    round_trip(&Preset::Trusted, json!("trusted"));
    round_trip(&Preset::FullAccess, json!("full_access"));

    round_trip(&RuleDecision::Allow, json!("allow"));
    round_trip(&RuleDecision::Deny, json!("deny"));

    round_trip(
        &RuleScope::Thread {
            thread_id: ThreadId("t1".into()),
        },
        json!({"type": "thread", "thread_id": "t1"}),
    );
    round_trip(&RuleScope::Home, json!({"type": "home"}));
    round_trip(&RuleScope::Global, json!({"type": "global"}));

    round_trip(&RememberScope::Thread, json!("thread"));
    round_trip(&RememberScope::Home, json!("home"));
    round_trip(&RememberScope::Global, json!("global"));

    round_trip(&RuleMatcher::Any, json!({"type": "any"}));
    round_trip(
        &RuleMatcher::CommandPrefix {
            argv: vec!["git".into(), "status".into()],
        },
        json!({"type": "command_prefix", "argv": ["git", "status"]}),
    );
    round_trip(
        &RuleMatcher::CommandExact {
            argv: vec!["rm".into(), "-rf".into(), "build".into()],
        },
        json!({"type": "command_exact", "argv": ["rm", "-rf", "build"]}),
    );
    round_trip(
        &RuleMatcher::PathGlob {
            glob: "**/*.rs".into(),
        },
        json!({"type": "path_glob", "glob": "**/*.rs"}),
    );

    round_trip(&RuleId("rule_123".into()), json!("rule_123"));

    round_trip(
        &ToolSubject::Command {
            line: "git status".into(),
            cwd: Some("/repo".into()),
        },
        json!({"type": "command", "line": "git status", "cwd": "/repo"}),
    );
    round_trip(
        &ToolSubject::Paths {
            paths: vec!["src/lib.rs".into()],
            glob: Some("*.rs".into()),
            recursive: true,
        },
        json!({
            "type": "paths",
            "paths": ["src/lib.rs"],
            "glob": "*.rs",
            "recursive": true
        }),
    );
    round_trip(
        &ToolSubject::Fetch {
            url: "https://example.com".into(),
        },
        json!({"type": "fetch", "url": "https://example.com"}),
    );
    round_trip(
        &ToolSubject::Mcp {
            server: "fetch-mcp".into(),
            tool: "get".into(),
        },
        json!({"type": "mcp", "server": "fetch-mcp", "tool": "get"}),
    );
    round_trip(&ToolSubject::None, json!({"type": "none"}));
    round_trip(&ToolSubject::Unknown, json!({"type": "unknown"}));

    round_trip(
        &RuleDraft {
            scope: RuleScope::Home,
            tool_kind: ToolKind::Execute,
            tool_name: Some("Bash".into()),
            matcher: RuleMatcher::CommandPrefix {
                argv: vec!["cargo".into(), "check".into()],
            },
            path_scope: Some("/repo".into()),
            decision: RuleDecision::Allow,
        },
        json!({
            "scope": {"type": "home"},
            "tool_kind": "execute",
            "tool_name": "Bash",
            "matcher": {"type": "command_prefix", "argv": ["cargo", "check"]},
            "path_scope": "/repo",
            "decision": "allow"
        }),
    );

    round_trip(
        &RuleOrigin::Approval {
            request_id: "req1".into(),
        },
        json!({"type": "approval", "request_id": "req1"}),
    );
    round_trip(&RuleOrigin::Settings, json!({"type": "settings"}));

    round_trip(
        &Rule {
            id: RuleId("r1".into()),
            scope: RuleScope::Global,
            tool_kind: ToolKind::Read,
            tool_name: None,
            matcher: RuleMatcher::Any,
            path_scope: None,
            decision: RuleDecision::Allow,
            origin: RuleOrigin::Settings,
            created_at_ms: 1700000000,
        },
        json!({
            "id": "r1",
            "scope": {"type": "global"},
            "tool_kind": "read",
            "tool_name": null,
            "matcher": {"type": "any"},
            "path_scope": null,
            "decision": "allow",
            "origin": {"type": "settings"},
            "created_at_ms": 1700000000
        }),
    );

    round_trip(
        &PolicyContext {
            rules_revision: 5,
            preset: Preset::Home,
        },
        json!({"rules_revision": 5, "preset": "home"}),
    );
}

#[test]
fn thread_and_inbox_types() {
    round_trip(&ThreadId("t-abc".into()), json!("t-abc"));

    round_trip(&Cleanup::Verified, json!("verified"));
    round_trip(&Cleanup::Unverified, json!("unverified"));

    round_trip(&ThreadState::Starting, json!({"type": "starting"}));
    round_trip(&ThreadState::Running, json!({"type": "running"}));
    round_trip(
        &ThreadState::NeedsApproval,
        json!({"type": "needs_approval"}),
    );
    round_trip(&ThreadState::NeedsInput, json!({"type": "needs_input"}));
    round_trip(&ThreadState::Idle, json!({"type": "idle"}));
    round_trip(
        &ThreadState::Paused {
            resets_at_ms: Some(1700001000),
        },
        json!({"type": "paused", "resets_at_ms": 1700001000}),
    );
    round_trip(
        &ThreadState::Failed {
            message: "cli crashed".into(),
        },
        json!({"type": "failed", "message": "cli crashed"}),
    );
    round_trip(
        &ThreadState::Interrupted {
            cleanup: Cleanup::Verified,
        },
        json!({"type": "interrupted", "cleanup": "verified"}),
    );
    round_trip(&ThreadState::Ended, json!({"type": "ended"}));

    round_trip(&Location::Home, json!({"type": "home"}));

    round_trip(
        &ThreadSummary {
            thread_id: ThreadId("t1".into()),
            title: "Build Atrium".into(),
            provider: Provider::Claude,
            model: Some("claude-3-7-sonnet".into()),
            location: Location::Home,
            state: ThreadState::Running,
            enforcement: None,
            incarnation: 1,
            native_session_id: Some("ns1".into()),
            last_event: Some(EventId(10)),
            unread: false,
            pending_approvals: 0,
            pending_questions: 0,
            attention_since_ms: Some(1700000000),
            attention_detail: Some("Review changes".into()),
            created_at_ms: 1699999000,
            updated_at_ms: 1700000000,
        },
        json!({
            "thread_id": "t1",
            "title": "Build Atrium",
            "provider": "claude",
            "model": "claude-3-7-sonnet",
            "location": {"type": "home"},
            "state": {"type": "running"},
            "enforcement": null,
            "incarnation": 1,
            "native_session_id": "ns1",
            "last_event": 10,
            "unread": false,
            "pending_approvals": 0,
            "pending_questions": 0,
            "attention_since_ms": 1700000000,
            "attention_detail": "Review changes",
            "created_at_ms": 1699999000,
            "updated_at_ms": 1700000000
        }),
    );

    round_trip(&InboxReason::StorageFull, json!("storage_full"));
    round_trip(&InboxReason::Approval, json!("approval"));
    round_trip(&InboxReason::Question, json!("question"));
    round_trip(&InboxReason::Failed, json!("failed"));
    round_trip(&InboxReason::CleanupUnverified, json!("cleanup_unverified"));
    round_trip(&InboxReason::Reply, json!("reply"));

    round_trip(
        &InboxItem {
            thread_id: Some(ThreadId("t1".into())),
            reason: InboxReason::Approval,
            since_ms: 1700000500,
            title: "Approve bash command".into(),
            detail: Some("rm -rf target".into()),
        },
        json!({
            "thread_id": "t1",
            "reason": "approval",
            "since_ms": 1700000500,
            "title": "Approve bash command",
            "detail": "rm -rf target"
        }),
    );
}

#[test]
fn wire_types_and_messages() {
    round_trip(&EventId(100), json!(100));

    round_trip(&Authority::ThreadScoped, json!("thread_scoped"));
    round_trip(&Authority::HumanControl, json!("human_control"));

    round_trip(&ClientRole::App, json!({"type": "app"}));
    round_trip(&ClientRole::Dev, json!({"type": "dev"}));
    round_trip(
        &ClientRole::Agent {
            token: "tok123".into(),
        },
        json!({"type": "agent", "token": "tok123"}),
    );

    round_trip(&ExportFormat::Markdown, json!("markdown"));
    round_trip(&ExportFormat::Jsonl, json!("jsonl"));

    round_trip(
        &AgentLaunch::Native {
            provider: Provider::Codex,
            model: Some("o3".into()),
            effort: Some("high".into()),
            preset: Preset::Home,
        },
        json!({
            "type": "native",
            "provider": "codex",
            "model": "o3",
            "effort": "high",
            "preset": "home"
        }),
    );
    round_trip(
        &AgentLaunch::Fake {
            tokens_per_second: 100,
            chunk_chars: 4,
            duration_ms: 5000,
            tool_call_every_ms: 1000,
        },
        json!({
            "type": "fake",
            "tokens_per_second": 100,
            "chunk_chars": 4,
            "duration_ms": 5000,
            "tool_call_every_ms": 1000
        }),
    );

    round_trip(
        &ProviderProbe {
            provider: Provider::Claude,
            enforcement: None,
            error: Some("cli not found".into()),
        },
        json!({
            "provider": "claude",
            "enforcement": null,
            "error": "cli not found"
        }),
    );

    round_trip(
        &StoredEvent {
            id: EventId(1),
            thread_id: ThreadId("t1".into()),
            envelope: AgentEventEnvelope {
                incarnation: 1,
                seq: 0,
                at_ms: 1700000000,
                event: AgentEvent::TurnStarted { turn: 1 },
            },
        },
        json!({
            "id": 1,
            "thread_id": "t1",
            "envelope": {
                "incarnation": 1,
                "seq": 0,
                "at_ms": 1700000000,
                "event": {"type": "turn_started", "turn": 1}
            }
        }),
    );
}

#[test]
fn wire_client_messages_and_daemon_commands() {
    round_trip(
        &ClientMessage::Hello {
            protocol_version: 1,
            client_name: "atrium-app".into(),
            role: ClientRole::App,
        },
        json!({
            "type": "hello",
            "protocol_version": 1,
            "client_name": "atrium-app",
            "role": {"type": "app"}
        }),
    );
    round_trip(
        &ClientMessage::Subscribe {
            request_id: 1,
            thread_id: ThreadId("t1".into()),
            after: Some(EventId(5)),
        },
        json!({
            "type": "subscribe",
            "request_id": 1,
            "thread_id": "t1",
            "after": 5
        }),
    );
    round_trip(
        &ClientMessage::Unsubscribe {
            thread_id: ThreadId("t1".into()),
        },
        json!({"type": "unsubscribe", "thread_id": "t1"}),
    );
    round_trip(
        &ClientMessage::SubscribeInbox { request_id: 2 },
        json!({"type": "subscribe_inbox", "request_id": 2}),
    );
    round_trip(
        &ClientMessage::Command {
            request_id: 3,
            command: DaemonCommand::ListThreads,
        },
        json!({
            "type": "command",
            "request_id": 3,
            "command": {"type": "list_threads"}
        }),
    );

    // All DaemonCommand variants
    round_trip(
        &DaemonCommand::StartThread {
            agent: AgentLaunch::Native {
                provider: Provider::Claude,
                model: None,
                effort: None,
                preset: Preset::Home,
            },
            message: Some("hello".into()),
        },
        json!({
            "type": "start_thread",
            "agent": {
                "type": "native",
                "provider": "claude",
                "model": null,
                "effort": null,
                "preset": "home"
            },
            "message": "hello"
        }),
    );
    round_trip(&DaemonCommand::ListThreads, json!({"type": "list_threads"}));
    round_trip(
        &DaemonCommand::ProbeProviders {
            preset: Preset::Home,
        },
        json!({"type": "probe_providers", "preset": "home"}),
    );
    round_trip(
        &DaemonCommand::RenameThread {
            thread_id: ThreadId("t1".into()),
            title: "New Title".into(),
        },
        json!({"type": "rename_thread", "thread_id": "t1", "title": "New Title"}),
    );
    round_trip(
        &DaemonCommand::GetThread {
            thread_id: ThreadId("t1".into()),
        },
        json!({"type": "get_thread", "thread_id": "t1"}),
    );
    round_trip(
        &DaemonCommand::ThreadCommand {
            thread_id: ThreadId("t1".into()),
            command: SessionCommand::Interrupt,
        },
        json!({
            "type": "thread_command",
            "thread_id": "t1",
            "command": {"type": "interrupt"}
        }),
    );
    round_trip(
        &DaemonCommand::ResolveApproval {
            thread_id: ThreadId("t1".into()),
            incarnation: 1,
            request_id: "req1".into(),
            decision: ApprovalDecision::Allow,
            remember: Some(RememberScope::Home),
        },
        json!({
            "type": "resolve_approval",
            "thread_id": "t1",
            "incarnation": 1,
            "request_id": "req1",
            "decision": {"type": "allow"},
            "remember": "home"
        }),
    );
    round_trip(
        &DaemonCommand::AnswerQuestion {
            thread_id: ThreadId("t1".into()),
            incarnation: 1,
            request_id: "q1".into(),
            answers: vec![],
        },
        json!({
            "type": "answer_question",
            "thread_id": "t1",
            "incarnation": 1,
            "request_id": "q1",
            "answers": []
        }),
    );
    round_trip(
        &DaemonCommand::ResumeThread {
            thread_id: ThreadId("t1".into()),
        },
        json!({"type": "resume_thread", "thread_id": "t1"}),
    );
    round_trip(
        &DaemonCommand::ArchiveThread {
            thread_id: ThreadId("t1".into()),
        },
        json!({"type": "archive_thread", "thread_id": "t1"}),
    );
    round_trip(
        &DaemonCommand::MarkRead {
            thread_id: ThreadId("t1".into()),
        },
        json!({"type": "mark_read", "thread_id": "t1"}),
    );
    round_trip(
        &DaemonCommand::SetFocus {
            thread_id: Some(ThreadId("t1".into())),
        },
        json!({"type": "set_focus", "thread_id": "t1"}),
    );
    round_trip(&DaemonCommand::ListInbox, json!({"type": "list_inbox"}));
    round_trip(&DaemonCommand::ListRules, json!({"type": "list_rules"}));
    round_trip(
        &DaemonCommand::CreateRule {
            rule: RuleDraft {
                scope: RuleScope::Home,
                tool_kind: ToolKind::Execute,
                tool_name: None,
                matcher: RuleMatcher::Any,
                path_scope: None,
                decision: RuleDecision::Allow,
            },
        },
        json!({
            "type": "create_rule",
            "rule": {
                "scope": {"type": "home"},
                "tool_kind": "execute",
                "tool_name": null,
                "matcher": {"type": "any"},
                "path_scope": null,
                "decision": "allow"
            }
        }),
    );
    round_trip(
        &DaemonCommand::DeleteRule {
            rule_id: RuleId("r1".into()),
        },
        json!({"type": "delete_rule", "rule_id": "r1"}),
    );
    round_trip(
        &DaemonCommand::ExportThread {
            thread_id: ThreadId("t1".into()),
            format: ExportFormat::Markdown,
            path: "/tmp/export.md".into(),
        },
        json!({
            "type": "export_thread",
            "thread_id": "t1",
            "format": "markdown",
            "path": "/tmp/export.md"
        }),
    );
    round_trip(&DaemonCommand::GetSettings, json!({"type": "get_settings"}));
    round_trip(
        &DaemonCommand::SetSetting {
            key: "theme".into(),
            value: json!("calm-dark"),
        },
        json!({"type": "set_setting", "key": "theme", "value": "calm-dark"}),
    );
    round_trip(
        &DaemonCommand::ShutdownDaemon { force: true },
        json!({"type": "shutdown_daemon", "force": true}),
    );
}

#[test]
fn wire_server_messages_and_error_codes() {
    round_trip(&ErrorCode::Expired, json!("expired"));
    round_trip(&ErrorCode::UnknownThread, json!("unknown_thread"));
    round_trip(&ErrorCode::NotRunning, json!("not_running"));
    round_trip(&ErrorCode::StorageReadOnly, json!("storage_read_only"));
    round_trip(&ErrorCode::TurnsRunning, json!("turns_running"));
    round_trip(&ErrorCode::Forbidden, json!("forbidden"));
    round_trip(&ErrorCode::Invalid, json!("invalid"));
    round_trip(&ErrorCode::Internal, json!("internal"));

    round_trip(
        &CommandResult::Ok {
            value: json!({"status": "fine"}),
        },
        json!({"status": "ok", "value": {"status": "fine"}}),
    );
    round_trip(
        &CommandResult::Err {
            code: ErrorCode::Expired,
            message: "Request expired".into(),
        },
        json!({
            "status": "err",
            "code": "expired",
            "message": "Request expired"
        }),
    );

    round_trip(
        &ServerMessage::Welcome {
            protocol_version: 1,
            daemon_version: "0.1.0".into(),
            daemon_pid: 1234,
            role: ClientRole::App,
        },
        json!({
            "type": "welcome",
            "protocol_version": 1,
            "daemon_version": "0.1.0",
            "daemon_pid": 1234,
            "role": {"type": "app"}
        }),
    );

    round_trip(
        &ServerMessage::Incompatible {
            protocol_version: 1,
            message: "version mismatch".into(),
            daemon_version: "0.0.9".into(),
            daemon_pid: 1230,
            running_turns: 2,
        },
        json!({
            "type": "incompatible",
            "protocol_version": 1,
            "message": "version mismatch",
            "daemon_version": "0.0.9",
            "daemon_pid": 1230,
            "running_turns": 2
        }),
    );

    round_trip(
        &ServerMessage::Event {
            event: StoredEvent {
                id: EventId(1),
                thread_id: ThreadId("t1".into()),
                envelope: AgentEventEnvelope {
                    incarnation: 1,
                    seq: 0,
                    at_ms: 1700000000,
                    event: AgentEvent::TurnStarted { turn: 1 },
                },
            },
        },
        json!({
            "type": "event",
            "event": {
                "id": 1,
                "thread_id": "t1",
                "envelope": {
                    "incarnation": 1,
                    "seq": 0,
                    "at_ms": 1700000000,
                    "event": {"type": "turn_started", "turn": 1}
                }
            }
        }),
    );

    round_trip(
        &ServerMessage::ReplayDone {
            thread_id: ThreadId("t1".into()),
            last: Some(EventId(10)),
        },
        json!({
            "type": "replay_done",
            "thread_id": "t1",
            "last": 10
        }),
    );

    round_trip(
        &ServerMessage::Response {
            request_id: 42,
            result: CommandResult::Ok { value: json!(null) },
        },
        json!({
            "type": "response",
            "request_id": 42,
            "result": {"status": "ok", "value": null}
        }),
    );

    round_trip(
        &ServerMessage::ThreadUpdated {
            summary: ThreadSummary {
                thread_id: ThreadId("t1".into()),
                title: "Title".into(),
                provider: Provider::Claude,
                model: None,
                location: Location::Home,
                state: ThreadState::Idle,
                enforcement: None,
                incarnation: 1,
                native_session_id: None,
                last_event: None,
                unread: false,
                pending_approvals: 0,
                pending_questions: 0,
                attention_since_ms: None,
                attention_detail: None,
                created_at_ms: 1700000000,
                updated_at_ms: 1700000000,
            },
        },
        json!({
            "type": "thread_updated",
            "summary": {
                "thread_id": "t1",
                "title": "Title",
                "provider": "claude",
                "model": null,
                "location": {"type": "home"},
                "state": {"type": "idle"},
                "enforcement": null,
                "incarnation": 1,
                "native_session_id": null,
                "last_event": null,
                "unread": false,
                "pending_approvals": 0,
                "pending_questions": 0,
                "attention_since_ms": null,
                "attention_detail": null,
                "created_at_ms": 1700000000,
                "updated_at_ms": 1700000000
            }
        }),
    );

    round_trip(
        &ServerMessage::InboxUpdated {
            items: vec![InboxItem {
                thread_id: Some(ThreadId("t1".into())),
                reason: InboxReason::Reply,
                since_ms: 1700000000,
                title: "New reply".into(),
                detail: None,
            }],
        },
        json!({
            "type": "inbox_updated",
            "items": [{
                "thread_id": "t1",
                "reason": "reply",
                "since_ms": 1700000000,
                "title": "New reply",
                "detail": null
            }]
        }),
    );

    round_trip(
        &ServerMessage::SubscriptionLagged {
            thread_id: ThreadId("t1".into()),
            last_sent: Some(EventId(200)),
        },
        json!({
            "type": "subscription_lagged",
            "thread_id": "t1",
            "last_sent": 200
        }),
    );

    assert_eq!(FrameKind::try_from(0), Ok(FrameKind::Control));
    assert_eq!(FrameKind::try_from(1), Ok(FrameKind::Stream));
    assert_eq!(FrameKind::try_from(99), Err(99));
}

#[test]
fn serde_default_fields_omitted() {
    let dummy_enforcement = EnforcementDescriptor {
        level: EnforcementLevel::Full,
        reasons: vec![],
        provider: Provider::Claude,
        cli_version: Some("1.0.0".into()),
        version_tested: true,
        coverage: vec![],
        config_sources: vec![],
    };

    // 1. AgentEvent::SessionStarted with `tools` omitted
    let json_session_started = json!({
        "type": "session_started",
        "provider": "claude",
        "native_session_id": "s1",
        "model": null,
        "enforcement": {
            "level": "full",
            "reasons": [],
            "provider": "claude",
            "cli_version": "1.0.0",
            "version_tested": true,
            "coverage": [],
            "config_sources": []
        }
    });
    let event: AgentEvent = serde_json::from_value(json_session_started).unwrap();
    assert_eq!(
        event,
        AgentEvent::SessionStarted {
            provider: Provider::Claude,
            native_session_id: "s1".into(),
            model: None,
            tools: vec![],
            enforcement: dummy_enforcement,
        }
    );

    // 2. AgentEvent::ToolCallFinished with `truncated_bytes` omitted
    let json_tool_finished = json!({
        "type": "tool_call_finished",
        "call_id": "c1",
        "status": "succeeded",
        "output": "done",
        "error": null
    });
    let event: AgentEvent = serde_json::from_value(json_tool_finished).unwrap();
    assert_eq!(
        event,
        AgentEvent::ToolCallFinished {
            call_id: "c1".into(),
            status: ToolStatus::Succeeded,
            output: Some("done".into()),
            error: None,
            truncated_bytes: 0,
        }
    );

    // 3. AgentEvent::ApprovalRequested with `rememberable` omitted
    let json_approval = json!({
        "type": "approval_requested",
        "request_id": "r1",
        "call_id": null,
        "name": "Bash",
        "kind": "execute",
        "title": null,
        "input": {},
        "reason": null
    });
    let event: AgentEvent = serde_json::from_value(json_approval).unwrap();
    assert_eq!(
        event,
        AgentEvent::ApprovalRequested {
            request_id: "r1".into(),
            call_id: None,
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: None,
            input: json!({}),
            reason: None,
            rememberable: false,
        }
    );

    // 4. AgentEvent::Error with `details` omitted
    let json_error = json!({
        "type": "error",
        "message": "fatal crash",
        "fatal": true
    });
    let event: AgentEvent = serde_json::from_value(json_error).unwrap();
    assert_eq!(
        event,
        AgentEvent::Error {
            message: "fatal crash".into(),
            fatal: true,
            details: vec![],
        }
    );

    // 5. Question with `options`, `multi_select`, and `allow_text` omitted
    let json_question = json!({
        "id": "q1",
        "prompt": "What is your preference?"
    });
    let question: Question = serde_json::from_value(json_question).unwrap();
    assert_eq!(
        question,
        Question {
            id: "q1".into(),
            prompt: "What is your preference?".into(),
            options: vec![],
            multi_select: false,
            allow_text: false,
        }
    );

    // 6. QuestionAnswer with `selected` omitted
    let json_answer = json!({
        "question_id": "q1",
        "text": "my custom answer"
    });
    let answer: QuestionAnswer = serde_json::from_value(json_answer).unwrap();
    assert_eq!(
        answer,
        QuestionAnswer {
            question_id: "q1".into(),
            selected: vec![],
            text: Some("my custom answer".into()),
        }
    );
}
