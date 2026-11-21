//! M1-32-link-globs: globs and following searches that reach credentials or
//! ordinary folders through symlinks (S11).

mod common_adversarial;

use std::os::unix::fs::{PermissionsExt, symlink};

use atrium_agents::{GateDecision, ToolCheck, ToolGate};
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope};
use common_adversarial::{Tmp, block_on, cmd_check, paths_check, rule};

fn any_rule(kind: ToolKind) -> Rule {
    rule(
        RuleScope::Global,
        kind,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any",
    )
}

fn gate_short(d: &GateDecision) -> &'static str {
    match d {
        GateDecision::Allow => "Allow",
        GateDecision::Ask { .. } => "Ask",
        GateDecision::Deny { .. } => "Deny",
    }
}

fn world() -> Tmp {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write(".ssh/sub/inner_key", "secret");
    t.write(".npmrc", "token");
    t.write(".aws/credentials", "secret");
    t.write(".local/share/atrium/atrium.db", "db");
    t.write(".config/atrium/config.toml", "cfg");
    t.write("proj/a.txt", "ordinary");
    t.write("proj/sub/b.txt", "ordinary");
    t.write("other/sub/mid.txt", "ordinary");
    t.write("elsewhere/deep/x.txt", "ordinary");
    t.write("proj_clean/a.txt", "ordinary");
    t.write("other_proj/b.txt", "ordinary");
    t.write("other_proj/sub/c.txt", "ordinary");

    let h = &t.home;

    // One-hop links into credentials
    symlink(h.join(".ssh"), h.join("proj/link-ssh")).unwrap();
    symlink(h.join(".ssh/id_rsa"), h.join("proj/link-key")).unwrap();
    symlink(h.join(".ssh/sub"), h.join("proj/link-sub")).unwrap();
    symlink(h.join(".aws"), h.join("proj/link-aws")).unwrap();

    // Two-hop links into credentials and ancestor
    symlink(h.join("proj/link-ssh"), h.join("proj/hop2-ssh")).unwrap();
    symlink(h.join("proj/link-key"), h.join("proj/hop2-key")).unwrap();
    symlink(h.join("other/sub"), h.join("proj/hop2-mid")).unwrap();
    symlink(h.join(".ssh"), h.join("other/sub/to_ssh")).unwrap();
    symlink(h, h.join("other/sub/to_home")).unwrap();

    // Dangling links (one-hop, two-hop, and relative inside a symlinked dir)
    symlink(h.join(".ssh/missing_key"), h.join("proj/dangling-key")).unwrap();
    symlink(h.join("proj/dangling-key"), h.join("proj/dangling-2hop")).unwrap();
    let up = "..";
    symlink(format!("{up}/missing_rel"), h.join(".ssh/sub/dangling_rel")).unwrap();

    // Links to ancestors of a credential tree (and of proj itself)
    symlink(h, h.join("proj/link-home")).unwrap();
    symlink(h.join("proj/link-home"), h.join("proj/link-home2")).unwrap();

    // Link loops
    symlink(h.join("proj/loop-b"), h.join("proj/loop-a")).unwrap();
    symlink(h.join("proj/loop-a"), h.join("proj/loop-b")).unwrap();

    // Subfolder for follow-flag searches
    t.write("proj_sub/plain.txt", "ordinary");
    symlink(h.join(".ssh"), h.join("proj_sub/to_ssh")).unwrap();
    t.write("proj_anc_sub/plain.txt", "ordinary");
    symlink(h, h.join("proj_anc_sub/to_home")).unwrap();

    // Ordinary project links (no credentials reachable)
    symlink(h.join("elsewhere/deep"), h.join("proj/link-deep")).unwrap();
    symlink(h.join("other_proj"), h.join("proj_clean/to_other")).unwrap();
    symlink(
        h.join("other_proj/b.txt"),
        h.join("proj_clean/to_other_file"),
    )
    .unwrap();

    t
}

fn eval_cmd_3(t: &Tmp, line: &str) -> [&'static str; 3] {
    let (cwd, cmd) = match line.split_once('|') {
        Some((c, rest)) if c.starts_with("cwd=") => (t.home.join(&c[4..]), rest),
        _ => (t.home.clone(), line),
    };
    let settings = [
        (Preset::Home, vec![]),
        (Preset::Home, vec![any_rule(ToolKind::Execute)]),
        (Preset::FullAccess, vec![]),
    ];
    let mut out = [""; 3];
    for (idx, (preset, rules)) in settings.into_iter().enumerate() {
        let g = t.gate_with("t1", 1, &cwd, preset, rules, None);
        let d = block_on(g.gate.check(&cmd_check(cmd)));
        out[idx] = gate_short(&d);
    }
    out
}

