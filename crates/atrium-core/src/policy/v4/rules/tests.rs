use atrium_agents::ToolCheck;
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{
    Preset, Rule, RuleDecision, RuleId, RuleMatcher, RuleOrigin, RuleScope, ToolSubject,
};
use atrium_protocol::thread::ThreadId;
use serde_json::json;

use crate::policy::model::{
    AccessMode, BestEffortRecord, BestEffortUnit, ByteProvenance, CallOutcome, CommandUnit,
    ComputeBudget, ExhaustReason, FoldRules, Hit, HitMatch, HitSource, ModelRedirect, PartId,
    PartOutcome, PartResult, RemovedWrapper, RuleEvalContext, RuleValidationError, Separator,
    Target, TargetOrigin, TargetResolution, ToolEffects, UnitIndex, UnreadReason, Word,
    WrapperKind,
};
use crate::policy::v4::tools::classify_unit;

use super::{evaluate_rules, format_validation_error, remember_matcher, validate_rule_matcher};

fn w(s: &str) -> Word {
    Word::from_parts(
        s.as_bytes().to_vec(),
        vec![ByteProvenance::Unquoted; s.len()],
    )
    .expect("equal len")
}

fn make_unit(idx: usize, args: &[&str]) -> CommandUnit {
    CommandUnit {
        index: UnitIndex(idx),
        argv: args.iter().map(|s| w(s)).collect(),
        removed_wrappers: Vec::new(),
        redirects: Vec::new(),
        separator_before: Separator::Start,
        pipeline_id: None,
        depth: 0,
        parent_unit: None,
    }
}

fn classify_all(units: &[CommandUnit]) -> Vec<ToolEffects> {
    let mut budget = ComputeBudget::with_default_limits();
    units
        .iter()
        .map(|u| classify_unit(u, &FoldRules::REV4, &mut budget).expect("classify"))
        .collect()
}

fn cmd_check(line: &str) -> ToolCheck {
    ToolCheck {
        call_id: None,
        name: "Bash".into(),
        kind: ToolKind::Execute,
        title: None,
        input: json!({ "command": line }),
        subject: ToolSubject::Command {
            line: line.into(),
            cwd: None,
        },
        turn: 1,
    }
}

fn file_check(kind: ToolKind, name: &str, paths: &[&str]) -> ToolCheck {
    ToolCheck {
        call_id: None,
        name: name.into(),
        kind,
        title: None,
        input: json!({}),
        subject: ToolSubject::Paths {
            paths: paths.iter().map(|s| (*s).to_owned()).collect(),
            glob: None,
            recursive: false,
        },
        turn: 1,
    }
}

fn make_rule(
    id: &str,
    scope: RuleScope,
    tool_kind: ToolKind,
    matcher: RuleMatcher,
    decision: RuleDecision,
) -> Rule {
    Rule {
        id: RuleId(id.into()),
        scope,
        tool_kind,
        tool_name: None,
        matcher,
        path_scope: None,
        decision,
        origin: RuleOrigin::Settings,
        created_at_ms: 1,
    }
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| (*s).to_owned()).collect()
}

fn resolved_target(origin: TargetOrigin, path: &str) -> Target {
    Target {
        origin,
        access: AccessMode::Read,
        resolution: TargetResolution::Resolved {
            canonical: path.as_bytes().to_vec(),
            identities: Vec::new(),
            dangling_symlink: false,
            multiply_linked: false,
        },
        written: Some(path.as_bytes().to_vec()),
    }
}

fn be_unit(idx: usize, words: &[&str]) -> BestEffortUnit {
    let v = argv(words);
    BestEffortUnit {
        index: idx,
        raw_argv: v.clone(),
        effective_argv: v,
        alt_argv: Vec::new(),
        assignments: Vec::new(),
        redirects: Vec::new(),
        pipeline_id: None,
        piped: false,
        depth: 0,
        derived: false,
    }
}

// ---------------------------------------------------------------------------
// 1. `validate_rule_matcher` (§7, §10 row 13, §11 item 4)
// ---------------------------------------------------------------------------

#[test]
fn validate_rule_matcher_rejects_unpinned_allow_code_runner_prefixes() {
    // §10 row 13 and §7 / §11 item 4 invalid Allow prefixes.
    let invalid_prefixes: &[&[&str]] = &[
        &["awk"],
        &["gawk"],
        &["mawk"],
        &["awk", "-f"],
        &["awk", "-f", "-"],
        &["awk", "-f", "a.awk", "-e", "{print}"],
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
        &["git", "push", "--exec=x"],
        &["git", "push", "--receive-pack=x"],
        &["find"],
        &["find", "."],
        &["sed"],
        &["sed", "-f"],
        &["sed", "-f", "-"],
        &["sed", "1e id"],
        &["sed", "s/a/b/w out"],
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
        let matcher = RuleMatcher::CommandPrefix { argv: argv(prefix) };
        let Err(err) = validate_rule_matcher(&matcher, RuleDecision::Allow, &FoldRules::REV4)
        else {
            panic!("expected Allow prefix {prefix:?} to be rejected");
        };
        assert!(
            matches!(err, RuleValidationError::UnpinnedCodeRunner { .. }),
            "unexpected error for {prefix:?}: {err:?}"
        );
        assert!(!format_validation_error(&err).is_empty());

        // Deny prefixes on code runners remain valid (§7).
        if let Err(e) = validate_rule_matcher(&matcher, RuleDecision::Deny, &FoldRules::REV4) {
            panic!("Deny prefix {prefix:?} must be allowed: {e:?}");
        }
    }

    // Empty argv and invalid globs are rejected for both Allow and Deny.
    for decision in [RuleDecision::Allow, RuleDecision::Deny] {
        assert_eq!(
            validate_rule_matcher(
                &RuleMatcher::CommandPrefix { argv: Vec::new() },
                decision,
                &FoldRules::REV4
            ),
            Err(RuleValidationError::EmptyArgv)
        );
        assert_eq!(
            validate_rule_matcher(
                &RuleMatcher::CommandExact {
                    argv: vec![String::new()]
                },
                decision,
                &FoldRules::REV4
            ),
            Err(RuleValidationError::EmptyArgv)
        );
        for bad_glob in ["", "/a/[", "/a/[z-a]", "/a/{b"] {
            assert!(
                matches!(
                    validate_rule_matcher(
                        &RuleMatcher::PathGlob {
                            glob: bad_glob.into()
                        },
                        decision,
                        &FoldRules::REV4
                    ),
                    Err(RuleValidationError::InvalidGlob { .. })
                ),
                "{bad_glob}"
            );
        }
    }
}

