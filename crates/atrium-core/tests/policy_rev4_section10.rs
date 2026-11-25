//! Contract rev 4 §10 normative example table test suite (all 26 rows).
//!
//! Runs every row end-to-end through `SessionGate` with real temp-dir fixtures
//! (fake home, never reading real credential paths) across all four settings:
//! - Home (no rules)
//! - Prefix or exact rule
//! - Unscoped Any rule
//! - FullAccess
//!
//! Also verifies §9's `ApprovalTicket.unread` (`UnreadNotice::Unread` / `None`)
//! and `rememberable == false` invariants on every `Ask` row.

mod common_adversarial;

use atrium_agents::{GateDecision, ToolCheck, ToolGate};
use atrium_core::policy::validate_rule_matcher;
use atrium_protocol::agent::{ToolKind, UnreadNotice};
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope, ToolSubject};
use common_adversarial::{Tmp, block_on, cmd_check, exact, paths_check, prefix, rule};

fn allow_any(kind: ToolKind, id: &str) -> Rule {
    rule(
        RuleScope::Global,
        kind,
        RuleMatcher::Any,
        RuleDecision::Allow,
        id,
    )
}

fn allow_prefix(argv: &[&str], id: &str) -> Rule {
    rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(argv),
        RuleDecision::Allow,
        id,
    )
}

fn allow_exact(argv: &[&str], id: &str) -> Rule {
    rule(
        RuleScope::Global,
        ToolKind::Execute,
        exact(argv),
        RuleDecision::Allow,
        id,
    )
}

fn allow_path_glob(kind: ToolKind, glob: String, id: &str) -> Rule {
    rule(
        RuleScope::Global,
        kind,
        RuleMatcher::PathGlob { glob },
        RuleDecision::Allow,
        id,
    )
}

fn check_gate(t: &Tmp, preset: Preset, rules: Vec<Rule>, check: &ToolCheck) -> GateDecision {
    let g = t.gate(preset, rules);
    block_on(g.gate.check(check))
}

fn assert_allow(d: GateDecision, ctx: &str) {
    assert_eq!(d, GateDecision::Allow, "{ctx}");
}

fn assert_hard_deny(d: GateDecision, expected_id: &str, ctx: &str) {
    match d {
        GateDecision::Deny { reason } => {
            assert!(
                reason.contains(expected_id),
                "{ctx}: expected hard-deny id `{expected_id}`, got `{reason}`"
            );
        }
        other => panic!("{ctx}: expected Deny({expected_id}), got {other:?}"),
    }
}

fn assert_deny_reason(d: GateDecision, needle: &str, ctx: &str) {
    match d {
        GateDecision::Deny { reason } => {
            assert!(
                reason.contains(needle),
                "{ctx}: expected deny containing `{needle}`, got `{reason}`"
            );
        }
        other => panic!("{ctx}: expected Deny({needle}), got {other:?}"),
    }
}

fn assert_ask_unread(d: GateDecision, ctx: &str) {
    match d {
        GateDecision::Ask { ticket } => {
            assert_eq!(
                ticket.unread,
                Some(UnreadNotice::Unread),
                "{ctx}: expected UnreadNotice::Unread (§9)"
            );
            assert!(
                !ticket.rememberable,
                "{ctx}: unread Ask must hide 'always...' (§9)"
            );
        }
        other => panic!("{ctx}: expected Ask(Unread), got {other:?}"),
    }
}

fn assert_ask_readable(d: GateDecision, ctx: &str) {
    match d {
        GateDecision::Ask { ticket } => {
            assert_eq!(
                ticket.unread, None,
                "{ctx}: expected readable Ask without UnreadNotice"
            );
        }
        other => panic!("{ctx}: expected Ask(None), got {other:?}"),
    }
}

/// Checks a call across all four §10 columns when the expected matrix is
/// `Allow / Allow / Allow / Allow`.
fn assert_allow_everywhere(t: &Tmp, check: &ToolCheck, covering_rule: Rule, ctx: &str) {
    assert_allow(
        check_gate(t, Preset::Home, vec![], check),
        &format!("{ctx} [Home]"),
    );
    assert_allow(
        check_gate(t, Preset::Home, vec![covering_rule], check),
        &format!("{ctx} [Rule]"),
    );
    assert_allow(
        check_gate(t, Preset::Home, vec![allow_any(check.kind, "any")], check),
        &format!("{ctx} [Any]"),
    );
    assert_allow(
        check_gate(t, Preset::FullAccess, vec![], check),
        &format!("{ctx} [FullAccess]"),
    );
}