fn eval_check_3(t: &Tmp, check: &ToolCheck) -> [&'static str; 3] {
    let settings = [
        (Preset::Home, vec![]),
        (Preset::Home, vec![any_rule(check.kind)]),
        (Preset::FullAccess, vec![]),
    ];
    let mut out = [""; 3];
    for (idx, (preset, rules)) in settings.into_iter().enumerate() {
        let g = t.gate(preset, rules);
        let d = block_on(g.gate.check(check));
        out[idx] = gate_short(&d);
    }
    out
}

#[test]
fn s11_globs_through_links_match_direct_verdicts() {
    let t = world();
    let mut failures = vec![];
    let deny = ["Deny", "Deny", "Deny"];

    let up = "..";
    let dotdot_after = format!("cat proj/link-s*/{up}/id_rsa");
    let cases = [
        // One-hop links with *, ?, character classes, **, braces
        "cat proj/*/id_rsa",
        "cat proj/link-*",
        "cat proj/[l]ink-ssh/id_rsa",
        "cat proj/link-?sh/id_[a-z]*",
        "cat proj/link-*/id_*",
        "cat proj/**/id_rsa",
        "cat proj/**",
        "cat proj/{link-ssh,sub}/id_rsa",
        "cat proj/{link-*,sub}",
        // Two-hop links
        "cat proj/*/*/id_rsa",
        "cat proj/hop2-*",
        "cat proj/hop2-ssh/id_*",
        "cat proj/hop2-ssh/*",
        "cat proj/hop2-mid/to_ssh/id_rsa",
        // Dangling links (one-hop, two-hop, relative inside linked dir)
        "cat proj/dangling*",
        "cat proj/dangling-*",
        "cat proj/dangling-2*",
        "echo x > proj/dangling*",
        "cat proj/link-sub/dangling_*",
        // Links to ancestors of a credential tree
        "cat proj/*/.ssh/id_rsa",
        "cat proj/*/.s*/id_rsa",
        "cat proj/*/.*",
        "cat proj/**/.ssh/id_rsa",
        "cat proj/**/.npmrc",
        "cat proj/link-home2/.s*/id_rsa",
        // .. after a globbed symlink
        dotdot_after.as_str(),
        // All search tools with follow flags (explicit dir, globbed dir, explicit dot)
        "rg -L id proj_sub",
        "rg --follow id proj_sub",
        "grep -R id proj_sub",
        "grep --dereference-recursive id proj_sub",
        "grep --deref id proj_sub",
        "find -L proj_sub -name '*.txt'",
        "find proj_sub -follow -name '*.txt'",
        "ag -f id proj_sub",
        "ag --follow id proj_sub",
        "ack --follow id proj_sub",
        "rg -L id proj_s*",
        "find -L proj_s* -name '*.txt'",
        "cwd=proj_sub|find -L .",
        "cwd=proj_sub|grep -R id .",
        // Search tool following link to an ancestor of a tree
        "rg -L id proj_anc_sub",
        "find -L proj_anc_sub -name '*.txt'",
    ];

    for line in cases {
        let got = eval_cmd_3(&t, line);
        if got != deny {
            failures.push(format!("{line:?}: wanted {deny:?}, got {got:?}"));
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn s11_file_tool_globs_through_links_are_blocked() {
    let t = world();
    let mut failures = vec![];
    let deny = ["Deny", "Deny", "Deny"];

    let checks = [
        paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("*/id_rsa")),
        paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("link-*")),
        paths_check(
            ToolKind::Search,
            "Glob",
            &["proj"],
            false,
            Some("dangling*"),
        ),
        paths_check(
            ToolKind::Search,
            "Glob",
            &["proj"],
            false,
            Some("**/id_rsa"),
        ),
        paths_check(
            ToolKind::Search,
            "Glob",
            &["proj"],
            false,
            Some("[l]ink-ssh/id_rsa"),
        ),
        paths_check(ToolKind::Search, "Glob", &[], false, Some("proj/*/id_rsa")),
        paths_check(ToolKind::Search, "Grep", &["proj"], true, Some("*/id_rsa")),
        paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("link-key")),
        paths_check(
            ToolKind::Search,
            "Glob",
            &["proj"],
            false,
            Some("*/*/.ssh/id_rsa"),
        ),
    ];

    for check in &checks {
        let got = eval_check_3(&t, check);
        if got != deny {
            failures.push(format!("{:?}: wanted {deny:?}, got {got:?}", check.subject));
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn s11_unbounded_brace_globs_fail_closed_to_ask() {
    let t = world();
    let got_seq = eval_cmd_3(&t, "cat proj/{1..65}");
    assert_eq!(got_seq, ["Ask", "Ask", "Ask"]);
    let got_overflow = eval_cmd_3(&t, "cat proj/{a,b}{c,d}{e,f}{g,h}{i,j}{k,l}{m,n}{o,p}{q,r}");
    assert_eq!(got_overflow, ["Deny", "Deny", "Deny"]);
}

#[test]
fn s11_ordinary_searches_and_globs_with_ordinary_links_keep_dev_verdicts() {
    let t = world();
    let mut failures = vec![];

    let cases: &[(&str, [&str; 3])] = &[
        ("rg id proj_clean", ["Allow", "Allow", "Allow"]),
        ("rg -L id proj_clean", ["Ask", "Allow", "Allow"]),
        ("rg --follow id proj_clean", ["Ask", "Allow", "Allow"]),
        ("grep -r id proj_clean", ["Allow", "Allow", "Allow"]),
        ("grep -R id proj_clean", ["Ask", "Allow", "Allow"]),
        ("grep --deref id proj_clean", ["Ask", "Allow", "Allow"]),
        ("find -L proj_clean -name x.txt", ["Ask", "Allow", "Allow"]),
        ("ag -f id proj_clean", ["Ask", "Allow", "Allow"]),
        ("ack --follow id proj_clean", ["Ask", "Allow", "Allow"]),
        ("cat proj_clean/*/b.txt", ["Ask", "Allow", "Allow"]),
        ("cat proj_clean/**/b.txt", ["Ask", "Allow", "Allow"]),
        (
            "cat proj_clean/{a.txt,to_other/b.txt}",
            ["Ask", "Allow", "Allow"],
        ),
        ("cat proj_clean/to_other_file", ["Allow", "Allow", "Allow"]),
        ("ls proj_clean/to_other", ["Allow", "Allow", "Allow"]),
        ("cat proj_clean/to_other", ["Allow", "Allow", "Allow"]),
        ("cat proj/link-deep/x.txt", ["Allow", "Allow", "Allow"]),
        // Non-following search in a folder that happens to contain a link to .ssh
        ("rg id proj_sub", ["Allow", "Allow", "Allow"]),
        ("grep -r id proj_sub", ["Allow", "Allow", "Allow"]),
    ];

    for &(line, want) in cases {
        let got = eval_cmd_3(&t, line);
        if got != want {
            failures.push(format!("{line:?}: wanted {want:?}, got {got:?}"));
        }
    }

    let ordinary_checks: &[(ToolCheck, [&str; 3])] = &[
        (
            paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("a*")),
            ["Allow", "Allow", "Allow"],
        ),
        (
            paths_check(
                ToolKind::Search,
                "Glob",
                &["proj"],
                false,
                Some("link-deep/*"),
            ),
            ["Allow", "Allow", "Allow"],
        ),
        (
            paths_check(
                ToolKind::Search,
                "Glob",
                &["proj_clean"],
                false,
                Some("*/b.txt"),
            ),
            ["Allow", "Allow", "Allow"],
        ),
    ];

    for (check, want) in ordinary_checks {
        let got = eval_check_3(&t, check);
        if got != *want {
            failures.push(format!("{:?}: wanted {want:?}, got {got:?}", check.subject));
        }
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn rework1_finding1_file_tool_globs_match_dotfiles_and_recursive_basenames() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write("proj_clean/a.txt", "ordinary");
    t.write("other_proj/b.txt", "ordinary");
    t.write("other_proj/sub/c.txt", "ordinary");
    std::fs::create_dir_all(t.home.join("proj/sub")).unwrap();
    std::fs::create_dir_all(t.home.join("proj/.hidden_dir")).unwrap();
    symlink(t.home.join(".ssh/id_rsa"), t.home.join("proj/.key")).unwrap();
    symlink(t.home.join(".ssh/id_rsa"), t.home.join("proj/sub/key")).unwrap();
    symlink(
        t.home.join(".ssh/id_rsa"),
        t.home.join("proj/.hidden_dir/secret_key"),
    )
    .unwrap();
    symlink(
        t.home.join("other_proj"),
        t.home.join("proj_clean/to_other"),
    )
    .unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    let allow = ["Allow", "Allow", "Allow"];
    for check in [
        paths_check(ToolKind::Search, "Glob", &["proj"], true, Some("*")),
        paths_check(ToolKind::Search, "Grep", &["proj"], true, Some("key*")),
        paths_check(
            ToolKind::Search,
            "Glob",
            &["proj"],
            true,
            Some("**/secret_key"),
        ),
    ] {
        assert_eq!(eval_check_3(&t, &check), deny, "{:?}", check.subject);
    }
    for check in [
        paths_check(ToolKind::Search, "Glob", &["proj_clean"], true, Some("*")),
        paths_check(ToolKind::Search, "Grep", &["proj_clean"], true, Some("b*")),
    ] {
        assert_eq!(eval_check_3(&t, &check), allow, "{:?}", check.subject);
    }
}

#[test]
fn rework1_finding2_dangling_links_to_absent_credential_files_are_denied() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    let up = "..";
    symlink(format!("{up}/.npmrc"), t.home.join("proj/dangling-npm")).unwrap();
    symlink(
        format!("{up}/.config/atrium/config.toml"),
        t.home.join("proj/dangling-cfg"),
    )
    .unwrap();

    assert_eq!(
        eval_cmd_3(&t, "printf x > proj/dangling-*"),
        ["Deny", "Deny", "Deny"]
    );
    assert_eq!(
        eval_cmd_3(&t, "printf x > proj/dangling-c*"),
        ["Deny", "Deny", "Deny"]
    );
    assert_eq!(
        eval_cmd_3(&t, "cat proj/dangling-c*"),
        ["Ask", "Allow", "Allow"]
    );
}

#[test]
fn rework1_finding3_dangling_targets_resolve_links_before_dotdot() {
    let t = Tmp::new();
    std::fs::create_dir_all(t.home.join(".ssh/sub")).unwrap();
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    let up = "..";
    symlink(format!("{up}/.ssh/sub"), t.home.join("proj/dir")).unwrap();
    symlink(
        format!("dir/{up}/new_key"),
        t.home.join("proj/dangling-key"),
    )
    .unwrap();
    symlink("dangling-key", t.home.join("proj/dangling-2hop")).unwrap();

    assert_eq!(
        eval_cmd_3(&t, "printf x > proj/dangling-*"),
        ["Deny", "Deny", "Deny"]
    );
    assert_eq!(
        eval_cmd_3(&t, "printf x > proj/dangling-2*"),
        ["Deny", "Deny", "Deny"]
    );
}

#[test]
fn rework1_finding4_following_searches_do_not_skip_read_dirs() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    for dir in ["proj_nm/node_modules", "proj_git/.git", "proj_tgt/target"] {
        std::fs::create_dir_all(t.home.join(dir)).unwrap();
        symlink(t.home.join(".ssh/id_rsa"), t.home.join(dir).join("key")).unwrap();
    }

    let deny = ["Deny", "Deny", "Deny"];
    for line in [
        "grep -R secret proj_nm",
        "grep -R secret proj_git",
        "grep -R secret proj_tgt",
        "find -L proj_nm -name '*'",
        "find -L proj_git -name '*'",
        "find -L proj_tgt -name '*'",
        "rg -L secret proj_nm",
        "rg -L secret proj_tgt",
        "rg -L --hidden secret proj_git",
        "rg -L -. secret proj_git",
        "rg -L -uu secret proj_git",
    ] {
        assert_eq!(eval_cmd_3(&t, line), deny, "{line}");
    }
    assert_eq!(
        eval_cmd_3(&t, "rg -L secret proj_git"),
        ["Ask", "Allow", "Allow"]
    );
}

#[test]
fn rework1_finding5_public_ssh_and_readable_config_keep_ordinary_verdicts() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa.pub", "ssh-ed25519 AAAA");
    t.write(".ssh/known_hosts", "github.com ssh-ed25519 AAAA");
    t.write(".ssh/config", "Host *");
    t.write(".ssh/authorized_keys", "ssh-ed25519 AAAA");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    symlink(
        t.home.join(".ssh/id_rsa.pub"),
        t.home.join("proj/public-key"),
    )
    .unwrap();
    symlink(t.home.join(".ssh/known_hosts"), t.home.join("proj/kh")).unwrap();
    symlink(t.home.join(".ssh/config"), t.home.join("proj/sshcfg")).unwrap();
    symlink(
        t.home.join(".ssh/authorized_keys"),
        t.home.join("proj/authkeys"),
    )
    .unwrap();

    let allow = ["Allow", "Allow", "Allow"];
    for pat in ["public-*", "kh", "sshcfg", "authkeys"] {
        let check = paths_check(ToolKind::Search, "Glob", &["proj"], true, Some(pat));
        assert_eq!(eval_check_3(&t, &check), allow, "Glob {pat}");
    }
    assert_eq!(
        eval_cmd_3(&t, "cat proj/public-*"),
        ["Ask", "Allow", "Allow"]
    );
    assert_eq!(eval_cmd_3(&t, "cat proj/ssh*"), ["Ask", "Allow", "Allow"]);
    assert_eq!(
        eval_cmd_3(&t, "echo x > proj/ssh*"),
        ["Deny", "Deny", "Deny"]
    );
}