#[test]
fn validate_rule_matcher_accepts_pinned_allow_prefixes_and_exact_rules() {
    let valid_prefixes: &[&[&str]] = &[
        &["cargo", "test"],
        &["cargo", "clippy"],
        &["python", "build.py"],
        &["python3", "scripts/build.py", "--fast"],
        &["pipx", "run", "ruff"],
        &["awk", "-f", "prog.awk"],
        &["sed", "-f", "edit.sed"],
        &["sed", "s/error/warn/g"],
        &["sh", "x.sh"],
        &["bash", "scripts/deploy.sh"],
        &["source", "env.sh"],
        &[".", "./env.sh"],
        &["npx", "prettier"],
        &["git", "status"],
        &["git", "diff"],
        &["git", "bisect", "start"],
        &["git", "submodule", "status"],
        &["cat"],
        &["rm", "-rf", "build"],
    ];

    for &prefix in valid_prefixes {
        let matcher = RuleMatcher::CommandPrefix { argv: argv(prefix) };
        if let Err(e) = validate_rule_matcher(&matcher, RuleDecision::Allow, &FoldRules::REV4) {
            panic!("expected Allow prefix {prefix:?} to be valid: {e:?}");
        }
    }

    // CommandExact is valid even on code runners.
    for exact in [
        &["python", "build.py"][..],
        &["find", ".", "-name", "x"][..],
        &["awk", "{print $1}", "a.txt"][..],
    ] {
        validate_rule_matcher(
            &RuleMatcher::CommandExact { argv: argv(exact) },
            RuleDecision::Allow,
            &FoldRules::REV4,
        )
        .expect("CommandExact valid");
    }

    validate_rule_matcher(&RuleMatcher::Any, RuleDecision::Allow, &FoldRules::REV4).unwrap();
    validate_rule_matcher(
        &RuleMatcher::PathGlob {
            glob: "/proj/src/**".into(),
        },
        RuleDecision::Allow,
        &FoldRules::REV4,
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// 2. `evaluate_rules` (§1 step 3, §2, §7, §10 rows 14, 19, 26, §11 items 5 & 8)
// ---------------------------------------------------------------------------

#[test]
fn evaluate_rules_precedence_and_row_26_thread_allow_beats_global_deny() {
    // §10 row 26: global Deny `cat` + thread Allow `cat` on `cat a` → thread Allow wins,
    // even when `best_effort.units` also contains `cat a`.
    let tid = ThreadId("t1".into());
    let check = cmd_check("cat a");
    let units = vec![make_unit(0, &["cat", "a"])];
    let effects = classify_all(&units);
    let be = BestEffortRecord {
        raw_texts: vec!["cat a".into()],
        units: vec![be_unit(0, &["cat", "a"])],
    };
    let ctx = RuleEvalContext {
        check: &check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units,
        effects: &effects,
        targets: &[],
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };

    let rules = vec![
        make_rule(
            "r_global_deny",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["cat"]),
            },
            RuleDecision::Deny,
        ),
        make_rule(
            "r_thread_allow",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["cat"]),
            },
            RuleDecision::Allow,
        ),
    ];

    let mut budget = ComputeBudget::with_default_limits();
    let cov = evaluate_rules(&ctx, &rules, &mut budget).unwrap();
    assert!(cov.deny.is_none());
    assert_eq!(
        cov.per_unit[0].allow_rule,
        Some(RuleId("r_thread_allow".into()))
    );

    // Within the SAME rank (both Thread), Deny beats Allow.
    let same_rank = vec![
        make_rule(
            "r_thread_allow",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["cat"]),
            },
            RuleDecision::Allow,
        ),
        make_rule(
            "r_thread_deny",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["cat"]),
            },
            RuleDecision::Deny,
        ),
    ];
    let cov2 = evaluate_rules(&ctx, &same_rank, &mut budget).unwrap();
    assert_eq!(
        cov2.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_thread_deny".into()))
    );
}

#[test]
fn evaluate_rules_multi_unit_and_row_19_best_effort_deny() {
    let tid = ThreadId("t1".into());
    let mut budget = ComputeBudget::with_default_limits();

    // §10 row 19: `cat "$X"` (no exact units, only best-effort) + Deny `cat` → Deny!
    let check_be = cmd_check("cat \"$X\"");
    let be_only = BestEffortRecord {
        raw_texts: vec!["cat \"$X\"".into()],
        units: vec![be_unit(0, &["cat", "$X"])],
    };
    let ctx_be = RuleEvalContext {
        check: &check_be,
        thread_id: &tid,
        preset: Preset::Home,
        units: &[],
        effects: &[],
        targets: &[],
        outcomes: &[],
        best_effort: &be_only,
        folds: &FoldRules::REV4,
    };
    let deny_cat = vec![make_rule(
        "r_deny_cat",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["cat"]),
        },
        RuleDecision::Deny,
    )];
    let cov_be = evaluate_rules(&ctx_be, &deny_cat, &mut budget).unwrap();
    assert_eq!(
        cov_be.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_deny_cat".into()))
    );

    // Finding 2: partial coverage at a higher rank MUST NOT suppress a lower-ranked Deny.
    // `cat a && ls b` with Thread Allow `cat` + Global Deny `cat`:
    // Thread rank covers only unit 0 (`cat a`), not unit 1 (`ls b`), so the call
    // remains uncovered and Global Deny `cat` still matches unit 0 -> Deny!
    let check_multi = cmd_check("cat a && ls b");
    let units = vec![make_unit(0, &["cat", "a"]), make_unit(1, &["ls", "b"])];
    let effects = classify_all(&units);
    let be_multi = BestEffortRecord {
        raw_texts: vec!["cat a && ls b".into()],
        units: vec![be_unit(0, &["cat", "a"]), be_unit(1, &["ls", "b"])],
    };
    let ctx_multi = RuleEvalContext {
        check: &check_multi,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units,
        effects: &effects,
        targets: &[],
        outcomes: &[],
        best_effort: &be_multi,
        folds: &FoldRules::REV4,
    };
    let rules_multi = vec![
        make_rule(
            "r_t_allow_cat",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["cat"]),
            },
            RuleDecision::Allow,
        ),
        make_rule(
            "r_g_deny_cat",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["cat"]),
            },
            RuleDecision::Deny,
        ),
    ];
    let cov_multi = evaluate_rules(&ctx_multi, &rules_multi, &mut budget).unwrap();
    assert_eq!(
        cov_multi.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_g_deny_cat".into()))
    );

    // When Thread rank covers ALL units (`cat` + `ls`), Global Deny IS suppressed.
    let mut rules_full_cover = rules_multi.clone();
    rules_full_cover.push(make_rule(
        "r_t_allow_ls",
        RuleScope::Thread {
            thread_id: tid.clone(),
        },
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["ls"]),
        },
        RuleDecision::Allow,
    ));
    let cov_full = evaluate_rules(&ctx_multi, &rules_full_cover, &mut budget).unwrap();
    assert!(cov_full.deny.is_none());
    assert_eq!(
        cov_full.per_unit[0].allow_rule,
        Some(RuleId("r_t_allow_cat".into()))
    );
    assert_eq!(
        cov_full.per_unit[1].allow_rule,
        Some(RuleId("r_t_allow_ls".into()))
    );

    // Partial parse: unit 0 (`cat a`) covered by Thread Allow, best-effort also has `rm "$X"`,
    // Global Deny `rm` MUST still fire!
    let be_partial = BestEffortRecord {
        raw_texts: vec!["cat a; rm \"$X\"".into()],
        units: vec![be_unit(0, &["cat", "a"]), be_unit(1, &["rm", "$X"])],
    };
    let ctx_partial = RuleEvalContext {
        check: &check_multi,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units[..1],
        effects: &effects[..1],
        targets: &[],
        outcomes: &[],
        best_effort: &be_partial,
        folds: &FoldRules::REV4,
    };
    let rules_partial = vec![
        rules_multi[0].clone(),
        make_rule(
            "r_g_deny_rm",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["rm"]),
            },
            RuleDecision::Deny,
        ),
    ];
    let cov_partial = evaluate_rules(&ctx_partial, &rules_partial, &mut budget).unwrap();
    assert_eq!(
        cov_partial.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_g_deny_rm".into()))
    );
}