/// Checks a call across all four §10 columns when the expected matrix is
/// `HardDeny(id) everywhere`.
fn assert_hard_deny_everywhere(
    t: &Tmp,
    check: &ToolCheck,
    covering_rule: Option<Rule>,
    expected_id: &str,
    ctx: &str,
) {
    assert_hard_deny(
        check_gate(t, Preset::Home, vec![], check),
        expected_id,
        &format!("{ctx} [Home]"),
    );
    if let Some(r) = covering_rule {
        assert_hard_deny(
            check_gate(t, Preset::Home, vec![r], check),
            expected_id,
            &format!("{ctx} [Rule]"),
        );
    }
    assert_hard_deny(
        check_gate(t, Preset::Home, vec![allow_any(check.kind, "any")], check),
        expected_id,
        &format!("{ctx} [Any]"),
    );
    assert_hard_deny(
        check_gate(t, Preset::FullAccess, vec![], check),
        expected_id,
        &format!("{ctx} [FullAccess]"),
    );
}

/// Checks a call across all four §10 columns when the expected matrix is
/// `Ask(Unread) / Ask(Unread) / Allow / Allow`.
fn assert_unread_matrix(t: &Tmp, check: &ToolCheck, prefix_rule: Rule, ctx: &str) {
    assert_ask_unread(
        check_gate(t, Preset::Home, vec![], check),
        &format!("{ctx} [Home]"),
    );
    assert_ask_unread(
        check_gate(t, Preset::Home, vec![prefix_rule], check),
        &format!("{ctx} [Rule]"),
    );
    assert_allow(
        check_gate(t, Preset::Home, vec![allow_any(check.kind, "any")], check),
        &format!("{ctx} [Any]"),
    );
    assert_allow(
        check_gate(t, Preset::FullAccess, vec![], check),
        &format!("{ctx} [FullAccess]"),
    );
}

// ============================================================================
// §10 Rows 1–7
// ============================================================================

#[test]
fn section10_row01_cat_readme_allow_everywhere() {
    let t = Tmp::new();
    t.write("README.md", "hello\n");
    let c = cmd_check("cat README.md");
    assert_allow_everywhere(&t, &c, allow_prefix(&["cat"], "p"), "row 1");
}

#[test]
fn section10_row02_cat_ssh_id_rsa_hard_deny_everywhere() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "PRIVATE KEY");
    let c = cmd_check("cat ~/.ssh/id_rsa");
    assert_hard_deny_everywhere(
        &t,
        &c,
        Some(allow_prefix(&["cat"], "p")),
        "credential-read",
        "row 2",
    );
}

#[test]
fn section10_row03_folded_sudo_hard_deny_everywhere() {
    let t = Tmp::new();
    for line in ["SUDO id", "/usr/bin/sudo id"] {
        let c = cmd_check(line);
        assert_hard_deny_everywhere(
            &t,
            &c,
            Some(allow_prefix(&["sudo"], "p")),
            "privilege-escalation",
            &format!("row 3: {line}"),
        );
    }
}

#[test]
fn section10_row04_sh_c_with_positional_and_sudo_hard_deny_everywhere() {
    let t = Tmp::new();
    let c = cmd_check("sh -c 'X=\"$1\" sudo id' _ foo");
    assert_hard_deny_everywhere(&t, &c, None, "privilege-escalation", "row 4");
}