#[test]
fn rework1_finding6_unreadable_glob_dir_fails_closed_to_ask() {
    struct RestorePerm(std::path::PathBuf);
    impl Drop for RestorePerm {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
        }
    }

    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    let closed = t.home.join("proj/closed");
    std::fs::create_dir_all(&closed).unwrap();
    symlink(t.home.join(".ssh/id_rsa"), closed.join("key")).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = RestorePerm(closed.clone());

    if std::fs::read_dir(&closed).is_err() {
        let check = paths_check(ToolKind::Search, "Glob", &["proj"], true, Some("closed/*"));
        assert_eq!(eval_check_3(&t, &check), ["Ask", "Ask", "Ask"]);

        symlink(t.home.join(".ssh/id_rsa"), t.home.join("proj/link-key")).unwrap();
        let check_both = paths_check(ToolKind::Search, "Glob", &["proj"], true, Some("*"));
        assert_eq!(eval_check_3(&t, &check_both), ["Deny", "Deny", "Deny"]);
    }
}

#[test]
fn rework2_over_budget_glob_and_non_ssh_id_rsa_do_not_hard_deny() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write("plain/sub/id_rsa", "not-an-ssh-key");
    std::fs::create_dir_all(t.home.join("big_dir")).unwrap();
    std::fs::create_dir_all(t.home.join("big_plain")).unwrap();
    for i in 0..4150 {
        std::fs::create_dir(t.home.join(format!("big_dir/d{i:04}"))).unwrap();
        std::fs::create_dir(t.home.join(format!("big_plain/d{i:04}"))).unwrap();
    }
    symlink(t.home.join(".ssh"), t.home.join("big_dir/z_ssh")).unwrap();

    assert_eq!(
        eval_cmd_3(&t, "cat big_dir/*/id_rsa"),
        ["Ask", "Ask", "Ask"]
    );
    assert_eq!(
        eval_cmd_3(&t, "cat big_plain/*/id_rsa"),
        ["Ask", "Ask", "Ask"]
    );
    assert_eq!(
        eval_cmd_3(&t, "cat plain/*/id_rsa"),
        ["Ask", "Allow", "Allow"]
    );
    for check in [
        paths_check(ToolKind::Search, "Glob", &["plain"], true, Some("id_rsa")),
        paths_check(ToolKind::Search, "Glob", &["plain"], true, Some("*/id_rsa")),
    ] {
        assert_eq!(
            eval_check_3(&t, &check),
            ["Allow", "Allow", "Allow"],
            "{:?}",
            check.subject
        );
    }
    for check in [
        paths_check(
            ToolKind::Search,
            "Glob",
            &["big_dir"],
            true,
            Some("*/id_rsa"),
        ),
        paths_check(ToolKind::Search, "Glob", &["big_dir"], true, Some("id_rsa")),
        paths_check(
            ToolKind::Search,
            "Glob",
            &["big_plain"],
            true,
            Some("*/id_rsa"),
        ),
    ] {
        assert_eq!(
            eval_check_3(&t, &check),
            ["Ask", "Ask", "Ask"],
            "{:?}",
            check.subject
        );
    }
}