#[test]
fn evaluate_rules_row_14_pinned_program_and_stored_invalid_rules() {
    let tid = ThreadId("t1".into());
    let mut budget = ComputeBudget::with_default_limits();

    // §10 row 14: `python build.py` produces Unread (program).
    let check = cmd_check("python build.py");
    let units = vec![make_unit(0, &["python", "build.py"])];
    let effects = classify_all(&units);
    let be = BestEffortRecord::default();
    let ctx = RuleEvalContext {
        check: &check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units,
        effects: &effects,
        targets: &[],
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };

    // Stored invalid Allow prefix `["python"]` stops covering anything (§11 item 5).
    let stored_invalid = vec![make_rule(
        "r_invalid",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["python"]),
        },
        RuleDecision::Allow,
    )];
    let cov_inv = evaluate_rules(&ctx, &stored_invalid, &mut budget).unwrap();
    assert!(cov_inv.per_unit[0].allow_rule.is_none());
    assert!(!cov_inv.per_unit[0].covers_pinned_program);

    // Pinned CommandExact `["python", "build.py"]` covers `Unread (program)`.
    let exact_pin = vec![make_rule(
        "r_exact",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandExact {
            argv: argv(&["python", "build.py"]),
        },
        RuleDecision::Allow,
    )];
    let cov_exact = evaluate_rules(&ctx, &exact_pin, &mut budget).unwrap();
    assert_eq!(
        cov_exact.per_unit[0].allow_rule,
        Some(RuleId("r_exact".into()))
    );
    assert!(cov_exact.per_unit[0].covers_pinned_program);

    // Pinned CommandPrefix `["python", "build.py"]` also covers `Unread (program)`.
    let prefix_pin = vec![make_rule(
        "r_prefix",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["python", "build.py"]),
        },
        RuleDecision::Allow,
    )];
    let cov_pref = evaluate_rules(&ctx, &prefix_pin, &mut budget).unwrap();
    assert_eq!(
        cov_pref.per_unit[0].allow_rule,
        Some(RuleId("r_prefix".into()))
    );
    assert!(cov_pref.per_unit[0].covers_pinned_program);

    // Inline code (`python -c '1'`) is plain Unread: NOT covered by CommandExact, IS covered by unscoped Any.
    let check_inline = cmd_check("python -c 1");
    let units_inline = vec![make_unit(0, &["python", "-c", "1"])];
    let effects_inline = classify_all(&units_inline);
    let ctx_inline = RuleEvalContext {
        check: &check_inline,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units_inline,
        effects: &effects_inline,
        targets: &[],
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };
    let exact_inline = vec![make_rule(
        "r_exact_inline",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandExact {
            argv: argv(&["python", "-c", "1"]),
        },
        RuleDecision::Allow,
    )];
    let cov_no = evaluate_rules(&ctx_inline, &exact_inline, &mut budget).unwrap();
    assert!(cov_no.per_unit[0].allow_rule.is_none());

    let unscoped_any = vec![make_rule(
        "r_any",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::Any,
        RuleDecision::Allow,
    )];
    let cov_any = evaluate_rules(&ctx_inline, &unscoped_any, &mut budget).unwrap();
    assert_eq!(cov_any.unscoped_any_rule, Some(RuleId("r_any".into())));
    assert!(cov_any.per_unit[0].unscoped_any_allow);
}

