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

    // 7. Rework round 3: option values without scripts, git CLI exec config, invalid matcher/kind, malformed rules
    let before_count = serde_json::from_value::<Vec<atrium_protocol::policy::Rule>>(
        wire.ok(DaemonCommand::ListRules).await,
    )
    .expect("rules")
    .len();

    let round3_refused = [
        // Finding 4: interpreter/runner option values mistaken for scripts or packages
        cmd_draft(
            RuleDecision::Allow,
            exact(&["python3", "-W", "ignore"]),
            None,
        ),
        cmd_draft(RuleDecision::Allow, exact(&["python3", "-X", "dev"]), None),
        cmd_draft(RuleDecision::Allow, exact(&["python3", "-m"]), None),
        cmd_draft(RuleDecision::Allow, exact(&["node", "-r", "preload"]), None),
        cmd_draft(RuleDecision::Allow, exact(&["npx", "-p", "prettier"]), None),
        // Finding 3: git command-line shell aliases / exec options
        cmd_draft(RuleDecision::Allow, prefix(&["git", "-c"]), None),
        cmd_draft(
            RuleDecision::Allow,
            exact(&["git", "-c", "alias.clean=!rm -f victim", "clean"]),
            None,
        ),
        cmd_draft(
            RuleDecision::Allow,
            prefix(&["git", "--exec-path=/tmp/bin"]),
            None,
        ),
        // Finding 5: Execute PathGlob (and command matcher with non-Execute kind)
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Execute,
            tool_name: None,
            matcher: RuleMatcher::PathGlob {
                glob: "/h/u/work/**".into(),
            },
            path_scope: Some("/h/u/work/**".into()),
            decision: RuleDecision::Allow,
        },
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Execute,
            tool_name: None,
            matcher: RuleMatcher::PathGlob {
                glob: "/h/u/work/**".into(),
            },
            path_scope: None,
            decision: RuleDecision::Allow,
        },
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Read,
            tool_name: None,
            matcher: prefix(&["cat"]),
            path_scope: None,
            decision: RuleDecision::Allow,
        },
        // Finding 6: malformed globs (relative, unexpanded ~, .., .) and command control chars / newlines
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Edit,
            tool_name: None,
            matcher: RuleMatcher::PathGlob {
                glob: "relative/**".into(),
            },
            path_scope: None,
            decision: RuleDecision::Allow,
        },
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Edit,
            tool_name: None,
            matcher: RuleMatcher::PathGlob {
                glob: "~/work/**".into(),
            },
            path_scope: None,
            decision: RuleDecision::Allow,
        },
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Edit,
            tool_name: None,
            matcher: RuleMatcher::PathGlob {
                glob: "/tmp/../etc/**".into(),
            },
            path_scope: None,
            decision: RuleDecision::Allow,
        },
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Read,
            tool_name: None,
            matcher: RuleMatcher::Any,
            path_scope: Some("relative/**".into()),
            decision: RuleDecision::Allow,
        },
        cmd_draft(RuleDecision::Allow, exact(&["cat", "a\nb"]), None),
        cmd_draft(RuleDecision::Deny, prefix(&["cat", "a\x1bb"]), None),
        RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Edit,
            tool_name: Some("two words".into()),
            matcher: RuleMatcher::Any,
            path_scope: None,
            decision: RuleDecision::Allow,
        },
    ];
    for rule in round3_refused {
        let code = wire
            .refused(DaemonCommand::CreateRule { rule: rule.clone() })
            .await;
        assert_eq!(code, ErrorCode::Invalid, "expected Invalid for {rule:?}");
    }

    // Verify none of the refused malformed rules were persisted
    let after_count = serde_json::from_value::<Vec<atrium_protocol::policy::Rule>>(
        wire.ok(DaemonCommand::ListRules).await,
    )
    .expect("rules")
    .len();
    assert_eq!(after_count, before_count);

    // Valid exact interpreter with value flag + script, and git commit -c HEAD, succeed
    for matcher in [
        exact(&["python3", "-W", "ignore", "build.py"]),
        prefix(&["git"]),
        exact(&["git", "commit", "-c", "HEAD"]),
        exact(&["git", "-c", "color.ui=false", "status"]),
    ] {
        wire.ok(DaemonCommand::CreateRule {
            rule: cmd_draft(RuleDecision::Allow, matcher, None),
        })
        .await;
    }

    // 8. Rework round 5: pwsh option-value scripts, normalized stdin paths, safe git exact vs prefix, thread scopes
    let round5_before_count = serde_json::from_value::<Vec<atrium_protocol::policy::Rule>>(
        wire.ok(DaemonCommand::ListRules).await,
    )
    .expect("rules")
    .len();

    let round5_refused_both: &[&[&str]] = &[
        // Finding 2: PowerShell option values cannot masquerade as .ps1 scripts
        &["pwsh", "-WorkingDirectory", "work.ps1"],
        &["pwsh", "-wd", "work.ps1"],
        &["pwsh", "-WorkingDirectory=work.ps1"],
        &["pwsh", "/WorkingDirectory", "work.ps1"],
        &["pwsh", "-UnknownOpt", "work.ps1"],
        &["pwsh", "-File"],
        &["pwsh", "-File", "-"],
        // Finding 3: normalized stdin aliases
        &["python3", "/dev/./stdin"],
        &["python3", "///dev//stdin"],
        &["python3", "/dev/foo/../stdin"],
        &["node", "/dev/./fd/0"],
        &["perl", "/proc/self/./fd/0"],
        &["ruby", "/proc/thread-self/fd/0"],
    ];
    for argv in round5_refused_both {
        for matcher in [prefix(argv), exact(argv)] {
            let code = wire
                .refused(DaemonCommand::CreateRule {
                    rule: cmd_draft(RuleDecision::Allow, matcher.clone(), None),
                })
                .await;
            assert_eq!(code, ErrorCode::Invalid, "expected Invalid for {matcher:?}");
        }
    }

    // Finding 6: prefix `git -c color.ui=false status` is refused (exact already succeeded above)
    assert_eq!(
        wire.refused(DaemonCommand::CreateRule {
            rule: cmd_draft(
                RuleDecision::Allow,
                prefix(&["git", "-c", "color.ui=false", "status"]),
                None,
            ),
        })
        .await,
        ErrorCode::Invalid
    );

    // Finding 7: malformed / unknown thread scopes are refused with Invalid and not stored
    for bad_tid in ["", "   ", "nonexistent-thread"] {
        let code = wire
            .refused(DaemonCommand::CreateRule {
                rule: RuleDraft {
                    scope: RuleScope::Thread {
                        thread_id: atrium_protocol::thread::ThreadId(bad_tid.into()),
                    },
                    tool_kind: ToolKind::Execute,
                    tool_name: None,
                    matcher: prefix(&["cargo", "test"]),
                    path_scope: None,
                    decision: RuleDecision::Allow,
                },
            })
            .await;
        assert_eq!(
            code,
            ErrorCode::Invalid,
            "expected Invalid for thread_id={bad_tid:?}"
        );
    }

    let round5_after_count = serde_json::from_value::<Vec<atrium_protocol::policy::Rule>>(
        wire.ok(DaemonCommand::ListRules).await,
    )
    .expect("rules")
    .len();
    assert_eq!(round5_after_count, round5_before_count);

    // A thread-scoped rule targeting an existing thread succeeds
    let existing = wire.start_thread(common::native(), None).await.thread_id;
    wire.ok(DaemonCommand::CreateRule {
        rule: RuleDraft {
            scope: RuleScope::Thread { thread_id: existing },
            tool_kind: ToolKind::Execute,
            tool_name: None,
            matcher: prefix(&["cargo", "test"]),
            path_scope: None,
            decision: RuleDecision::Allow,
        },
    })
    .await;

    rig.stop(true).await;
}