// ---- Escalation (D-127): review round 2 findings 1-5 ----

struct RestorePerm2(std::path::PathBuf);
impl Drop for RestorePerm2 {
    fn drop(&mut self) {
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}

#[test]
fn rework2_finding1_relocated_ssh_store_keeps_credential_protection() {
    let t = Tmp::new();
    t.write("vault/id_rsa", "secret");
    t.write("vault/id_rsa.pub", "public");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    symlink("vault", t.home.join(".ssh")).unwrap();
    let up = "..";
    symlink(format!("{up}/vault/id_rsa"), t.home.join("proj/key")).unwrap();
    symlink(format!("{up}/vault/id_rsa.pub"), t.home.join("proj/pub")).unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "cat proj/key*"), deny);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("key*"));
    assert_eq!(eval_check_3(&t, &check), deny);
    // the public-file exception still holds through the relocated store
    assert_eq!(eval_cmd_3(&t, "cat proj/pub*"), ["Ask", "Allow", "Allow"]);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("pub*"));
    assert_eq!(eval_check_3(&t, &check), ["Allow", "Allow", "Allow"]);
}

#[test]
fn rework2_finding2_non_utf8_link_names_are_resolved_by_their_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    let name = std::ffi::OsStr::from_bytes(&[b'k', 0xff]);
    let up = "..";
    symlink(format!("{up}/.ssh/id_rsa"), t.home.join("proj").join(name)).unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "cat proj/k*"), deny);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("k*"));
    assert_eq!(eval_check_3(&t, &check), deny);
}