#[test]
fn evaluate_rules_simple_command_guards_unknown_subject_and_canonical_path_scopes() {
    let tid = ThreadId("t1".into());
    let mut budget = ComputeBudget::with_default_limits();
    let be = BestEffortRecord::default();

    // Unquoted glob or non-harmless redirect blocks CommandPrefix Allow.
    let check_glob = cmd_check("cat *.rs");
    let units_glob = vec![make_unit(0, &["cat", "*.rs"])];
    let eff_glob = classify_all(&units_glob);
    let allow_cat = vec![make_rule(
        "r_cat",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["cat"]),
        },
        RuleDecision::Allow,
    )];
    let cov_glob = evaluate_rules(
        &RuleEvalContext {
            check: &check_glob,
            thread_id: &tid,
            preset: Preset::Home,
            units: &units_glob,
            effects: &eff_glob,
            targets: &[],
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &allow_cat,
        &mut budget,
    )
    .unwrap();
    assert!(cov_glob.per_unit[0].allow_rule.is_none());

    // Harmless redirect `2>/dev/null` allows CommandPrefix; `> out` blocks it.
    let mut u_ok = make_unit(0, &["cat", "a"]);
    u_ok.redirects = vec![ModelRedirect::ErrWrite {
        target: w("/dev/null"),
    }];
    let eff_ok = classify_all(&[u_ok.clone()]);
    let check_cat = cmd_check("cat a");
    let cov_ok = evaluate_rules(
        &RuleEvalContext {
            check: &check_cat,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[u_ok],
            effects: &eff_ok,
            targets: &[],
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &allow_cat,
        &mut budget,
    )
    .unwrap();
    assert_eq!(cov_ok.per_unit[0].allow_rule, Some(RuleId("r_cat".into())));

    let mut u_redir = make_unit(0, &["cat", "a"]);
    u_redir.redirects = vec![ModelRedirect::Write { target: w("out") }];
    let eff_redir = classify_all(&[u_redir.clone()]);
    let cov_redir = evaluate_rules(
        &RuleEvalContext {
            check: &check_cat,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[u_redir],
            effects: &eff_redir,
            targets: &[],
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &allow_cat,
        &mut budget,
    )
    .unwrap();
    assert!(cov_redir.per_unit[0].allow_rule.is_none());

    // ToolSubject::Unknown is never covered by Allow (even unscoped Any), but IS matched by Deny.
    let check_unk = ToolCheck {
        call_id: None,
        name: "Mystery".into(),
        kind: ToolKind::Other,
        title: None,
        input: json!({}),
        subject: ToolSubject::Unknown,
        turn: 1,
    };
    let allow_any_other = vec![make_rule(
        "r_any_other",
        RuleScope::Global,
        ToolKind::Other,
        RuleMatcher::Any,
        RuleDecision::Allow,
    )];
    let cov_unk = evaluate_rules(
        &RuleEvalContext {
            check: &check_unk,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[],
            effects: &[],
            targets: &[],
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &allow_any_other,
        &mut budget,
    )
    .unwrap();
    assert!(cov_unk.unscoped_any_rule.is_none());
    assert!(cov_unk.per_unit[0].allow_rule.is_none());

    // Canonical path scopes: require ALL targets resolved inside scope, ZERO Unread or variant_only.
    let fcheck = file_check(ToolKind::Edit, "Edit", &["/proj/src/a.rs"]);
    let path_rule = vec![make_rule(
        "r_proj",
        RuleScope::Global,
        ToolKind::Edit,
        RuleMatcher::PathGlob {
            glob: "/proj/**".into(),
        },
        RuleDecision::Allow,
    )];

    let good_targets = vec![resolved_target(
        TargetOrigin::SubjectPath { path_index: 0 },
        "/proj/src/a.rs",
    )];
    let cov_good = evaluate_rules(
        &RuleEvalContext {
            check: &fcheck,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[],
            effects: &[],
            targets: &good_targets,
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &path_rule,
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        cov_good.per_unit[0].allow_rule,
        Some(RuleId("r_proj".into()))
    );

    // An Unread target prevents Allow PathGlob from covering.
    let unread_targets = vec![
        good_targets[0].clone(),
        Target {
            origin: TargetOrigin::SubjectPath { path_index: 1 },
            access: AccessMode::Write,
            resolution: TargetResolution::Unread(UnreadReason::UnresolvablePath {
                path: b"/proj/missing".to_vec(),
            }),
            written: None,
        },
    ];
    let cov_unread = evaluate_rules(
        &RuleEvalContext {
            check: &fcheck,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[],
            effects: &[],
            targets: &unread_targets,
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &path_rule,
        &mut budget,
    )
    .unwrap();
    assert!(cov_unread.per_unit[0].allow_rule.is_none());

    // A `variant_only` target prevents Allow PathGlob from covering.
    let variant_targets = vec![resolved_target(
        TargetOrigin::ExpandedFrom {
            source: PartId::SubjectPath { path_index: 0 },
            variant_only: true,
        },
        "/proj/src/a.rs",
    )];
    let cov_var = evaluate_rules(
        &RuleEvalContext {
            check: &fcheck,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[],
            effects: &[],
            targets: &variant_targets,
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &path_rule,
        &mut budget,
    )
    .unwrap();
    assert!(cov_var.per_unit[0].allow_rule.is_none());
}

// ---------------------------------------------------------------------------
// 3. `remember_matcher` (§7, §9)
// ---------------------------------------------------------------------------

#[test]
fn remember_matcher_builds_exact_or_path_glob_and_returns_none_on_unread_or_exhausted() {
    let tid = ThreadId("t1".into());
    let be = BestEffortRecord::default();

    // Simple command with Clear outcome → CommandExact.
    let check = cmd_check("cat README.md");
    let units = vec![make_unit(0, &["cat", "README.md"])];
    let effects = classify_all(&units);
    let ctx = RuleEvalContext {
        check: &check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units,
        effects: &effects,
        targets: &[],
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };
    let clear_outcome = CallOutcome {
        parts: vec![PartResult {
            part: PartId::Unit(UnitIndex(0)),
            outcome: PartOutcome::Clear,
        }],
        multiply_linked_file: false,
    };
    assert_eq!(
        remember_matcher(&ctx, &clear_outcome),
        Some(RuleMatcher::CommandExact {
            argv: argv(&["cat", "README.md"])
        })
    );

    // StoreHit is still rememberable (Ask without Unread/Exhausted).
    let store_outcome = CallOutcome {
        parts: vec![PartResult {
            part: PartId::Unit(UnitIndex(0)),
            outcome: PartOutcome::StoreHit(Hit {
                hard_deny_id: None,
                source: HitSource::Model,
                matched: HitMatch::StoreFamily { family: "npmrc" },
                part: PartId::Unit(UnitIndex(0)),
            }),
        }],
        multiply_linked_file: false,
    };
    assert!(remember_matcher(&ctx, &store_outcome).is_some());

    // Any Unread (including Program!) or Exhausted part forces `None` (§7, §9).
    for blocking in [
        PartOutcome::Unread(UnreadReason::Program {
            program_words: argv(&["python", "build.py"]),
        }),
        PartOutcome::Unread(UnreadReason::UnknownCwd),
        PartOutcome::Exhausted(ExhaustReason::ComputeBudget),
    ] {
        let out = CallOutcome {
            parts: vec![
                PartResult {
                    part: PartId::Unit(UnitIndex(0)),
                    outcome: PartOutcome::Clear,
                },
                PartResult {
                    part: PartId::WholeCall,
                    outcome: blocking,
                },
            ],
            multiply_linked_file: false,
        };
        assert_eq!(remember_matcher(&ctx, &out), None);
    }

    // Single-path file tool escapes glob metacharacters in canonical path.
    let fcheck = file_check(ToolKind::Edit, "Edit", &["/proj/a[1].rs"]);
    let targets = vec![resolved_target(
        TargetOrigin::SubjectPath { path_index: 0 },
        "/proj/a[1].rs",
    )];
    let fctx = RuleEvalContext {
        check: &fcheck,
        thread_id: &tid,
        preset: Preset::Home,
        units: &[],
        effects: &[],
        targets: &targets,
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };
    assert_eq!(
        remember_matcher(&fctx, &clear_outcome),
        Some(RuleMatcher::PathGlob {
            glob: "/proj/a\\[1\\].rs".into()
        })
    );
}

// ---------------------------------------------------------------------------
// 4. §8 Budget exhaustion in `evaluate_rules`
// ---------------------------------------------------------------------------

#[test]
fn evaluate_rules_respects_compute_and_alloc_budgets() {
    let tid = ThreadId("t1".into());
    let check = cmd_check("cat a");
    let units = vec![make_unit(0, &["cat", "a"])];
    let effects = classify_all(&units);
    let be = BestEffortRecord::default();
    let ctx = RuleEvalContext {
        check: &check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units,
        effects: &effects,
        targets: &[],
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };
    let rules = vec![make_rule(
        "r1",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["sed", "s/error/warn/g"]),
        },
        RuleDecision::Allow,
    )];

    let mut zero_compute = ComputeBudget::new(0, 1024 * 1024);
    assert_eq!(
        evaluate_rules(&ctx, &rules, &mut zero_compute)
            .unwrap_err()
            .reason,
        ExhaustReason::ComputeBudget
    );

    let mut zero_alloc = ComputeBudget::new(100_000, 0);
    assert_eq!(
        evaluate_rules(&ctx, &rules, &mut zero_alloc)
            .unwrap_err()
            .reason,
        ExhaustReason::AllocationBudget
    );
}

// ---------------------------------------------------------------------------
// 5. Round 2 regression tests (Findings 1, 2, 3, 6, 7; D-153)
// ---------------------------------------------------------------------------

#[test]
fn finding_1_p1_p2_unread_and_exhausted_outcomes_block_allow_and_preserve_deny() {
    let tid = ThreadId("t1".into());
    let mut budget = ComputeBudget::with_default_limits();

    let check = cmd_check("cat a");
    let units = vec![make_unit(0, &["cat", "a"])];
    let effects = classify_all(&units);
    let be = BestEffortRecord {
        raw_texts: vec!["cat a".into()],
        units: vec![be_unit(0, &["cat", "a"])],
    };
    let rules = vec![
        make_rule(
            "r_t_allow_cat",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["cat"]),
            },
            RuleDecision::Allow,
        ),
        make_rule(
            "r_g_deny_cat",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["cat"]),
            },
            RuleDecision::Deny,
        ),
    ];

    // 1a. Unread canonical target on `cat a` blocks Thread Allow `["cat"]` and
    //     does NOT suppress Global Deny `["cat"]`.
    let unread_targets = vec![Target {
        origin: TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 1,
        },
        access: AccessMode::ReadWrite,
        resolution: TargetResolution::Unread(UnreadReason::UnresolvablePath {
            path: b"/proj/a".to_vec(),
        }),
        written: Some(b"/proj/a".to_vec()),
    }];
    let ctx_unread_target = RuleEvalContext {
        check: &check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units,
        effects: &effects,
        targets: &unread_targets,
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };
    let cov = evaluate_rules(&ctx_unread_target, &rules, &mut budget).unwrap();
    assert!(cov.per_unit[0].allow_rule.is_none());
    assert_eq!(
        cov.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_g_deny_cat".into()))
    );

    // 1b. P1/P2 unit-level Unread in `outcomes` (e.g. `Assignment` or
    //     `AfterPossibleWriter`) blocks Thread Allow and lets Global Deny fire.
    for prior in [
        PartResult {
            part: PartId::Unit(UnitIndex(0)),
            outcome: PartOutcome::Unread(UnreadReason::Assignment),
        },
        PartResult {
            part: PartId::Operand {
                unit: UnitIndex(0),
                argv_index: 1,
            },
            outcome: PartOutcome::Unread(UnreadReason::AfterPossibleWriter {
                writer_unit: UnitIndex(0),
            }),
        },
        PartResult {
            part: PartId::WholeCall,
            outcome: PartOutcome::Unread(UnreadReason::UnknownCwd),
        },
    ] {
        let outcomes = [prior];
        let ctx_p = RuleEvalContext {
            check: &check,
            thread_id: &tid,
            preset: Preset::Home,
            units: &units,
            effects: &effects,
            targets: &[],
            outcomes: &outcomes,
            best_effort: &be,
            folds: &FoldRules::REV4,
        };
        let cov_p = evaluate_rules(&ctx_p, &rules, &mut budget).unwrap();
        assert!(cov_p.per_unit[0].allow_rule.is_none());
        assert_eq!(
            cov_p.deny.as_ref().map(|(_, id)| id),
            Some(&RuleId("r_g_deny_cat".into()))
        );
    }

    // 1c. Exhausted outcome blocks EVEN unscoped Thread Any Allow and does NOT
    //     suppress Global Deny.
    let exhausted_outcomes = [PartResult {
        part: PartId::Unit(UnitIndex(0)),
        outcome: PartOutcome::Exhausted(ExhaustReason::ComputeBudget),
    }];
    let any_vs_deny = vec![
        make_rule(
            "r_t_any",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            RuleMatcher::Any,
            RuleDecision::Allow,
        ),
        rules[1].clone(),
    ];
    let ctx_ex = RuleEvalContext {
        check: &check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units,
        effects: &effects,
        targets: &[],
        outcomes: &exhausted_outcomes,
        best_effort: &be,
        folds: &FoldRules::REV4,
    };
    let cov_ex = evaluate_rules(&ctx_ex, &any_vs_deny, &mut budget).unwrap();
    assert!(cov_ex.unscoped_any_rule.is_none());
    assert!(cov_ex.per_unit[0].allow_rule.is_none());
    assert!(!cov_ex.per_unit[0].unscoped_any_allow);
    assert_eq!(
        cov_ex.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_g_deny_cat".into()))
    );

    // 1d. Pinned `python build.py` that ALSO has a P2 Unread target cannot be
    //     covered by a pinned Allow rule (§1 step 6) and cannot suppress Deny.
    let py_check = cmd_check("python build.py");
    let py_units = vec![make_unit(0, &["python", "build.py"])];
    let py_effects = classify_all(&py_units);
    let py_be = BestEffortRecord {
        raw_texts: vec!["python build.py".into()],
        units: vec![be_unit(0, &["python", "build.py"])],
    };
    let py_rules = vec![
        make_rule(
            "r_t_py",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            RuleMatcher::CommandExact {
                argv: argv(&["python", "build.py"]),
            },
            RuleDecision::Allow,
        ),
        make_rule(
            "r_g_deny_py",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["python"]),
            },
            RuleDecision::Deny,
        ),
    ];
    let ctx_py_bad = RuleEvalContext {
        check: &py_check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &py_units,
        effects: &py_effects,
        targets: &unread_targets,
        outcomes: &[],
        best_effort: &py_be,
        folds: &FoldRules::REV4,
    };
    let cov_py = evaluate_rules(&ctx_py_bad, &py_rules, &mut budget).unwrap();
    assert!(cov_py.per_unit[0].allow_rule.is_none());
    assert!(!cov_py.per_unit[0].covers_pinned_program);
    assert_eq!(
        cov_py.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_g_deny_py".into()))
    );
}

