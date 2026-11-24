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
    PartOutcome, PartResult, RuleEvalContext, RuleValidationError, Separator, Target, TargetOrigin,
    TargetResolution, ToolEffects, UnitIndex, UnreadReason, Word,
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

    // Multi-unit: `cat a && ls b` with Thread Allow `cat` + Global Deny `cat`:
    // unit 0 (`cat a`) is covered by Thread Allow so its best-effort twin is suppressed,
    // and unit 1 (`ls b`) does not match `cat` → NO deny!
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
    assert!(cov_multi.deny.is_none());
    assert_eq!(
        cov_multi.per_unit[0].allow_rule,
        Some(RuleId("r_t_allow_cat".into()))
    );
    assert!(cov_multi.per_unit[1].allow_rule.is_none());

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