#[test]
fn rework2_finding3_dereferencing_tar_walks_pruned_dirs() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    for dir in ["proj/node_modules", "proj_git/.git", "proj_tgt/target"] {
        std::fs::create_dir_all(t.home.join(dir)).unwrap();
        symlink(t.home.join(".ssh/id_rsa"), t.home.join(dir).join("key")).unwrap();
    }
    let deny = ["Deny", "Deny", "Deny"];
    for line in [
        "tar --dereference -cf out.tar proj",
        "tar --dereference -cf out.tar proj_git",
        "tar --dereference -cf out.tar proj_tgt",
        "tar -h -cf out.tar proj",
    ] {
        assert_eq!(eval_cmd_3(&t, line), deny, "{line}");
    }
}

#[test]
fn rework2_finding4_unreadable_dir_in_following_search_fails_closed() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write("proj/a.txt", "ordinary");
    let closed = t.home.join("proj/closed");
    std::fs::create_dir_all(&closed).unwrap();
    let up = "..";
    symlink(format!("{up}/{up}/.ssh/id_rsa"), closed.join("key")).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = RestorePerm2(closed.clone());
    if std::fs::read_dir(&closed).is_ok() {
        return; // running as root: the folder is readable
    }
    assert_eq!(eval_cmd_3(&t, "grep -R secret proj"), ["Ask", "Ask", "Ask"]);
}