#[test]
fn finding_3_matched_deny_never_lost_to_post_match_exhaustion() {
    let tid = ThreadId("t1".into());
    let fcheck = file_check(ToolKind::Read, "Read", &["/proj/a.rs"]);
    let be = BestEffortRecord::default();
    let ctx = RuleEvalContext {
        check: &fcheck,
        thread_id: &tid,
        preset: Preset::Home,
        units: &[],
        effects: &[],
        targets: &[],
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };
    let rules = vec![make_rule(
        "r_deny_any",
        RuleScope::Global,
        ToolKind::Read,
        RuleMatcher::Any,
        RuleDecision::Deny,
    )];

    // Sweep compute units and allocation bytes across every boundary, including
    // the review's exact `ComputeBudget::new(12, 1_048_576)` case: the result
    // is monotonic — once enough budget exists to match, Deny is NEVER lost.
    let mut first_ok_units = None;
    for units in 0..=80u64 {
        let mut b = ComputeBudget::new(units, 1_048_576);
        match evaluate_rules(&ctx, &rules, &mut b) {
            Err(e) => {
                assert_eq!(e.reason, ExhaustReason::ComputeBudget);
                assert!(
                    first_ok_units.is_none(),
                    "Deny disappeared at units={units} after succeeding at {first_ok_units:?}"
                );
            }
            Ok(cov) => {
                assert_eq!(
                    cov.deny.as_ref().map(|(_, id)| id),
                    Some(&RuleId("r_deny_any".into())),
                    "units={units}"
                );
                first_ok_units.get_or_insert(units);
            }
        }
    }
    assert!(first_ok_units.is_some());

    let mut first_ok_alloc = None;
    for alloc in 0..=256usize {
        let mut b = ComputeBudget::new(100_000, alloc);
        match evaluate_rules(&ctx, &rules, &mut b) {
            Err(e) => {
                assert_eq!(e.reason, ExhaustReason::AllocationBudget);
                assert!(
                    first_ok_alloc.is_none(),
                    "Deny disappeared at alloc={alloc} after succeeding at {first_ok_alloc:?}"
                );
            }
            Ok(cov) => {
                assert_eq!(
                    cov.deny.as_ref().map(|(_, id)| id),
                    Some(&RuleId("r_deny_any".into())),
                    "alloc={alloc}"
                );
                first_ok_alloc.get_or_insert(alloc);
            }
        }
    }
    assert!(first_ok_alloc.is_some());
}

