//! Daemon `CreateRule` and `ListRules` validation under policy rev 4 §7 & §11
//! items 4–5 (owned by P3).

mod common;

use atrium_core::Store;
use atrium_core::store::{SqliteStore, StoreOptions};
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{
    ListedRule, Rule, RuleDecision, RuleDraft, RuleId, RuleMatcher, RuleOrigin, RuleScope,
};
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{DaemonCommand, ErrorCode};
use common::Rig;

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| (*s).to_owned()).collect()
}

fn cmd_prefix_draft(decision: RuleDecision, words: &[&str]) -> RuleDraft {
    RuleDraft {
        scope: RuleScope::Global,
        tool_kind: ToolKind::Execute,
        tool_name: None,
        matcher: RuleMatcher::CommandPrefix { argv: argv(words) },
        path_scope: None,
        decision,
    }
}

#[tokio::test]
async fn create_rule_rejects_unpinned_allow_code_runner_prefixes_and_invalid_globs() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let invalid_prefixes: &[&[&str]] = &[
        &[],
        &[""],
        &["awk"],
        &["gawk"],
        &["mawk"],
        &["awk", "-f"],
        &["awk", "-f", "-"],
        &["awk", "-f", "a.awk", "-e", "{print}"],
        &["gawk", "-f", "prog.awk", "-Wsource=BEGIN{print 1}"],
        &["gawk", "-f", "prog.awk", "-W", "source=BEGIN{print 1}"],
        &["gawk", "-f", "prog.awk", "-W"],
        &["python"],
        &["python3"],
        &["python3", "-c"],
        &["python3", "-u", "build.py"],
        &["node"],
        &["perl"],
        &["ruby"],
        &["pipx"],
        &["pipx", "run"],
        &["pipx", "install"],
        &["cargo"],
        &["git"],
        &["git", "merge-index"],
        &["git", "filter-branch"],
        &["git", "bisect"],
        &["git", "bisect", "run"],
        &["git", "submodule"],
        &["git", "submodule", "foreach"],
        &["git", "rebase", "-x", "id"],
        &["git", "diff", "--ext-diff"],
        &["git", "diff", "--unified", "--ext-diff"],
        &["git", "diff", "-S", "--ext-diff"],
        &["git", "push", "--exec=x"],
        &["git", "push", "--receive-pack=x"],
        &["find"],
        &["find", "."],
        &["sed"],
        &["sed", "-f"],
        &["sed", "-f", "-"],
        &["sed", "1e id"],
        &["sed", "s/a/b/w out"],
        &["sed", "s€a€b€"],
        &["sed", "-n", "s€a€b€p"],
        &["sed", "-n", "y€ab€cd€"],
        &["sed", "-n", "\\€re€p"],
        &["sh"],
        &["bash"],
        &["bash", "-c"],
        &["dash"],
        &["zsh"],
        &["fish"],
        &["nohup"],
        &["chroot"],
        &["eval"],
        &["xargs"],
        &["ssh"],
        &["docker"],
        &["kubectl"],
        &["source"],
        &["."],
        &["npx"],
        // CR-19 folded command words.
        &["/usr/bin/PYTHON"],
        &["/bin/ba\u{017F}h"],
        &["/bin/\u{212A}sh"],
        &["AWK"],
    ];

    for &prefix in invalid_prefixes {
        let res = wire
            .command(DaemonCommand::CreateRule {
                rule: cmd_prefix_draft(RuleDecision::Allow, prefix),
            })
            .await;
        match res {
            Err((code, msg)) => {
                assert_eq!(code, ErrorCode::Invalid, "prefix {prefix:?}");
                assert!(!msg.is_empty(), "expected reason for {prefix:?}");
            }
            Ok(v) => panic!("expected Invalid for Allow prefix {prefix:?}, got Ok({v})"),
        }
    }

    // Invalid PathGlob and path_scope globs.
    for bad_glob in ["", "/a/[", "/a/[z-a]", "/a/{b", "/a/x\\"] {
        let err = wire
            .refused(DaemonCommand::CreateRule {
                rule: RuleDraft {
                    scope: RuleScope::Global,
                    tool_kind: ToolKind::Edit,
                    tool_name: None,
                    matcher: RuleMatcher::PathGlob {
                        glob: bad_glob.into(),
                    },
                    path_scope: None,
                    decision: RuleDecision::Allow,
                },
            })
            .await;
        assert_eq!(err, ErrorCode::Invalid, "glob {bad_glob:?}");

        let scope_err = wire
            .refused(DaemonCommand::CreateRule {
                rule: RuleDraft {
                    scope: RuleScope::Global,
                    tool_kind: ToolKind::Edit,
                    tool_name: None,
                    matcher: RuleMatcher::Any,
                    path_scope: Some(bad_glob.into()),
                    decision: RuleDecision::Allow,
                },
            })
            .await;
        assert_eq!(scope_err, ErrorCode::Invalid, "path_scope {bad_glob:?}");
    }

    // Empty thread scope id.
    assert_eq!(
        wire.refused(DaemonCommand::CreateRule {
            rule: RuleDraft {
                scope: RuleScope::Thread {
                    thread_id: ThreadId(String::new()),
                },
                tool_kind: ToolKind::Execute,
                tool_name: None,
                matcher: RuleMatcher::CommandPrefix {
                    argv: argv(&["cat"])
                },
                path_scope: None,
                decision: RuleDecision::Allow,
            },
        })
        .await,
        ErrorCode::Invalid
    );

    // None of the rejected rules were stored.
    let rules: Vec<Rule> =
        serde_json::from_value(wire.ok(DaemonCommand::ListRules).await).expect("rules");
    assert!(rules.is_empty(), "rejected rules must not be stored");

    rig.stop(true).await;
}