#[test]
fn rework2_finding5_file_tool_dangling_targets_resolve_links_before_dotdot() {
    let t = Tmp::new();
    std::fs::create_dir_all(t.home.join(".ssh/sub")).unwrap();
    std::fs::create_dir_all(t.home.join("proj/sub")).unwrap();
    let up = "..";
    symlink(format!("{up}/{up}/.ssh/sub"), t.home.join("proj/sub/dir")).unwrap();
    symlink(
        format!("dir/{up}/new_key"),
        t.home.join("proj/sub/dangling-key"),
    )
    .unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    let read = paths_check(
        ToolKind::Read,
        "Read",
        &["proj/sub/dangling-key"],
        false,
        None,
    );
    assert_eq!(eval_check_3(&t, &read), deny);
    let glob = paths_check(
        ToolKind::Search,
        "Glob",
        &["proj"],
        true,
        Some("sub/dangling-key"),
    );
    assert_eq!(eval_check_3(&t, &glob), deny);
}

// ---- Escalation round 2 (D-127): review round 3 findings 1-5 ----

#[test]
fn rework3_finding1_relocated_parent_of_a_store_keeps_protection() {
    let t = Tmp::new();
    std::fs::rename(t.home.join(".config"), t.home.join("config_store")).unwrap();
    t.write("config_store/gcloud/token", "secret");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    symlink("config_store", t.home.join(".config")).unwrap();
    let up = "..";
    symlink(
        format!("{up}/config_store/gcloud/token"),
        t.home.join("proj/key"),
    )
    .unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "cat proj/key*"), deny);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("key*"));
    assert_eq!(eval_check_3(&t, &check), deny);
}

#[test]
fn rework3_finding2_non_utf8_glob_prefix_is_kept_by_its_bytes() {
    use std::os::unix::ffi::OsStrExt;
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    let store = t.home.join(std::ffi::OsStr::from_bytes(b"store\xff"));
    std::fs::create_dir_all(&store).unwrap();
    let up = "..";
    symlink(format!("{up}/.ssh/id_rsa"), store.join("key")).unwrap();
    symlink(
        std::ffi::OsStr::from_bytes(b"store\xff"),
        t.home.join("proj"),
    )
    .unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "cat proj/key*"), deny);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("key*"));
    assert_eq!(eval_check_3(&t, &check), deny);
}

#[test]
fn rework3_finding3_ssh_member_linked_out_of_ssh_is_protected() {
    let t = Tmp::new();
    t.write("vault/key", "secret");
    std::fs::create_dir_all(t.home.join(".ssh")).unwrap();
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    let up = "..";
    symlink(format!("{up}/vault/key"), t.home.join(".ssh/id_rsa")).unwrap();
    symlink(format!("{up}/vault/key"), t.home.join("proj/key")).unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "cat proj/key*"), deny);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("key*"));
    assert_eq!(eval_check_3(&t, &check), deny);
}

#[test]
fn rework3_finding4_literal_file_tool_path_with_brackets_is_resolved() {
    let t = Tmp::new();
    std::fs::create_dir_all(t.home.join(".ssh/sub")).unwrap();
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    let up = "..";
    symlink(format!("{up}/.ssh/sub"), t.home.join("proj/dir")).unwrap();
    symlink(
        format!("dir/{up}/new_key"),
        t.home.join("proj/dangling-key[1]"),
    )
    .unwrap();

    let read = paths_check(
        ToolKind::Read,
        "Read",
        &["proj/dangling-key[1]"],
        false,
        None,
    );
    assert_eq!(eval_check_3(&t, &read), ["Deny", "Deny", "Deny"]);
}