#[test]
fn finding_6_deny_path_scope_requires_every_target_and_rejects_unread() {
    let tid = ThreadId("t1".into());
    let mut budget = ComputeBudget::with_default_limits();
    let fcheck = file_check(ToolKind::Read, "Read", &["/proj/a", "/elsewhere/b"]);
    let be = BestEffortRecord::default();

    let mut scoped_deny_any = make_rule(
        "r_deny_scoped",
        RuleScope::Global,
        ToolKind::Read,
        RuleMatcher::Any,
        RuleDecision::Deny,
    );
    scoped_deny_any.path_scope = Some("/proj/**".into());
    let rules_scoped = [scoped_deny_any];

    // Mixed targets (`/proj/a` + `/elsewhere/b`): NOT every target is in `/proj/**` -> no Deny.
    let mixed = vec![
        resolved_target(TargetOrigin::SubjectPath { path_index: 0 }, "/proj/a"),
        resolved_target(TargetOrigin::SubjectPath { path_index: 1 }, "/elsewhere/b"),
    ];
    let cov_mixed = evaluate_rules(
        &RuleEvalContext {
            check: &fcheck,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[],
            effects: &[],
            targets: &mixed,
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &rules_scoped,
        &mut budget,
    )
    .unwrap();
    assert!(cov_mixed.deny.is_none());

    // One in-scope target + one Unread target -> Unread rejects path_scope -> no Deny.
    let with_unread = vec![
        resolved_target(TargetOrigin::SubjectPath { path_index: 0 }, "/proj/a"),
        Target {
            origin: TargetOrigin::SubjectPath { path_index: 1 },
            access: AccessMode::Read,
            resolution: TargetResolution::Unread(UnreadReason::UnresolvablePath {
                path: b"/proj/unreadable".to_vec(),
            }),
            written: Some(b"/proj/unreadable".to_vec()),
        },
    ];
    let cov_unread = evaluate_rules(
        &RuleEvalContext {
            check: &fcheck,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[],
            effects: &[],
            targets: &with_unread,
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &rules_scoped,
        &mut budget,
    )
    .unwrap();
    assert!(cov_unread.deny.is_none());

    // All canonical targets inside `/proj/**` -> Deny fires!
    let all_in_scope = vec![
        resolved_target(TargetOrigin::SubjectPath { path_index: 0 }, "/proj/a"),
        resolved_target(TargetOrigin::SubjectPath { path_index: 1 }, "/proj/b"),
    ];
    let cov_all = evaluate_rules(
        &RuleEvalContext {
            check: &fcheck,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[],
            effects: &[],
            targets: &all_in_scope,
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &rules_scoped,
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        cov_all.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_deny_scoped".into()))
    );

    // Existential matching stays ONLY for Deny `PathGlob` matcher itself (no path_scope).
    let deny_path_glob = [make_rule(
        "r_deny_pg",
        RuleScope::Global,
        ToolKind::Read,
        RuleMatcher::PathGlob {
            glob: "/proj/**".into(),
        },
        RuleDecision::Deny,
    )];
    let cov_pg = evaluate_rules(
        &RuleEvalContext {
            check: &fcheck,
            thread_id: &tid,
            preset: Preset::Home,
            units: &[],
            effects: &[],
            targets: &mixed,
            outcomes: &[],
            best_effort: &be,
            folds: &FoldRules::REV4,
        },
        &deny_path_glob,
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        cov_pg.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_deny_pg".into()))
    );
}

#[test]
fn finding_7_public_core_validator_matches_v4_rules() {
    // `sed z` is a safe GNU zap script accepted by core (finding 7).
    assert!(
        crate::policy::validate_rule_matcher(
            &RuleMatcher::CommandPrefix {
                argv: argv(&["sed", "z"]),
            },
            RuleDecision::Allow,
            None,
        )
        .is_ok()
    );
    // Tools removed by D-141 (`deno`, `bun`, `php`, `lua`, `pwsh`) are unknown tools -> accepted.
    for unknown_tool in ["deno", "bun", "php", "lua", "pwsh"] {
        assert!(
            crate::policy::validate_rule_matcher(
                &RuleMatcher::CommandPrefix {
                    argv: argv(&[unknown_tool]),
                },
                RuleDecision::Allow,
                None,
            )
            .is_ok(),
            "{unknown_tool} must be accepted as unknown tool"
        );
    }
    // Unpinned code runner and bad path_scope return displayable reasons.
    let err_awk = crate::policy::validate_rule_matcher(
        &RuleMatcher::CommandPrefix {
            argv: argv(&["awk"]),
        },
        RuleDecision::Allow,
        None,
    )
    .unwrap_err();
    assert!(err_awk.contains("awk"));
    let err_scope =
        crate::policy::validate_rule_matcher(&RuleMatcher::Any, RuleDecision::Allow, Some("/a/["))
            .unwrap_err();
    assert!(!err_scope.is_empty());
}

// ---------------------------------------------------------------------------
// 6. Round 3 regression tests (Wrapped pinned programs, option-value parsing)
// ---------------------------------------------------------------------------

fn make_wrapped_unit(
    idx: usize,
    wrappers: &[(WrapperKind, &[&str])],
    effective_args: &[&str],
) -> CommandUnit {
    CommandUnit {
        index: UnitIndex(idx),
        argv: effective_args.iter().map(|s| w(s)).collect(),
        removed_wrappers: wrappers
            .iter()
            .map(|(kind, words)| RemovedWrapper {
                kind: *kind,
                words: words.iter().map(|s| w(s)).collect(),
            })
            .collect(),
        redirects: Vec::new(),
        separator_before: Separator::Start,
        pipeline_id: None,
        depth: 0,
        parent_unit: None,
    }
}

fn wrapped_be_unit(idx: usize, raw: &[&str], effective: &[&str]) -> BestEffortUnit {
    BestEffortUnit {
        index: idx,
        raw_argv: argv(raw),
        effective_argv: argv(effective),
        alt_argv: Vec::new(),
        assignments: Vec::new(),
        redirects: Vec::new(),
        pipeline_id: None,
        piped: false,
        depth: 0,
        derived: false,
    }
}

#[test]
fn round3_finding_1_wrapped_pinned_programs_receive_coverage_and_suppress_lower_deny() {
    let tid = ThreadId("t1".into());
    let mut budget = ComputeBudget::with_default_limits();

    // 1. Failing case from review: `command python build.py` with established targets
    //    and Thread Allow `CommandExact ["command", "python", "build.py"]` vs
    //    lower-ranked Global Deny `CommandPrefix ["python"]`.
    let check = cmd_check("command python build.py");
    let units = vec![make_wrapped_unit(
        0,
        &[(WrapperKind::Command, &["command"])],
        &["python", "build.py"],
    )];
    let effects = classify_all(&units);
    let targets = vec![resolved_target(
        TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 1,
        },
        "/proj/build.py",
    )];
    let be = BestEffortRecord {
        raw_texts: vec!["command python build.py".into()],
        units: vec![wrapped_be_unit(
            0,
            &["command", "python", "build.py"],
            &["python", "build.py"],
        )],
    };
    let global_deny_py = make_rule(
        "r_g_deny_py",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["python"]),
        },
        RuleDecision::Deny,
    );
    let global_deny_cmd = make_rule(
        "r_g_deny_cmd",
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["command"]),
        },
        RuleDecision::Deny,
    );
    let thread_exact = make_rule(
        "r_t_exact",
        RuleScope::Thread {
            thread_id: tid.clone(),
        },
        ToolKind::Execute,
        RuleMatcher::CommandExact {
            argv: argv(&["command", "python", "build.py"]),
        },
        RuleDecision::Allow,
    );
    let ctx = RuleEvalContext {
        check: &check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units,
        effects: &effects,
        targets: &targets,
        outcomes: &[],
        best_effort: &be,
        folds: &FoldRules::REV4,
    };

    let cov_exact = evaluate_rules(
        &ctx,
        &[
            thread_exact,
            global_deny_py.clone(),
            global_deny_cmd.clone(),
        ],
        &mut budget,
    )
    .unwrap();
    assert!(cov_exact.deny.is_none());
    assert_eq!(
        cov_exact.per_unit[0].allow_rule,
        Some(RuleId("r_t_exact".into()))
    );
    assert!(cov_exact.per_unit[0].covers_pinned_program);

    // 2. Prefix branch: `command python build.py --prod` with Thread Allow
    //    `CommandPrefix ["command", "python", "build.py"]` vs Global Deny `["python"]`.
    let check_pfx = cmd_check("command python build.py --prod");
    let units_pfx = vec![make_wrapped_unit(
        0,
        &[(WrapperKind::Command, &["command"])],
        &["python", "build.py", "--prod"],
    )];
    let effects_pfx = classify_all(&units_pfx);
    let be_pfx = BestEffortRecord {
        raw_texts: vec!["command python build.py --prod".into()],
        units: vec![wrapped_be_unit(
            0,
            &["command", "python", "build.py", "--prod"],
            &["python", "build.py", "--prod"],
        )],
    };
    let thread_pfx = make_rule(
        "r_t_pfx",
        RuleScope::Thread {
            thread_id: tid.clone(),
        },
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["command", "python", "build.py"]),
        },
        RuleDecision::Allow,
    );
    let ctx_pfx = RuleEvalContext {
        check: &check_pfx,
        thread_id: &tid,
        preset: Preset::Home,
        units: &units_pfx,
        effects: &effects_pfx,
        targets: &targets,
        outcomes: &[],
        best_effort: &be_pfx,
        folds: &FoldRules::REV4,
    };
    let cov_pfx =
        evaluate_rules(&ctx_pfx, &[thread_pfx, global_deny_py.clone()], &mut budget).unwrap();
    assert!(cov_pfx.deny.is_none());
    assert_eq!(
        cov_pfx.per_unit[0].allow_rule,
        Some(RuleId("r_t_pfx".into()))
    );
    assert!(cov_pfx.per_unit[0].covers_pinned_program);

    // 3. Unpinned wrapped prefix (`["command"]` or `["command", "python"]`) does NOT
    //    name the pinned program -> no coverage, and Global Deny still wins.
    for unpinned in [&["command"][..], &["command", "python"][..]] {
        let bad_allow = make_rule(
            "r_t_unpinned",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(unpinned),
            },
            RuleDecision::Allow,
        );
        let cov_bad =
            evaluate_rules(&ctx, &[bad_allow, global_deny_py.clone()], &mut budget).unwrap();
        assert!(cov_bad.per_unit[0].allow_rule.is_none(), "{unpinned:?}");
        assert!(!cov_bad.per_unit[0].covers_pinned_program, "{unpinned:?}");
        assert_eq!(
            cov_bad.deny.as_ref().map(|(_, id)| id),
            Some(&RuleId("r_g_deny_py".into())),
            "{unpinned:?}"
        );
    }

    // 4. Original-argv matching is preserved: an unwrapped rule `["python", "build.py"]`
    //    (exact or prefix) must NOT match wrapped `command python build.py`.
    for unwrapped_matcher in [
        RuleMatcher::CommandExact {
            argv: argv(&["python", "build.py"]),
        },
        RuleMatcher::CommandPrefix {
            argv: argv(&["python", "build.py"]),
        },
    ] {
        let unwrapped_allow = make_rule(
            "r_t_unwrapped",
            RuleScope::Thread {
                thread_id: tid.clone(),
            },
            ToolKind::Execute,
            unwrapped_matcher,
            RuleDecision::Allow,
        );
        let cov_unwrapped = evaluate_rules(
            &ctx,
            &[unwrapped_allow, global_deny_py.clone()],
            &mut budget,
        )
        .unwrap();
        assert!(cov_unwrapped.per_unit[0].allow_rule.is_none());
        assert!(!cov_unwrapped.per_unit[0].covers_pinned_program);
        assert_eq!(
            cov_unwrapped.deny.as_ref().map(|(_, id)| id),
            Some(&RuleId("r_g_deny_py".into()))
        );
    }

    // 5. Stacked multi-word wrappers: `nice -n 5 command -- python build.py --flag`
    //    (wrapper offset = 5).
    let stacked_check = cmd_check("nice -n 5 command -- python build.py --flag");
    let stacked_units = vec![make_wrapped_unit(
        0,
        &[
            (WrapperKind::Nice, &["nice", "-n", "5"]),
            (WrapperKind::Command, &["command", "--"]),
        ],
        &["python", "build.py", "--flag"],
    )];
    let stacked_effects = classify_all(&stacked_units);
    let stacked_be = BestEffortRecord {
        raw_texts: vec!["nice -n 5 command -- python build.py --flag".into()],
        units: vec![wrapped_be_unit(
            0,
            &[
                "nice", "-n", "5", "command", "--", "python", "build.py", "--flag",
            ],
            &["python", "build.py", "--flag"],
        )],
    };
    let stacked_ctx = RuleEvalContext {
        check: &stacked_check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &stacked_units,
        effects: &stacked_effects,
        targets: &targets,
        outcomes: &[],
        best_effort: &stacked_be,
        folds: &FoldRules::REV4,
    };
    let stacked_allow = make_rule(
        "r_t_stacked",
        RuleScope::Thread {
            thread_id: tid.clone(),
        },
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["nice", "-n", "5", "command", "--", "python", "build.py"]),
        },
        RuleDecision::Allow,
    );
    let cov_stacked = evaluate_rules(
        &stacked_ctx,
        &[stacked_allow, global_deny_py.clone()],
        &mut budget,
    )
    .unwrap();
    assert!(cov_stacked.deny.is_none());
    assert_eq!(
        cov_stacked.per_unit[0].allow_rule,
        Some(RuleId("r_t_stacked".into()))
    );
    assert!(cov_stacked.per_unit[0].covers_pinned_program);

    let stacked_short = make_rule(
        "r_t_stacked_short",
        RuleScope::Thread {
            thread_id: tid.clone(),
        },
        ToolKind::Execute,
        RuleMatcher::CommandPrefix {
            argv: argv(&["nice", "-n", "5", "command", "--", "python"]),
        },
        RuleDecision::Allow,
    );
    let cov_stacked_short =
        evaluate_rules(&stacked_ctx, &[stacked_short, global_deny_py], &mut budget).unwrap();
    assert!(cov_stacked_short.per_unit[0].allow_rule.is_none());
    assert!(!cov_stacked_short.per_unit[0].covers_pinned_program);
    assert_eq!(
        cov_stacked_short.deny.as_ref().map(|(_, id)| id),
        Some(&RuleId("r_g_deny_py".into()))
    );

    // 6. Other pinned program kinds behind wrappers (`timeout 30 cargo test`,
    //    `env awk -f prog.awk data.txt`, `time pipx run ruff check`).
    let cargo_units = vec![make_wrapped_unit(
        0,
        &[(WrapperKind::Timeout, &["timeout", "30"])],
        &["cargo", "test", "--lib"],
    )];
    let cargo_effects = classify_all(&cargo_units);
    let cargo_check = cmd_check("timeout 30 cargo test --lib");
    let cargo_ctx = RuleEvalContext {
        check: &cargo_check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &cargo_units,
        effects: &cargo_effects,
        targets: &[],
        outcomes: &[],
        best_effort: &BestEffortRecord::default(),
        folds: &FoldRules::REV4,
    };
    let cov_cargo_ok = evaluate_rules(
        &cargo_ctx,
        &[make_rule(
            "r_cargo_ok",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["timeout", "30", "cargo", "test"]),
            },
            RuleDecision::Allow,
        )],
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        cov_cargo_ok.per_unit[0].allow_rule,
        Some(RuleId("r_cargo_ok".into()))
    );
    assert!(cov_cargo_ok.per_unit[0].covers_pinned_program);

    let cov_cargo_short = evaluate_rules(
        &cargo_ctx,
        &[make_rule(
            "r_cargo_short",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["timeout", "30", "cargo"]),
            },
            RuleDecision::Allow,
        )],
        &mut budget,
    )
    .unwrap();
    assert!(cov_cargo_short.per_unit[0].allow_rule.is_none());
    assert!(!cov_cargo_short.per_unit[0].covers_pinned_program);

    let awk_units = vec![make_wrapped_unit(
        0,
        &[(WrapperKind::Env, &["env"])],
        &["awk", "-f", "prog.awk", "data.txt"],
    )];
    let awk_effects = classify_all(&awk_units);
    let awk_check = cmd_check("env awk -f prog.awk data.txt");
    let awk_ctx = RuleEvalContext {
        check: &awk_check,
        thread_id: &tid,
        preset: Preset::Home,
        units: &awk_units,
        effects: &awk_effects,
        targets: &[],
        outcomes: &[],
        best_effort: &BestEffortRecord::default(),
        folds: &FoldRules::REV4,
    };
    let cov_awk_ok = evaluate_rules(
        &awk_ctx,
        &[make_rule(
            "r_awk_ok",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandPrefix {
                argv: argv(&["env", "awk", "-f", "prog.awk"]),
            },
            RuleDecision::Allow,
        )],
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        cov_awk_ok.per_unit[0].allow_rule,
        Some(RuleId("r_awk_ok".into()))
    );
    assert!(cov_awk_ok.per_unit[0].covers_pinned_program);
}

