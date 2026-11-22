//! Comprehensive unit tests for P2 (`v4/paths.rs`, contract §2, §5, §8, §10, §11, §12).
//!
//! Covers:
//! 1. All contract §10 rows P2 decides (2, 8, 9, 11, 12, 15, 16, 17, 18, 21, 22, 23, 24);
//! 2. Boundary cases: absent & dangling targets, loops & `MAX_HOPS`, unreadable folders,
//!    case/fold variants on case-sensitive & case-insensitive fakes, NFC/NFD & unlisted
//!    equivalences (§11 item 13), process-dependent paths (§11 item 14), variant-only glob
//!    matches, walk caps vs compute exhaustion, hard links & `multiply_linked_file`, `cd`
//!    tracking, and intra-call writer effects across `;`, `&&`, `||`, and pipelines;
//! 3. Carried work from M1-29 (CR-17 stores), M1-30 (absent targets & probes), M1-31
//!    (S8 verdicts & shared `Sensitive`), and M1-32 (ordered resolution, dangling targets,
//!    I/O uncertainty, identity, `rg -L` verdict, non-UTF-8 bytes, literal `Read` brackets);
//! 4. Real-filesystem [`FsResolver`] tests in a temporary directory with a fake `HOME`.

use crate::policy::limits;
use crate::policy::model::{
    AccessMode, ByteProvenance, CapReached, CodeExecution, CommandUnit, ComputeBudget,
    DerivedDestinations, ExhaustReason, Exhausted, FilterDialect, FoldRules, FsCap, FsCaps,
    FsIoError, HitMatch, HitSource, LinkPolicy, ModelRedirect, OperandRole, PartId, PartOutcome,
    PartResult, PipelineId, ProtectedSet, ResolveError, SearchFilter, Separator, TargetOrigin,
    TargetResolution, ToolEffects, UnitIndex, UnreadReason, Word,
};
use crate::policy::v4::paths::resolver::fake::FakeFs;
use crate::policy::v4::paths::sensitive::{SharedSensitive, build_protected_set};
use crate::policy::v4::paths::{
    FsResolver, PathResolver, check_recursive_root, check_target, expand_shell_glob,
    resolve_command_targets, resolve_literal, resolve_subject_paths,
};

const HOME: &[u8] = b"/h/u";
const CWD: &[u8] = b"/h/u";

fn unquoted(s: &[u8]) -> Word {
    let mut w = Word::new();
    w.extend_with_provenance(s, ByteProvenance::Unquoted);
    w
}

fn single_quoted(s: &[u8]) -> Word {
    let mut w = Word::new();
    w.extend_with_provenance(s, ByteProvenance::SingleQuoted);
    w
}

fn default_protected(fs: &dyn PathResolver) -> ProtectedSet {
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();
    build_protected_set(
        HOME,
        CWD,
        vec![b"/h/u/.local/share/atrium".to_vec()],
        vec![b"/h/u/.config/atrium".to_vec()],
        FoldRules::REV4,
        fs,
        &mut caps,
        &mut budget,
    )
    .expect("build_protected_set fits default budget")
}

fn unit(
    index: usize,
    argv: Vec<Word>,
    redirects: Vec<ModelRedirect>,
    sep: Separator,
    pipe: Option<u32>,
) -> CommandUnit {
    CommandUnit {
        index: UnitIndex(index),
        argv,
        removed_wrappers: vec![],
        redirects,
        separator_before: sep,
        pipeline_id: pipe.map(PipelineId),
        depth: 0,
        parent_unit: None,
    }
}

fn reader_effects(tool: &'static str, argv_len: usize) -> ToolEffects {
    let mut roles = vec![OperandRole::Command];
    let mut access = vec![AccessMode::Read];
    for _ in 1..argv_len {
        roles.push(OperandRole::Path);
        access.push(AccessMode::Read);
    }
    ToolEffects {
        tool_name: Some(tool),
        subcommand: None,
        roles,
        operand_access: access,
        writes: false,
        derived_destinations: DerivedDestinations::None,
        reads_recursively: false,
        links: LinkPolicy::default(),
        code: CodeExecution::None,
        filter_dialect: None,
        filters: vec![],
        home_readonly_eligible: true,
        cd_target_operand: None,
        git_selects_by_name: false,
        unread: vec![],
    }
}

fn has_hard_deny(outcomes: &[PartResult], expected_id: &str) -> bool {
    outcomes.iter().any(|r| {
        matches!(
            &r.outcome,
            PartOutcome::HardDenyHit(h) if h.hard_deny_id == Some(expected_id)
        )
    })
}

fn has_any_hard_deny(outcomes: &[PartResult]) -> bool {
    outcomes
        .iter()
        .any(|r| matches!(&r.outcome, PartOutcome::HardDenyHit(_)))
}

fn has_store_hit(outcomes: &[PartResult]) -> bool {
    outcomes
        .iter()
        .any(|r| matches!(&r.outcome, PartOutcome::StoreHit(_)))
}

fn has_unread<F: Fn(&UnreadReason) -> bool>(outcomes: &[PartResult], pred: F) -> bool {
    outcomes
        .iter()
        .any(|r| matches!(&r.outcome, PartOutcome::Unread(reason) if pred(reason)))
}

fn all_clear(outcomes: &[PartResult]) -> bool {
    outcomes
        .iter()
        .all(|r| matches!(&r.outcome, PartOutcome::Clear))
}

// ---------------------------------------------------------------------------
// 1. Contract §10 rows decided by P2: 2, 8, 9, 11, 12, 15, 16, 17, 18, 21, 22, 23, 24
// ---------------------------------------------------------------------------

#[test]
fn section10_row_02_cat_ssh_id_rsa_is_hard_deny() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let u = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"~/.ssh/id_rsa")],
        vec![],
        Separator::Start,
        None,
    );
    let e = reader_effects("cat", 2);
    let res = resolve_command_targets(&[u], &[e], CWD, &prot, &fs, &mut caps, &mut budget).unwrap();
    assert!(has_hard_deny(&res.outcomes, "credential-read"));
}

#[test]
fn section10_row_08_glob_through_link_with_brackets_hits_credential() {
    // `cat proj/key*`: `proj -> store[1]`, `store[1]/key -> ../.ssh/id_rsa`
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.mkdir_all(b"/h/u/store[1]");
    fs.add_symlink(b"/h/u/proj", b"store[1]");
    fs.add_symlink(b"/h/u/store[1]/key", b"../.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let u = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"proj/key*")],
        vec![],
        Separator::Start,
        None,
    );
    let e = reader_effects("cat", 2);
    let res = resolve_command_targets(&[u], &[e], CWD, &prot, &fs, &mut caps, &mut budget).unwrap();
    assert!(
        has_hard_deny(&res.outcomes, "credential-read"),
        "expected HardDenyHit, got {:?}",
        res.outcomes
    );
}