#[tokio::test]
async fn create_rule_accepts_pinned_allow_prefixes_deny_rules_and_exact_commands() {
    let mut rig = Rig::start().await;
    let mut wire = rig.dev().await;

    let valid_allow_prefixes: &[&[&str]] = &[
        &["cargo", "test"],
        &["python", "build.py"],
        &["pipx", "run", "ruff"],
        &["awk", "-f", "prog.awk"],
        &["gawk", "-f", "prog.awk"],
        &["sed", "-f", "edit.sed"],
        &["sed", "s/error/warn/g"],
        &["sed", "s/é/x/"],
        &["sed", "s/x/é/"],
        &["sed", "z"],
        &["sh", "x.sh"],
        &["source", "env.sh"],
        &[".", "./env.sh"],
        &["npx", "prettier"],
        &["git", "status"],
        &["git", "diff"],
        &["git", "bisect", "start"],
        &["git", "submodule", "status"],
        &["cat"],
        // Tools removed by D-141 are unknown tools -> accepted by core and daemon.
        &["deno"],
        &["bun"],
        &["php"],
        &["lua"],
        &["pwsh"],
    ];

    for &prefix in valid_allow_prefixes {
        let created = wire
            .ok(DaemonCommand::CreateRule {
                rule: cmd_prefix_draft(RuleDecision::Allow, prefix),
            })
            .await;
        let parsed: Rule = serde_json::from_value(created).expect("Rule");
        assert_eq!(
            parsed.matcher,
            RuleMatcher::CommandPrefix { argv: argv(prefix) }
        );
    }

    // Deny prefixes on code runners remain allowed (§7).
    for deny_prefix in [&["awk"][..], &["python"][..], &["git"][..], &["bash"][..]] {
        wire.ok(DaemonCommand::CreateRule {
            rule: cmd_prefix_draft(RuleDecision::Deny, deny_prefix),
        })
        .await;
    }

    // CommandExact on code runners and valid PathGlob.
    wire.ok(DaemonCommand::CreateRule {
        rule: RuleDraft {
            scope: RuleScope::Home,
            tool_kind: ToolKind::Execute,
            tool_name: None,
            matcher: RuleMatcher::CommandExact {
                argv: argv(&["python", "build.py"]),
            },
            path_scope: None,
            decision: RuleDecision::Allow,
        },
    })
    .await;
    wire.ok(DaemonCommand::CreateRule {
        rule: RuleDraft {
            scope: RuleScope::Global,
            tool_kind: ToolKind::Edit,
            tool_name: None,
            matcher: RuleMatcher::PathGlob {
                glob: "/proj/src/**".into(),
            },
            path_scope: Some("/proj/**".into()),
            decision: RuleDecision::Allow,
        },
    })
    .await;

    let listed = wire.ok(DaemonCommand::ListRules).await;
    let listed_rules: Vec<ListedRule> =
        serde_json::from_value(listed).expect("deserializes as Vec<ListedRule>");
    assert_eq!(listed_rules.len(), valid_allow_prefixes.len() + 6);
    for item in &listed_rules {
        assert!(
            item.invalid_reason.is_none(),
            "valid rule must not carry invalid_reason: {item:?}"
        );
    }

    rig.stop(true).await;
}