#[test]
fn rework3_finding5_one_budget_per_check() {
    let t = Tmp::new();
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    for i in 0..4096 {
        t.write(&format!("proj/ka{i:04}"), "ordinary");
    }
    // the reviewer's case: 4096 identical roots x 256 identical alternatives
    let roots: Vec<&str> = vec!["proj"; 4096];
    let same = format!("k{{{}}}*", vec!["a"; 256].join(","));
    let start = std::time::Instant::now();
    let check = paths_check(ToolKind::Search, "Glob", &roots, false, Some(&same));
    let got = eval_check_3(&t, &check);
    assert!(!got.contains(&"Deny"), "{got:?}");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(20),
        "took {:?}",
        start.elapsed()
    );
    // 256 distinct alternatives over the same 4096 entries: more work than
    // one check may do, so it can't be shown safe and asks
    let alts: Vec<String> = (0..256).map(|i| format!("x{i}")).collect();
    let distinct = format!("{{{}}}*", alts.join(","));
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some(&distinct));
    assert_eq!(eval_check_3(&t, &check), ["Ask", "Ask", "Ask"]);
}

// ---- Added scope (D-120): resolve_fs order and identity for globs/walks ----

#[test]
fn added_scope_resolve_fs_follows_links_before_dotdot() {
    let t = Tmp::new();
    std::fs::create_dir_all(t.home.join(".ssh/sub")).unwrap();
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    let up = "..";
    symlink(format!("{up}/.ssh/sub"), t.home.join("proj/dir")).unwrap();
    symlink(
        format!("dir/{up}/new_key"),
        t.home.join("proj/dangling-key"),
    )
    .unwrap();

    // the resolver itself (every caller's view of the path)
    assert_eq!(
        atrium_core::policy::fs_resolve("proj/dangling-key", &t.home, &t.home),
        t.home.join(".ssh/new_key")
    );
    // ordinary paths keep their resolution
    t.write("proj/a.txt", "x");
    assert_eq!(
        atrium_core::policy::fs_resolve("proj/a.txt", &t.home, &t.home),
        t.home.join("proj/a.txt")
    );
    assert_eq!(
        atrium_core::policy::fs_resolve("proj/missing/../a.txt", &t.home, &t.home),
        t.home.join("proj/a.txt")
    );
    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "printf x > proj/dangling-key"), deny);
    assert_eq!(eval_cmd_3(&t, "cat proj/dangling-key"), deny);
}

#[test]
fn added_scope_hard_links_met_by_globs_and_walks_match_by_identity() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write("proj/a.txt", "ordinary");
    std::fs::hard_link(t.home.join(".ssh/id_rsa"), t.home.join("proj/hl")).unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "cat proj/h*"), deny);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("h*"));
    assert_eq!(eval_check_3(&t, &check), deny);
    assert_eq!(eval_cmd_3(&t, "grep -R secret proj"), deny);
    assert_eq!(eval_cmd_3(&t, "tar --dereference -cf out.tar proj"), deny);
    // an ordinary hard link keeps its verdict
    t.write("other/x.txt", "ordinary");
    std::fs::hard_link(t.home.join("other/x.txt"), t.home.join("other/y.txt")).unwrap();
    assert_eq!(
        eval_cmd_3(&t, "cat other/y*"),
        eval_cmd_3(&t, "cat other/y.txt")
    );
}

#[test]
fn added_scope_non_following_walks_check_hard_links_by_identity() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write("proj/a.txt", "ordinary");
    t.write("proj/sub/b.txt", "ordinary");
    std::fs::hard_link(t.home.join(".ssh/id_rsa"), t.home.join("proj/sub/h")).unwrap();
    t.write("clean/a.txt", "ordinary");
    t.write("clean/x.txt", "ordinary");
    std::fs::hard_link(t.home.join("clean/x.txt"), t.home.join("clean/y.txt")).unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    for line in [
        "grep -r secret proj",
        "rg secret proj",
        "find proj -type f -exec cat {} +",
        "tar -cf - proj",
    ] {
        assert_eq!(eval_cmd_3(&t, line), deny, "{line}");
        // ordinary tree (an ordinary hard link inside): not hard-denied
        let clean = line.replace("proj", "clean");
        assert_ne!(eval_cmd_3(&t, &clean), deny, "{clean}");
    }
}

#[test]
fn added_scope_non_reading_walks_do_not_hard_deny_on_hard_links() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write("proj/a.txt", "ordinary");
    std::fs::hard_link(t.home.join(".ssh/id_rsa"), t.home.join("proj/h")).unwrap();
    for line in ["rm -r proj", "mv proj proj2"] {
        assert_ne!(eval_cmd_3(&t, line), ["Deny", "Deny", "Deny"], "{line}");
    }
}

// ---- Escalation round 3 (D-127): review round 4 findings 1-6 ----