#[test]
fn section10_row_09_quoted_glob_stays_literal_and_clear() {
    // `cat 'proj/key*'`: literal `proj/key*` exists, and `proj/key1 -> ~/.ssh/id_rsa`
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/proj/key*");
    fs.add_symlink(b"/h/u/proj/key1", b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let u = unit(
        0,
        vec![unquoted(b"cat"), single_quoted(b"proj/key*")],
        vec![],
        Separator::Start,
        None,
    );
    let e = reader_effects("cat", 2);
    let res = resolve_command_targets(&[u], &[e], CWD, &prot, &fs, &mut caps, &mut budget).unwrap();
    assert!(
        all_clear(&res.outcomes),
        "expected Clear, got {:?}",
        res.outcomes
    );
    assert!(!res.multiply_linked_file);
}

#[test]
fn section10_row_11_read_path_is_always_literal() {
    // `Read /h/u/[!é]trium/secret` -> literal path, Clear when ordinary
    let mut fs = FakeFs::new();
    fs.add_file("/h/u/[!é]trium/secret".as_bytes());
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let res = resolve_subject_paths(
        &["/h/u/[!é]trium/secret".to_string()],
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        all_clear(&res.outcomes),
        "expected Clear, got {:?}",
        res.outcomes
    );
}

#[test]
fn section10_row_12_search_azure_msal_existing_store_hit() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.azure/msal_token_cache.json");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let res = resolve_subject_paths(
        &["~/.azure".to_string()],
        Some("msal*"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        has_store_hit(&res.outcomes),
        "expected StoreHit, got {:?}",
        res.outcomes
    );
    assert!(!has_any_hard_deny(&res.outcomes));
}

#[test]
fn section10_row_15_rg_follow_no_protected_link_is_clear() {
    // `rg -L x proj`, ordinary internal link only -> Clear (§11 item 15, D-139)
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/proj/a.txt");
    fs.add_file(b"/h/u/other/b.txt");
    fs.add_symlink(b"/h/u/proj/link_other", b"/h/u/other/b.txt");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let root = resolve_literal(
        b"proj",
        CWD,
        HOME,
        TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 3,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let res = check_recursive_root(
        &root,
        LinkPolicy {
            at_root: true,
            below_root: true,
        },
        &[],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        all_clear(&res.outcomes),
        "expected Clear, got {:?}",
        res.outcomes
    );
    assert!(!res.multiply_linked_file);
}

#[test]
fn section10_row_16_s8_aws_hard_deny_config_and_home_exempt() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.aws/credentials");
    fs.add_file(b"/h/u/.config/gh/hosts.yml");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // `rg x ~/.aws` -> S8 HardDenyHit
    let aws_root = resolve_literal(
        b"~/.aws",
        CWD,
        HOME,
        TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 2,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let aws = check_recursive_root(
        &aws_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&aws.outcomes, "credential-read"));

    // `rg x ~/.config` -> D-133 exception: StoreHit, NOT HardDenyHit
    let cfg_root = resolve_literal(
        b"~/.config",
        CWD,
        HOME,
        TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 2,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let cfg = check_recursive_root(
        &cfg_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        !has_any_hard_deny(&cfg.outcomes),
        "D-133: ~/.config must not HardDeny"
    );
    assert!(
        has_store_hit(&cfg.outcomes),
        "expected StoreHit for ~/.config"
    );

    // `rg x ~` -> home root exception: StoreHit, NOT HardDenyHit
    let home_root = resolve_literal(
        b"~",
        CWD,
        HOME,
        TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 2,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let home_res = check_recursive_root(
        &home_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(!has_any_hard_deny(&home_res.outcomes));
    assert!(has_store_hit(&home_res.outcomes));
}

#[test]
fn section10_row_17_ln_target_read_hard_denies_and_taints_following_cat() {
    // `ln -s ~/.ssh/id_rsa x; cat x`
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let u0 = unit(
        0,
        vec![
            unquoted(b"ln"),
            unquoted(b"-s"),
            unquoted(b"~/.ssh/id_rsa"),
            unquoted(b"x"),
        ],
        vec![],
        Separator::Start,
        None,
    );
    let e0 = ToolEffects {
        tool_name: Some("ln"),
        subcommand: None,
        roles: vec![
            OperandRole::Command,
            OperandRole::Flag,
            OperandRole::Path,
            OperandRole::Path,
        ],
        operand_access: vec![
            AccessMode::Read,
            AccessMode::Read,
            AccessMode::Read,
            AccessMode::Write,
        ],
        writes: true,
        derived_destinations: DerivedDestinations::None,
        reads_recursively: false,
        links: LinkPolicy::default(),
        code: CodeExecution::None,
        filter_dialect: None,
        filters: vec![],
        home_readonly_eligible: false,
        cd_target_operand: None,
        git_selects_by_name: false,
        unread: vec![],
    };
    let u1 = unit(
        1,
        vec![unquoted(b"cat"), unquoted(b"x")],
        vec![],
        Separator::Semi,
        None,
    );
    let e1 = reader_effects("cat", 2);

    let res = resolve_command_targets(
        &[u0, u1],
        &[e0, e1],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res.outcomes, "credential-read"));
    assert!(has_unread(&res.outcomes, |r| matches!(
        r,
        UnreadReason::AfterPossibleWriter {
            writer_unit: UnitIndex(0)
        }
    )));
}

#[test]
fn section10_row_18_cp_r_derived_destinations_is_unread_and_protected_dst_hits() {
    // `cp -R src/. proj`, with `proj/plain -> ~/.ssh/id_rsa`
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/src/plain");
    fs.add_symlink(b"/h/u/proj/plain", b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let u = unit(
        0,
        vec![
            unquoted(b"cp"),
            unquoted(b"-R"),
            unquoted(b"src/."),
            unquoted(b"proj"),
        ],
        vec![],
        Separator::Start,
        None,
    );
    let e = ToolEffects {
        tool_name: Some("cp"),
        subcommand: None,
        roles: vec![
            OperandRole::Command,
            OperandRole::Flag,
            OperandRole::Path,
            OperandRole::DestinationRoot,
        ],
        operand_access: vec![
            AccessMode::Read,
            AccessMode::Read,
            AccessMode::Read,
            AccessMode::Write,
        ],
        writes: true,
        derived_destinations: DerivedDestinations::Always,
        reads_recursively: true,
        links: LinkPolicy::default(),
        code: CodeExecution::None,
        filter_dialect: None,
        filters: vec![],
        home_readonly_eligible: false,
        cd_target_operand: None,
        git_selects_by_name: false,
        unread: vec![],
    };
    let res = resolve_command_targets(&[u], &[e.clone()], CWD, &prot, &fs, &mut caps, &mut budget)
        .unwrap();
    assert!(!has_any_hard_deny(&res.outcomes));
    assert!(has_unread(&res.outcomes, |r| matches!(
        r,
        UnreadReason::DerivedDestinations { tool } if tool == "cp"
    )));

    // Destination root equal to a protected tree (`~/.ssh`) is STILL a HardDenyHit!
    let u_bad = unit(
        0,
        vec![
            unquoted(b"cp"),
            unquoted(b"-R"),
            unquoted(b"src/."),
            unquoted(b"~/.ssh"),
        ],
        vec![],
        Separator::Start,
        None,
    );
    let res_bad =
        resolve_command_targets(&[u_bad], &[e], CWD, &prot, &fs, &mut caps, &mut budget).unwrap();
    assert!(has_hard_deny(&res_bad.outcomes, "credential-read"));
}

#[test]
fn section10_row_21_pipeline_with_writer_taints_earlier_stage() {
    // `cat a | python mutate.py`
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/a");
    fs.add_file(b"/h/u/mutate.py");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let u0 = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"a")],
        vec![],
        Separator::Start,
        Some(1),
    );
    let e0 = reader_effects("cat", 2);
    let u1 = unit(
        1,
        vec![unquoted(b"python"), unquoted(b"mutate.py")],
        vec![],
        Separator::Pipe,
        Some(1),
    );
    let e1 = ToolEffects {
        tool_name: Some("python"),
        subcommand: None,
        roles: vec![OperandRole::Command, OperandRole::ProgramFile],
        operand_access: vec![AccessMode::Read, AccessMode::Read],
        writes: true,
        derived_destinations: DerivedDestinations::None,
        reads_recursively: false,
        links: LinkPolicy::default(),
        code: CodeExecution::Program {
            program_operand: Some(1),
            program_words: vec!["python".into(), "mutate.py".into()],
        },
        filter_dialect: None,
        filters: vec![],
        home_readonly_eligible: false,
        cd_target_operand: None,
        git_selects_by_name: false,
        unread: vec![],
    };

    let res = resolve_command_targets(
        &[u0, u1],
        &[e0, e1],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(res.outcomes.iter().any(|r| {
        r.part
            == PartId::Operand {
                unit: UnitIndex(0),
                argv_index: 1,
            }
            && r.outcome
                == PartOutcome::Unread(UnreadReason::PipelineWithPossibleWriter {
                    writer_unit: UnitIndex(1),
                })
    }));
}

#[test]
fn section10_row_22_dev_null_redirect_is_not_a_writer() {
    // `cat README.md >/dev/null; cat LICENSE`
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/README.md");
    fs.add_file(b"/h/u/LICENSE");
    fs.add_file(b"/dev/null");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let u0 = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"README.md")],
        vec![ModelRedirect::Write {
            target: unquoted(b"/dev/null"),
        }],
        Separator::Start,
        None,
    );
    let u1 = unit(
        1,
        vec![unquoted(b"cat"), unquoted(b"LICENSE")],
        vec![],
        Separator::Semi,
        None,
    );
    let res = resolve_command_targets(
        &[u0, u1],
        &[reader_effects("cat", 2), reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        all_clear(&res.outcomes),
        "expected Clear, got {:?}",
        res.outcomes
    );
}