#[test]
fn section10_row05_max_wrap_reached_unread_and_best_effort() {
    let t = Tmp::new();
    // 13 `command` wrappers (> MAX_WRAP = 8) wrapping a benign command:
    // pure `Ask / Ask / Allow / Allow` with `UnreadNotice::Unread`.
    let benign = format!("{}echo ok", "command ".repeat(13));
    let c_benign = cmd_check(&benign);
    assert_unread_matrix(
        &t,
        &c_benign,
        allow_prefix(&["command"], "p"),
        "row 5 (benign)",
    );

    // 13 `command` wrappers around `sudo id` (§10 row 5 exact example):
    // In Home & under prefix rule: never Allow (Ask(Unread) or best-effort HardDeny).
    // Under Any & FullAccess: Allow* (HardDeny if best-effort hits `sudo`, else Allow).
    let sudo_wrapped = format!("{}sudo id", "command ".repeat(13));
    let c_sudo = cmd_check(&sudo_wrapped);
    for (label, preset, rules) in [
        ("Home", Preset::Home, vec![]),
        (
            "Prefix",
            Preset::Home,
            vec![allow_prefix(&["command"], "p")],
        ),
    ] {
        match check_gate(&t, preset, rules, &c_sudo) {
            GateDecision::Deny { reason } => {
                assert!(
                    reason.contains("privilege-escalation"),
                    "row 5 [{label}]: unexpected deny `{reason}`"
                );
            }
            GateDecision::Ask { ticket } => {
                assert_eq!(ticket.unread, Some(UnreadNotice::Unread));
                assert!(!ticket.rememberable);
            }
            GateDecision::Allow => {
                panic!("row 5 [{label}]: must never Allow `command` x13 `sudo id`");
            }
        }
    }
    for (label, preset, rules) in [
        ("Any", Preset::Home, vec![allow_any(ToolKind::Execute, "a")]),
        ("FullAccess", Preset::FullAccess, vec![]),
    ] {
        match check_gate(&t, preset, rules, &c_sudo) {
            GateDecision::Deny { reason } => {
                assert!(
                    reason.contains("privilege-escalation"),
                    "row 5 [{label}]: unexpected deny `{reason}`"
                );
            }
            GateDecision::Allow => {}
            GateDecision::Ask { ticket } => {
                panic!("row 5 [{label}]: expected HardDeny or Allow*, got Ask({ticket:?})");
            }
        }
    }
}

#[test]
fn section10_row06_ansi_c_quoting_unread() {
    let t = Tmp::new();
    let c = cmd_check(r"cat $'\xff'");
    assert_unread_matrix(&t, &c, allow_prefix(&["cat"], "p"), "row 6");
}

#[test]
fn section10_row07_for_loop_reserved_word_unread() {
    let t = Tmp::new();
    let c = cmd_check("for f in *.rs; do wc -l \"$f\"; done");
    assert_unread_matrix(&t, &c, allow_prefix(&["for"], "p"), "row 7");
}

// ============================================================================
// §10 Rows 8–11
// ============================================================================

#[cfg(unix)]
#[test]
fn section10_row08_shell_glob_through_symlink_to_ssh_key_hard_deny() {
    use std::os::unix::fs::symlink;
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "PRIVATE KEY");
    std::fs::create_dir_all(t.home.join("store[1]")).unwrap();
    symlink(t.home.join("store[1]"), t.home.join("proj")).unwrap();
    symlink("../.ssh/id_rsa", t.home.join("store[1]/key")).unwrap();

    let c = cmd_check("cat proj/key*");
    assert_hard_deny_everywhere(
        &t,
        &c,
        Some(allow_prefix(&["cat"], "p")),
        "credential-read",
        "row 8",
    );
}

#[cfg(unix)]
#[test]
fn section10_row09_quoted_literal_with_star_not_a_glob() {
    use std::os::unix::fs::symlink;
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "PRIVATE KEY");
    t.write("proj/key*", "ordinary literal file\n");
    symlink(t.home.join(".ssh/id_rsa"), t.home.join("proj/key1")).unwrap();

    let c = cmd_check("cat 'proj/key*'");
    assert_allow_everywhere(&t, &c, allow_prefix(&["cat"], "p"), "row 9");
}

#[test]
fn section10_row10_unquoted_non_ascii_shell_word_unread() {
    let t = Tmp::new();
    t.write("[!é]trium/secret", "ordinary\n");
    let line = format!("cat {}/[!é]trium/secret", t.home.display());
    let c = cmd_check(&line);
    assert_unread_matrix(&t, &c, allow_prefix(&["cat"], "p"), "row 10");
}

#[test]
fn section10_row11_read_tool_literal_path_with_brackets_and_non_ascii() {
    let t = Tmp::new();
    t.write("[!é]trium/secret", "ordinary\n");
    let path = format!("{}/[!é]trium/secret", t.home.display());
    let c = paths_check(ToolKind::Read, "Read", &[&path], false, None);
    let covering = allow_path_glob(
        ToolKind::Read,
        format!("{}/**", t.home.display()),
        "read-home",
    );
    assert_allow_everywhere(&t, &c, covering, "row 11");
}

// ============================================================================
// §10 Rows 12–18
// ============================================================================

