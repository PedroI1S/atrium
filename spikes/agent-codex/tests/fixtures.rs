use atrium_agents::{AdapterInput, Normalizer, fixture};
use atrium_protocol::agent::*;
use spike_agent_codex::CodexNormalizer;
use std::path::Path;
fn path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../fixtures/agents/codex/{name}.jsonl"))
}
fn events(name: &str) -> Vec<AgentEvent> {
    fixture::normalize(
        &fixture::read(&path(name)).unwrap(),
        &mut CodexNormalizer::default(),
    )
}
macro_rules! golden {($($test:ident=>$name:literal),* $(,)?)=>{$(#[test]fn $test(){fixture::assert_golden(&path($name),&mut CodexNormalizer::default());})*};}
golden! {simple=>"simple",tool_read=>"tool-read",gate_deny=>"gate-deny",gate_deny_execute=>"gate-deny-execute",approval_allow=>"approval-allow",approval_deny=>"approval-deny",question=>"question",interrupt=>"interrupt",multi_turn=>"multi-turn",resume=>"resume",startup_error=>"startup-error",patch=>"patch",quota_usage=>"quota-usage"}
#[test]
fn lifecycle_and_security_semantics() {
    for name in [
        "simple",
        "tool-read",
        "gate-deny",
        "gate-deny-execute",
        "approval-allow",
        "approval-deny",
        "question",
        "interrupt",
        "multi-turn",
        "resume",
        "patch",
        "quota-usage",
    ] {
        let actual = events(name);
        assert!(
            matches!(actual.first(), Some(AgentEvent::SessionStarted { .. })),
            "{name}"
        );
        assert!(
            matches!(actual.last(), Some(AgentEvent::SessionEnded { .. })),
            "{name}"
        );
        let mut calls = std::collections::BTreeSet::new();
        let mut pending = std::collections::BTreeSet::new();
        for e in &actual {
            match e {
                AgentEvent::ToolCallStarted { call_id, .. } => {
                    assert!(calls.insert(call_id));
                }
                AgentEvent::ToolCallFinished { call_id, .. } => {
                    assert!(calls.remove(call_id));
                }
                AgentEvent::ApprovalRequested { request_id, .. }
                | AgentEvent::QuestionAsked { request_id, .. } => {
                    assert!(pending.insert(request_id));
                }
                AgentEvent::RequestResolved { request_id, .. } => {
                    assert!(pending.remove(request_id));
                }
                _ => {}
            }
        }
        assert!(calls.is_empty() && pending.is_empty(), "{name}");
    }
    for name in ["gate-deny", "gate-deny-execute"] {
        let actual = events(name);
        assert!(actual.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Denied,
                ..
            }
        )));
        assert!(
            !serde_json::to_string(&actual)
                .unwrap()
                .contains("FIXTURE_SECRET_MUST_NOT_APPEAR")
        );
    }
    for (name, outcome) in [
        ("approval-allow", RequestOutcome::Approved),
        ("approval-deny", RequestOutcome::Denied),
        ("question", RequestOutcome::Answered),
        ("interrupt", RequestOutcome::Cancelled),
    ] {
        assert!(
            events(name)
                .iter()
                .any(|e| matches!(e,AgentEvent::RequestResolved {outcome:o,..} if o==&outcome)),
            "{name}"
        );
    }
    assert!(events("interrupt").iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Cancelled,
            ..
        }
    )));
    assert!(events("interrupt").iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            outcome: TurnOutcome::Interrupted,
            ..
        }
    )));
    let startup = events("startup-error");
    assert!(matches!(
        startup.first(),
        Some(AgentEvent::Error { fatal: true, .. })
    ));
    assert!(
        !startup
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
    );
}
#[test]
fn totals_replace_and_reset_per_turn() {
    let actual = events("quota-usage");
    let usage = actual
        .iter()
        .filter_map(|e| {
            if let AgentEvent::Usage { usage } = e {
                usage.input_tokens
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(usage, vec![10, 25, 5]);
    assert_eq!(
        actual
            .iter()
            .filter(|e| matches!(e, AgentEvent::RateLimited { .. }))
            .count(),
        1
    );
    assert_eq!(
        actual
            .iter()
            .filter(|e| matches!(e, AgentEvent::QuotaUpdated { .. }))
            .count(),
        1
    );
    assert!(actual.iter().any(
        |e| matches!(e,AgentEvent::Unmapped {record_type,..} if record_type=="future/notification")
    ));
}
#[test]
fn duplicate_resolution_is_rejected() {
    let mut n = CodexNormalizer::default();
    let lines = fixture::read(&path("approval-allow")).unwrap();
    let mut actual = vec![];
    for line in lines {
        if matches!(line.input, AdapterInput::Exit { .. }) {
            break;
        }
        actual.extend(n.on_input(&line.input));
    }
    let result = n.on_input(&AdapterInput::Command {
        command: SessionCommand::ResolveApproval {
            request_id: "17".into(),
            decision: ApprovalDecision::Allow,
        },
    });
    assert!(matches!(
        result.as_slice(),
        [AgentEvent::Error { fatal: false, .. }]
    ));
}
#[test]
fn exit_closes_in_contract_order_and_is_idempotent() {
    let mut n = CodexNormalizer::default();
    let lines = fixture::read(&path("interrupt")).unwrap();
    for line in lines {
        if matches!(
            &line.input,
            AdapterInput::Command {
                command: SessionCommand::Interrupt
            }
        ) {
            break;
        }
        n.on_input(&line.input);
    }
    n.on_input(&AdapterInput::Stdout {line:serde_json::json!({"method":"item/agentMessage/delta","params":{"itemId":"open-message","delta":"partial"}}).to_string()});
    let end = n.on_input(&AdapterInput::Exit { code: Some(1) });
    assert!(matches!(
        end.as_slice(),
        [
            AgentEvent::MessageCompleted { .. },
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Cancelled,
                ..
            },
            AgentEvent::RequestResolved {
                outcome: RequestOutcome::Cancelled,
                ..
            },
            AgentEvent::TurnCompleted {
                outcome: TurnOutcome::Failed,
                ..
            },
            AgentEvent::SessionEnded { .. }
        ]
    ));
    assert!(n.on_input(&AdapterInput::Exit { code: None }).is_empty());
}
#[test]
fn fixtures_are_sanitized() {
    let s = fixture::Sanitizer::new("/Users/pedro", "pedro", &["FIXTURE_SECRET_MUST_NOT_APPEAR"]);
    for entry in std::fs::read_dir(path("simple").parent().unwrap()).unwrap() {
        let p = entry.unwrap().path();
        assert!(s.is_clean(&std::fs::read_to_string(p).unwrap()));
    }
}

#[test]
fn queued_messages_are_bounded_and_unknown_records_are_retained() {
    let mut n = CodexNormalizer::default();
    for line in fixture::read(&path("interrupt")).unwrap() {
        if matches!(&line.input, AdapterInput::Gate { .. }) {
            break;
        }
        n.on_input(&line.input);
    }
    for _ in 0..atrium_agents::MAX_QUEUED_MESSAGES {
        assert!(
            n.on_input(&AdapterInput::Command {
                command: SessionCommand::SendUserMessage {
                    text: "queued".into()
                }
            })
            .is_empty()
        );
    }
    assert!(matches!(
        n.on_input(&AdapterInput::Command {
            command: SessionCommand::SendUserMessage {
                text: "overflow".into()
            }
        })
        .as_slice(),
        [AgentEvent::Error { fatal: false, .. }]
    ));
    assert!(
        matches!(n.on_input(&AdapterInput::Stdout {line:"malformed".into()}).as_slice(),[AgentEvent::Unmapped {record_type,..}] if record_type=="invalid_json")
    );
}

#[test]
fn resume_without_historical_usage_does_not_charge_old_tokens() {
    let mut n = CodexNormalizer::default();
    n.on_input(&AdapterInput::Stdin {line:serde_json::json!({"id":1,"method":"thread/resume","params":{"model":"fixture-model","threadId":"thread-fixture"}}).to_string()});
    n.on_input(&AdapterInput::Stdout {
        line: serde_json::json!({"id":1,"result":{"thread":{"id":"thread-fixture"}}}).to_string(),
    });
    n.on_input(&AdapterInput::Stdout {
        line: serde_json::json!({"method":"turn/started","params":{"turn":{"id":"resumed-turn"}}})
            .to_string(),
    });
    let result=n.on_input(&AdapterInput::Stdout {line:serde_json::json!({"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":1000,"outputTokens":100},"last":{"inputTokens":5,"outputTokens":2}}}}).to_string()});
    assert!(
        matches!(result.as_slice(),[AgentEvent::Unmapped {record_type,..}] if record_type.ends_with("missing_resume_baseline"))
    );
}
