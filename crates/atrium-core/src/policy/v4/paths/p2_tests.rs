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
    AccessMode, ByteProvenance, Cap, CapReached, CodeExecution, CommandUnit, ComputeBudget,
    DerivedDestinations, ExhaustReason, Exhausted, FilterDialect, FoldRules, FsCap, FsCaps,
    FsIoError, GapScope, HitMatch, HitSource, LinkPolicy, ModelRedirect, OperandRole, PartId,
    PartOutcome, PartResult, PipelineId, ProtectedSet, ResolveError, SearchFilter, Separator,
    TargetOrigin, TargetResolution, ToolEffects, UnitIndex, UnreadReason, Word,
};
use crate::policy::v4::paths::resolver::fake::FakeFs;
use crate::policy::v4::paths::sensitive::{
    SharedSensitive, build_protected_set, fold_listed_component, fold_path_listed,
};
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
    let res = resolve_command_targets(
        &[u],
        std::slice::from_ref(&e),
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
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
        PartOutcome::StoreHit(h)
            if h.source == HitSource::Model
                && matches!(&h.matched, HitMatch::ProbeName { probe } if probe.contains("msal_token_cache."))
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
    cs_fs.add_file("/h/u/.aws/credent\u{0130}als".as_bytes());
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

    // Unlisted equivalences (ß->ss, ﬁ->fi, İ->i, NFC/NFD Café) -> Unread(UnlistedNameEquivalence)
    for p in [
        "/h/u/.ßh/id_rsa",
        "/h/u/.con\u{FB01}g/gh/hosts.yml",
        "/h/u/.aws/credent\u{0130}als",
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
        std::slice::from_ref(&mv_effects),
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

    // M1-30 K8 probes on non-recursive `~`: `**/*.rs` -> StoreHit (`dev`'s
    // retained ASK_TREES directory-prefix probe matches `**` against
    // `.aws/sso/cache`, never HardDeny; §5, §11), `**/*.session` -> StoreHit,
    // `**/*.json` -> HardDenyHit.
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
        has_store_hit(&k8_rs.outcomes),
        "**/*.rs on ~ should produce StoreHit under retained ASK_TREES prefix probe, got {:?}",
        k8_rs.outcomes
    );
    assert!(
        !has_any_hard_deny(&k8_rs.outcomes),
        "**/*.rs on ~ must not HardDeny, got {:?}",
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
        std::slice::from_ref(&grep_inc),
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
    // Review 4, finding 7: discovery runs from `/`, so the build lists the
    // temporary folder's parent, whose size is the host's. The build is its
    // own phase with its own unbounded caps and budget (P5 gives the builder
    // its own caps too); the assertions below get fresh default ones.
    let prot = build_protected_set(
        &home_bytes,
        &home_bytes,
        vec![],
        vec![],
        FoldRules::REV4,
        &resolver,
        &mut FsCaps::new(usize::MAX, usize::MAX, usize::MAX, usize::MAX, usize::MAX),
        &mut ComputeBudget::new(u64::MAX, usize::MAX),
    )
    .unwrap();
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

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

#[test]
fn regression_posix_classes_dotfile_fallback_dangling_chains_ssh_pub_and_cd_nonexistent() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/.ssh/sub/key.pub");
    fs.add_file(b"/h/u/.ssh/sub/secret_key");
    fs.add_hard_link(b"/h/u/.ssh/sub/secret_key", b"/h/u/proj/hl_sub_secret");
    fs.add_symlink(b"/h/u/.ssh/k1", b"/tmp/k2");
    fs.add_symlink(b"/tmp/k2", b"/tmp/nowhere");
    fs.mkdir_all(b"/h/u/real_dir");
    fs.add_symlink(b"/h/u/proj/link_dir", b"/h/u/real_dir");
    fs.add_symlink(b"/h/u/proj/sub_ssh", b"/h/u/.ssh");
    fs.mkdir_all(b"/h/u/.aws");
    fs.add_file(b"/h/u/other/hl_target");
    fs.add_hard_link(b"/h/u/other/hl_target", b"/h/u/other/hl_second");
    fs.add_symlink(b"/h/u/follow_proj/to_hl", b"/h/u/other/hl_target");

    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // 1. POSIX bracket class `[[:alpha:]]` expands properly (does not truncate at inner `]`).
    let u_posix = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"~/.[[:alpha:]]sh/id_rsa")],
        vec![],
        Separator::Start,
        None,
    );
    let res_posix = resolve_command_targets(
        &[u_posix],
        &[reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res_posix.outcomes, "credential-read"));

    // Negated class `[!a]` inside shell glob -> Unread(NegatedGlobClass).
    let exp_neg = expand_shell_glob(
        &unquoted(b"proj/[!a]*"),
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
    assert!(exp_neg.unread.contains(&UnreadReason::NegatedGlobClass));

    // 2. No-match literal fallback even when only dotfiles exist under `~/.ssh`.
    let mut dot_only_fs = FakeFs::new();
    dot_only_fs.add_file(b"/h/u/.ssh/.hidden");
    let prot_dot = default_protected(&dot_only_fs);
    let u_dot_ssh = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"~/.ssh/*")],
        vec![],
        Separator::Start,
        None,
    );
    let res_dot_ssh = resolve_command_targets(
        &[u_dot_ssh],
        &[reader_effects("cat", 2)],
        CWD,
        &prot_dot,
        &dot_only_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&res_dot_ssh.outcomes, "credential-read"));
    assert!(has_unread(&res_dot_ssh.outcomes, |r| matches!(
        r,
        UnreadReason::VariantOnlyGlobMatch { .. }
    )));

    // 3. Valid symlink to existing dir + absent child has `dangling_symlink == false`.
    let not_dangling = resolve_literal(
        b"proj/link_dir/absent_child",
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
        not_dangling.resolution,
        TargetResolution::Resolved {
            dangling_symlink: false,
            ..
        }
    ));

    // 2-hop dangling chain originating in `~/.ssh/k1` retains `~/.ssh/k1`'s identity -> HardDenyHit.
    let chain_dangling = resolve_literal(
        b"~/.ssh/k1",
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
        chain_dangling.resolution,
        TargetResolution::Resolved {
            dangling_symlink: true,
            ..
        }
    ));
    assert!(matches!(
        check_target(&chain_dangling, &prot, &mut budget)
            .unwrap()
            .outcome,
        PartOutcome::HardDenyHit(_)
    ));

    // 4. `~/.ssh/sub/key.pub` keeps `.pub` exception (Clear), while `~/.ssh/sub/secret_key`
    //    and its hard link `proj/hl_sub_secret` are HardDenyHit.
    let pub_sub = resolve_subject_paths(
        &["~/.ssh/sub/key.pub".to_string()],
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
        all_clear(&pub_sub.outcomes),
        "~/.ssh/sub/key.pub must stay Clear, got {:?}",
        pub_sub.outcomes
    );
    let hl_sub = resolve_subject_paths(
        &["proj/hl_sub_secret".to_string()],
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&hl_sub.outcomes, "credential-read"));

    // 5. `Glob(paths=["proj"], glob="sub_ssh/*")` follows symlinked dir `proj/sub_ssh -> ~/.ssh` -> HardDenyHit.
    let glob_sym_dir = resolve_subject_paths(
        &["proj".to_string()],
        Some("sub_ssh/*"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&glob_sym_dir.outcomes, "credential-read"));

    // 6. `Search(paths=["~"], glob=".zsh_sessions/*.foo")` matches ASK_TREES prefix -> StoreHit.
    let zsh_tree = resolve_subject_paths(
        &["~".to_string()],
        Some(".zsh_sessions/*.foo"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_store_hit(&zsh_tree.outcomes));

    // 7. Escaping selector probe on absent credential: `Search(paths=["proj"], glob="../.aws/credentia?s")` -> HardDenyHit (BestEffort).
    let esc_probe = resolve_subject_paths(
        &["proj".to_string()],
        Some("../.aws/credentia?s"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(esc_probe.outcomes.iter().any(|r| matches!(
        &r.outcome,
        PartOutcome::HardDenyHit(h)
            if h.hard_deny_id == Some("credential-read") && h.source == HitSource::BestEffort
    )));

    // 8. `cd nonexistent; cat id_rsa` starting in `~/.ssh` must NOT move cwd to `/h/u/.ssh/nonexistent`!
    let cd_missing = unit(
        0,
        vec![unquoted(b"cd"), unquoted(b"nonexistent")],
        vec![],
        Separator::Start,
        None,
    );
    let mut cd_eff = reader_effects("cd", 2);
    cd_eff.cd_target_operand = Some(1);
    let cat_id = unit(
        1,
        vec![unquoted(b"cat"), unquoted(b"id_rsa")],
        vec![],
        Separator::Semi,
        None,
    );
    let res_cd_fail = resolve_command_targets(
        &[cd_missing, cat_id],
        &[cd_eff, reader_effects("cat", 2)],
        b"/h/u/.ssh",
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_unread(&res_cd_fail.outcomes, |r| matches!(
        r,
        UnreadReason::UnknownCwd
    )));

    // 9. Firmlink-prefixed process-dependent path `/System/Volumes/Data/dev/fd/3` -> ProcessDependentPath.
    let firmlink_fd = resolve_literal(
        b"/System/Volumes/Data/dev/fd/3",
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
        firmlink_fd.resolution,
        TargetResolution::Unread(UnreadReason::ProcessDependentPath { .. })
    ));

    // 10. Followed-link walk propagates `multiply_linked_file` for symlinked hard-linked regular files.
    let follow_root = resolve_literal(
        b"follow_proj",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let follow_res = check_recursive_root(
        &follow_root,
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
    assert!(follow_res.multiply_linked_file);
}

// ---------------------------------------------------------------------------
// 5. Rework Round 2 regressions (all 14 review-round1 findings & test gaps)
// ---------------------------------------------------------------------------

fn has_exhausted(outcomes: &[PartResult], expected: ExhaustReason) -> bool {
    outcomes
        .iter()
        .any(|r| matches!(r.outcome, PartOutcome::Exhausted(reason) if reason == expected))
}

#[test]
fn round2_finding01_hit_preserved_after_later_exhaustion() {
    // Round 3 (review round 2, finding 1): the previous version measured its
    // baseline on a cold `SharedSensitive` cache and then ran on a warm one,
    // so "baseline minus 25" left budget to spare and never exhausted. Each
    // collector is now measured warm (two identical runs must cost the same),
    // and then run with exactly one unit less than its full cost: the charge
    // that fails is the run's last one, which comes after the first hit, so
    // the hit must stand and an Exhausted part must be appended (§8).
    fn cost_of(run: &mut dyn FnMut(&mut ComputeBudget)) -> u64 {
        let mut warm = ComputeBudget::with_default_limits();
        run(&mut warm);
        let mut b1 = ComputeBudget::with_default_limits();
        run(&mut b1);
        let mut b2 = ComputeBudget::with_default_limits();
        run(&mut b2);
        assert_eq!(
            b1.remaining_units(),
            b2.remaining_units(),
            "warm runs must cost the same"
        );
        limits::COMPUTE_BUDGET_UNITS - b1.remaining_units()
    }

    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/ordinary");
    fs.add_file(b"/h/u/.aws/credentials");
    for i in 0..8 {
        fs.add_file(format!("/h/u/.aws/extra_{i}.txt").as_bytes());
    }
    let prot = default_protected(&fs);
    let paths = ["~/.ssh/id_rsa".to_string(), "ordinary".to_string()];

    // 1. Subject collector: `["~/.ssh/id_rsa", "ordinary"]`.
    let subj_cost = cost_of(&mut |b| {
        let mut caps = FsCaps::with_default_limits();
        resolve_subject_paths(
            &paths,
            None,
            false,
            AccessMode::Read,
            &prot,
            &fs,
            &mut caps,
            b,
        )
        .expect("full budget");
    });
    let mut caps = FsCaps::with_default_limits();
    let mut tight = ComputeBudget::new(subj_cost - 1, limits::COMPUTE_BUDGET_ALLOC_BYTES);
    let subj = resolve_subject_paths(
        &paths,
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut caps,
        &mut tight,
    )
    .expect("earlier HardDenyHit must not be discarded as Err(Exhausted)");
    assert!(
        has_hard_deny(&subj.outcomes, "credential-read"),
        "expected HardDenyHit preserved, got {:?}",
        subj.outcomes
    );
    assert!(
        has_exhausted(&subj.outcomes, ExhaustReason::ComputeBudget),
        "expected Exhausted appended, got {:?}",
        subj.outcomes
    );

    // 2. Command collector: `cat ~/.ssh/id_rsa; cat ordinary`.
    let units = [
        unit(
            0,
            vec![unquoted(b"cat"), unquoted(b"~/.ssh/id_rsa")],
            vec![],
            Separator::Start,
            None,
        ),
        unit(
            1,
            vec![unquoted(b"cat"), unquoted(b"ordinary")],
            vec![],
            Separator::Semi,
            None,
        ),
    ];
    let effects = [reader_effects("cat", 2), reader_effects("cat", 2)];
    let cmd_cost = cost_of(&mut |b| {
        let mut caps = FsCaps::with_default_limits();
        resolve_command_targets(&units, &effects, CWD, &prot, &fs, &mut caps, b)
            .expect("full budget");
    });
    let mut caps2 = FsCaps::with_default_limits();
    let mut tight_cmd = ComputeBudget::new(cmd_cost - 1, limits::COMPUTE_BUDGET_ALLOC_BYTES);
    let cmd = resolve_command_targets(
        &units,
        &effects,
        CWD,
        &prot,
        &fs,
        &mut caps2,
        &mut tight_cmd,
    )
    .expect("earlier command HardDenyHit must not be discarded on later Exhausted");
    assert!(has_hard_deny(&cmd.outcomes, "credential-read"));
    assert!(has_exhausted(&cmd.outcomes, ExhaustReason::ComputeBudget));

    // 3. Recursive collector: `~/.aws` establishes its S8 HardDenyHit before
    //    the hard-link walk, which then runs out.
    let mut caps3 = FsCaps::with_default_limits();
    let mut plenty = ComputeBudget::with_default_limits();
    let aws_root = resolve_literal(
        b"~/.aws",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps3,
        &mut plenty,
    )
    .unwrap();
    let rec_cost = cost_of(&mut |b| {
        let mut caps = FsCaps::with_default_limits();
        let full = check_recursive_root(
            &aws_root,
            LinkPolicy::default(),
            &[],
            &prot,
            &fs,
            &mut caps,
            b,
        )
        .expect("full budget");
        assert!(has_hard_deny(&full.outcomes, "credential-read"));
    });
    let mut rec_c = FsCaps::with_default_limits();
    let mut tight_rec_b = ComputeBudget::new(rec_cost - 1, limits::COMPUTE_BUDGET_ALLOC_BYTES);
    let rec = check_recursive_root(
        &aws_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut rec_c,
        &mut tight_rec_b,
    )
    .expect("S8 HardDenyHit must be preserved when the later hard-link walk exhausts");
    assert!(has_hard_deny(&rec.outcomes, "credential-read"));
    assert!(has_exhausted(&rec.outcomes, ExhaustReason::ComputeBudget));
}

#[test]
fn round2_finding02_exempt_root_identity_walk_preserves_s8_exemptions() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.aws/credentials");
    fs.add_file(b"/h/u/.config/gh/hosts.yml");
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/.config/ordinary_a");
    fs.add_hard_link(b"/h/u/.config/ordinary_a", b"/h/u/.config/ordinary_b");

    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    for exempt in [b"~/.config".as_slice(), b"~".as_slice(), b"/h".as_slice()] {
        let root = resolve_literal(
            exempt,
            CWD,
            HOME,
            TargetOrigin::SubjectPath { path_index: 0 },
            AccessMode::Read,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        let res = check_recursive_root(
            &root,
            LinkPolicy::default(),
            &[],
            &prot,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(
            !has_any_hard_deny(&res.outcomes),
            "exempt root {:?} must NOT HardDeny, got {:?}",
            String::from_utf8_lossy(exempt),
            res.outcomes
        );
        assert!(
            has_store_hit(&res.outcomes),
            "exempt root {:?} should yield StoreHit, got {:?}",
            String::from_utf8_lossy(exempt),
            res.outcomes
        );
        assert!(
            res.multiply_linked_file,
            "hard-linked ordinary file under exempt root {:?} still sets multiply_linked_file",
            String::from_utf8_lossy(exempt)
        );
    }
}

#[test]
fn round2_finding03_non_utf8_filter_matches_symlink_to_credential() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_symlink(b"/h/u/proj/key\xff", b"/h/u/.ssh/id_rsa");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // `Glob(paths=["proj"], glob="key*")` selects `proj/key\xff` -> HardDenyHit.
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
    assert!(
        has_hard_deny(&res.outcomes, "credential-read"),
        "non-UTF-8 selected alias must HardDeny, got {:?}",
        res.outcomes
    );

    // Same for `rg -g 'key*'` and `grep --include 'key*'` via `check_recursive_root`.
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
    for dialect in [FilterDialect::RgGlob, FilterDialect::GrepInclude] {
        let f = SearchFilter {
            dialect,
            pattern: unquoted(b"key*"),
            case_insensitive: false,
            argv_index: Some(1),
        };
        let rec = check_recursive_root(
            &proj_root,
            LinkPolicy::default(),
            &[f],
            &prot,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(
            has_hard_deny(&rec.outcomes, "credential-read"),
            "{dialect:?} filter must select non-UTF-8 alias, got {:?}",
            rec.outcomes
        );
    }
}

#[test]
fn round2_finding04_s8_matches_listed_fold_credential_spellings() {
    // Case-sensitive fake with ONLY uppercase `/h/u/.aws/CREDENTIALS`.
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.aws/CREDENTIALS");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let aws_root = resolve_literal(
        b"~/.aws",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();

    // Unfiltered recursive read of `~/.aws` -> S8 HardDenyHit.
    let unfilt = check_recursive_root(
        &aws_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        has_hard_deny(&unfilt.outcomes, "credential-read"),
        "expected S8 HardDenyHit for .aws/CREDENTIALS, got {:?}",
        unfilt.outcomes
    );

    // Filtered to `*.rs` -> S8 still HardDenyHit!
    let filter_rs = SearchFilter {
        dialect: FilterDialect::RgGlob,
        pattern: unquoted(b"*.rs"),
        case_insensitive: false,
        argv_index: Some(1),
    };
    let filt = check_recursive_root(
        &aws_root,
        LinkPolicy::default(),
        &[filter_rs],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        has_hard_deny(&filt.outcomes, "credential-read"),
        "filtered recursive read of ~/.aws with CREDENTIALS must HardDeny, got {:?}",
        filt.outcomes
    );

    // Hard link to `.aws/CREDENTIALS` is also recorded by `build_protected_set`.
    fs.add_hard_link(b"/h/u/.aws/CREDENTIALS", b"/h/u/proj/hl_creds");
    let prot2 = default_protected(&fs);
    let hl_res = resolve_subject_paths(
        &["proj/hl_creds".to_string()],
        None,
        false,
        AccessMode::Read,
        &prot2,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&hl_res.outcomes, "credential-read"));
}

#[test]
fn round2_finding05_browser_s8_deeper_levels_and_depth_cap_unread() {
    let mut fs = FakeFs::new();
    // 4 levels below `~/.config/google-chrome`: a/b/c/Cookies.
    fs.add_file(b"/h/u/.config/google-chrome/a/b/c/Cookies");
    // 17 levels below `~/.config/chromium` (> WALK_MAX_DEPTH = 16).
    let mut deep_path = b"/h/u/.config/chromium".to_vec();
    for i in 0..=(limits::WALK_MAX_DEPTH as usize) {
        deep_path.extend_from_slice(format!("/d{i}").as_bytes());
    }
    deep_path.extend_from_slice(b"/Cookies");
    fs.add_file(&deep_path);

    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let chrome_root = resolve_literal(
        b"~/.config/google-chrome",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let chrome_res = check_recursive_root(
        &chrome_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        has_hard_deny(&chrome_res.outcomes, "credential-read"),
        "a/b/c/Cookies under google-chrome must S8 HardDeny, got {:?}",
        chrome_res.outcomes
    );

    let chromium_root = resolve_literal(
        b"~/.config/chromium",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let chromium_res = check_recursive_root(
        &chromium_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        has_unread(&chromium_res.outcomes, |r| *r
            == UnreadReason::WalkCapReached(FsCap::WalkDepth)),
        "exceeding WALK_MAX_DEPTH in browser S8 scan must emit Unread(WalkCapReached(WalkDepth)), got {:?}",
        chromium_res.outcomes
    );
}

#[test]
fn round2_finding06_dangling_protected_entries_outside_ssh() {
    let mut fs = FakeFs::new();
    fs.add_symlink(b"/h/u/.aws/credentials", b"/tmp/missing_aws");
    fs.add_symlink(b"/h/u/.gem/credentials", b"/tmp/missing_gem");
    fs.add_symlink(b"/h/u/.local/share/atrium", b"/tmp/missing_atrium");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let aws = resolve_subject_paths(
        &["~/.aws/credentials".to_string()],
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
        has_hard_deny(&aws.outcomes, "credential-read"),
        "dangling ~/.aws/credentials must HardDeny, got {:?}",
        aws.outcomes
    );

    let gem = resolve_subject_paths(
        &["~/.gem/credentials".to_string()],
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
        has_store_hit(&gem.outcomes),
        "dangling ~/.gem/credentials must StoreHit, got {:?}",
        gem.outcomes
    );
    assert!(!has_any_hard_deny(&gem.outcomes));

    let atrium = resolve_subject_paths(
        &["~/.local/share/atrium".to_string()],
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
        has_hard_deny(&atrium.outcomes, "atrium-self"),
        "dangling Atrium data path must HardDeny(atrium-self), got {:?}",
        atrium.outcomes
    );
}

#[test]
fn round2_finding07_write_only_hard_link_aliases_in_resolver_flows() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/config");
    fs.add_file(b"/h/u/.ssh/authorized_keys");
    fs.add_file(b"/h/u/.ssh/id_rsa.pub");
    fs.add_file(b"/h/u/.config/atrium/config.toml");
    fs.add_file(b"/h/u/proj/input.txt");
    fs.add_hard_link(b"/h/u/.ssh/config", b"/h/u/proj/alias_ssh_cfg");
    fs.add_hard_link(b"/h/u/.ssh/authorized_keys", b"/h/u/proj/alias_auth");
    fs.add_hard_link(
        b"/h/u/.config/atrium/config.toml",
        b"/h/u/proj/alias_atrium",
    );

    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // 1. Writing through hard-linked aliases -> HardDenyHit!
    for (alias, expected_id) in [
        ("proj/alias_ssh_cfg", "credential-read"),
        ("proj/alias_auth", "credential-read"),
        ("proj/alias_atrium", "atrium-self"),
    ] {
        let w = resolve_subject_paths(
            &[alias.to_string()],
            None,
            false,
            AccessMode::Write,
            &prot,
            &fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        assert!(
            has_hard_deny(&w.outcomes, expected_id),
            "Write via hard-linked alias {alias} must HardDeny({expected_id}), got {:?}",
            w.outcomes
        );
    }

    // 2. Reading through those same aliases -> Clear + multiply_linked_file (NOT HardDeny!).
    for alias in ["proj/alias_ssh_cfg", "proj/alias_atrium"] {
        let r = resolve_subject_paths(
            &[alias.to_string()],
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
            all_clear(&r.outcomes),
            "Read via hard-linked write-only alias {alias} must stay Clear, got {:?}",
            r.outcomes
        );
        assert!(r.multiply_linked_file);
    }

    // 3. Command output redirect to hard-linked write-only alias -> HardDenyHit!
    let u_redir = unit(
        0,
        vec![unquoted(b"cat"), unquoted(b"proj/input.txt")],
        vec![ModelRedirect::Write {
            target: unquoted(b"proj/alias_ssh_cfg"),
        }],
        Separator::Start,
        None,
    );
    let cmd_res = resolve_command_targets(
        &[u_redir],
        &[reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(has_hard_deny(&cmd_res.outcomes, "credential-read"));

    // 4. Writing to `~/.ssh/id_rsa.pub` keeps its `.pub` exception.
    let pub_w = resolve_subject_paths(
        &["~/.ssh/id_rsa.pub".to_string()],
        None,
        false,
        AccessMode::Write,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(all_clear(&pub_w.outcomes));
}

#[test]
fn round2_finding08_canonicalization_preserves_listed_protected_spelling() {
    // Case-sensitive fake with NO lowercase `.ssh` tree; `.SSH/ID_RSA` is a
    // symlink to an ordinary file.
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/ordinary.txt");
    fs.add_symlink(b"/h/u/.SSH/ID_RSA", b"/h/u/ordinary.txt");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let t = resolve_literal(
        b"~/.SSH/ID_RSA",
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
        "~/.SSH/ID_RSA -> ordinary.txt must HardDeny, got {:?}",
        checked.outcome
    );
}

#[test]
fn round2_finding09_unicode_normalization_u01fb_and_independent_conformance() {
    // 1. Case-sensitive fake: Atrium data root containing U+01FB (`ǻ`),
    //    checked via its NFD spelling `a\u{030A}\u{0301}` -> Unread(UnlistedNameEquivalence).
    let mut cs_fs = FakeFs::new();
    cs_fs.mkdir_all("/h/u/\u{01FB}/atrium".as_bytes());
    cs_fs.add_file("/h/u/a\u{030A}\u{0301}/atrium/db".as_bytes());
    cs_fs.add_file("/h/u/\u{01FB}/atrium/db".as_bytes());
    cs_fs.add_file("/h/u/proj/a\u{030A}\u{0301}.txt".as_bytes());
    // Uncovered non-Latin precomposed character (Tibetan U+0F43 vs NFD U+0F42 U+0FB7).
    cs_fs.mkdir_all("/h/u/\u{0F43}/atrium".as_bytes());
    cs_fs.add_file("/h/u/\u{0F42}\u{0FB7}/atrium/db".as_bytes());
    // Hangul NFC (`가` U+AC00) vs NFD (`\u{1100}\u{1161}`) and CCC reordering.
    cs_fs.mkdir_all("/h/u/\u{AC00}/atrium".as_bytes());
    cs_fs.add_file("/h/u/\u{1100}\u{1161}/atrium/db".as_bytes());
    cs_fs.mkdir_all("/h/u/e\u{0327}\u{0301}/atrium".as_bytes());
    cs_fs.add_file("/h/u/e\u{0301}\u{0327}/atrium/db".as_bytes());
    // Distinct Latin-1 vowels (à vs á) stay Clear.
    cs_fs.mkdir_all("/h/u/\u{00E0}/atrium".as_bytes());
    cs_fs.add_file("/h/u/\u{00E1}/atrium/db".as_bytes());

    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();
    let prot = build_protected_set(
        HOME,
        CWD,
        vec![
            "/h/u/\u{01FB}/atrium".as_bytes().to_vec(),
            "/h/u/\u{0F43}/atrium".as_bytes().to_vec(),
            "/h/u/\u{AC00}/atrium".as_bytes().to_vec(),
            "/h/u/e\u{0327}\u{0301}/atrium".as_bytes().to_vec(),
            "/h/u/\u{00E0}/atrium".as_bytes().to_vec(),
        ],
        vec![],
        FoldRules::REV4,
        &cs_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();

    for nfd_path in [
        "/h/u/a\u{030A}\u{0301}/atrium/db",
        "/h/u/\u{0F42}\u{0FB7}/atrium/db",
        "/h/u/\u{1100}\u{1161}/atrium/db",
        "/h/u/e\u{0301}\u{0327}/atrium/db",
    ] {
        let t = resolve_literal(
            nfd_path.as_bytes(),
            CWD,
            HOME,
            TargetOrigin::SubjectPath { path_index: 0 },
            AccessMode::Read,
            &cs_fs,
            &mut caps,
            &mut budget,
        )
        .unwrap();
        let res = check_target(&t, &prot, &mut budget).unwrap();
        assert!(
            matches!(
                res.outcome,
                PartOutcome::Unread(UnreadReason::UnlistedNameEquivalence { .. })
            ),
            "expected UnlistedNameEquivalence for {nfd_path:?}, got {:?}",
            res.outcome
        );
    }

    // Exact NFC U+01FB stays HardDenyHit (never downgraded).
    let exact_u01fb = resolve_literal(
        "/h/u/\u{01FB}/atrium/db".as_bytes(),
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &cs_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(matches!(
        check_target(&exact_u01fb, &prot, &mut budget)
            .unwrap()
            .outcome,
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("atrium-self")
    ));

    // An ordinary NFD file outside every protected path stays Clear.
    let ordinary = resolve_literal(
        "/h/u/proj/a\u{030A}\u{0301}.txt".as_bytes(),
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
        check_target(&ordinary, &prot, &mut budget).unwrap().outcome,
        PartOutcome::Clear
    );

    // Round 3 (review round 2, finding 2): in this combined fixture the
    // protected set holds Tibetan U+0F43, which the exact table does not
    // decompose, so comparing it with `á` takes §11 item 13's conservative
    // fallback: Unread is the contract's verdict (no hit, nothing downgraded).
    let acute = resolve_literal(
        "/h/u/\u{00E1}/atrium/db".as_bytes(),
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &cs_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(matches!(
        check_target(&acute, &prot, &mut budget).unwrap().outcome,
        PartOutcome::Unread(UnreadReason::UnlistedNameEquivalence { .. })
    ));

    // With only covered components in the protected set, the distinct
    // Latin-1 vowels `à` and `á` are not equivalent under any listed or
    // unlisted equivalence: Clear.
    let mut covered_fs = FakeFs::new();
    covered_fs.mkdir_all("/h/u/\u{00E0}/atrium".as_bytes());
    covered_fs.add_file("/h/u/\u{00E1}/atrium/db".as_bytes());
    let covered_prot = build_protected_set(
        HOME,
        CWD,
        vec!["/h/u/\u{00E0}/atrium".as_bytes().to_vec()],
        vec![],
        FoldRules::REV4,
        &covered_fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .unwrap();
    let acute_alone = resolve_literal(
        "/h/u/\u{00E1}/atrium/db".as_bytes(),
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &covered_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        check_target(&acute_alone, &covered_prot, &mut budget)
            .unwrap()
            .outcome,
        PartOutcome::Clear
    );

    // 2. Independent normalizing fake (not reusing production folding):
    //    checking NFD `a\u{030A}\u{0301}` resolves to stored NFC `ǻ` inode -> HardDenyHit.
    let mut nfd_fs = FakeFs::nfc_nfd_insensitive();
    nfd_fs.add_equivalent_component("\u{0F43}".as_bytes(), "\u{0F42}\u{0FB7}".as_bytes());
    nfd_fs.add_file("/h/u/\u{01FB}/atrium/db".as_bytes());
    let prot_nfd = build_protected_set(
        HOME,
        CWD,
        vec!["/h/u/\u{01FB}/atrium".as_bytes().to_vec()],
        vec![],
        FoldRules::REV4,
        &nfd_fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    let t_norm = resolve_literal(
        "/h/u/a\u{030A}\u{0301}/atrium/db".as_bytes(),
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
        check_target(&t_norm, &prot_nfd, &mut budget)
            .unwrap()
            .outcome,
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("atrium-self")
    ));
}

#[test]
fn round2_finding10_global_fs_caps_in_hard_link_walk_are_unread() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/proj/a.txt");
    let prot = default_protected(&fs);

    let mut init_caps = FsCaps::with_default_limits();
    let mut init_budget = ComputeBudget::with_default_limits();
    let proj_root = resolve_literal(
        b"proj",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut init_caps,
        &mut init_budget,
    )
    .unwrap();

    // Zero dir opens -> Unread(WalkCapReached(DirOpens)), NEVER Clear!
    let mut zero_dir_opens = FsCaps::new(100, 0, 100, 100, 100);
    let mut budget = ComputeBudget::with_default_limits();
    let res_dir = check_recursive_root(
        &proj_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut zero_dir_opens,
        &mut budget,
    )
    .unwrap();
    assert!(
        has_unread(&res_dir.outcomes, |r| *r
            == UnreadReason::WalkCapReached(FsCap::DirOpens)),
        "zero dir_opens in hard-link walk must yield WalkCapReached(DirOpens), got {:?}",
        res_dir.outcomes
    );

    // Zero stats -> Unread(WalkCapReached(Stats)), NEVER Clear!
    let mut zero_stats = FsCaps::new(100, 10, 0, 100, 100);
    let res_stats = check_recursive_root(
        &proj_root,
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut zero_stats,
        &mut budget,
    )
    .unwrap();
    assert!(has_unread(&res_stats.outcomes, |r| *r
        == UnreadReason::WalkCapReached(FsCap::Stats)));

    // Local hard-link walk cutoff (> HARD_LINK_WALK_ENTRIES) still counts as Clear (§8).
    let mut big_fs = FakeFs::new();
    for i in 0..(limits::HARD_LINK_WALK_ENTRIES + 5) {
        big_fs.add_file(format!("/h/u/big/f{i:05}.txt").as_bytes());
    }
    let big_prot = default_protected(&big_fs);
    let mut big_caps = FsCaps::with_default_limits();
    let mut big_budget = ComputeBudget::with_default_limits();
    let big_root = resolve_literal(
        b"big",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &big_fs,
        &mut big_caps,
        &mut big_budget,
    )
    .unwrap();
    let big_res = check_recursive_root(
        &big_root,
        LinkPolicy::default(),
        &[],
        &big_prot,
        &big_fs,
        &mut big_caps,
        &mut big_budget,
    )
    .unwrap();
    assert!(
        all_clear(&big_res.outcomes),
        "local HARD_LINK_WALK_ENTRIES cutoff should stay Clear, got {:?}",
        big_res.outcomes
    );
}

#[cfg(unix)]
#[test]
fn round2_finding11_budgets_all_fs_caps_and_small_nonzero_alloc() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let fake_home = std::fs::canonicalize(tmp.path()).expect("canonicalize tempdir");
    let home_bytes = fake_home.as_os_str().as_bytes().to_vec();
    std::fs::create_dir_all(fake_home.join("proj")).unwrap();
    std::fs::write(fake_home.join("proj/ordinary.txt"), b"hello").unwrap();
    symlink(
        fake_home.join("proj/ordinary.txt"),
        fake_home.join("proj/link_ordinary"),
    )
    .unwrap();

    let resolver = FsResolver::new();
    // Review 4, finding 7: discovery runs from `/`, so the build lists the
    // temporary folder's parent, whose size is the host's. The build is its
    // own phase with its own unbounded caps and budget (P5 gives the builder
    // its own caps too); the assertions below get fresh default ones.
    let prot = build_protected_set(
        &home_bytes,
        &home_bytes,
        vec![],
        vec![],
        FoldRules::REV4,
        &resolver,
        &mut FsCaps::new(usize::MAX, usize::MAX, usize::MAX, usize::MAX, usize::MAX),
        &mut ComputeBudget::new(u64::MAX, usize::MAX),
    )
    .unwrap();
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // 1. `check_target` on a Clear target fails with 0 AND with small nonzero alloc budgets!
    let clear_target = resolve_literal(
        b"proj/ordinary.txt",
        &home_bytes,
        &home_bytes,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &resolver,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    for alloc in [0usize, 4usize] {
        let mut small = ComputeBudget::new(10_000, alloc);
        let err = check_target(&clear_target, &prot, &mut small).unwrap_err();
        assert_eq!(
            err.reason,
            ExhaustReason::AllocationBudget,
            "check_target must exhaust on alloc={alloc}"
        );
    }

    // 2. Small nonzero alloc budgets fail BEFORE allocating in real `FsResolver`.
    let file_path = fake_home.join("proj/ordinary.txt");
    let link_path = fake_home.join("proj/link_ordinary");
    let dir_path = fake_home.join("proj");

    let mut small_rl = ComputeBudget::new(10_000, 4);
    assert_eq!(
        resolver.read_link(link_path.as_os_str().as_bytes(), &mut caps, &mut small_rl),
        Err(ResolveError::Exhausted(Exhausted {
            reason: ExhaustReason::AllocationBudget
        }))
    );

    let mut small_canon = ComputeBudget::new(10_000, 4);
    assert_eq!(
        resolver.canonicalize(
            file_path.as_os_str().as_bytes(),
            &mut caps,
            &mut small_canon
        ),
        Err(ResolveError::Exhausted(Exhausted {
            reason: ExhaustReason::AllocationBudget
        }))
    );

    let mut small_rd = ComputeBudget::new(10_000, 4);
    assert_eq!(
        resolver.read_dir(dir_path.as_os_str().as_bytes(), &mut caps, &mut small_rd),
        Err(ResolveError::Exhausted(Exhausted {
            reason: ExhaustReason::AllocationBudget
        }))
    );

    // 3. Exercise every global `FsCap` individually on real `FsResolver`.
    let mut plenty = ComputeBudget::with_default_limits();
    assert_eq!(
        resolver.read_dir(
            dir_path.as_os_str().as_bytes(),
            &mut FsCaps::new(0, 10, 10, 10, 10),
            &mut plenty
        ),
        Err(ResolveError::CapReached(CapReached {
            cap: FsCap::EntriesRead
        }))
    );
    assert_eq!(
        resolver.read_dir(
            dir_path.as_os_str().as_bytes(),
            &mut FsCaps::new(10, 0, 10, 10, 10),
            &mut plenty
        ),
        Err(ResolveError::CapReached(CapReached {
            cap: FsCap::DirOpens
        }))
    );
    assert_eq!(
        resolver.stat(
            file_path.as_os_str().as_bytes(),
            &mut FsCaps::new(10, 10, 0, 10, 10),
            &mut plenty
        ),
        Err(ResolveError::CapReached(CapReached { cap: FsCap::Stats }))
    );
    assert_eq!(
        resolver.read_link(
            link_path.as_os_str().as_bytes(),
            &mut FsCaps::new(10, 10, 10, 0, 10),
            &mut plenty
        ),
        Err(ResolveError::CapReached(CapReached {
            cap: FsCap::LinkReads
        }))
    );
    assert_eq!(
        resolver.canonicalize(
            file_path.as_os_str().as_bytes(),
            &mut FsCaps::new(10, 10, 10, 10, 0),
            &mut plenty
        ),
        Err(ResolveError::CapReached(CapReached {
            cap: FsCap::Canonicalizations
        }))
    );
}

#[test]
fn round2_finding12_unclosed_brackets_parse_linearly() {
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u/proj");
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    // 8,192 unquoted `[` bytes — well below MAX_BYTES; must parse linearly
    // without exhausting the default compute budget (§8).
    let mut raw = b"proj/".to_vec();
    raw.extend(std::iter::repeat_n(b'[', 8_192));
    let word = unquoted(&raw);

    let start_units = budget.remaining_units();
    let exp = expand_shell_glob(
        &word,
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
    .expect("8,192 unclosed `[` bytes must not exhaust default compute budget");
    let spent = start_units - budget.remaining_units();
    assert!(
        spent < 500_000,
        "expected linear compute for 8,192 `[` bytes, spent {spent}"
    );
    assert!(!exp.exact_matches.is_empty());

    // 4,096 unclosed `[` bytes followed by a POSIX class `[:alpha:]` also stays linear.
    let mut raw_posix = b"proj/".to_vec();
    raw_posix.extend(std::iter::repeat_n(b'[', 4_096));
    raw_posix.extend_from_slice(b"[:alpha:]");
    let mut budget2 = ComputeBudget::with_default_limits();
    let start2 = budget2.remaining_units();
    let _ = expand_shell_glob(
        &unquoted(&raw_posix),
        CWD,
        HOME,
        PartId::Operand {
            unit: UnitIndex(0),
            argv_index: 1,
        },
        AccessMode::Read,
        &fs,
        &mut caps,
        &mut budget2,
    )
    .expect("unclosed `[` prefix before POSIX class must parse linearly");
    let spent2 = start2 - budget2.remaining_units();
    assert!(spent2 < 500_000, "expected linear compute, spent {spent2}");
}

#[test]
fn round2_finding13_nested_shell_cwd_scope_and_isolation() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/id_rsa");
    fs.add_file(b"/h/u/b");
    fs.mkdir_all(b"/h/u/sub");
    fs.mkdir_all(b"/h/u/other");
    fs.add_symlink(b"/h/u/sub/key", b"/h/u/.ssh/id_rsa");
    fs.add_file(b"/h/u/other/key");
    let prot = default_protected(&fs);
    let mut caps = FsCaps::with_default_limits();
    let mut budget = ComputeBudget::with_default_limits();

    let mut cd_eff = reader_effects("cd", 2);
    cd_eff.cd_target_operand = Some(1);
    let sh_eff = ToolEffects {
        tool_name: Some("sh"),
        subcommand: None,
        roles: vec![OperandRole::Command, OperandRole::Flag, OperandRole::Code],
        operand_access: vec![AccessMode::Read, AccessMode::Read, AccessMode::Read],
        writes: false,
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

    // 1. `cd sub; sh -c 'cat key'; cd other` with child unit APPENDED after outer `cd other`:
    //    child `cat key` must resolve at `/h/u/sub` -> HardDenyHit!
    let u0_cd_sub = unit(
        0,
        vec![unquoted(b"cd"), unquoted(b"sub")],
        vec![],
        Separator::Start,
        None,
    );
    let u1_sh = unit(
        1,
        vec![unquoted(b"sh"), unquoted(b"-c"), single_quoted(b"cat key")],
        vec![],
        Separator::Semi,
        None,
    );
    let u2_cd_other = unit(
        2,
        vec![unquoted(b"cd"), unquoted(b"other")],
        vec![],
        Separator::Semi,
        None,
    );
    let u3_child_cat = CommandUnit {
        index: UnitIndex(3),
        argv: vec![unquoted(b"cat"), unquoted(b"key")],
        removed_wrappers: vec![],
        redirects: vec![],
        separator_before: Separator::Start,
        pipeline_id: None,
        depth: 1,
        parent_unit: Some(UnitIndex(1)),
    };
    let res_appended = resolve_command_targets(
        &[u0_cd_sub, u1_sh, u2_cd_other, u3_child_cat],
        &[
            cd_eff.clone(),
            sh_eff.clone(),
            cd_eff.clone(),
            reader_effects("cat", 2),
        ],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(
        has_hard_deny(&res_appended.outcomes, "credential-read"),
        "appended child `cat key` must inherit parent's `sub` cwd -> HardDenyHit, got {:?}",
        res_appended.outcomes
    );

    // 2. `sh -c 'cd ~/.ssh; cat id_rsa'; cat id_rsa`:
    //    child `cd ~/.ssh` must NOT leak into outer `cat id_rsa` (which reads ordinary `/h/u/id_rsa`)!
    let p0_sh = unit(
        0,
        vec![
            unquoted(b"sh"),
            unquoted(b"-c"),
            single_quoted(b"cd ~/.ssh; cat id_rsa"),
        ],
        vec![],
        Separator::Start,
        None,
    );
    let p1_child_cd = CommandUnit {
        index: UnitIndex(1),
        argv: vec![unquoted(b"cd"), unquoted(b"~/.ssh")],
        removed_wrappers: vec![],
        redirects: vec![],
        separator_before: Separator::Start,
        pipeline_id: None,
        depth: 1,
        parent_unit: Some(UnitIndex(0)),
    };
    let p2_child_cat = CommandUnit {
        index: UnitIndex(2),
        argv: vec![unquoted(b"cat"), unquoted(b"id_rsa")],
        removed_wrappers: vec![],
        redirects: vec![],
        separator_before: Separator::Semi,
        pipeline_id: None,
        depth: 1,
        parent_unit: Some(UnitIndex(0)),
    };
    let p3_outer_cat = unit(
        3,
        vec![unquoted(b"cat"), unquoted(b"id_rsa")],
        vec![],
        Separator::Semi,
        None,
    );
    let res_isolated = resolve_command_targets(
        &[p0_sh, p1_child_cd, p2_child_cat, p3_outer_cat],
        &[
            sh_eff.clone(),
            cd_eff.clone(),
            reader_effects("cat", 2),
            reader_effects("cat", 2),
        ],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(res_isolated.outcomes.iter().any(|r| {
        r.part
            == PartId::Operand {
                unit: UnitIndex(2),
                argv_index: 1,
            }
            && matches!(r.outcome, PartOutcome::HardDenyHit(_))
    }));
    assert!(
        res_isolated.outcomes.iter().any(|r| {
            r.part
                == PartId::Operand {
                    unit: UnitIndex(3),
                    argv_index: 1,
                }
                && r.outcome == PartOutcome::Clear
        }),
        "outer `cat id_rsa` must resolve at `/h/u` (Clear), not child's `~/.ssh`: {:?}",
        res_isolated.outcomes
    );

    // 3. `sh -c 'cd -; cat b'; cat b`: child `cd -` UnknownCwd stays isolated from outer `cat b`.
    let q0_sh = unit(
        0,
        vec![
            unquoted(b"sh"),
            unquoted(b"-c"),
            single_quoted(b"cd -; cat b"),
        ],
        vec![],
        Separator::Start,
        None,
    );
    let q1_child_cd_dash = CommandUnit {
        index: UnitIndex(1),
        argv: vec![unquoted(b"cd"), unquoted(b"-")],
        removed_wrappers: vec![],
        redirects: vec![],
        separator_before: Separator::Start,
        pipeline_id: None,
        depth: 1,
        parent_unit: Some(UnitIndex(0)),
    };
    let q2_child_cat = CommandUnit {
        index: UnitIndex(2),
        argv: vec![unquoted(b"cat"), unquoted(b"b")],
        removed_wrappers: vec![],
        redirects: vec![],
        separator_before: Separator::Semi,
        pipeline_id: None,
        depth: 1,
        parent_unit: Some(UnitIndex(0)),
    };
    let q3_outer_cat = unit(
        3,
        vec![unquoted(b"cat"), unquoted(b"b")],
        vec![],
        Separator::Semi,
        None,
    );
    let res_dash = resolve_command_targets(
        &[q0_sh, q1_child_cd_dash, q2_child_cat, q3_outer_cat],
        &[
            sh_eff,
            cd_eff,
            reader_effects("cat", 2),
            reader_effects("cat", 2),
        ],
        CWD,
        &prot,
        &fs,
        &mut caps,
        &mut budget,
    )
    .unwrap();
    assert!(res_dash.outcomes.iter().any(|r| {
        r.part
            == PartId::Operand {
                unit: UnitIndex(2),
                argv_index: 1,
            }
            && r.outcome == PartOutcome::Unread(UnreadReason::UnknownCwd)
    }));
    assert!(
        res_dash.outcomes.iter().any(|r| {
            r.part
                == PartId::Operand {
                    unit: UnitIndex(3),
                    argv_index: 1,
                }
                && r.outcome == PartOutcome::Clear
        }),
        "outer `cat b` must not inherit child's UnknownCwd: {:?}",
        res_dash.outcomes
    );
}

// ---------------------------------------------------------------------------
// 6. Rework Round 3 regressions (review round 2 and the open round-1 findings)
// ---------------------------------------------------------------------------

fn build_with(fs: &dyn PathResolver, data: &[&[u8]], protected: &[&[u8]]) -> ProtectedSet {
    build_protected_set(
        HOME,
        CWD,
        data.iter().map(|d| d.to_vec()).collect(),
        protected.iter().map(|d| d.to_vec()).collect(),
        FoldRules::REV4,
        fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .expect("build_protected_set fits default budget")
}

fn default_build(fs: &dyn PathResolver) -> ProtectedSet {
    build_with(
        fs,
        &[b"/h/u/.local/share/atrium"],
        &[b"/h/u/.config/atrium"],
    )
}

fn subject(
    path: &str,
    access: AccessMode,
    prot: &ProtectedSet,
    fs: &dyn PathResolver,
) -> crate::policy::model::ResolvedCallPaths {
    resolve_subject_paths(
        &[path.to_string()],
        None,
        false,
        access,
        prot,
        fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .expect("fits default budget")
}

fn literal(path: &[u8], access: AccessMode, fs: &dyn PathResolver) -> crate::policy::model::Target {
    resolve_literal(
        path,
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        access,
        fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .expect("fits default budget")
}

#[test]
fn round3_r2f3_write_only_members_are_collected_and_early_stops_are_unread() {
    // Review round 2, finding 3: a member nine levels down (past the old
    // depth-8 cutoff), hard-linked into `proj`.
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.config/atrium/a/b/c/d/e/f/g/h/key");
    fs.add_hard_link(
        b"/h/u/.config/atrium/a/b/c/d/e/f/g/h/key",
        b"/h/u/proj/alias",
    );
    // A member after 1,100 siblings (past the old 1,024-entry cutoff).
    for i in 0..1100 {
        fs.add_file(format!("/h/u/.config/atrium/many/f{i:04}").as_bytes());
    }
    fs.add_file(b"/h/u/.config/atrium/many/zz_key");
    fs.add_hard_link(b"/h/u/.config/atrium/many/zz_key", b"/h/u/proj/alias2");
    let build = default_build(&fs);
    assert!(
        build.identity_gaps.is_empty(),
        "complete walk, got {:?}",
        build.identity_gaps
    );
    let prot = &build;

    for alias in ["proj/alias", "proj/alias2"] {
        // §5, D-142: a write through a hard-linked alias of a write-only
        // protected member is a hard-deny hit.
        let w = subject(alias, AccessMode::Write, prot, &fs);
        assert!(
            has_hard_deny(&w.outcomes, "atrium-self"),
            "Write {alias}: {:?}",
            w.outcomes
        );
        // Reading it is not protected; the multiply-linked flag still stands.
        let r = subject(alias, AccessMode::Read, prot, &fs);
        assert!(
            !has_any_hard_deny(&r.outcomes),
            "Read {alias}: {:?}",
            r.outcomes
        );
        assert!(r.multiply_linked_file);
    }

    // Round-1 finding 7 / CR-P2-1: the pure `check_target`, with no
    // resolver, reaches the same verdict from the `ProtectedSet` alone.
    let t = literal(b"proj/alias", AccessMode::Write, &fs);
    assert!(matches!(
        check_target(&t, prot, &mut ComputeBudget::with_default_limits()).unwrap().outcome,
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("atrium-self")
    ));
    let t_rw = literal(b"proj/alias", AccessMode::ReadWrite, &fs);
    assert!(matches!(
        check_target(&t_rw, prot, &mut ComputeBudget::with_default_limits())
            .unwrap()
            .outcome,
        PartOutcome::HardDenyHit(_)
    ));
    let t_read = literal(b"proj/alias", AccessMode::Read, &fs);
    assert_eq!(
        check_target(&t_read, prot, &mut ComputeBudget::with_default_limits())
            .unwrap()
            .outcome,
        PartOutcome::Clear
    );
}

#[test]
fn round5_d145_member_walk_cutoff_is_clear_and_still_asks_by_its_link_count() {
    // D-145 rule 2 (replaces round 4's `AnyTarget` expectation): members are
    // collected for hard links within the walk caps, and a cutoff there is
    // Clear, as in §5's bounded hard-link walk. The uncollected hard-linked
    // alias is still Ask in every setting (§1 step 4, multiply-linked flag).
    let mut fs = FakeFs::new();
    for i in 0..(limits::WALK_MAX_ENTRIES + 4) {
        fs.add_file(format!("/h/u/.config/atrium/big/f{i:05}").as_bytes());
    }
    fs.add_file(b"/h/u/.config/atrium/big/zz_key");
    fs.add_hard_link(b"/h/u/.config/atrium/big/zz_key", b"/h/u/proj/alias");
    fs.add_file(b"/h/u/proj/plain");
    let mut deep = b"/h/u/.config/atrium".to_vec();
    for i in 0..=(limits::WALK_MAX_DEPTH as usize) {
        deep.extend_from_slice(format!("/d{i}").as_bytes());
    }
    deep.extend_from_slice(b"/key");
    fs.add_file(&deep);
    fs.add_hard_link(&deep, b"/h/u/proj/deep_alias");
    let prot = default_build(&fs);
    assert!(prot.identity_gaps.is_empty(), "{:?}", prot.identity_gaps);

    for alias in ["proj/alias", "proj/deep_alias"] {
        let w = subject(alias, AccessMode::Write, &prot, &fs);
        assert!(all_clear(&w.outcomes), "{alias}: {:?}", w.outcomes);
        assert!(
            w.multiply_linked_file,
            "{alias} still asks by its link count"
        );
    }
    assert!(all_clear(
        &subject("proj/plain", AccessMode::Write, &prot, &fs).outcomes
    ));
    // Spelled through the protected folder, every member is a hit by name.
    let named = subject("~/.config/atrium/big/zz_key", AccessMode::Write, &prot, &fs);
    assert!(has_hard_deny(&named.outcomes, "atrium-self"));
}

#[test]
fn round5_d145_gaps_only_where_a_root_cannot_be_established() {
    // D-145 rule 4 (replaces round 4's walk-cap gaps).
    // (a) A large tree is not a gap: Clear stays Clear.
    let mut big = FakeFs::new();
    for i in 0..(limits::WALK_MAX_ENTRIES + 4) {
        big.add_file(format!("/h/u/.config/google-chrome/Cache/c{i:05}").as_bytes());
    }
    big.add_file(b"/h/u/proj/readme");
    let big_prot = default_build(&big);
    assert!(
        big_prot.identity_gaps.is_empty(),
        "{:?}",
        big_prot.identity_gaps
    );
    assert!(all_clear(
        &subject("proj/readme", AccessMode::Read, &big_prot, &big).outcomes
    ));

    // (b) A credential tree root that cannot be listed: a gap through it,
    //     which reaches multiply-linked targets but not ordinary ones; paths
    //     spelled through the root stay exact hits.
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.gnupg/x");
    fs.set_dir_read_error(b"/h/u/.gnupg", FsIoError::PermissionDenied);
    fs.add_file(b"/h/u/proj/readme");
    fs.add_file(b"/h/u/proj/a");
    fs.add_hard_link(b"/h/u/proj/a", b"/h/u/proj/b");
    let prot = default_build(&fs);
    assert!(
        prot.identity_gaps
            .iter()
            .any(|g| matches!(g.scope, GapScope::Through(_)) && !g.write_only),
        "{:?}",
        prot.identity_gaps
    );
    assert!(all_clear(
        &subject("proj/readme", AccessMode::Read, &prot, &fs).outcomes
    ));
    assert!(has_unread(
        &subject("proj/b", AccessMode::Read, &prot, &fs).outcomes,
        |r| { matches!(r, UnreadReason::UnresolvablePath { .. }) }
    ));
    let x = subject("~/.gnupg/x", AccessMode::Read, &prot, &fs);
    assert!(has_hard_deny(&x.outcomes, "credential-read"));
    assert!(!has_unread(&x.outcomes, |_| true));

    // (c) `~/.ssh` that cannot be listed: `AnyTarget`. Every target that is
    //     not a hit is Unread; a Store hit keeps its record beside the doubt,
    //     and a single-result check reports the stricter Unread.
    let mut ssh_fs = FakeFs::new();
    ssh_fs.mkdir_all(b"/h/u/.ssh");
    ssh_fs.set_dir_read_error(b"/h/u/.ssh", FsIoError::PermissionDenied);
    ssh_fs.add_file(b"/h/u/proj/readme");
    ssh_fs.add_file(b"/h/u/.pypirc");
    ssh_fs.add_file(b"/h/u/.aws/credentials");
    let ssh_prot = default_build(&ssh_fs);
    assert!(
        ssh_prot
            .identity_gaps
            .iter()
            .any(|g| g.scope == GapScope::AnyTarget && !g.write_only)
    );
    assert!(has_unread(
        &subject("proj/readme", AccessMode::Read, &ssh_prot, &ssh_fs).outcomes,
        |r| matches!(r, UnreadReason::UnresolvablePath { .. })
    ));
    let store = subject("~/.pypirc", AccessMode::Read, &ssh_prot, &ssh_fs);
    assert!(has_store_hit(&store.outcomes) && has_unread(&store.outcomes, |_| true));
    let t = literal(b"~/.pypirc", AccessMode::Read, &ssh_fs);
    assert!(matches!(
        check_target(&t, &ssh_prot, &mut ComputeBudget::with_default_limits())
            .unwrap()
            .outcome,
        PartOutcome::Unread(_)
    ));
    let key = subject("~/.aws/credentials", AccessMode::Read, &ssh_prot, &ssh_fs);
    assert!(has_hard_deny(&key.outcomes, "credential-read"));
    assert!(!has_unread(&key.outcomes, |_| true));
}

#[cfg(target_os = "linux")]
#[test]
fn round3_r2f4_fs_resolver_makes_no_uncharged_metadata_call() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    std::fs::write(root.join("file"), b"x").unwrap();
    symlink(root.join("file"), root.join("link")).unwrap();
    let resolver = FsResolver::new();

    // Review round 2, finding 4: with no stat left but a link read (or a
    // canonicalization) available, the call may only succeed if it makes no
    // metadata call. It succeeds, and only its own cap moves.
    let mut caps = FsCaps::new(10, 10, 0, 10, 10);
    let target = resolver
        .read_link(
            root.join("link").as_os_str().as_bytes(),
            &mut caps,
            &mut ComputeBudget::with_default_limits(),
        )
        .expect("readlink needs no stat");
    assert_eq!(target, root.join("file").as_os_str().as_bytes());
    assert_eq!(caps, FsCaps::new(10, 10, 0, 9, 10));

    let mut caps = FsCaps::new(10, 10, 0, 10, 10);
    let canon = resolver
        .canonicalize(
            root.join("link").as_os_str().as_bytes(),
            &mut caps,
            &mut ComputeBudget::with_default_limits(),
        )
        .expect("canonicalize needs no stat on Linux");
    assert_eq!(canon, root.join("file").as_os_str().as_bytes());
    // Round 3 review, finding 5: reading `/proc/self/fd/N` is a link read,
    // charged before it runs; a canonicalization with no link read left
    // stops before it.
    assert_eq!(caps, FsCaps::new(10, 10, 0, 9, 9));
    assert_eq!(
        resolver.canonicalize(
            root.join("file").as_os_str().as_bytes(),
            &mut FsCaps::new(10, 10, 10, 0, 10),
            &mut ComputeBudget::with_default_limits(),
        ),
        Err(ResolveError::CapReached(CapReached {
            cap: FsCap::LinkReads
        }))
    );

    // A non-link is still told apart from a link, without a metadata call.
    assert_eq!(
        resolver.read_link(
            root.join("file").as_os_str().as_bytes(),
            &mut FsCaps::new(10, 10, 0, 10, 10),
            &mut ComputeBudget::with_default_limits(),
        ),
        Err(ResolveError::Io(FsIoError::NotASymlink))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn round3_r2f5_canonicalize_charges_its_expanded_output_before_allocating() {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;

    // Review round 2, finding 5: a short directory link leads to a much
    // longer real path; the final component is not a link.
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let long_dir = root.join("l".repeat(200));
    std::fs::create_dir(&long_dir).unwrap();
    std::fs::write(long_dir.join("file"), b"x").unwrap();
    symlink(&long_dir, root.join("short")).unwrap();
    let input = root.join("short/file");
    let input = input.as_os_str().as_bytes();
    let output = long_dir.join("file");
    let output = output.as_os_str().as_bytes();
    assert!(output.len() > input.len() + 150);
    let resolver = FsResolver::new();

    // An allocation budget that covers the input but not the output fails
    // before the output is allocated.
    let mut small = ComputeBudget::new(1_000_000, input.len() + 8);
    assert_eq!(
        resolver.canonicalize(input, &mut FsCaps::with_default_limits(), &mut small),
        Err(ResolveError::Exhausted(Exhausted {
            reason: ExhaustReason::AllocationBudget
        }))
    );
    // The output is charged at its exact size.
    let mut exact = ComputeBudget::new(1_000_000, output.len());
    assert_eq!(
        resolver.canonicalize(input, &mut FsCaps::with_default_limits(), &mut exact),
        Ok(output.to_vec())
    );
    assert_eq!(exact.remaining_alloc_bytes(), 0);
}

#[test]
fn round3_r2f6_combining_marks_with_decompositions_are_unlisted_equivalences() {
    // Review round 2, finding 6: U+0341 is canonically U+0301 and U+0344 is
    // U+0308 U+0301, so these spellings are equivalent under NFD: Unread.
    let mut fs = FakeFs::new();
    fs.mkdir_all("/h/u/e\u{0341}/atrium".as_bytes());
    fs.add_file("/h/u/\u{00E9}/atrium/db".as_bytes());
    fs.mkdir_all("/h/u/u\u{0344}/atrium".as_bytes());
    fs.add_file("/h/u/u\u{0308}\u{0301}/atrium/db".as_bytes());
    fs.add_file("/h/u/\u{4E2D}\u{6587}/notes.txt".as_bytes());
    let build = build_with(
        &fs,
        &[
            "/h/u/e\u{0341}/atrium".as_bytes(),
            "/h/u/u\u{0344}/atrium".as_bytes(),
        ],
        &[],
    );
    for path in [
        "/h/u/\u{00E9}/atrium/db",
        "/h/u/u\u{0308}\u{0301}/atrium/db",
    ] {
        let t = literal(path.as_bytes(), AccessMode::Read, &fs);
        assert!(
            matches!(
                check_target(&t, &build, &mut ComputeBudget::with_default_limits())
                    .unwrap()
                    .outcome,
                PartOutcome::Unread(UnreadReason::UnlistedNameEquivalence { .. })
            ),
            "{path:?}"
        );
    }
    // An uncovered scalar never makes a name equivalent to an ASCII one:
    // a CJK folder in home is Clear.
    let cjk = literal(
        "/h/u/\u{4E2D}\u{6587}/notes.txt".as_bytes(),
        AccessMode::Read,
        &fs,
    );
    assert_eq!(
        check_target(&cjk, &build, &mut ComputeBudget::with_default_limits())
            .unwrap()
            .outcome,
        PartOutcome::Clear
    );
}

#[test]
fn round3_r2f7_nested_unclosed_brackets_parse_linearly() {
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u/proj");
    let source = PartId::Operand {
        unit: UnitIndex(0),
        argv_index: 1,
    };
    // Review round 2, finding 7: `[[:digit:]` repeated 2,048 times (20,480
    // bytes) took 16.8 million closing-bracket steps; plus two neighbours.
    for (unit_text, repeat) in [
        (b"[[:digit:]".as_slice(), 2_048usize),
        (b"[[[:alpha:]".as_slice(), 2_048),
        (b"[[:".as_slice(), 6_000),
        (b"[a[:x".as_slice(), 4_000),
    ] {
        let mut raw = b"proj/".to_vec();
        for _ in 0..repeat {
            raw.extend_from_slice(unit_text);
        }
        let mut budget = ComputeBudget::with_default_limits();
        let start = budget.remaining_units();
        expand_shell_glob(
            &unquoted(&raw),
            CWD,
            HOME,
            source.clone(),
            AccessMode::Read,
            &fs,
            &mut FsCaps::with_default_limits(),
            &mut budget,
        )
        .expect("must not exhaust the default budget");
        let spent = start - budget.remaining_units();
        assert!(
            spent < 40 * raw.len() as u64,
            "{:?}: expected linear work, spent {spent} on {} bytes",
            String::from_utf8_lossy(unit_text),
            raw.len()
        );
    }
}

#[test]
fn round3_r2f8_listed_fold_is_string_lowercase_with_final_sigma() {
    let mut budget = ComputeBudget::with_default_limits();
    assert_eq!(
        fold_listed_component("\u{039F}\u{03A3}".as_bytes(), FoldRules::REV4, &mut budget).unwrap(),
        "\u{03BF}\u{03C2}".as_bytes()
    );
    assert_eq!(
        fold_path_listed(
            "/h/u/\u{039F}\u{03A3}/atrium/db".as_bytes(),
            FoldRules::REV4,
            &mut budget
        )
        .unwrap(),
        "/h/u/\u{03BF}\u{03C2}/atrium/db".as_bytes()
    );
    // `dev`'s substitutions still apply after lowercasing (long s, Kelvin).
    assert_eq!(
        fold_listed_component(
            "\u{017F}SH\u{212A}".as_bytes(),
            FoldRules::REV4,
            &mut budget
        )
        .unwrap(),
        b"sshk"
    );

    // Review round 2, finding 8: Atrium data at `ος`, a distinct `ΟΣ` folder
    // on a case-sensitive volume: a listed-fold hard-deny hit.
    let mut fs = FakeFs::new();
    fs.mkdir_all("/h/u/\u{03BF}\u{03C2}/atrium".as_bytes());
    fs.add_file("/h/u/\u{039F}\u{03A3}/atrium/db".as_bytes());
    let build = build_with(&fs, &["/h/u/\u{03BF}\u{03C2}/atrium".as_bytes()], &[]);
    let t = literal(
        "/h/u/\u{039F}\u{03A3}/atrium/db".as_bytes(),
        AccessMode::Read,
        &fs,
    );
    assert!(matches!(
        check_target(&t, &build, &mut budget).unwrap().outcome,
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("atrium-self")
    ));
}

#[test]
fn round3_r2f9_existing_lowercase_browser_root_does_not_hide_fold_siblings() {
    // Review round 2, finding 9.
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u/library/application support/google/chrome");
    fs.add_file(b"/h/u/library/APPLICATION SUPPORT/google/chrome/Cookies");
    let build = default_build(&fs);
    let root = literal(b"~/library", AccessMode::Read, &fs);
    let filter = SearchFilter {
        dialect: FilterDialect::RgGlob,
        pattern: unquoted(b"*.rs"),
        case_insensitive: false,
        argv_index: Some(1),
    };
    let res = check_recursive_root(
        &root,
        LinkPolicy::default(),
        &[filter],
        &build,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .unwrap();
    assert!(
        has_hard_deny(&res.outcomes, "credential-read"),
        "S8 must find the APPLICATION SUPPORT spelling, got {:?}",
        res.outcomes
    );
}

#[test]
fn round3_neighbour_paths_spelled_through_protected_folders_keep_their_hit() {
    // Neighbour of round-1 finding 8: a link inside a protected folder that
    // leads out of it, placed past the builder's walk cap so its own identity
    // (and its target's) is not collected. The path as spelled names the
    // protected folder, and resolution keeps that folder's identity (§5).
    let mut fs = FakeFs::new();
    fs.add_file(b"/tmp/ordinary");
    fs.add_file(b"/tmp/ordinary2");
    for i in 0..(limits::WALK_MAX_ENTRIES + 4) {
        fs.add_file(format!("/h/u/.gnupg/f{i:05}").as_bytes());
        fs.add_file(format!("/h/u/.config/atrium/f{i:05}").as_bytes());
    }
    fs.add_symlink(b"/h/u/.gnupg/zz_link", b"/tmp/ordinary");
    fs.add_symlink(b"/h/u/.config/atrium/zz_link", b"/tmp/ordinary2");
    fs.add_symlink(b"/h/u/proj/chain", b"/h/u/.gnupg/zz_link");
    let build = default_build(&fs);
    let prot = &build;

    for p in ["~/.gnupg/zz_link", "proj/chain"] {
        let r = subject(p, AccessMode::Read, prot, &fs);
        assert!(
            has_hard_deny(&r.outcomes, "credential-read"),
            "{p}: {:?}",
            r.outcomes
        );
    }
    let w = subject("~/.config/atrium/zz_link", AccessMode::Write, prot, &fs);
    assert!(
        has_hard_deny(&w.outcomes, "atrium-self"),
        "{:?}",
        w.outcomes
    );
    let r = subject("~/.config/atrium/zz_link", AccessMode::Read, prot, &fs);
    assert!(!has_any_hard_deny(&r.outcomes), "{:?}", r.outcomes);
    // The links' targets, named directly: D-145 rules 2-3. Link targets are
    // recorded only for `~/.ssh` members, and the member walk's cutoff is
    // not a gap, so a direct read of the external target is Clear, as on
    // `dev` (not a hit, not Unread).
    assert!(prot.identity_gaps.is_empty(), "{:?}", prot.identity_gaps);
    for (p, access) in [
        ("/tmp/ordinary", AccessMode::Read),
        ("/tmp/ordinary2", AccessMode::Write),
    ] {
        let r = subject(p, access, prot, &fs);
        assert!(all_clear(&r.outcomes), "{p}: {:?}", r.outcomes);
    }
}

#[test]
fn round3_neighbour_hard_links_to_tree_members_and_folded_abs_paths() {
    let mut fs = FakeFs::new();
    // Credential-tree member and store-tree member (members are now walked).
    fs.add_file(b"/h/u/.gnupg/private-keys-v1.d/k.key");
    fs.add_hard_link(b"/h/u/.gnupg/private-keys-v1.d/k.key", b"/h/u/proj/gk");
    fs.add_file(b"/h/u/.aws/sso/cache/tok.json");
    fs.add_hard_link(b"/h/u/.aws/sso/cache/tok.json", b"/h/u/proj/tok");
    // Atrium data outside home configured as `/opt/atrium`, present only
    // as `/OPT/atrium` on this case-sensitive volume.
    fs.add_file(b"/OPT/atrium/db");
    fs.add_hard_link(b"/OPT/atrium/db", b"/h/u/proj/db_alias");
    let build = build_with(&fs, &[b"/opt/atrium"], &[]);
    assert!(build.identity_gaps.is_empty(), "{:?}", build.identity_gaps);
    let prot = &build;

    assert!(has_hard_deny(
        &subject("proj/gk", AccessMode::Read, prot, &fs).outcomes,
        "credential-read"
    ));
    let tok = subject("proj/tok", AccessMode::Read, prot, &fs);
    assert!(has_store_hit(&tok.outcomes) && !has_any_hard_deny(&tok.outcomes));
    assert!(has_hard_deny(
        &subject("proj/db_alias", AccessMode::Read, prot, &fs).outcomes,
        "atrium-self"
    ));
}

#[test]
fn round3_neighbour_written_recursive_root_checks_write_only_identities() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.ssh/config");
    fs.add_hard_link(b"/h/u/.ssh/config", b"/h/u/proj/cfg_alias");
    let build = default_build(&fs);
    let run = |access| {
        let root = literal(b"proj", access, &fs);
        check_recursive_root(
            &root,
            LinkPolicy::default(),
            &[],
            &build,
            &fs,
            &mut FsCaps::with_default_limits(),
            &mut ComputeBudget::with_default_limits(),
        )
        .unwrap()
    };
    let written = run(AccessMode::Write);
    assert!(
        has_hard_deny(&written.outcomes, "credential-read"),
        "{:?}",
        written.outcomes
    );
    let read = run(AccessMode::Read);
    assert!(!has_any_hard_deny(&read.outcomes), "{:?}", read.outcomes);
    assert!(read.multiply_linked_file);
}

#[test]
fn round3_neighbour_hard_link_walk_reports_unreadable_folders() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/proj/closed/secret");
    fs.set_dir_unreadable(b"/h/u/proj/closed", FsIoError::PermissionDenied);
    let build = default_build(&fs);
    let root = literal(b"proj", AccessMode::Read, &fs);
    let res = check_recursive_root(
        &root,
        LinkPolicy::default(),
        &[],
        &build,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .unwrap();
    assert!(
        has_unread(&res.outcomes, |r| matches!(
            r,
            UnreadReason::UnresolvablePath { .. }
        )),
        "an unreadable folder is not the local cutoff: {:?}",
        res.outcomes
    );
}

#[test]
fn round3_neighbour_ask_tree_prefix_rule_stays_linear() {
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u/.aws/sso/cache");
    let build = default_build(&fs);
    let mut pattern = "a/".repeat(5_000);
    pattern.push('x');
    let mut budget = ComputeBudget::with_default_limits();
    let start = budget.remaining_units();
    // Before round 3 this built one matcher per `/` (5,000 of them, each
    // charged at its length) and ran out of the default budget.
    let res = resolve_subject_paths(
        &["~".to_string()],
        Some(&pattern),
        true,
        AccessMode::Read,
        &build,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut budget,
    )
    .expect("fits default budget");
    assert!(!budget.is_exhausted());
    assert!(
        !res.outcomes
            .iter()
            .any(|r| matches!(r.outcome, PartOutcome::Exhausted(_)))
    );
    let spent = start - budget.remaining_units();
    assert!(spent < 4_000_000, "spent {spent}");
}

// ---------------------------------------------------------------------------
// 7. Rework Round 4 regressions (review round 3, CR-P2-2 / D-144)
// ---------------------------------------------------------------------------

fn read_outcome(path: &[u8], prot: &ProtectedSet, fs: &dyn PathResolver) -> PartOutcome {
    let t = literal(path, AccessMode::Read, fs);
    check_target(&t, prot, &mut ComputeBudget::with_default_limits())
        .unwrap()
        .outcome
}

#[test]
fn round4_f1_browser_profile_reached_through_a_link_is_collected() {
    // Review round 3, finding 1.
    let mut fs = FakeFs::new();
    fs.add_file(b"/tmp/profile/Cookies");
    fs.add_file(b"/tmp/profile/Preferences");
    fs.mkdir_all(b"/h/u/.config/google-chrome");
    fs.add_symlink(b"/h/u/.config/google-chrome/Default", b"/tmp/profile");
    let prot = default_build(&fs);
    assert!(prot.identity_gaps.is_empty(), "{:?}", prot.identity_gaps);

    // The protected spelling, through the linked profile: a hit by the
    // identity collected below the link (§5).
    assert!(matches!(
        read_outcome(b"~/.config/google-chrome/Default/Cookies", &prot, &fs),
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("credential-read")
    ));
    let r = subject(
        "~/.config/google-chrome/Default/Cookies",
        AccessMode::Read,
        &prot,
        &fs,
    );
    assert!(has_hard_deny(&r.outcomes, "credential-read"));
    // An ordinary profile file stays ordinary.
    assert_eq!(
        read_outcome(b"~/.config/google-chrome/Default/Preferences", &prot, &fs),
        PartOutcome::Clear
    );
}

#[test]
fn round5_d145_unlistable_tree_root_is_a_gap_through_it() {
    // Review round 3, finding 2, under D-145: `.gnupg` is searchable but not
    // listable, so the root cannot be listed: a gap through the root (rule
    // 4). The external target of a link inside the tree is not recorded even
    // by a complete walk (rule 3), so its direct read is Clear, as on `dev`.
    let mut fs = FakeFs::new();
    fs.add_file(b"/tmp/token");
    fs.add_symlink(b"/h/u/.gnupg/key", b"/tmp/token");
    fs.set_dir_read_error(b"/h/u/.gnupg", FsIoError::PermissionDenied);
    fs.add_file(b"/h/u/proj/a");
    fs.add_hard_link(b"/h/u/proj/a", b"/h/u/proj/b");
    let prot = default_build(&fs);
    assert!(prot.identity_gaps.iter().any(|g| !g.write_only
        && matches!(g.scope, GapScope::Through(_))
        && matches!(g.reason, UnreadReason::UnresolvablePath { .. })));
    assert!(
        !prot
            .identity_gaps
            .iter()
            .any(|g| g.scope == GapScope::AnyTarget)
    );

    assert_eq!(read_outcome(b"/tmp/token", &prot, &fs), PartOutcome::Clear);
    // The protected spelling stays an exact hit (the root's identity).
    assert!(matches!(
        read_outcome(b"~/.gnupg/key", &prot, &fs),
        PartOutcome::HardDenyHit(_)
    ));
    // A multiply-linked file could be an uncollected member: Unread.
    assert!(matches!(
        read_outcome(b"proj/b", &prot, &fs),
        PartOutcome::Unread(UnreadReason::UnresolvablePath { .. })
    ));
}

#[test]
fn round4_f3_cached_listing_failure_is_recorded_for_every_access_scope() {
    // Review round 3, finding 3: the write-protected Atrium path is the first
    // to list `.azure` (and fail); the store discovery that reuses the cached
    // failure is read-sensitive. Under D-145 rule 4 an unreadable folder on
    // the path to a root may hide fold spellings whose identity is unknown,
    // so each consumer records an `AnyTarget` gap in its own access scope.
    let mut fs = FakeFs::new();
    fs.add_file(b"/tmp/token");
    fs.mkdir_all(b"/h/u/.azure/atrium");
    fs.add_symlink(b"/h/u/.azure/msal_token_cache.json", b"/tmp/token");
    fs.set_dir_read_error(b"/h/u/.azure", FsIoError::PermissionDenied);
    let prot = build_with(&fs, &[], &[b"/h/u/.azure/atrium"]);
    assert!(prot.identity_gaps.iter().any(|g| g.write_only));
    assert!(
        prot.identity_gaps
            .iter()
            .any(|g| !g.write_only && g.scope == GapScope::AnyTarget)
    );
    assert!(matches!(
        read_outcome(b"/tmp/token", &prot, &fs),
        PartOutcome::Unread(UnreadReason::UnresolvablePath { .. })
    ));
}

#[test]
fn round4_f4_fold_variants_of_home_components_are_discovered() {
    // Review round 3, finding 4: home is `/h/u`; a distinct `/H/U` exists.
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u");
    fs.add_file(b"/tmp/token");
    fs.add_file(b"/tmp/db2");
    fs.add_symlink(b"/H/U/.aws/credentials", b"/tmp/token");
    fs.mkdir_all(b"/h/u/.local/share/atrium");
    fs.add_symlink(b"/H/U/.local/share/atrium/db", b"/tmp/db2");
    let prot = default_build(&fs);
    assert!(prot.identity_gaps.is_empty(), "{:?}", prot.identity_gaps);

    assert!(matches!(
        read_outcome(b"/H/U/.aws/credentials", &prot, &fs),
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("credential-read")
    ));
    assert!(matches!(
        read_outcome(b"/H/U/.local/share/atrium/db", &prot, &fs),
        PartOutcome::HardDenyHit(ref h) if h.hard_deny_id == Some("atrium-self")
    ));
}

#[test]
fn round4_f6_error_path_copies_are_charged_before_allocating() {
    // Review round 3, finding 6: enough compute, no allocation budget.
    let fs = FakeFs::new();
    let mut budget = ComputeBudget::new(1_000_000, 0);
    let err = resolve_literal(
        b"x\0",
        CWD,
        HOME,
        TargetOrigin::SubjectPath { path_index: 0 },
        AccessMode::Read,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut budget,
    )
    .unwrap_err();
    assert_eq!(err.reason, ExhaustReason::AllocationBudget);

    let mut word = unquoted(b"x");
    word.push(0, ByteProvenance::Unquoted);
    word.push(b'*', ByteProvenance::Unquoted);
    let mut budget = ComputeBudget::new(1_000_000, 0);
    let err = expand_shell_glob(
        &word,
        CWD,
        HOME,
        PartId::Operand {
            unit: UnitIndex(0),
            argv_index: 1,
        },
        AccessMode::Read,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut budget,
    )
    .unwrap_err();
    assert_eq!(err.reason, ExhaustReason::AllocationBudget);
}

#[test]
fn round4_f7_too_large_survives_an_exhausted_budget() {
    // Review round 3, finding 7: §1 puts TooLarge above Exhausted.
    let fs = FakeFs::new();
    let prot = default_build(&fs);
    let paths: Vec<String> = (0..=limits::MAX_USES).map(|i| format!("f{i}")).collect();
    for (units, alloc) in [(1_000_000u64, 0usize), (0, 0)] {
        let res = resolve_subject_paths(
            &paths,
            None,
            false,
            AccessMode::Read,
            &prot,
            &fs,
            &mut FsCaps::with_default_limits(),
            &mut ComputeBudget::new(units, alloc),
        )
        .expect("a structural limit is decided, not lost");
        assert!(
            res.outcomes
                .iter()
                .any(|r| r.outcome == PartOutcome::TooLarge(Cap::Uses)),
            "{:?}",
            res.outcomes
        );
    }

    // Neighbour: a command line whose path operands alone exceed MAX_PATHS.
    let mut argv = vec![unquoted(b"cat")];
    for _ in 0..=limits::MAX_PATHS {
        argv.push(unquoted(b"f"));
    }
    let n = argv.len();
    let u = unit(0, argv, vec![], Separator::Start, None);
    let res = resolve_command_targets(
        &[u],
        &[reader_effects("cat", n)],
        CWD,
        &prot,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::new(0, 0),
    )
    .expect("a structural limit is decided, not lost");
    assert!(
        res.outcomes
            .iter()
            .any(|r| r.outcome == PartOutcome::TooLarge(Cap::Paths))
    );
}

#[test]
fn round4_f8_mv_trigger_is_a_directory_source_not_the_destination() {
    // Review round 3, finding 8: the annex gives derived-destination writers
    // all-`Path`, ReadWrite operands.
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/ordinary_file");
    fs.mkdir_all(b"/h/u/ordinary_directory");
    fs.mkdir_all(b"/h/u/source_dir");
    let prot = default_build(&fs);
    let mv_effects = |n: usize| ToolEffects {
        tool_name: Some("mv"),
        subcommand: None,
        roles: std::iter::once(OperandRole::Command)
            .chain(std::iter::repeat_n(OperandRole::Path, n - 1))
            .collect(),
        operand_access: std::iter::once(AccessMode::Read)
            .chain(std::iter::repeat_n(AccessMode::ReadWrite, n - 1))
            .collect(),
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
    let derived = |argv: &[&[u8]]| {
        let words: Vec<Word> = argv.iter().map(|w| unquoted(w)).collect();
        let n = words.len();
        let res = resolve_command_targets(
            &[unit(0, words, vec![], Separator::Start, None)],
            &[mv_effects(n)],
            CWD,
            &prot,
            &fs,
            &mut FsCaps::with_default_limits(),
            &mut ComputeBudget::with_default_limits(),
        )
        .unwrap();
        has_unread(&res.outcomes, |r| {
            matches!(r, UnreadReason::DerivedDestinations { .. })
        })
    };
    // A file moved into a directory: no directory source, no derived destinations.
    assert!(!derived(&[b"mv", b"ordinary_file", b"ordinary_directory"]));
    // A directory moved: derived destinations, the writer is Unread (§5).
    assert!(derived(&[b"mv", b"source_dir", b"ordinary_directory"]));
    // `-t DIR` names the destination first: every operand may be a source.
    assert!(derived(&[
        b"mv",
        b"-t",
        b"ordinary_directory",
        b"ordinary_file"
    ]));
}

#[test]
fn round4_cr_p2_2_gaps_reach_standalone_checks_and_recursive_walks() {
    // CR-P2-2 failing case 2: a filter-selected link, checked inside the
    // recursive walk, into a browser root with a gap. Under D-145 a walk cap
    // is no longer a gap, so the root here cannot be listed (rule 4).
    let mut fs = FakeFs::new();
    fs.add_file(b"/h/u/.config/google-chrome/Default/x.toml");
    fs.add_symlink(
        b"/h/u/proj/l.toml",
        b"/h/u/.config/google-chrome/Default/x.toml",
    );
    fs.set_dir_read_error(b"/h/u/.config/google-chrome", FsIoError::PermissionDenied);
    let prot = default_build(&fs);
    let root = literal(b"proj", AccessMode::Read, &fs);
    let filter = SearchFilter {
        dialect: FilterDialect::RgGlob,
        pattern: unquoted(b"*.toml"),
        case_insensitive: false,
        argv_index: Some(1),
    };
    let res = check_recursive_root(
        &root,
        LinkPolicy::default(),
        &[filter],
        &prot,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .unwrap();
    assert!(!has_any_hard_deny(&res.outcomes));
    assert!(
        has_unread(&res.outcomes, |r| matches!(
            r,
            UnreadReason::UnresolvablePath { .. }
        )),
        "{:?}",
        res.outcomes
    );
    // (Failing case 1, a write through a hard link past the member walk's cap,
    // is Clear with the multiply-linked Ask under D-145 rule 2:
    // `round5_d145_member_walk_cutoff_is_clear_and_still_asks_by_its_link_count`.)
}

#[test]
fn round4_neighbour_protected_folder_reached_through_a_link_is_walked() {
    // A credential-tree folder that is a link: walked like the folder, so its
    // members (and the linked folder) are protected under any name.
    let mut fs = FakeFs::new();
    fs.add_file(b"/tmp/gsub/k");
    fs.mkdir_all(b"/h/u/.gnupg");
    fs.add_symlink(b"/h/u/.gnupg/sub", b"/tmp/gsub");
    fs.add_hard_link(b"/tmp/gsub/k", b"/h/u/proj/k_alias");
    let prot = default_build(&fs);
    assert!(prot.identity_gaps.is_empty(), "{:?}", prot.identity_gaps);
    for p in [&b"~/.gnupg/sub/k"[..], b"/tmp/gsub/k", b"proj/k_alias"] {
        assert!(
            matches!(read_outcome(p, &prot, &fs), PartOutcome::HardDenyHit(_)),
            "{}",
            String::from_utf8_lossy(p)
        );
    }
}

// ---------------------------------------------------------------------------
// 8. Rework Round 6 regressions (review round 4, D-147)
// ---------------------------------------------------------------------------

#[test]
fn round6_f1_protected_names_on_the_written_spelling_below_aliased_roots() {
    // Review 4, finding 1 / D-147: as on `dev` (`Sensitive::hit` over the
    // lexical spelling), a protected name stays a hit when its root is a link,
    // even when the name does not exist yet.
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/tmp/profile");
    fs.mkdir_all(b"/h/u/.config");
    fs.add_symlink(b"/h/u/.config/google-chrome", b"/tmp/profile");
    fs.mkdir_all(b"/tmp/keys");
    fs.add_symlink(b"/h/u/.ssh", b"/tmp/keys");
    let prot = default_build(&fs);

    // File-tool paths.
    for p in ["~/.config/google-chrome/Cookies", "~/.ssh/id_rsa"] {
        let r = subject(p, AccessMode::Read, &prot, &fs);
        assert!(
            has_hard_deny(&r.outcomes, "credential-read"),
            "{p}: {:?}",
            r.outcomes
        );
    }
    // dev's exceptions stay: a public `.ssh` name, a readable `config`.
    for p in ["~/.ssh/id_rsa.pub", "~/.ssh/config"] {
        let r = subject(p, AccessMode::Read, &prot, &fs);
        assert!(!has_any_hard_deny(&r.outcomes), "{p}: {:?}", r.outcomes);
    }
    assert!(has_hard_deny(
        &subject("~/.ssh/config", AccessMode::Write, &prot, &fs).outcomes,
        "credential-read"
    ));

    // Command operands, a path relative to a `cd` into the aliased root (the
    // written working directory), and a redirect. The redirect's writer comes
    // last: a `cd` after a possible writer makes the cwd unknown (§5).
    let mut cd_eff = reader_effects("cd", 2);
    cd_eff.cd_target_operand = Some(1);
    let units = [
        unit(
            0,
            vec![
                unquoted(b"cat"),
                unquoted(b"~/.config/google-chrome/Cookies"),
            ],
            vec![],
            Separator::Start,
            None,
        ),
        unit(
            1,
            vec![unquoted(b"cd"), unquoted(b"~/.config/google-chrome")],
            vec![],
            Separator::Semi,
            None,
        ),
        unit(
            2,
            vec![unquoted(b"cat"), unquoted(b"Cookies")],
            vec![],
            Separator::Semi,
            None,
        ),
        unit(
            3,
            vec![unquoted(b"echo"), unquoted(b"x")],
            vec![ModelRedirect::Write {
                target: unquoted(b"~/.ssh/id_ed25519"),
            }],
            Separator::Semi,
            None,
        ),
    ];
    let res = resolve_command_targets(
        &units,
        &[
            reader_effects("cat", 2),
            cd_eff,
            reader_effects("cat", 2),
            reader_effects("echo", 2),
        ],
        CWD,
        &prot,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .unwrap();
    for part in [
        PartId::Operand {
            unit: UnitIndex(0),
            argv_index: 1,
        },
        PartId::Operand {
            unit: UnitIndex(2),
            argv_index: 1,
        },
        PartId::Redirect {
            unit: UnitIndex(3),
            redirect_index: 0,
        },
    ] {
        assert!(
            res.outcomes
                .iter()
                .any(|r| r.part == part && matches!(r.outcome, PartOutcome::HardDenyHit(_))),
            "{part:?}: {:?}",
            res.outcomes
        );
    }
}

#[test]
fn round6_f1_glob_matches_are_checked_on_their_written_spelling() {
    let mut fs = FakeFs::new();
    fs.add_file(b"/tmp/profile/Cookies");
    fs.mkdir_all(b"/h/u/.config");
    fs.add_symlink(b"/h/u/.config/google-chrome", b"/tmp/profile");
    let prot = default_build(&fs);
    let res = resolve_command_targets(
        &[unit(
            0,
            vec![unquoted(b"cat"), unquoted(b"~/.config/google-chrome/Cook*")],
            vec![],
            Separator::Start,
            None,
        )],
        &[reader_effects("cat", 2)],
        CWD,
        &prot,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .unwrap();
    assert!(
        has_hard_deny(&res.outcomes, "credential-read"),
        "{:?}",
        res.outcomes
    );
}

#[test]
fn round6_f2_s8_walks_linked_browser_profile_folders() {
    // Review 4, finding 2: `rg -g '*.rs' x ~/.config/google-chrome` with a
    // linked `Default` profile holding `Cookies`: S8 is HardDeny whatever the
    // filter (§5, D-145 keeps walking linked folders).
    let mut fs = FakeFs::new();
    fs.add_file(b"/tmp/profile/Cookies");
    fs.mkdir_all(b"/h/u/.config/google-chrome");
    fs.add_symlink(b"/h/u/.config/google-chrome/Default", b"/tmp/profile");
    let prot = default_build(&fs);
    let root = literal(b"~/.config/google-chrome", AccessMode::Read, &fs);
    let filter = SearchFilter {
        dialect: FilterDialect::RgGlob,
        pattern: unquoted(b"*.rs"),
        case_insensitive: false,
        argv_index: Some(1),
    };
    let res = check_recursive_root(
        &root,
        LinkPolicy::default(),
        &[filter],
        &prot,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .unwrap();
    assert!(
        has_hard_deny(&res.outcomes, "credential-read"),
        "{:?}",
        res.outcomes
    );

    // A link cycle inside the profile ends.
    fs.add_symlink(b"/tmp/profile/loop", b"/h/u/.config/google-chrome");
    let prot = default_build(&fs);
    let res = check_recursive_root(
        &literal(b"~/.config/google-chrome", AccessMode::Read, &fs),
        LinkPolicy::default(),
        &[],
        &prot,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut ComputeBudget::with_default_limits(),
    )
    .unwrap();
    assert!(has_hard_deny(&res.outcomes, "credential-read"));
}

#[test]
fn round6_f3_root_link_with_unknown_referent_is_any_target() {
    // Review 4, finding 3: the link's own identity does not establish its
    // referent's, so the gap is AnyTarget (D-145 rule 4). Here the referent
    // cannot be stat'ed.
    let mut fs = FakeFs::new();
    fs.add_file(b"/tmp/token");
    fs.add_symlink(b"/h/u/.azure/msal_token_cache.json", b"/tmp/token");
    fs.set_stat_error(b"/tmp/token", FsIoError::PermissionDenied);
    fs.add_file(b"/h/u/proj/readme");
    let prot = default_build(&fs);
    assert!(
        prot.identity_gaps
            .iter()
            .any(|g| g.scope == GapScope::AnyTarget && !g.write_only),
        "{:?}",
        prot.identity_gaps
    );
    assert!(
        !prot
            .identity_gaps
            .iter()
            .any(|g| matches!(g.scope, GapScope::Through(_)))
    );
    assert!(matches!(
        read_outcome(b"proj/readme", &prot, &fs),
        PartOutcome::Unread(UnreadReason::UnresolvablePath { .. })
    ));
}

#[test]
fn round6_f4_named_store_root_after_many_siblings_is_found() {
    // Review 4, finding 4: 4,096 earlier ordinary files, then the named root.
    let mut fs = FakeFs::new();
    for i in 0..limits::WALK_MAX_ENTRIES {
        fs.add_file(format!("/h/u/.azure/a{i:05}").as_bytes());
    }
    fs.add_file(b"/tmp/token");
    fs.add_symlink(b"/h/u/.azure/msal_token_cache.json", b"/tmp/token");
    let prot = default_build(&fs);
    // The protected spelling: a Store hit (by name, and by the root's identity).
    let r = subject(
        "~/.azure/msal_token_cache.json",
        AccessMode::Read,
        &prot,
        &fs,
    );
    assert!(has_store_hit(&r.outcomes), "{:?}", r.outcomes);
    assert!(matches!(
        read_outcome(b"~/.azure/msal_token_cache.json", &prot, &fs),
        PartOutcome::StoreHit(_)
    ));
}

#[test]
fn round6_f5_subject_resolution_charges_before_allocating() {
    // Review 4, finding 5.
    let fs = FakeFs::new();
    let prot = default_build(&fs);
    let mut budget = ComputeBudget::new(1, 0);
    let res = resolve_subject_paths(
        &[],
        None,
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut budget,
    )
    .expect("nothing to resolve, nothing to allocate");
    assert!(res.outcomes.is_empty());
    assert_eq!(budget.remaining_alloc_bytes(), 0);
    assert!(!budget.is_exhausted());

    // A relative filter's buffers are charged before they are built.
    let mut budget = ComputeBudget::new(1_000_000, 7);
    let err = resolve_subject_paths(
        &[],
        Some("*.rs"),
        false,
        AccessMode::Read,
        &prot,
        &fs,
        &mut FsCaps::with_default_limits(),
        &mut budget,
    )
    .unwrap_err();
    assert_eq!(err.reason, ExhaustReason::AllocationBudget);
}

#[test]
fn round6_f6_mv_of_a_link_to_a_directory_moves_the_link() {
    // Review 4, finding 6: `mv` moves the link itself, which is not a
    // directory, so there are no derived destinations.
    let mut fs = FakeFs::new();
    fs.mkdir_all(b"/h/u/directory");
    fs.add_symlink(b"/h/u/link", b"/h/u/directory");
    fs.mkdir_all(b"/h/u/destination");
    let prot = default_build(&fs);
    let eff = ToolEffects {
        tool_name: Some("mv"),
        subcommand: None,
        roles: vec![OperandRole::Command, OperandRole::Path, OperandRole::Path],
        operand_access: vec![
            AccessMode::Read,
            AccessMode::ReadWrite,
            AccessMode::ReadWrite,
        ],
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
    let run = |src: &[u8]| {
        let res = resolve_command_targets(
            &[unit(
                0,
                vec![unquoted(b"mv"), unquoted(src), unquoted(b"destination")],
                vec![],
                Separator::Start,
                None,
            )],
            std::slice::from_ref(&eff),
            CWD,
            &prot,
            &fs,
            &mut FsCaps::with_default_limits(),
            &mut ComputeBudget::with_default_limits(),
        )
        .unwrap();
        has_unread(&res.outcomes, |r| {
            matches!(r, UnreadReason::DerivedDestinations { .. })
        })
    };
    assert!(!run(b"link"));
    assert!(run(b"directory"));
}
