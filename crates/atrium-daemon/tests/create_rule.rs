//! Daemon `CreateRule` validation tests (D-121 / M1-29).

mod common;

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{RuleDecision, RuleDraft, RuleMatcher, RuleScope};
use atrium_protocol::wire::{DaemonCommand, ErrorCode};
use common::Rig;

fn cmd_draft(decision: RuleDecision, matcher: RuleMatcher, path_scope: Option<&str>) -> RuleDraft {
    RuleDraft {
        scope: RuleScope::Global,
        tool_kind: ToolKind::Execute,
        tool_name: None,
        matcher,
        path_scope: path_scope.map(str::to_string),
        decision,
    }
}

fn prefix(words: &[&str]) -> RuleMatcher {
    RuleMatcher::CommandPrefix {
        argv: words.iter().map(|s| (*s).to_string()).collect(),
    }
}

fn exact(words: &[&str]) -> RuleMatcher {
    RuleMatcher::CommandExact {
        argv: words.iter().map(|s| (*s).to_string()).collect(),
    }
}

#[tokio::test]
async fn create_rule_refuses_unsafe_command_allow_rules_and_invalid_path_scopes() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    // 1. Refused for both CommandPrefix and CommandExact Allow rules
    let refused_both: &[&[&str]] = &[
        &["sh"],
        &["bash", "-c"],
        &["/bin/BASH", "-c", "ls"],
        &["/bin/ba\u{017F}h"],
        &["/bin/\u{212A}sh"],
        &["zsh"],
        &["fish"],
        &["eval"],
        &["EVAL", "ls"],
        &["source", "env.sh"],
        &[".", "env.sh"],
        &["env"],
        &["/usr/bin/ENV", "make"],
        &["xargs", "rm"],
        &["nohup", "make"],
        &["timeout", "60", "cargo", "test"],
        &["sudo", "ls"],
        &["SUDO", "id"],
        &["sudoedit", "/etc/hosts"],
        &["doas", "ls"],
        &["su", "-"],
        &["pkexec", "ls"],
        &["run0", "ls"],
        &["python3", "-c", "print(1)"],
        &["python3", "-cprint(1)"],
        &["python3", "-Ic", "x"],
        &["python3"],
        &["python3", "-u"],
        &["python3", "-"],
        &["python3", "/dev/stdin"],
        &["node", "-e", "1"],
        &["node", "--eval=1"],
        &["node", "/dev/fd/0"],
        &["node"],
        &["deno", "eval", "1"],
        &["deno", "-q", "eval", "1"],
        &["deno", "run"],
        &["bun", "exec", "rm -rf build"],
        &["bun", "eval", "1"],
        &["bun", "run"],
        &["perl", "-0e", "1"],
        &["ruby", "-e", "1"],
        &["php", "-r", "1;"],
        &["lua", "-e", "1"],
        &["osascript", "-e", "1"],
        &["pwsh", "-c", "ls"],
        &["pwsh", "-Command", "ls"],
        &["pwsh", "-ec", "AA"],
        &["pwsh"],
        &["npx", "-c", "sh"],
        &["npx", "-yc", "sh"],
        &["npx", "--call=sh"],
        &["npx", "prettier", "-c'sh'"],
        &["npx"],
        &["find", ".", "-exec", "rm", "{}", "+"],
        &["find", ".", "-execdir", "rm", "{}", "+"],
        &["find", ".", "-ok", "rm", "{}", ";"],
        &["if", "true"],
    ];

    for argv in refused_both {
        for matcher in [prefix(argv), exact(argv)] {
            let code = wire
                .refused(DaemonCommand::CreateRule {
                    rule: cmd_draft(RuleDecision::Allow, matcher.clone(), None),
                })
                .await;
            assert_eq!(code, ErrorCode::Invalid, "expected Invalid for {matcher:?}");
        }
    }

    // 2. Refused as CommandPrefix Allow (allowed as CommandExact)
    let refused_prefix_only: &[&[&str]] = &[
        &["find"],
        &["find", "."],
        &["find", ".", "-name", "x"],
        &["FIND", ".", "-name", "x"],
        &["python3", "-u", "build.py"],
        &["python3", "-m", "pytest"],
        &["npx", "-y", "prettier"],
        &["deno", "run", "-A", "x.ts"],
    ];

    for argv in refused_prefix_only {
        let code = wire
            .refused(DaemonCommand::CreateRule {
                rule: cmd_draft(RuleDecision::Allow, prefix(argv), None),
            })
            .await;
        assert_eq!(
            code,
            ErrorCode::Invalid,
            "expected prefix Invalid for {argv:?}"
        );

        wire.ok(DaemonCommand::CreateRule {
            rule: cmd_draft(RuleDecision::Allow, exact(argv), None),
        })
        .await;
    }

    // 3. Ordinary Allow rules succeed
    for (matcher, kind) in [
        (prefix(&["cargo", "test"]), ToolKind::Execute),
        (prefix(&["python3", "scripts/build.py"]), ToolKind::Execute),
        (prefix(&["deno", "run", "x.ts"]), ToolKind::Execute),
        (prefix(&["bun", "run", "build"]), ToolKind::Execute),
        (prefix(&["npx", "prettier"]), ToolKind::Execute),
        (exact(&["find", ".", "-delete"]), ToolKind::Execute),
        (exact(&["pwsh", "-File", "build.ps1"]), ToolKind::Execute),
    ] {
        wire.ok(DaemonCommand::CreateRule {
            rule: RuleDraft {
                scope: RuleScope::Global,
                tool_kind: kind,
                tool_name: None,
                matcher,
                path_scope: None,
                decision: RuleDecision::Allow,
            },
        })
        .await;
    }

    // 4. Deny rules on shells / wrappers / inline code are allowed (without path_scope)
    for argv in [
        &["bash"][..],
        &["sh", "-c"][..],
        &["sudo"][..],
        &["env"][..],
        &["python3", "-c"][..],
        &["find", ".", "-exec"][..],
    ] {
        wire.ok(DaemonCommand::CreateRule {
            rule: cmd_draft(RuleDecision::Deny, prefix(argv), None),
        })
        .await;
    }

    // 5. Path scope is refused on command rules, deny rules, and non-file tool kinds
    let bad_scoped = [
        cmd_draft(
            RuleDecision::Allow,
            prefix(&["cargo", "test"]),
            Some("/tmp/proj/**"),
        ),
        cmd_draft(
            RuleDecision::Allow,
            exact(&["cargo", "test"]),
            Some("/tmp/proj/**"),
        ),
        cmd_draft(RuleDecision::Allow, RuleMatcher::Any, Some("/tmp/proj/**")),
        cmd_draft(
            RuleDecision::Deny,
            exact(&["git", "push"]),
            Some("/tmp/proj/**"),
        ),
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Edit,
            tool_name: None,
            matcher: RuleMatcher::Any,
            path_scope: Some("/tmp/proj/**".into()),
            decision: RuleDecision::Deny,
        },
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Edit,
            tool_name: None,
            matcher: RuleMatcher::PathGlob {
                glob: "/tmp/proj/**".into(),
            },
            path_scope: Some("/tmp/proj/**".into()),
            decision: RuleDecision::Deny,
        },
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Fetch,
            tool_name: None,
            matcher: RuleMatcher::Any,
            path_scope: Some("/tmp/proj/**".into()),
            decision: RuleDecision::Allow,
        },
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Edit,
            tool_name: None,
            matcher: RuleMatcher::Any,
            path_scope: Some("/tmp/[unclosed".into()),
            decision: RuleDecision::Allow,
        },
    ];
    for rule in bad_scoped {
        let code = wire.refused(DaemonCommand::CreateRule { rule }).await;
        assert_eq!(code, ErrorCode::Invalid);
    }

    // 6. Path scope is accepted on Allow rules for file tools
    for (kind, matcher) in [
        (ToolKind::Edit, RuleMatcher::Any),
        (
            ToolKind::Read,
            RuleMatcher::PathGlob {
                glob: "/tmp/proj/**".into(),
            },
        ),
    ] {
        wire.ok(DaemonCommand::CreateRule {
            rule: RuleDraft {
                scope: RuleScope::Global,
                tool_kind: kind,
                tool_name: None,
                matcher,
                path_scope: Some("/tmp/proj/**".into()),
                decision: RuleDecision::Allow,
            },
        })
        .await;
    }

    rig.stop(true).await;
}
