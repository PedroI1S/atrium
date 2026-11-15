#![doc = "Golden fixture and CodexNormalizer contract tests for Codex fixtures."]

use std::fs;
use std::path::{Path, PathBuf};

use atrium_agents::codex::CodexNormalizer;
use atrium_agents::fixture::{self, FixtureLine};
use atrium_protocol::agent::{
    AgentEvent, MessageKind, Provider, RequestOutcome, ToolKind, ToolStatus, TurnOutcome,
};

const RECORDED: &[&str] = &[
    "approval-allow",
    "approval-allow-startup-failure",
    "approval-deny",
    "approval-deny-startup-failure",
    "gate-deny",
    "gate-deny-execute",
    "gate-deny-execute-startup-failure",
    "gate-deny-startup-failure",
    "interrupt",
    "interrupt-startup-failure",
    "multi-turn",
    "multi-turn-startup-failure",
    "patch",
    "patch-startup-failure",
    "question",
    "question-startup-failure",
    "quota-usage",
    "quota-usage-startup-failure",
    "resume",
    "resume-origin",
    "simple",
    "simple-startup-failure",
    "startup-error",
    "tool-read",
    "tool-read-startup-failure",
];

const HANDMADE: &[&str] = &[
    "approval-allow",
    "interrupt",
    "patch",
    "question",
    "quota-usage",
];

fn codex_fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .join("fixtures")
        .join("agents")
        .join("codex")
}

fn fixture_paths() -> Vec<(String, PathBuf)> {
    let base = codex_fixtures_dir();
    let mut out = Vec::new();
    for name in RECORDED {
        out.push(((*name).to_owned(), base.join(format!("{name}.jsonl"))));
    }
    let hm = base.join("handmade");
    for name in HANDMADE {
        out.push((format!("handmade/{name}"), hm.join(format!("{name}.jsonl"))));
    }
    out
}

fn normalize_fixture(path: &Path) -> Vec<AgentEvent> {
    let lines = fixture::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    fixture::normalize(&lines, &mut CodexNormalizer::new())
}

#[test]
fn every_scenario_matches_its_golden_file() {
    for (label, path) in fixture_paths() {
        assert!(path.exists(), "missing fixture {label}: {}", path.display());
        let md = path.with_extension("md");
        assert!(md.exists(), "missing provenance note {}", md.display());
        fixture::assert_golden(&path, &mut CodexNormalizer::new());
    }
}

#[test]
fn fixtures_are_sanitized() {
    let users_pat = format!("{}Users{}", '/', '/');
    let home_pat = format!("{}home{}", '/', '/');
    let scratch_private = format!("{}private{}tmp{}", '/', '/', '/');
    for (label, path) in fixture_paths() {
        let text = fs::read_to_string(&path).expect("read fixture");
        for bad in [
            users_pat.as_str(),
            home_pat.as_str(),
            scratch_private.as_str(),
            "@icloud.com",
            "ATRIUM-7Q2X-CANARY",
            "America/",
        ] {
            assert!(!text.contains(bad), "{label}.jsonl contains {bad}");
        }
        for (idx, line) in text.lines().enumerate() {
            let parsed: FixtureLine = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{label}.jsonl line {}: {e}", idx + 1));
            let _ = parsed;
        }
    }
}

#[test]
fn recorded_simple_and_multi_turn_conform_to_m1() {
    let base = codex_fixtures_dir();
    let simple = normalize_fixture(&base.join("simple.jsonl"));
    assert!(matches!(
        simple.first(),
        Some(AgentEvent::SessionStarted {
            provider: Provider::Codex,
            ..
        })
    ));
    assert!(simple.iter().any(|e| matches!(
        e,
        AgentEvent::MessageCompleted {
            kind: MessageKind::User,
            ..
        }
    )));
    assert!(simple.iter().any(|e| matches!(
        e,
        AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Completed,
            ..
        }
    )));
    assert!(matches!(
        simple.last(),
        Some(AgentEvent::SessionEnded { .. })
    ));

    let multi = normalize_fixture(&base.join("multi-turn.jsonl"));
    let turns: Vec<u32> = multi
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnCompleted { turn, .. } => Some(*turn),
            _ => None,
        })
        .collect();
    assert_eq!(turns, vec![1, 2]);
}