#[tokio::test]
async fn list_rules_keeps_stored_invalid_rules_visible_and_marked_invalid() {
    // §11 item 5: pre-existing stored rules that fail rev 4 §7 validation stay
    // listed, are marked invalid with a reason, and remain deletable.
    let mut rig = Rig::in_folder(common::folder());
    // Finding 14: create data_dir with `secure_dir` (0700) so daemon startup accepts it.
    atrium_host::lifecycle::secure_dir(&rig.paths.data_dir).expect("secures data_dir");
    {
        let store = SqliteStore::open(&rig.paths.store, StoreOptions::default())
            .await
            .expect("open store");
        store
            .create_rule(Rule {
                id: RuleId("rule_legacy_awk".into()),
                scope: RuleScope::Global,
                tool_kind: ToolKind::Execute,
                tool_name: None,
                matcher: RuleMatcher::CommandPrefix {
                    argv: argv(&["awk"]),
                },
                path_scope: None,
                decision: RuleDecision::Allow,
                origin: RuleOrigin::Settings,
                created_at_ms: 1,
            })
            .await
            .expect("seed invalid rule");
        store
            .create_rule(Rule {
                id: RuleId("rule_valid_sed_z".into()),
                scope: RuleScope::Global,
                tool_kind: ToolKind::Execute,
                tool_name: None,
                matcher: RuleMatcher::CommandPrefix {
                    argv: argv(&["sed", "z"]),
                },
                path_scope: None,
                decision: RuleDecision::Allow,
                origin: RuleOrigin::Settings,
                created_at_ms: 2,
            })
            .await
            .expect("seed valid rule");
        store
            .create_rule(Rule {
                id: RuleId("rule_legacy_git_diff_unified_ext".into()),
                scope: RuleScope::Global,
                tool_kind: ToolKind::Execute,
                tool_name: None,
                matcher: RuleMatcher::CommandPrefix {
                    argv: argv(&["git", "diff", "--unified", "--ext-diff"]),
                },
                path_scope: None,
                decision: RuleDecision::Allow,
                origin: RuleOrigin::Settings,
                created_at_ms: 3,
            })
            .await
            .expect("seed invalid git diff rule");
        store
            .create_rule(Rule {
                id: RuleId("rule_legacy_gawk_wsource".into()),
                scope: RuleScope::Global,
                tool_kind: ToolKind::Execute,
                tool_name: None,
                matcher: RuleMatcher::CommandPrefix {
                    argv: argv(&["gawk", "-f", "prog.awk", "-Wsource=BEGIN{print 1}"]),
                },
                path_scope: None,
                decision: RuleDecision::Allow,
                origin: RuleOrigin::Settings,
                created_at_ms: 4,
            })
            .await
            .expect("seed invalid gawk -Wsource rule");
    }

    rig.boot(|_| {}).await;
    let mut wire = rig.dev().await;

    let listed = wire.ok(DaemonCommand::ListRules).await;
    let items: Vec<ListedRule> =
        serde_json::from_value(listed.clone()).expect("deserializes as Vec<ListedRule>");
    assert_eq!(items.len(), 4);

    for invalid_id in [
        "rule_legacy_awk",
        "rule_legacy_git_diff_unified_ext",
        "rule_legacy_gawk_wsource",
    ] {
        let item = items
            .iter()
            .find(|r| r.id == RuleId(invalid_id.into()))
            .unwrap_or_else(|| panic!("stored invalid rule {invalid_id} must stay listed"));
        let reason = item
            .invalid_reason
            .as_deref()
            .unwrap_or_else(|| panic!("stored invalid rule {invalid_id} must have invalid_reason"));
        assert!(!reason.is_empty(), "{invalid_id}");
    }

    // Finding 7 parity: `["sed", "z"]` is accepted by core, so daemon marks it valid.
    let valid = items
        .iter()
        .find(|r| r.id == RuleId("rule_valid_sed_z".into()))
        .expect("valid rule stays listed");
    assert!(valid.invalid_reason.is_none());

    // Wire-compatibility: callers deserializing `Vec<Rule>` still succeed.
    let as_rules: Vec<Rule> = serde_json::from_value(listed).expect("deserializes as Vec<Rule>");
    assert_eq!(as_rules.len(), 4);

    // Stored invalid rule can still be deleted via DeleteRule.
    wire.ok(DaemonCommand::DeleteRule {
        rule_id: RuleId("rule_legacy_awk".into()),
    })
    .await;
    let remaining: Vec<ListedRule> =
        serde_json::from_value(wire.ok(DaemonCommand::ListRules).await).expect("remaining rules");
    assert_eq!(remaining.len(), 3);
    assert!(
        remaining
            .iter()
            .any(|r| r.id == RuleId("rule_valid_sed_z".into()))
    );

    rig.stop(true).await;
}