#[test]
fn section10_row12_search_azure_msal_existing_store_hit() {
    let t = Tmp::new();
    t.write(".azure/msal_token_cache.json", "{}");
    let c = paths_check(ToolKind::Search, "Grep", &["~/.azure"], true, Some("msal*"));
    let h = t.home.display();
    let covering = allow_path_glob(
        ToolKind::Search,
        format!("{h}/{{.azure,.azure/**}}"),
        "allow-azure",
    );

    assert_ask_readable(check_gate(&t, Preset::Home, vec![], &c), "row 12 [Home]");
    assert_allow(
        check_gate(&t, Preset::Home, vec![covering], &c),
        "row 12 [Rule]",
    );
    assert_allow(
        check_gate(&t, Preset::Home, vec![allow_any(ToolKind::Search, "a")], &c),
        "row 12 [Any]",
    );
    assert_allow(
        check_gate(&t, Preset::FullAccess, vec![], &c),
        "row 12 [FullAccess]",
    );
}

#[test]
fn section10_row13_awk_inline_code_unread_and_prefix_rule_invalid() {
    let t = Tmp::new();
    let awk_prefix = prefix(&["awk"]);
    assert!(
        validate_rule_matcher(&awk_prefix, RuleDecision::Allow, None).is_err(),
        "row 13: Allow prefix `[\"awk\"]` must be Invalid (§7, §11 item 4)"
    );

    let c = cmd_check("awk 'BEGIN{system(\"touch v\")}'");
    assert_unread_matrix(&t, &c, allow_prefix(&["awk"], "invalid-awk"), "row 13");
}

#[test]
fn section10_row14_python_pinned_program_covered_by_exact_rule() {
    let t = Tmp::new();
    t.write("build.py", "print('ok')\n");
    let c = cmd_check("python build.py");

    // Home without rule: Unread (program) -> Ask with UnreadNotice & !rememberable.
    assert_ask_unread(check_gate(&t, Preset::Home, vec![], &c), "row 14 [Home]");
    // Exact rule naming `["python", "build.py"]`: Allow (§7, §11 item 8).
    assert_allow(
        check_gate(
            &t,
            Preset::Home,
            vec![allow_exact(&["python", "build.py"], "py-build")],
            &c,
        ),
        "row 14 [ExactRule]",
    );
    // Any rule: Allow.
    assert_allow(
        check_gate(
            &t,
            Preset::Home,
            vec![allow_any(ToolKind::Execute, "a")],
            &c,
        ),
        "row 14 [Any]",
    );
    // FullAccess: Allow.
    assert_allow(
        check_gate(&t, Preset::FullAccess, vec![], &c),
        "row 14 [FullAccess]",
    );
}

#[test]
fn section10_row15_rg_follow_links_clean_proj_allow_everywhere() {
    let t = Tmp::new();
    t.write("proj/a.txt", "hello x\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink("a.txt", t.home.join("proj/link.txt")).unwrap();

    let c = cmd_check("rg -L x proj");
    assert_allow_everywhere(&t, &c, allow_prefix(&["rg"], "p"), "row 15");
}

#[test]
fn section10_row16_s8_aws_hard_deny_vs_d133_config_and_home_root() {
    let t = Tmp::new();
    t.write(".aws/credentials", "[default]\naws_secret_access_key=x\n");
    t.write(".config/app/note.txt", "hello\n");

    // 1. `rg x ~/.aws` -> HardDeny everywhere (S8, §11 item 12).
    let c_aws = cmd_check("rg x ~/.aws");
    assert_hard_deny_everywhere(
        &t,
        &c_aws,
        Some(allow_prefix(&["rg"], "p")),
        "credential-read",
        "row 16 (rg x ~/.aws)",
    );

    // 2. `rg x ~/.config` -> Ask / Allow / Allow / Allow (D-133).
    let c_cfg = cmd_check("rg x ~/.config");
    assert_ask_readable(
        check_gate(&t, Preset::Home, vec![], &c_cfg),
        "row 16 (rg x ~/.config) [Home]",
    );
    assert_allow(
        check_gate(&t, Preset::Home, vec![allow_prefix(&["rg"], "p")], &c_cfg),
        "row 16 (rg x ~/.config) [Prefix]",
    );
    assert_allow(
        check_gate(
            &t,
            Preset::Home,
            vec![allow_any(ToolKind::Execute, "a")],
            &c_cfg,
        ),
        "row 16 (rg x ~/.config) [Any]",
    );
    assert_allow(
        check_gate(&t, Preset::FullAccess, vec![], &c_cfg),
        "row 16 (rg x ~/.config) [FullAccess]",
    );

    // 3. `rg x ~` -> Ask / Allow / Allow / Allow (home root exception).
    let c_home = cmd_check("rg x ~");
    assert_ask_readable(
        check_gate(&t, Preset::Home, vec![], &c_home),
        "row 16 (rg x ~) [Home]",
    );
    assert_allow(
        check_gate(&t, Preset::Home, vec![allow_prefix(&["rg"], "p")], &c_home),
        "row 16 (rg x ~) [Prefix]",
    );
    assert_allow(
        check_gate(
            &t,
            Preset::Home,
            vec![allow_any(ToolKind::Execute, "a")],
            &c_home,
        ),
        "row 16 (rg x ~) [Any]",
    );
    assert_allow(
        check_gate(&t, Preset::FullAccess, vec![], &c_home),
        "row 16 (rg x ~) [FullAccess]",
    );
}

#[test]
fn section10_row17_ln_symlink_to_ssh_key_hard_deny_everywhere() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "PRIVATE KEY");
    let c = cmd_check("ln -s ~/.ssh/id_rsa x; cat x");
    assert_hard_deny_everywhere(
        &t,
        &c,
        Some(allow_prefix(&["ln"], "p")),
        "credential-read",
        "row 17",
    );
}