#[test]
fn section10_row_23_glob_tool_resolves_selected_symlink_to_hard_deny() {
    // `Glob(paths=["proj"], glob="key*")`, with `proj/key -> ~/.ssh/id_rsa`
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_symlink(b"/h/u/proj/key", b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let res = resolve_subject_paths(
        &["proj".to_string()],
        Some("key*"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res.outcomes, "credential-read"));
}

#[test]
fn section10_row_24_search_azure_msal_absent_matches_store_probe() {
    // `Search(paths=["~/.azure"], glob="msal*")`, no cache file present -> StoreHit (probe)
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u/.azure");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let res = resolve_subject_paths(
        &["~/.azure".to_string()],
        Some("msal*"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(res.outcomes.iter().any(|r| matches!(
        &r.outcome,
        PartOutcome::StoreHit(h) if matches!(&h.matched, HitMatch::ProbeName { probe } if probe.contains("msal_token_cache."))
    )));
    assert!(!has_any_hard_deny(&res.outcomes));
}

// ---------------------------------------------------------------------------
// 2. Boundaries: absent/dangling, loops/MAX_HOPS, unreadable, folds, NFC/NFD,
//    process-dependent, variant-only globs, caps vs compute, hard links,
//    writers & cd
// ---------------------------------------------------------------------------

#[test]
fn boundary_absent_and_dangling_targets_and_ordered_dotdot() {
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u/.ssh/sub");
    fs.add_symlink(b"/h/u/proj/dangling_key", b"/h/u/.ssh/missing_key");
    fs.add_symlink(b"/h/u/proj/dangling_2hop", b"/h/u/proj/dangling_key");
    // M1-32 ordered resolution: `dir/../new_key` where `dir -> ~/.ssh/sub`
    fs.add_symlink(b"/h/u/proj/link_sub", b"/h/u/.ssh/sub");
    // Ordinary absent file & dangling link to ordinary absent file
    fs.add_symlink(b"/h/u/proj/dangling_ok", b"/h/u/proj/not_here");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    for raw in [
        b"~/.ssh/missing_key".as_slice(),
        b"proj/dangling_key".as_slice(),
        b"proj/dangling_2hop".as_slice(),
        b"proj/link_sub/../new_key".as_slice(),
    ] {
        let t = resolve_literal(
            raw,
            CWD,
            HOME,
            TargetOrigin::SubjectPath { path_index: 0 },
            AccessMode::Read,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        let checked = check_target(&t, &prot, &mut budget).unwrap();
        assert!(
            matches!(
                checked.outcome,
                PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("credential-read")
            ),
            "expected HardDenyHit for {:?}, got {:?}",
            String::from_utf8_lossy(raw),
            checked.outcome
        );
    }

    let ok_t = resolve_literal(
        b"proj/dangling_ok",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(matches!(
        ok_t.resolution,
        TargetResolution::Resolved {
            dangling_symlink: true,
            ..
        }
    ));
    let ok_checked = check_target(&ok_t, &prot, &mut budget).unwrap();
    assert_eq!(ok_checked.outcome, PartOutcome::Clear);
}

#[test]
fn boundary_symlink_loops_and_max_hops() {
    let mut fs = FakeFs::new();
    fs.add_symlink(b"/h/u/proj/loop_a", b"/h/u/proj/loop_b");
    fs.add_symlink(b"/h/u/proj/loop_b", b"/h/u/proj/loop_a");

    // Exact MAX_HOPS (40) chain succeeds; MAX_HOPS + 1 (41) fails -> UnresolvablePath
    fs.add_file(b"/h/u/chain/target");
    for i in 0..=limits::MAX_HOPS {
        let link = format!("/h/u/chain/l{i}");
        let dst = if i == 0 {
            "/h/u/chain/target".to_string()
        } else {
            format!("/h/u/chain/l{}", i - 1)
        };
        fs.add_symlink(link.as_bytes(), dst.as_bytes());
    }

    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let loop_t = resolve_literal(
        b"proj/loop_a",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(matches!(
        loop_t.resolution,
        TargetResolution::Unread(UnreadReason::UnresolvablePath { .. })
    ));

    // l39 = 40 hops -> succeeds
    let ok_chain = resolve_literal(
        format!("/h/u/chain/l{}", limits::MAX_HOPS - 1).as_bytes(),
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(matches!(
        ok_chain.resolution,
        TargetResolution::Resolved { ref canonical, .. } if canonical == b"/h/u/chain/target"
    ));

    // l40 = 41 hops -> Unread(UnresolvablePath)
    let over_chain = resolve_literal(
        format!("/h/u/chain/l{}", limits::MAX_HOPS).as_bytes(),
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(matches!(
        over_chain.resolution,
        TargetResolution::Unread(UnreadReason::UnresolvablePath { .. })
    ));
}

#[test]
fn boundary_unreadable_folder_is_unread_and_never_hides_hard_deny() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/proj/closed/secret");
    fs.set_dir_unreadable(b"/h/u/proj/closed", FsIoError::PermissionDenied);
    fs.add_symlink(b"/h/u/proj/link_key", b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // Literal inside unreadable dir -> Unread(UnresolvablePath)
    let res = resolve_subject_paths(
        &["proj/closed/secret".to_string()],
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_unread(&res.outcomes, |r| matches!(
        r,
        UnreadReason::UnresolvablePath { .. }
    )));

    // Unreadable operand alongside a credential link -> keeps HardDenyHit!
    let res_both = resolve_subject_paths(
        &[
            "proj/closed/secret".to_string(),
            "proj/link_key".to_string(),
        ],
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res_both.outcomes, "credential-read"));
    assert!(has_unread(&res_both.outcomes, |r| matches!(
        r,
        UnreadReason::UnresolvablePath { .. }
    )));
}

#[test]
fn boundary_case_and_fold_variants_sensitive_and_insensitive_and_nfc_nfd() {
    // 1. Case-sensitive fake: ~/.SSH/ID_RSA, ~/.ſsh/id_rsa, ~/Library/Keychains/login
    //    still match by listed folds (in addition to identity; §5).
    let mut cs_fs = FakeFs::new();
    cs_fs.add_file("/h/u/.SSH/ID_RSA".as_bytes());
    cs_fs.add_file("/h/u/.ſsh/id_rsa".as_bytes());
    cs_fs.add_file("/h/u/Library/\u{212A}eychains/login.keychain-db".as_bytes());
    // Unlisted case folds on case-sensitive fake:
    cs_fs.add_file("/h/u/.ßh/id_rsa".as_bytes());
    cs_fs.add_file("/h/u/.con\u{FB01}g/gh/hosts.yml".as_bytes());
    cs_fs.add_file("/h/u/.ssh/\u{0130}d_rsa".as_bytes());
    // NFC vs NFD on Atrium data path (`/h/u/Café/atrium` NFC vs `Cafe\u{0301}` NFD):
    cs_fs.mkdir_all("/h/u/Caf\u{00E9}/atrium".as_bytes());
    cs_fs.add_file("/h/u/Cafe\u{0301}/atrium/db".as_bytes());
    cs_fs.add_file("/h/u/proj/Cafe\u{0301}.txt".as_bytes());

    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();
    let prot_cs = build_protected_set(
        HOME,
        CWD,
        vec!["/h/u/Caf\u{00E9}/atrium".as_bytes().to_vec()],
        vec![],
        FoldRules::REV4,
        &cs_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();

    // Listed folds -> HardDenyHit (never downgraded!)
    for p in [
        "/h/u/.SSH/ID_RSA",
        "/h/u/.ſsh/id_rsa",
        "/h/u/Library/\u{212A}eychains/login.keychain-db",
    ] {
        let t = resolve_literal(
            p.as_bytes(),
            CWD,
            HOME,
            TargetOrigin::SubjectPath { path_index: 0 },
            AccessMode::Read,
            &cs_fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        let res = check_target(&t, &prot_cs, &mut budget).unwrap();
        assert!(
            matches!(res.outcome, PartOutcome::HardDenyHit(_)),
            "expected HardDenyHit for listed fold {p}, got {:?}",
            res.outcome
        );
    }

    // Unlisted equivalences (ß->ss, ﬁ->fi, NFC/NFD Café) -> Unread(UnlistedNameEquivalence)
    for p in [
        "/h/u/.ßh/id_rsa",
        "/h/u/.con\u{FB01}g/gh/hosts.yml",
        "/h/u/Cafe\u{0301}/atrium/db",
    ] {
        let t = resolve_literal(
            p.as_bytes(),
            CWD,
            HOME,
            TargetOrigin::SubjectPath { path_index: 0 },
            AccessMode::Read,
            &cs_fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        let res = check_target(&t, &prot_cs, &mut budget).unwrap();
        assert!(
            matches!(
                res.outcome,
                PartOutcome::Unread(UnreadReason::UnlistedNameEquivalence { .. })
            ),
            "expected UnlistedNameEquivalence for {p}, got {:?}",
            res.outcome
        );
    }

    // Ordinary NFD file NOT matching any protected path stays Clear.
    let ok_nfd = resolve_literal(
        "/h/u/proj/Cafe\u{0301}.txt".as_bytes(),
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &cs_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        check_target(&ok_nfd, &prot_cs, &mut budget)
            .unwrap()
            .outcome,
        PartOutcome::Clear
    );

    // 2. Case-insensitive fake (simulating APFS): `~/.SSH/ID_RSA` canonicalizes
    //    to `/h/u/.ssh/id_rsa` and shares identity with `.ssh/id_rsa` -> HardDenyHit.
    let mut ci_fs = FakeFs::case_insensitive();
    ci_fs.add_file(b"/h/u/.ssh/id_rsa");
    let prot_ci = default_protected(&ci_fs);
    let t_ci = resolve_literal(
        b"~/.SSH/ID_RSA",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &ci_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(matches!(
        t_ci.resolution,
        TargetResolution::Resolved { ref canonical, .. } if canonical == b"/h/u/.ssh/id_rsa"
    ));
    assert!(matches!(
        check_target(&t_ci, &prot_ci, &mut budget).unwrap().outcome,
        PartOutcome::HardDenyHit(_)
    ));

    // 3. NFC/NFD-insensitive fake (simulating APFS NFD normalization):
    //    looking up NFC `/h/u/Café/atrium/db` resolves to the stored inode
    //    and hits by identity (`atrium-self`).
    let mut nfd_fs = FakeFs::nfc_nfd_insensitive();
    nfd_fs.add_file("/h/u/Cafe\u{0301}/atrium/db".as_bytes());
    let prot_nfd = build_protected_set(
        HOME,
        CWD,
        vec!["/h/u/Caf\u{00E9}/atrium".as_bytes().to_vec()],
        vec![],
        FoldRules::REV4,
        &nfd_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let t_nfd = resolve_literal(
        "/h/u/Caf\u{00E9}/atrium/db".as_bytes(),
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &nfd_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(matches!(
        check_target(&t_nfd, &prot_nfd, &mut budget).unwrap().outcome,
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("atrium-self")
    ));
}

#[test]
fn boundary_process_dependent_paths_direct_and_via_symlink() {
    let mut fs = FakeFs::new();
    fs.add_symlink(b"/h/u/proj/link_proc", b"/proc/self/environ");
    fs.add_symlink(b"/h/u/proj/link_stdin", b"/dev/stdin");
    fs.add_symlink(b"/h/u/proj/link_fd", b"/dev/fd/3");
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    for p in [
        b"/proc/self/environ".as_slice(),
        b"/proc/thread-self/fd/0".as_slice(),
        b"/proc/12345/cmdline".as_slice(),
        b"/dev/fd/3".as_slice(),
        b"/dev/stdin".as_slice(),
        b"/dev/stdout".as_slice(),
        b"/dev/stderr".as_slice(),
        b"proj/link_proc".as_slice(),
        b"proj/link_stdin".as_slice(),
        b"proj/link_fd".as_slice(),
    ] {
        let t = resolve_literal(
            p,
            CWD,
            HOME,
            TargetOrigin::SubjectPath { path_index: 0 },
            AccessMode::Read,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(
            matches!(
                t.resolution,
                TargetResolution::Unread(UnreadReason::ProcessDependentPath { .. })
            ),
            "expected ProcessDependentPath for {:?}, got {:?}",
            String::from_utf8_lossy(p),
            t.resolution
        );
    }
}

#[test]
fn boundary_variant_only_glob_matches_are_unread_never_hits() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/proj/plain.txt");
    // Dotfile link to credential: matched ONLY under dotglob variant!
    fs.add_symlink(b"/h/u/proj/.hidden_key", b"/h/u/.ssh/id_rsa");
    // Deep link to credential (2 levels under `proj/**`): matched ONLY under recursive `**`!
    fs.add_symlink(b"/h/u/proj/a/b/deep_key", b"/h/u/.ssh/id_rsa");
    // 1-level link under `proj/**/one_key`: matched in ALL three dialects (sh/bash/zsh)!
    fs.add_symlink(b"/h/u/proj/a/one_key", b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // 1. `cat proj/*`: `plain.txt` is exact (Clear); `.hidden_key` is variant-only -> Unread, NEVER HardDeny!
    let u_dot = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"proj/*")],
        vec![],
        Separator::Start,
        None,
    );
    let res_dot = resolve_command_targets(
        &[u_dot],
        &[reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(!has_any_hard_deny(&res_dot.outcomes));
    assert!(has_unread(&res_dot.outcomes, |r| matches!(
        r,
        UnreadReason::VariantOnlyGlobMatch { .. }
    )));

    // 2. `cat proj/**/deep_key`: 2 levels -> variant-only -> Unread, NEVER HardDeny!
    let u_deep = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"proj/**/deep_key")],
        vec![],
        Separator::Start,
        None,
    );
    let res_deep = resolve_command_targets(
        &[u_deep],
        &[reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(!has_any_hard_deny(&res_deep.outcomes));
    assert!(has_unread(&res_deep.outcomes, |r| matches!(
        r,
        UnreadReason::VariantOnlyGlobMatch { .. }
    )));

    // 3. `cat proj/**/one_key`: 1 level -> intersection of sh/bash/zsh -> HardDenyHit!
    let u_one = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"proj/**/one_key")],
        vec![],
        Separator::Start,
        None,
    );
    let res_one = resolve_command_targets(
        &[u_one],
        &[reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res_one.outcomes, "credential-read"));

    // 4. Even calling `check_target` directly on a `variant_only: true` target never produces a hit.
    let exp = expand_shell_glob(
        &unquoted(b"proj/*"),
        CWD,
        HOME,
        PartId::Operand {
            unit: UnitIndex(0),
            argv_index: 1,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(!exp.variant_only_matches.is_empty());
    for vt in &exp.variant_only_matches {
        let checked = check_target(vt, &prot, &mut budget).unwrap();
        assert!(matches!(
            checked.outcome,
            PartOutcome::Unread(UnreadReason::VariantOnlyGlobMatch { .. })
        ));
    }
}

#[test]
fn boundary_walk_caps_are_unread_while_compute_budget_is_exhausted() {
    let mut fs = FakeFs::new();
    for i in 0..20 {
        fs.add_file(format!("/h/u/proj/f{i:02}.txt").as_bytes());
    }
    let prot = default_protected(&fs);

    // 1. Tight FsCaps -> Unread(WalkCapReached), NOT Exhausted and NOT HardDeny!
    let mut tiny_caps = FsCaps::new(5, 10, 100, 100, 100);
    let mut plenty = ComputeBudget::with_default_limits();
    let exp = expand_shell_glob(
        &unquoted(b"proj/*.txt"),
        CWD,
        HOME,
        PartId::Operand {
            unit: UnitIndex(0),
            argv_index: 1,
        },
        AccessMode::Read,
        &fs,
        &mut tiny_caps,
        &mut plenty,
    )
    .unwrap();
    assert!(
        exp.unread
            .contains(&UnreadReason::WalkCapReached(FsCap::EntriesRead))
    );
    assert!(!plenty.is_exhausted());

    // 2. Tight ComputeBudget -> Err(Exhausted), NOT Unread!
    let mut plenty_caps = FsCaps::with_default_limits();
    let mut tiny_budget = ComputeBudget::new(5, 100_000);
    let err = expand_shell_glob(
        &unquoted(b"proj/*.txt"),
        CWD,
        HOME,
        PartId::Operand {
            unit: UnitIndex(0),
            argv_index: 1,
        },
        AccessMode::Read,
        &fs,
        &mut plenty_caps,
        &mut tiny_budget,
    )
    .unwrap_err();
    assert_eq!(err.reason, ExhaustReason::ComputeBudget);

    // 3. Followed-link walk reaching FsCaps -> Unread(WalkCapReached), never HardDeny (M1-32 `rg -L` fix)
    let mut cap2 = FsCaps::new(5, 10, 100, 100, 100);
    let mut b2 = ComputeBudget::with_default_limits();
    let root = resolve_literal(
        b"proj",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut cap2,
        &mut b2,
    )
    .unwrap();
    let rec = check_recursive_root(
        &root,
        LinkPolicy {
            at_root: true,
            below_root: true,
        },
        &[],
        &prot,
        &fs,
        &mut cap2,
        &mut b2,
    )
    .unwrap();
    assert!(!has_any_hard_deny(&rec.outcomes));
    assert!(has_unread(&rec.outcomes, |r| matches!(
        r,
        UnreadReason::WalkCapReached(FsCap::EntriesRead)
    )));
}

#[test]
fn boundary_hard_links_multiply_linked_flag_and_skip_dirs() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/other/ordinary.txt");
    fs.add_hard_link(b"/h/u/other/ordinary.txt", b"/h/u/proj/hl_ordinary.txt");
    // Hard link inside `node_modules` and `.git` is skipped by the bounded hard-link walk!
    fs.add_hard_link(
        b"/h/u/other/ordinary.txt",
        b"/h/u/clean_proj/node_modules/pkg.js",
    );
    fs.add_hard_link(b"/h/u/other/ordinary.txt", b"/h/u/clean_proj/.git/obj");
    fs.add_file(b"/h/u/clean_proj/main.rs");
    // Hard link to `~/.ssh/id_rsa` -> caught by identity!
    fs.add_hard_link(b"/h/u/.ssh/id_rsa", b"/h/u/sneaky/alias_key");

    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // Direct hard link to ordinary file -> Clear outcome, multiply_linked_file == true
    let direct = resolve_subject_paths(
        &["proj/hl_ordinary.txt".to_string()],
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(all_clear(&direct.outcomes));
    assert!(direct.multiply_linked_file);

    // Recursive read over `proj` finds `hl_ordinary.txt` -> multiply_linked_file == true
    let rec_proj = resolve_subject_paths(
        &["proj".to_string()],
        None,
        true,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(all_clear(&rec_proj.outcomes));
    assert!(rec_proj.multiply_linked_file);

    // Recursive read over `clean_proj` skips `node_modules` and `.git` -> multiply_linked_file == false
    let rec_clean = resolve_subject_paths(
        &["clean_proj".to_string()],
        None,
        true,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(all_clear(&rec_clean.outcomes));
    assert!(!rec_clean.multiply_linked_file);

    // Hard link to credential -> HardDenyHit by identity (both direct and recursive)
    let sneaky_direct = resolve_subject_paths(
        &["sneaky/alias_key".to_string()],
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&sneaky_direct.outcomes, "credential-read"));

    let sneaky_rec = resolve_subject_paths(
        &["sneaky".to_string()],
        None,
        true,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&sneaky_rec.outcomes, "credential-read"));
}

#[test]
fn boundary_writer_effects_and_mv_derived_destinations_and_cd() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/src_file");
    fs.mkdir_all(b"/h/u/src_dir");
    fs.add_file(b"/h/u/src_dir/a");
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/sub/file.txt");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // 1. Writer via output redirect (`cat sub/file.txt > out.txt && cat sub/file.txt`)
    let u0 = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"sub/file.txt")],
        vec![ModelRedirect::Write {
            target: unquoted(b"out.txt"),
        }],
        Separator::Start,
        None,
    );
    let u1 = unit(
        1,
        vec![unquoted(b"cat"), unquoted(b"sub/file.txt")],
        vec![],
        Separator::And,
        None,
    );
    let res = resolve_command_targets(
        &[u0, u1],
        &[reader_effects("cat", 2), reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_unread(&res.outcomes, |r| matches!(
        r,
        UnreadReason::AfterPossibleWriter {
            writer_unit: UnitIndex(0)
        }
    )));

    // 2. `mv src_file dst` (file source -> NOT DerivedDestinations) vs `mv src_dir dst` (dir source -> DerivedDestinations!)
    let mv_effects = ToolEffects {
        tool_name: Some("mv"),
        subcommand: None,
        roles: vec![
            OperandRole::Command,
            OperandRole::Path,
            OperandRole::DestinationRoot,
        ],
        operand_access: vec![AccessMode::Read, AccessMode::Read, AccessMode::Write],
        writes: true,
        derived_destinations: DerivedDestinations::WhenSourceIsDir,
        reads_recursively: false,
        links: LinkPolicy::default(),
        code: CodeExecution::None,
        filter_dialect: None,
        filters: vec![],
        home_readonly_eligible: false,
        cd_target_operand: None,
        git_selects_by_name: false,
        unread: vec![],
    };
    let mv_file = unit(
        0,
        vec![unquoted(b"mv"), unquoted(b"src_file"), unquoted(b"dst")],
        vec![],
        Separator::Start,
        None,
    );
    let res_mv_file = resolve_command_targets(
        &[mv_file],
        &[mv_effects.clone()],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(all_clear(&res_mv_file.outcomes));

    let mv_dir = unit(
        0,
        vec![unquoted(b"mv"), unquoted(b"src_dir"), unquoted(b"dst")],
        vec![],
        Separator::Start,
        None,
    );
    let res_mv_dir = resolve_command_targets(
        &[mv_dir],
        &[mv_effects],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_unread(&res_mv_dir.outcomes, |r| matches!(
        r,
        UnreadReason::DerivedDestinations { tool } if tool == "mv"
    )));

    // 3. `cd ~/.ssh; cat id_rsa` -> literal cd moves cwd -> HardDenyHit!
    let cd_ssh = unit(
        0,
        vec![unquoted(b"cd"), unquoted(b"~/.ssh")],
        vec![],
        Separator::Start,
        None,
    );
    let mut cd_eff = reader_effects("cd", 2);
    cd_eff.cd_target_operand = Some(1);
    let cat_key = unit(
        1,
        vec![unquoted(b"cat"), unquoted(b"id_rsa")],
        vec![],
        Separator::Semi,
        None,
    );
    let res_cd = resolve_command_targets(
        &[cd_ssh, cat_key],
        &[cd_eff.clone(), reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res_cd.outcomes, "credential-read"));

    // 4. `cd -; cat sub/file.txt` -> unknown cwd -> Unread(UnknownCwd), while
    //    `cd -; cat ~/.ssh/id_rsa` STILL HardDenies `~/.ssh/id_rsa`!
    let cd_dash = unit(
        0,
        vec![unquoted(b"cd"), unquoted(b"-")],
        vec![],
        Separator::Start,
        None,
    );
    let cat_rel = unit(
        1,
        vec![unquoted(b"cat"), unquoted(b"sub/file.txt")],
        vec![],
        Separator::Semi,
        None,
    );
    let res_unk = resolve_command_targets(
        &[cd_dash.clone(), cat_rel],
        &[cd_eff.clone(), reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_unread(&res_unk.outcomes, |r| matches!(
        r,
        UnreadReason::UnknownCwd
    )));

    let cat_abs_secret = unit(
        1,
        vec![unquoted(b"cat"), unquoted(b"~/.ssh/id_rsa")],
        vec![],
        Separator::Semi,
        None,
    );
    let res_unk_hd = resolve_command_targets(
        &[cd_dash, cat_abs_secret],
        &[cd_eff, reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res_unk_hd.outcomes, "credential-read"));
}

// ---------------------------------------------------------------------------
// 3. Carried work from M1-29, M1-30, M1-31, M1-32
// ---------------------------------------------------------------------------

#[test]
fn carried_m1_29_cr17_four_stores_direct_symlink_hardlink_and_recursive() {
    let mut fs = FakeFs::new();
    let cr17_paths = [
        b"/h/u/.gem/credentials".as_slice(),
        b"/h/u/.terraform.d/credentials.tfrc.json".as_slice(),
        b"/h/u/.config/op/config".as_slice(),
        b"/h/u/.yarnrc.yml".as_slice(),
    ];
    for p in cr17_paths {
        fs.add_file(p);
    }
    fs.add_file(b"/h/u/.gem/ordinary.txt");
    fs.add_file(b"/h/u/.terraform.d/plugins.json");
    fs.add_symlink(b"/h/u/proj/sym_gem", b"/h/u/.gem/credentials");
    fs.add_hard_link(b"/h/u/.yarnrc.yml", b"/h/u/proj/hl_yarnrc");

    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // Direct reads & symlink & hard link -> StoreHit (never HardDeny)
    for p in [
        "~/.gem/credentials",
        "~/.terraform.d/credentials.tfrc.json",
        "~/.config/op/config",
        "~/.yarnrc.yml",
        "proj/sym_gem",
        "proj/hl_yarnrc",
    ] {
        let res = resolve_subject_paths(
            &[p.to_string()],
            None,
            false,
            AccessMode::Read,
            &prot,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(has_store_hit(&res.outcomes), "expected StoreHit for {p}");
        assert!(
            !has_any_hard_deny(&res.outcomes),
            "CR-17 must not HardDeny {p}"
        );
    }

    // Ordinary siblings stay Clear
    for p in ["~/.gem/ordinary.txt", "~/.terraform.d/plugins.json"] {
        let res = resolve_subject_paths(
            &[p.to_string()],
            None,
            false,
            AccessMode::Read,
            &prot,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(
            all_clear(&res.outcomes),
            "ordinary sibling {p} must stay Clear"
        );
    }

    // Recursive search on CR-17 parent folders -> StoreHit (not HardDeny)
    for dir in ["~/.gem", "~/.terraform.d", "~/.config/op"] {
        let res = resolve_subject_paths(
            &[dir.to_string()],
            None,
            true,
            AccessMode::Read,
            &prot,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(
            has_store_hit(&res.outcomes),
            "expected StoreHit on recursive {dir}"
        );
        assert!(!has_any_hard_deny(&res.outcomes));
    }
}

#[test]
fn carried_m1_30_absent_targets_and_search_filter_probes() {
    // Empty fake filesystem: all credential paths are absent on disk!
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u/.aws");
    fs.mkdir_all(b"/h/u/proj");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // Absent hard-deny literal targets still HardDeny (M1-30).
    for (p, access, expected_id) in [
        ("~/.gnupg/secring.gpg", AccessMode::Read, "credential-read"),
        ("~/.aws/credentials", AccessMode::Read, "credential-read"),
        ("~/.ssh/id_ed25519", AccessMode::Read, "credential-read"),
        (
            "~/.local/share/atrium/atrium.db",
            AccessMode::Read,
            "atrium-self",
        ),
        (
            "~/.config/atrium/config.toml",
            AccessMode::Write,
            "atrium-self",
        ),
    ] {
        let res = resolve_subject_paths(
            &[p.to_string()],
            None,
            false,
            access,
            &prot,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(
            has_hard_deny(&res.outcomes, expected_id),
            "absent {p} ({access:?}) should HardDeny({expected_id}), got {:?}",
            res.outcomes
        );
    }

    // Reading Atrium protected config (write-only protection) stays Clear.
    let read_prot = resolve_subject_paths(
        &["~/.config/atrium/config.toml".to_string()],
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(all_clear(&read_prot.outcomes));

    // Contract §2 / M1-30: `Search(paths=["~/.aws"], glob="credentia?s")` on empty `~/.aws` -> HardDeny (probe)
    let aws_probe = resolve_subject_paths(
        &["~/.aws".to_string()],
        Some("credentia?s"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&aws_probe.outcomes, "credential-read"));

    // M1-30 K8 probes on non-recursive `~`: `**/*.rs` Clear, `**/*.session` StoreHit, `**/*.json` HardDenyHit
    let k8_rs = resolve_subject_paths(
        &["~".to_string()],
        Some("**/*.rs"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        all_clear(&k8_rs.outcomes),
        "**/*.rs should be Clear, got {:?}",
        k8_rs.outcomes
    );

    let k8_sess = resolve_subject_paths(
        &["~".to_string()],
        Some("**/*.session"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_store_hit(&k8_sess.outcomes));
    assert!(!has_any_hard_deny(&k8_sess.outcomes));

    let k8_json = resolve_subject_paths(
        &["~".to_string()],
        Some("**/*.json"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&k8_json.outcomes, "credential-read"));
}

#[test]
fn carried_m1_31_s8_filters_atrium_outside_home_browser_and_shared_sensitive() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.aws/credentials");
    fs.add_file(b"/opt/atrium/data/atrium.db");
    fs.add_file(b"/h/u/.config/google-chrome/Default/Preferences");
    fs.add_file(b"/h/u/.config/google-chrome/Default/Cookies");

    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();
    let prot = build_protected_set(
        HOME,
        CWD,
        vec![b"/opt/atrium/data".to_vec()],
        vec![],
        FoldRules::REV4,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();

    // 1. S8 blocks `~/.aws` EVEN WITH a non-matching filter (`rg -g '*.txt' x ~/.aws`)
    let aws_root = resolve_literal(
        b"~/.aws",
        CWD,
        HOME,
        TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 3,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let filter_txt = SearchFilter {
        dialect: FilterDialect::RgGlob,
        pattern: unquoted(b"*.txt"),
        case_insensitive: false,
        argv_index: Some(1),
    };
    let aws_filtered = check_recursive_root(
        &aws_root,
        LinkPolicy::default(),
        &[filter_txt],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&aws_filtered.outcomes, "credential-read"));

    // 2. S8 extends to ancestors of Atrium data paths OUTSIDE home (`/opt` -> `/opt/atrium/data`)
    let opt_root = resolve_literal(
        b"/opt",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let opt_res = check_recursive_root(
        &opt_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&opt_res.outcomes, "atrium-self"));

    // 3. M1-31 review1 finding 4: ordinary browser file `Default/Preferences` is Clear,
    //    while profile folder `Default` (containing `Cookies`) is S8 HardDeny!
    let prefs_res = resolve_subject_paths(
        &["~/.config/google-chrome/Default/Preferences".to_string()],
        None,
        true,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        all_clear(&prefs_res.outcomes),
        "Preferences should be Clear"
    );

    let profile_res = resolve_subject_paths(
        &["~/.config/google-chrome/Default".to_string()],
        None,
        true,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&profile_res.outcomes, "credential-read"));

    // 4. M1-31 review1 finding 1: `SharedSensitive` cache keyed strictly by value on folded strings
    let s1 = SharedSensitive::for_protected(&prot);
    let mut prot_same_folded = prot.clone();
    prot_same_folded.home = b"/H/U/".to_vec();
    let s2 = SharedSensitive::for_protected(&prot_same_folded);
    assert!(std::rc::Rc::ptr_eq(&s1, &s2));

    let mut prot_different = prot.clone();
    prot_different.atrium_data_paths = vec![b"/other/atrium".to_vec()];
    let s3 = SharedSensitive::for_protected(&prot_different);
    assert!(!std::rc::Rc::ptr_eq(&s1, &s3));
}

#[test]
fn carried_m1_32_followed_link_under_filter_non_utf8_and_dangling_ssh() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_symlink(b"/h/u/proj/to_key", b"/h/u/.ssh/id_rsa");
    // Non-UTF-8 link name pointing to `~/.ssh/id_rsa`
    fs.add_symlink(b"/h/u/proj/bad\xfflink", b"/h/u/.ssh/id_rsa");
    // Dangling symlink inside `~/.ssh` pointing outside
    fs.add_symlink(b"/h/u/.ssh/dangling_member", b"/tmp/nonexistent_outside");

    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // 1. `rg -L` WITHOUT filter on `proj` (has `to_key -> ~/.ssh/id_rsa`) -> HardDenyHit
    let proj_root = resolve_literal(
        b"proj",
        CWD,
        HOME,
        TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 2,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let follow = LinkPolicy {
        at_root: true,
        below_root: true,
    };
    let no_filter =
        check_recursive_root(&proj_root, follow, &[], &prot, &fs, &mut caps, &mut budget).unwrap();
    assert!(has_hard_deny(&no_filter.outcomes, "credential-read"));

    // 2. `rg -L -g '*.rs'` WITH a filter that does NOT select `to_key`:
    //    followed link reaching a protected path under a filter -> Unread(FollowedLinkUnderFilter), NEVER HardDeny!
    let filter_rs = SearchFilter {
        dialect: FilterDialect::RgGlob,
        pattern: unquoted(b"*.rs"),
        case_insensitive: false,
        argv_index: Some(2),
    };
    let with_filter = check_recursive_root(
        &proj_root,
        follow,
        &[filter_rs],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        !has_any_hard_deny(&with_filter.outcomes),
        "followed link under non-matching filter must be Unread, never HardDeny: {:?}",
        with_filter.outcomes
    );
    assert!(has_unread(&with_filter.outcomes, |r| matches!(
        r,
        UnreadReason::FollowedLinkUnderFilter { .. }
    )));

    // 3. Non-UTF-8 symlink name resolves by raw bytes -> HardDenyHit
    let non_utf8 = resolve_literal(
        b"proj/bad\xfflink",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let checked_nu = check_target(&non_utf8, &prot, &mut budget).unwrap();
    assert!(matches!(
        checked_nu.outcome,
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("credential-read")
    ));

    // 4. Dangling symlink inside `~/.ssh` retains its own identity -> HardDenyHit!
    let dangling_ssh = resolve_literal(
        b"~/.ssh/dangling_member",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let checked_ds = check_target(&dangling_ssh, &prot, &mut budget).unwrap();
    assert!(matches!(
        checked_ds.outcome,
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("credential-read")
    ));
}

#[test]
fn search_selectors_and_filters_negated_multiple_and_grep_include() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/proj/a.rs");
    fs.add_symlink(b"/h/u/proj/sub/k.txt", b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // 1. Escaping selector (`../.ssh/id_*`) -> HardDenyHit + D-100 Unread
    let esc_hit = resolve_subject_paths(
        &["proj".to_string()],
        Some("../.ssh/id_*"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&esc_hit.outcomes, "credential-read"));

    // 2. Escaping selector to ordinary sibling (`../other.txt`) -> Unread (D-100), not HardDeny
    fs.add_file(b"/h/u/other.txt");
    let esc_ord = resolve_subject_paths(
        &["proj".to_string()],
        Some("../other.txt"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(!has_any_hard_deny(&esc_ord.outcomes));
    assert!(has_unread(&esc_ord.outcomes, |r| matches!(
        r,
        UnreadReason::UnsupportedFilterForm
    )));

    // 3. `grep --include '*.txt'` selects `proj/sub/k.txt -> ~/.ssh/id_rsa` -> HardDenyHit!
    let proj_root = resolve_literal(
        b"proj",
        CWD,
        HOME,
        TargetOrigin::Operand {
            unit: UnitIndex(0),
            argv_index: 3,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let grep_inc = SearchFilter {
        dialect: FilterDialect::GrepInclude,
        pattern: unquoted(b"*.txt"),
        case_insensitive: false,
        argv_index: Some(1),
    };
    let res_grep = check_recursive_root(
        &proj_root,
        LinkPolicy::default(),
        &[grep_inc.clone()],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res_grep.outcomes, "credential-read"));

    // 4. Multiple filters -> Unread(UnsupportedFilterForm)
    let res_multi = check_recursive_root(
        &proj_root,
        LinkPolicy::default(),
        &[grep_inc.clone(), grep_inc],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_unread(&res_multi.outcomes, |r| matches!(
        r,
        UnreadReason::UnsupportedFilterForm
    )));

    // 5. Negated filter (`rg -g '!*.txt'`) -> Unread(UnsupportedFilterForm)
    let neg_filter = SearchFilter {
        dialect: FilterDialect::RgGlob,
        pattern: unquoted(b"!*.txt"),
        case_insensitive: false,
        argv_index: Some(1),
    };
    let res_neg = check_recursive_root(
        &proj_root,
        LinkPolicy::default(),
        &[neg_filter],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_unread(&res_neg.outcomes, |r| matches!(
        r,
        UnreadReason::UnsupportedFilterForm
    )));
}

// ---------------------------------------------------------------------------
// 4. Real-filesystem FsResolver tests (temp folder + fake HOME; never real ~)
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn fs_resolver_real_tempdir_symlinks_dangling_hardlinks_unreadable_and_budgets() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{PermissionsExt, symlink};

    struct RestorePerm(std::path::PathBuf);
    impl Drop for RestorePerm {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    let tmp = tempfile::tempdir().expect("tempdir");
    let fake_home = std::fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
    let home_bytes = fake_home.as_os_str().as_bytes().to_vec();

    std::fs::create_dir_all(fake_home.join(".ssh")).unwrap();
    std::fs::create_dir_all(fake_home.join("proj")).unwrap();
    std::fs::create_dir_all(fake_home.join("closed")).unwrap();
    std::fs::write(fake_home.join(".ssh/id_rsa"), b"fake-key").unwrap();
    std::fs::write(fake_home.join("proj/ordinary.txt"), b"hello").unwrap();
    std::fs::write(fake_home.join("closed/inside.txt"), b"locked").unwrap();

    // Symlink to fake secret, dangling link to absent fake secret, hard link to ordinary file.
    symlink(
        fake_home.join(".ssh/id_rsa"),
        fake_home.join("proj/link_key"),
    )
    .unwrap();
    symlink(
        fake_home.join(".ssh/absent_key"),
        fake_home.join("proj/dangling_key"),
    )
    .unwrap();
    std::fs::hard_link(
        fake_home.join("proj/ordinary.txt"),
        fake_home.join("proj/hardlink.txt"),
    )
    .unwrap();

    let resolver = FsResolver::new();
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();
    let prot = build_protected_set(
        &home_bytes,
        &home_bytes,
        vec![],
        vec![],
        FoldRules::REV4,
        &resolver,
        &mut caps,
        &mut budget,
    )
    .unwrap();

    // 1. Symlink & dangling link resolve to fake `~/.ssh/...` -> HardDenyHit
    for rel in ["proj/link_key", "proj/dangling_key"] {
        let res = resolve_subject_paths(
            &[rel.to_string()],
            None,
            false,
            AccessMode::Read,
            &prot,
            &resolver,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(has_hard_deny(&res.outcomes, "credential-read"), "{rel}");
    }

    // 2. Hard link to ordinary file -> Clear + multiply_linked_file == true
    let hl = resolve_subject_paths(
        &["proj/hardlink.txt".to_string()],
        None,
        false,
        AccessMode::Read,
        &prot,
        &resolver,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(all_clear(&hl.outcomes));
    assert!(hl.multiply_linked_file);

    // 3. Absence vs I/O failure on real filesystem
    assert_eq!(
        resolver.stat(
            fake_home.join("proj/does_not_exist").as_os_str().as_bytes(),
            &mut caps,
            &mut budget
        ),
        Err(ResolveError::NotFound)
    );

    // 4. Precharging FsCaps & ComputeBudget BEFORE running syscalls
    let mut zero_stat_caps = FsCaps::new(10, 10, 0, 10, 10);
    let mut plenty = ComputeBudget::with_default_limits();
    assert_eq!(
        resolver.stat(
            fake_home.join("proj/ordinary.txt").as_os_str().as_bytes(),
            &mut zero_stat_caps,
            &mut plenty
        ),
        Err(ResolveError::CapReached(CapReached { cap: FsCap::Stats }))
    );
    assert!(!plenty.is_exhausted());

    let mut plenty_caps = FsCaps::with_default_limits();
    let mut zero_alloc = ComputeBudget::new(10_000, 0);
    assert_eq!(
        resolver.canonicalize(
            fake_home.join("proj/ordinary.txt").as_os_str().as_bytes(),
            &mut plenty_caps,
            &mut zero_alloc
        ),
        Err(ResolveError::Exhausted(Exhausted {
            reason: ExhaustReason::AllocationBudget
        }))
    );

    // 5. Unreadable folder (skip permission assertion only when running as root)
    let closed_dir = fake_home.join("closed");
    std::fs::set_permissions(&closed_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = RestorePerm(closed_dir.clone());
    if std::fs::read_dir(&closed_dir).is_err() {
        let unread_res = resolve_subject_paths(
            &["closed/inside.txt".to_string()],
            None,
            false,
            AccessMode::Read,
            &prot,
            &resolver,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(has_unread(&unread_res.outcomes, |r| matches!(
            r,
            UnreadReason::UnresolvablePath { .. }
        )));
    }
}