#[test]
fn rework4_finding1_metacharacters_from_a_resolved_link_stay_literal() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    std::fs::create_dir_all(t.home.join("store[1]")).unwrap();
    let up = "..";
    symlink(format!("{up}/.ssh/id_rsa"), t.home.join("store[1]/key")).unwrap();
    symlink("store[1]", t.home.join("proj")).unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "cat proj/key*"), deny);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], true, Some("key*"));
    assert_eq!(eval_check_3(&t, &check), deny);
}

#[test]
fn rework4_finding2_dangling_ssh_members_are_in_the_credential_set() {
    let t = Tmp::new();
    std::fs::create_dir_all(t.home.join(".ssh")).unwrap();
    std::fs::create_dir_all(t.home.join("vault")).unwrap();
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    let up = "..";
    symlink(format!("{up}/vault/new_key"), t.home.join(".ssh/id_rsa")).unwrap();
    symlink(format!("{up}/.ssh/id_rsa"), t.home.join("proj/key")).unwrap();

    let deny = ["Deny", "Deny", "Deny"];
    assert_eq!(eval_cmd_3(&t, "printf x > proj/key*"), deny);
    let check = paths_check(ToolKind::Search, "Glob", &["proj"], false, Some("key*"));
    assert_eq!(eval_check_3(&t, &check), deny);
}

#[test]
fn rework4_finding3_literal_read_paths_are_not_globs() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write("proj/file[1]", "ordinary");
    let up = "..";
    symlink(format!("{up}/.ssh/id_rsa"), t.home.join("proj/file1")).unwrap();
    let read = paths_check(ToolKind::Read, "Read", &["proj/file[1]"], false, None);
    assert_eq!(eval_check_3(&t, &read), ["Allow", "Allow", "Allow"]);
}

#[test]
fn rework4_finding4_unreadable_literal_path_fails_closed() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    let closed = t.home.join("proj/closed");
    std::fs::create_dir_all(&closed).unwrap();
    let up = "..";
    symlink(format!("{up}/{up}/.ssh/id_rsa"), closed.join("key")).unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
    let _guard = RestorePerm2(closed.clone());
    if std::fs::read_dir(&closed).is_ok() {
        return; // running as root
    }
    let read = paths_check(ToolKind::Read, "Read", &["proj/closed/key"], false, None);
    assert_eq!(eval_check_3(&t, &read), ["Ask", "Ask", "Ask"]);
}

#[test]
fn rework4_finding5_budget_covers_the_whole_check() {
    // Case A: links in ~/.ssh to many folders, many glob operands
    let t = Tmp::new();
    std::fs::create_dir_all(t.home.join(".ssh")).unwrap();
    for i in 0..1024 {
        let d = t.home.join(format!("ext/d{i:04}"));
        std::fs::create_dir_all(&d).unwrap();
        symlink(&d, t.home.join(format!(".ssh/l{i:04}"))).unwrap();
    }
    for i in 0..4096 {
        t.write(&format!("proj/ka{i:04}"), "ordinary");
    }
    let line = format!("cat{}", " proj/k*".repeat(4095));
    let start = std::time::Instant::now();
    assert_eq!(eval_cmd_3(&t, &line), ["Ask", "Ask", "Ask"]);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(60),
        "case A took {:?}",
        start.elapsed()
    );

    // Case B: 4096 empty folders x 256 alternatives
    let t = Tmp::new();
    let mut dirs = vec![];
    for i in 0..4096 {
        let d = format!("e/d{i:04}");
        std::fs::create_dir_all(t.home.join(&d)).unwrap();
        dirs.push(d);
    }
    let roots: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let alts: Vec<String> = (0..256).map(|i| format!("x{i}")).collect();
    let g = format!("{{{}}}*", alts.join(","));
    let start = std::time::Instant::now();
    let check = paths_check(ToolKind::Search, "Glob", &roots, false, Some(&g));
    assert_eq!(eval_check_3(&t, &check), ["Ask", "Ask", "Ask"]);
    assert!(
        start.elapsed() < std::time::Duration::from_secs(60),
        "case B took {:?}",
        start.elapsed()
    );
}

#[test]
fn rework4_finding6_archive_listing_does_not_scan_operands() {
    let t = Tmp::new();
    t.write(".ssh/id_rsa", "secret");
    t.write("proj/a.txt", "ordinary");
    t.write("plain.tar", "not really a tar");
    std::fs::hard_link(t.home.join(".ssh/id_rsa"), t.home.join("proj/h")).unwrap();
    assert_eq!(
        eval_cmd_3(&t, "tar -tf plain.tar proj"),
        ["Ask", "Allow", "Allow"]
    );
    assert_eq!(
        eval_cmd_3(&t, "tar -xf plain.tar proj"),
        eval_cmd_3(&t, "tar -xf plain.tar a")
    );
    // creating an archive still reads the folder
    assert_eq!(
        eval_cmd_3(&t, "tar -cf out.tar proj"),
        ["Deny", "Deny", "Deny"]
    );
}