#[cfg(unix)]
#[test]
fn section10_row18_cp_recursive_derived_destinations_unread() {
    use std::os::unix::fs::symlink;
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "PRIVATE KEY");
    t.write("src/plain", "payload");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    symlink(t.home.join(".ssh/id_rsa"), t.home.join("proj/plain")).unwrap();

    let c = cmd_check("cp -R src/. proj");
    assert_unread_matrix(&t, &c, allow_prefix(&["cp"], "p"), "row 18");
}

// ============================================================================
// §10 Rows 19–26 (numbering follows contract §10 table)
// ============================================================================

#[test]
fn section10_row19_deny_rule_matches_best_effort_unit() {
    let t = Tmp::new();
    let deny_cat = rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["cat"]),
        RuleDecision::Deny,
        "deny-cat",
    );
    let c = cmd_check("cat \"$X\"");

    for (label, preset, rules) in [
        ("Home", Preset::Home, vec![deny_cat.clone()]),
        ("FullAccess", Preset::FullAccess, vec![deny_cat.clone()]),
        (
            "SameRankAllowDoesNotBeatDeny",
            Preset::Home,
            vec![deny_cat.clone(), allow_any(ToolKind::Execute, "a")],
        ),
    ] {
        assert_deny_reason(
            check_gate(&t, preset, rules, &c),
            "denied",
            &format!("row 19 [{label}]"),
        );
    }
}

#[test]
fn section10_row20_over_max_units_too_large_deny_everywhere() {
    let t = Tmp::new();
    let line = "cat a; ".repeat(600);
    let c = cmd_check(&line);

    for (label, preset, rules) in [
        ("Home", Preset::Home, vec![]),
        ("Prefix", Preset::Home, vec![allow_prefix(&["cat"], "p")]),
        ("Any", Preset::Home, vec![allow_any(ToolKind::Execute, "a")]),
        ("FullAccess", Preset::FullAccess, vec![]),
    ] {
        assert_deny_reason(
            check_gate(&t, preset, rules, &c),
            "too large to check",
            &format!("row 20 [{label}]"),
        );
    }
}

#[test]
fn section10_row21_pipeline_with_possible_writer_unread() {
    let t = Tmp::new();
    t.write("a", "data\n");
    t.write("mutate.py", "print('x')\n");
    let c = cmd_check("cat a | python mutate.py");
    let covering_both = vec![
        allow_exact(&["cat", "a"], "allow-cat"),
        allow_exact(&["python", "mutate.py"], "allow-py"),
    ];

    assert_ask_unread(check_gate(&t, Preset::Home, vec![], &c), "row 21 [Home]");
    assert_ask_unread(
        check_gate(&t, Preset::Home, covering_both, &c),
        "row 21 [RulesCoveringBoth]",
    );
    assert_allow(
        check_gate(
            &t,
            Preset::Home,
            vec![allow_any(ToolKind::Execute, "a")],
            &c,
        ),
        "row 21 [Any]",
    );
    assert_allow(
        check_gate(&t, Preset::FullAccess, vec![], &c),
        "row 21 [FullAccess]",
    );
}