#[test]
fn recorded_tool_read_and_gate_denies_keep_execute_kind() {
    let base = codex_fixtures_dir();
    for name in ["tool-read", "gate-deny", "gate-deny-execute"] {
        let events = normalize_fixture(&base.join(format!("{name}.jsonl")));
        let started_kinds: Vec<ToolKind> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolCallStarted { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect();
        assert!(
            !started_kinds.is_empty() && started_kinds.iter().all(|k| *k == ToolKind::Execute),
            "{name} commandExecution must map strictly to ToolKind::Execute, got {started_kinds:?}"
        );
    }

    for denied_name in ["gate-deny", "gate-deny-execute"] {
        let events = normalize_fixture(&base.join(format!("{denied_name}.jsonl")));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ApprovalRequested { .. })),
            "{denied_name} must not emit ApprovalRequested when gate denies"
        );
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolCallFinished {
                status: ToolStatus::Denied,
                ..
            }
        )));
    }
}

#[test]
fn recorded_approvals_resolve_as_expected() {
    let base = codex_fixtures_dir();
    let allow = normalize_fixture(&base.join("approval-allow.jsonl"));
    assert!(allow.iter().any(|e| matches!(
        e,
        AgentEvent::ApprovalRequested {
            request_id,
            kind: ToolKind::Execute,
            rememberable: true,
            ..
        } if request_id == "0"
    )));
    assert!(allow.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Approved,
        } if request_id == "0"
    )));
    assert!(allow.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    )));

    let deny = normalize_fixture(&base.join("approval-deny.jsonl"));
    assert!(deny.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Denied,
        } if request_id == "0"
    )));
    assert!(deny.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Denied,
            ..
        }
    )));
}

#[test]
fn startup_failures_emit_only_fatal_error_and_session_ended() {
    let base = codex_fixtures_dir();
    for name in RECORDED
        .iter()
        .copied()
        .filter(|n| *n == "startup-error" || n.ends_with("-startup-failure"))
    {
        let events = normalize_fixture(&base.join(format!("{name}.jsonl")));
        assert_eq!(
            events.len(),
            2,
            "{name} should emit exactly Error{{fatal:true}} then SessionEnded, got {events:?}"
        );
        assert!(matches!(
            events.first(),
            Some(AgentEvent::Error { fatal: true, .. })
        ));
        assert!(matches!(
            events.last(),
            Some(AgentEvent::SessionEnded { .. })
        ));
    }
}

#[test]
fn handmade_patch_question_interrupt_and_quota_conform() {
    let hm = codex_fixtures_dir().join("handmade");

    let patch = normalize_fixture(&hm.join("patch.jsonl"));
    assert!(patch.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallStarted {
            kind: ToolKind::Edit,
            ..
        }
    )));
    assert!(patch.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    )));

    let question = normalize_fixture(&hm.join("question.jsonl"));
    assert!(question.iter().any(|e| matches!(
        e,
        AgentEvent::QuestionAsked { request_id, .. } if request_id == "question-1"
    )));
    assert!(question.iter().any(|e| matches!(
        e,
        AgentEvent::RequestResolved {
            request_id,
            outcome: RequestOutcome::Answered,
        } if request_id == "question-1"
    )));

    let interrupt = normalize_fixture(&hm.join("interrupt.jsonl"));
    let cancel_tool_pos = interrupt
        .iter()
        .position(|e| {
            matches!(
                e,
                AgentEvent::ToolCallFinished {
                    status: ToolStatus::Cancelled,
                    ..
                }
            )
        })
        .expect("cancelled tool call");
    let cancel_req_pos = interrupt
        .iter()
        .position(|e| {
            matches!(
                e,
                AgentEvent::RequestResolved {
                    outcome: RequestOutcome::Cancelled,
                    ..
                }
            )
        })
        .expect("cancelled approval");
    let turn_done_pos = interrupt
        .iter()
        .position(|e| {
            matches!(
                e,
                AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Interrupted,
                    ..
                }
            )
        })
        .expect("interrupted turn completed");
    assert!(cancel_tool_pos < cancel_req_pos);
    assert!(cancel_req_pos < turn_done_pos);

    let quota = normalize_fixture(&hm.join("quota-usage.jsonl"));
    assert!(
        quota
            .iter()
            .any(|e| matches!(e, AgentEvent::QuotaUpdated { .. }))
    );
    assert!(
        quota
            .iter()
            .any(|e| matches!(e, AgentEvent::ContextUpdated { .. }))
    );
    assert!(quota.iter().any(|e| matches!(e, AgentEvent::Usage { .. })));
}
