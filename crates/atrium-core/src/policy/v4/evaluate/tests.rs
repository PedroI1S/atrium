use std::path::Path;

use atrium_agents::ToolCheck;
use atrium_protocol::agent::{Provider, ToolKind, UnreadNotice};
use atrium_protocol::policy::{
    Preset, Rule, RuleDecision, RuleId, RuleMatcher, RuleOrigin, RuleScope, ToolSubject,
};
use atrium_protocol::thread::ThreadId;
use serde_json::json;

use crate::policy::limits;
use crate::policy::model::{
    BestEffortRecord, CallOutcome, Cap, ComputeBudget, DirEntry, ExhaustReason, FileType,
    FoldRules, FsCaps, Hit, HitMatch, HitSource, PartId, PartOutcome, PartResult, PathMetadata,
    ProtectedSet, ResolveError, RuleCoverage, UnitIndex, UnitRuleCoverage, UnreadReason,
};
use crate::policy::v4::lex::lex_best_effort;
use crate::policy::v4::paths::PathResolver;
use crate::{PolicyInput, PolicyVerdict};

use super::{
    decide, evaluate_call, evaluate_call_with_budget, run_best_effort, unread_notice_for_outcome,
};

struct VirtualTestResolver;

impl PathResolver for VirtualTestResolver {
    fn stat(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError> {
        caps.charge_stat()?;
        budget.charge(path.len().max(1) as u64)?;
        Ok(PathMetadata {
            file_type: FileType::Dir,
            nlink: 1,
            identity: None,
        })
    }

    fn lstat(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<PathMetadata, ResolveError> {
        caps.charge_stat()?;
        budget.charge(path.len().max(1) as u64)?;
        Err(ResolveError::NotFound)
    }

    fn read_link(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError> {
        caps.charge_link_read()?;
        budget.charge(path.len().max(1) as u64)?;
        Err(ResolveError::NotFound)
    }

    fn canonicalize(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<u8>, ResolveError> {
        caps.charge_canonicalize()?;
        budget.charge(path.len().max(1) as u64)?;
        Err(ResolveError::NotFound)
    }

    fn read_dir(
        &self,
        path: &[u8],
        caps: &mut FsCaps,
        budget: &mut ComputeBudget,
    ) -> Result<Vec<DirEntry>, ResolveError> {
        caps.charge_dir_open()?;
        budget.charge(path.len().max(1) as u64)?;
        Err(ResolveError::NotFound)
    }
}

fn default_protected() -> ProtectedSet {
    ProtectedSet {
        home: b"/h/u".to_vec(),
        cwd: b"/h/u".to_vec(),
        atrium_data_paths: vec![b"/h/u/.local/share/atrium".to_vec()],
        atrium_protected_paths: vec![b"/h/u/.config/atrium".to_vec()],
        hard_deny_identities: Vec::new(),
        write_only_hard_deny_identities: Vec::new(),
        identity_gaps: Vec::new(),
        store_identities: Vec::new(),
        ssh_linked_dirs: Vec::new(),
        folds: FoldRules::REV4,
    }
}

fn cmd_check(line: &str) -> ToolCheck {
    ToolCheck {
        call_id: None,
        kind: ToolKind::Execute,
        name: "Bash".into(),
        title: None,
        input: json!({ "command": line }),
        subject: ToolSubject::Command {
            line: line.into(),
            cwd: None,
        },
        turn: 1,
    }
}

fn eval_cmd(line: &str, rules: &[Rule], preset: Preset) -> crate::policy::model::CallEvaluation {
    let check = cmd_check(line);
    let data = vec![Path::new("/h/u/.local/share/atrium").to_path_buf()];
    let prot_paths = vec![Path::new("/h/u/.config/atrium").to_path_buf()];
    let thread = ThreadId("t1".into());
    let input = PolicyInput {
        check: &check,
        thread: &thread,
        provider: Provider::Claude,
        preset,
        cwd: Path::new("/h/u"),
        home: Path::new("/h/u"),
        atrium_data_paths: &data,
        atrium_protected_paths: &prot_paths,
        resolved_paths: &[],
    };
    let protected = default_protected();
    evaluate_call(&input, rules, &protected, &VirtualTestResolver)
}

#[test]
fn decide_six_step_precedence_and_settings_table() {
    let empty_cov = RuleCoverage::default();

    // Step 1: TooLarge beats HardDenyHit and Deny rule.
    let mut o_tl = CallOutcome::default();
    o_tl.parts.push(PartResult {
        part: PartId::WholeCall,
        outcome: PartOutcome::HardDenyHit(Hit {
            hard_deny_id: Some("privilege-escalation"),
            source: HitSource::BestEffort,
            matched: HitMatch::CommandWord {
                word: "sudo".into(),
            },
            part: PartId::WholeCall,
        }),
    });
    o_tl.parts.push(PartResult {
        part: PartId::WholeCall,
        outcome: PartOutcome::TooLarge(Cap::Bytes),
    });
    assert!(matches!(
        decide(&o_tl, &empty_cov, Preset::FullAccess, true),
        PolicyVerdict::Deny { ref reason, rule: None } if reason == "too large to check"
    ));

    // Step 2: HardDenyHit beats Deny rule, multiply-linked, Exhausted, and FullAccess.
    let mut o_hd = CallOutcome {
        multiply_linked_file: true,
        parts: vec![
            PartResult {
                part: PartId::WholeCall,
                outcome: PartOutcome::Exhausted(ExhaustReason::ComputeBudget),
            },
            PartResult {
                part: PartId::Unit(UnitIndex(0)),
                outcome: PartOutcome::HardDenyHit(Hit {
                    hard_deny_id: Some("credential-read"),
                    source: HitSource::Model,
                    matched: HitMatch::ProtectedPath {
                        path: b"/h/u/.ssh/id_rsa".to_vec(),
                    },
                    part: PartId::Unit(UnitIndex(0)),
                }),
            },
        ],
    };
    let deny_cov = RuleCoverage {
        deny: Some(("denied".into(), RuleId("d1".into()))),
        ..Default::default()
    };
    assert_eq!(
        decide(&o_hd, &deny_cov, Preset::FullAccess, true),
        PolicyVerdict::HardDeny {
            id: "credential-read"
        }
    );

    // Step 3: Winning Deny beats multiply-linked, Exhausted, and FullAccess.
    o_hd.parts.pop();
    assert_eq!(
        decide(&o_hd, &deny_cov, Preset::FullAccess, true),
        PolicyVerdict::Deny {
            reason: "denied".into(),
            rule: Some(RuleId("d1".into())),
        }
    );

    // Step 4: multiply_linked_file asks even in FullAccess and under unscoped Any.
    let any_cov = RuleCoverage {
        deny: None,
        per_unit: vec![UnitRuleCoverage {
            allow_rule: Some(RuleId("any".into())),
            unscoped_any_allow: true,
            covers_pinned_program: false,
        }],
        unscoped_any_rule: Some(RuleId("any".into())),
    };
    let o_ml = CallOutcome {
        parts: vec![PartResult {
            part: PartId::Unit(UnitIndex(0)),
            outcome: PartOutcome::Clear,
        }],
        multiply_linked_file: true,
    };
    assert_eq!(
        decide(&o_ml, &any_cov, Preset::FullAccess, true),
        PolicyVerdict::Ask
    );

    // Step 5: Exhausted asks everywhere (Home, prefix, Any, FullAccess).
    let o_ex = CallOutcome {
        parts: vec![PartResult {
            part: PartId::WholeCall,
            outcome: PartOutcome::Exhausted(ExhaustReason::ComputeBudget),
        }],
        multiply_linked_file: false,
    };
    assert_eq!(
        decide(&o_ex, &any_cov, Preset::FullAccess, true),
        PolicyVerdict::Ask
    );

    // Step 6: Unread (program) covered ONLY by pinned program rule (not plain prefix).
    let o_prog = CallOutcome {
        parts: vec![PartResult {
            part: PartId::Unit(UnitIndex(0)),
            outcome: PartOutcome::Unread(UnreadReason::Program {
                program_words: vec!["python".into(), "build.py".into()],
            }),
        }],
        multiply_linked_file: false,
    };
    let unpinned_cov = RuleCoverage {
        deny: None,
        per_unit: vec![UnitRuleCoverage {
            allow_rule: Some(RuleId("r1".into())),
            unscoped_any_allow: false,
            covers_pinned_program: false,
        }],
        unscoped_any_rule: None,
    };
    assert_eq!(
        decide(&o_prog, &unpinned_cov, Preset::Home, false),
        PolicyVerdict::Ask
    );
    let pinned_cov = RuleCoverage {
        deny: None,
        per_unit: vec![UnitRuleCoverage {
            allow_rule: Some(RuleId("r1".into())),
            unscoped_any_allow: false,
            covers_pinned_program: true,
        }],
        unscoped_any_rule: None,
    };
    assert_eq!(
        decide(&o_prog, &pinned_cov, Preset::Home, false),
        PolicyVerdict::Allow {
            rule: Some(RuleId("r1".into()))
        }
    );

    // Careful preset asks on Home-readonly Clear when no rule covers it.
    let o_clear = CallOutcome {
        parts: vec![PartResult {
            part: PartId::Unit(UnitIndex(0)),
            outcome: PartOutcome::Clear,
        }],
        multiply_linked_file: false,
    };
    assert_eq!(
        decide(&o_clear, &empty_cov, Preset::Careful, true),
        PolicyVerdict::Ask
    );
    assert_eq!(
        decide(&o_clear, &empty_cov, Preset::Home, true),
        PolicyVerdict::Allow { rule: None }
    );
}

#[test]
fn d150_too_large_beats_exhausted_even_with_zero_budget() {
    let huge = "a".repeat(limits::MAX_BYTES + 1);
    let check = cmd_check(&huge);
    let data = vec![];
    let prot_paths = vec![];
    let thread = ThreadId("t1".into());
    let input = PolicyInput {
        check: &check,
        thread: &thread,
        provider: Provider::Claude,
        preset: Preset::FullAccess,
        cwd: Path::new("/h/u"),
        home: Path::new("/h/u"),
        atrium_data_paths: &data,
        atrium_protected_paths: &prot_paths,
        resolved_paths: &[],
    };
    let protected = default_protected();
    let mut zero_budget = ComputeBudget::new(0, 0);
    let ev = evaluate_call_with_budget(
        &input,
        &[],
        &protected,
        &VirtualTestResolver,
        &mut zero_budget,
    );
    assert!(matches!(
        ev.verdict,
        PolicyVerdict::Deny { ref reason, rule: None } if reason == "too large to check"
    ));
}

#[test]
fn exhaustion_asks_everywhere_and_never_creates_a_hit() {
    let check = cmd_check("cat README.md");
    let data = vec![];
    let prot_paths = vec![];
    let thread = ThreadId("t1".into());
    let input = PolicyInput {
        check: &check,
        thread: &thread,
        provider: Provider::Claude,
        preset: Preset::FullAccess,
        cwd: Path::new("/h/u"),
        home: Path::new("/h/u"),
        atrium_data_paths: &data,
        atrium_protected_paths: &prot_paths,
        resolved_paths: &[],
    };
    let protected = default_protected();
    let any_rule = [Rule {
        id: RuleId("any".into()),
        scope: RuleScope::Global,
        tool_kind: ToolKind::Execute,
        tool_name: None,
        matcher: RuleMatcher::Any,
        path_scope: None,
        decision: RuleDecision::Allow,
        origin: RuleOrigin::Settings,
        created_at_ms: 0,
    }];
    for units in [0u64, 1, 5, 20, 80] {
        let mut b = ComputeBudget::new(units, 1024);
        let ev =
            evaluate_call_with_budget(&input, &any_rule, &protected, &VirtualTestResolver, &mut b);
        assert!(
            !ev.outcome.parts.iter().any(|p| matches!(
                p.outcome,
                PartOutcome::HardDenyHit(_) | PartOutcome::StoreHit(_)
            )),
            "exhaustion at {units} created a hit: {:?}",
            ev.outcome.parts
        );
        if ev
            .outcome
            .parts
            .iter()
            .any(|p| matches!(p.outcome, PartOutcome::Exhausted(_)))
        {
            assert_eq!(ev.verdict, PolicyVerdict::Ask, "units={units}");
            assert_eq!(
                unread_notice_for_outcome(&ev.outcome),
                Some(UnreadNotice::Exhausted)
            );
            assert!(ev.remember_matcher.is_none());
        }
    }
}

#[test]
fn home_readonly_reads_ssh_config_and_atrium_protected_paths_and_grep_pattern_relaxed() {
    assert_eq!(
        eval_cmd("cat ~/.ssh/config", &[], Preset::Home).verdict,
        PolicyVerdict::Allow { rule: None }
    );
    assert_eq!(
        eval_cmd("ls ~/.ssh", &[], Preset::Home).verdict,
        PolicyVerdict::Allow { rule: None }
    );
    assert_eq!(
        eval_cmd("cat ~/.config/atrium/config.toml", &[], Preset::Home).verdict,
        PolicyVerdict::Allow { rule: None }
    );
    assert_eq!(
        eval_cmd("rm ~/.config/atrium/config.toml", &[], Preset::Home).verdict,
        PolicyVerdict::HardDeny { id: "atrium-self" }
    );
    // §11 item 9: `grep .npmrc README.md` stops being blocked!
    assert_eq!(
        eval_cmd("grep .npmrc README.md", &[], Preset::Home).verdict,
        PolicyVerdict::Allow { rule: None }
    );
    // Modeled `sh -c` expands inner unit without bogus `UnsupportedNestedShellForm`.
    let sh_ev = eval_cmd("sh -c 'cat README.md'", &[], Preset::Home);
    assert_eq!(sh_ev.verdict, PolicyVerdict::Ask);
    assert_eq!(unread_notice_for_outcome(&sh_ev.outcome), None);
    assert_eq!(
        eval_cmd("sh -c 'cat ~/.ssh/id_rsa'", &[], Preset::Home).verdict,
        PolicyVerdict::HardDeny {
            id: "credential-read"
        }
    );
}

#[test]
fn best_effort_direct_catches_folded_sudo_and_globs() {
    let protected = default_protected();
    let mut b = ComputeBudget::with_default_limits();
    let rec: BestEffortRecord = lex_best_effort(&["SUDO id"], &mut b).unwrap();
    let hits = run_best_effort(&rec, &protected, &mut b).unwrap();
    assert!(hits.iter().any(|r| matches!(
        &r.outcome,
        PartOutcome::HardDenyHit(h) if h.hard_deny_id == Some("privilege-escalation")
    )));
}