#[test]
fn section10_row22_dev_null_redirect_not_a_writer() {
    let t = Tmp::new();
    t.write("README.md", "r\n");
    t.write("LICENSE", "l\n");
    let c = cmd_check("cat README.md >/dev/null; cat LICENSE");
    assert_allow_everywhere(&t, &c, allow_prefix(&["cat"], "p"), "row 22");
}

#[cfg(unix)]
#[test]
fn section10_row23_glob_tool_resolves_selected_symlink_to_ssh_key() {
    use std::os::unix::fs::symlink;
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "PRIVATE KEY");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    symlink(t.home.join(".ssh/id_rsa"), t.home.join("proj/key")).unwrap();

    let c = paths_check(ToolKind::Search, "Glob", &["proj"], true, Some("key*"));
    let covering = allow_path_glob(
        ToolKind::Search,
        format!("{}/proj/**", t.home.display()),
        "allow-proj",
    );
    assert_hard_deny_everywhere(&t, &c, Some(covering), "credential-read", "row 23");
}

#[test]
fn section10_row24_search_azure_msal_absent_probe_store_hit() {
    let t = Tmp::new();
    std::fs::create_dir_all(t.home.join(".azure")).unwrap();
    let c = paths_check(ToolKind::Search, "Grep", &["~/.azure"], true, Some("msal*"));
    let h = t.home.display();
    let covering = allow_path_glob(
        ToolKind::Search,
        format!("{h}/{{.azure,.azure/**}}"),
        "allow-azure",
    );

    assert_ask_readable(check_gate(&t, Preset::Home, vec![], &c), "row 24 [Home]");
    assert_allow(
        check_gate(&t, Preset::Home, vec![covering], &c),
        "row 24 [Rule]",
    );
    assert_allow(
        check_gate(&t, Preset::Home, vec![allow_any(ToolKind::Search, "a")], &c),
        "row 24 [Any]",
    );
    assert_allow(
        check_gate(&t, Preset::FullAccess, vec![], &c),
        "row 24 [FullAccess]",
    );
}

#[test]
fn section10_row25_descriptor_3_unread() {
    let t = Tmp::new();
    let c = cmd_check("cat <&3");
    assert_unread_matrix(&t, &c, allow_prefix(&["cat"], "p"), "row 25");
}

#[test]
fn section10_row26_thread_allow_outranks_global_deny() {
    let t = Tmp::new();
    t.write("README.md", "hi\n");
    let global_deny = rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["cat"]),
        RuleDecision::Deny,
        "global-deny",
    );
    let thread_allow = rule(
        RuleScope::Thread {
            thread_id: atrium_protocol::thread::ThreadId("t1".into()),
        },
        ToolKind::Execute,
        prefix(&["cat"]),
        RuleDecision::Allow,
        "thread-allow",
    );
    let c = cmd_check("cat README.md");
    assert_allow(
        check_gate(&t, Preset::Home, vec![global_deny, thread_allow], &c),
        "row 26 [thread Allow beats global Deny]",
    );
}

#[test]
fn section10_unknown_subject_exhausted_notice_and_ask_everywhere() {
    let t = Tmp::new();
    let c = ToolCheck {
        call_id: None,
        name: "Custom".into(),
        kind: ToolKind::Other,
        title: None,
        input: serde_json::json!({"note": "hello"}),
        subject: ToolSubject::Unknown,
        turn: 1,
    };
    for (label, preset, rules) in [
        ("Home", Preset::Home, vec![]),
        ("Any", Preset::Home, vec![allow_any(ToolKind::Other, "a")]),
        ("FullAccess", Preset::FullAccess, vec![]),
    ] {
        match check_gate(&t, preset, rules, &c) {
            GateDecision::Ask { ticket } => {
                assert_eq!(
                    ticket.unread,
                    Some(UnreadNotice::Exhausted),
                    "Unknown [{label}]: expected UnreadNotice::Exhausted (§1, §9)"
                );
                assert!(
                    !ticket.rememberable,
                    "Unknown [{label}]: exhausted Ask must hide 'always...' (§9)"
                );
            }
            other => panic!("Unknown [{label}]: expected Ask(Exhausted), got {other:?}"),
        }
    }
}