#[test]
fn round3_finding_2_git_option_values_preserve_exact_rule_coverage_and_validation() {
    let tid = ThreadId("t1".into());
    let mut budget = ComputeBudget::with_default_limits();

    // `git -Csrc status` and `git -C src status` both receive exact Allow rule
    // coverage (no false `-c` trigger), whereas `git -cx=y status` and
    // `git -c x=y status` remain plain Unread and are NOT covered.
    for (cmd_line, words, expect_covered) in [
        ("git -Csrc status", &["git", "-Csrc", "status"][..], true),
        (
            "git -C src status",
            &["git", "-C", "src", "status"][..],
            true,
        ),
        ("git -cx=y status", &["git", "-cx=y", "status"][..], false),
        (
            "git -c x=y status",
            &["git", "-c", "x=y", "status"][..],
            false,
        ),
    ] {
        let check = cmd_check(cmd_line);
        let units = vec![make_unit(0, words)];
        let effects = classify_all(&units);
        let be = BestEffortRecord {
            raw_texts: vec![cmd_line.into()],
            units: vec![be_unit(0, words)],
        };
        let rule = make_rule(
            "r_exact",
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::CommandExact { argv: argv(words) },
            RuleDecision::Allow,
        );
        let cov = evaluate_rules(
            &RuleEvalContext {
                check: &check,
                thread_id: &tid,
                preset: Preset::Home,
                units: &units,
                effects: &effects,
                targets: &[],
                outcomes: &[],
                best_effort: &be,
                folds: &FoldRules::REV4,
            },
            &[rule],
            &mut budget,
        )
        .unwrap();
        assert_eq!(
            cov.per_unit[0].allow_rule.is_some(),
            expect_covered,
            "{cmd_line}"
        );
    }

    // `validate_rule_matcher`: Git subcommand option values do not falsely fail
    // validation, while real `-x` triggers (attached/clustered) still fail.
    for valid in [
        &["git", "rebase", "-Xcontext"][..],
        &["git", "rebase", "-S0x1234"][..],
        &["git", "difftool", "-txxdiff"][..],
        &["git", "mergetool", "-txxdiff"][..],
        &["git", "diff", "-S", "--ext-diff"][..],
        &["git", "push", "-o", "--exec"][..],
    ] {
        assert!(
            validate_rule_matcher(
                &RuleMatcher::CommandPrefix { argv: argv(valid) },
                RuleDecision::Allow,
                &FoldRules::REV4,
            )
            .is_ok(),
            "expected valid prefix {valid:?}"
        );
    }
    for invalid in [
        &["git", "rebase", "-xid"][..],
        &["git", "rebase", "-ix", "id"][..],
        &["git", "difftool", "-xid"][..],
        &["git", "difftool", "-yx", "id"][..],
        &["git", "mergetool", "-xid"][..],
    ] {
        assert!(
            validate_rule_matcher(
                &RuleMatcher::CommandPrefix {
                    argv: argv(invalid)
                },
                RuleDecision::Allow,
                &FoldRules::REV4,
            )
            .is_err(),
            "expected invalid prefix {invalid:?}"
        );
    }
}
