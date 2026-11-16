//! M1-32-link-globs: globs and following searches that reach credentials or
//! ordinary folders through symlinks (S11).

mod common_adversarial;

use std::os::unix::fs::symlink;

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
        "cat proj/link-s*/../id_rsa",
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
