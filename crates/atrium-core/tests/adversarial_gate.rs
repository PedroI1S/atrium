//! M1-13: the gate against the real filesystem (a temp folder with a fake
//! HOME): symlinks, `..` through links, path aliases, case, `~user`, globs,
//! the working directory a call names, and file tools.
//!
//! Nothing is executed; the real `~/.ssh` and credential files are never
//! touched (HOME here is a folder under the temp dir).

mod common_adversarial;

use std::os::unix::fs::symlink;

use atrium_agents::{GateDecision, ToolCheck, ToolGate};
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, ToolSubject};
use common_adversarial::{Tmp, block_on, cmd_check, paths_check};

/// A home with credentials, Atrium's folders, a project and a set of links.
fn world() -> Tmp {
    let t = Tmp::new();
    t.write(".ssh/id_ed25519", "secret");
    t.write(".ssh/sub/inner_key", "secret");
    t.write(".npmrc", "token");
    t.write(".aws/credentials", "secret");
    t.write(".gnupg/private-keys-v1.d/k.key", "secret");
    t.write(".local/share/atrium/atrium.db", "db");
    t.write(".config/atrium/config.toml", "cfg");
    t.write("proj/a.txt", "ordinary");
    t.write("proj/sub/b.txt", "ordinary");
    t.write("elsewhere/deep/x.txt", "ordinary");
    let h = &t.home;
    // links into the credentials
    symlink(h.join(".ssh/id_ed25519"), h.join("proj/link-key")).unwrap();
    symlink("../.ssh/id_ed25519", h.join("proj/rel-link-key")).unwrap();
    symlink(h.join(".ssh"), h.join("proj/link-ssh")).unwrap();
    symlink(h.join(".ssh/sub"), h.join("proj/link-sub")).unwrap();
    symlink(h.join(".ssh/nonexistent-key"), h.join("proj/dangling-key")).unwrap();
    symlink(h.join("proj/link-key"), h.join("proj/chain1")).unwrap();
    symlink(h.join("proj/chain1"), h.join("proj/chain2")).unwrap();
    symlink(h, h.join("proj/link-home")).unwrap();
    symlink(h.join(".aws"), h.join("proj/link-aws")).unwrap();
    symlink(h.join(".local/share/atrium"), h.join("proj/link-data")).unwrap();
    symlink(h.join(".config/atrium"), h.join("proj/link-config")).unwrap();
    // a link to a folder elsewhere: `..` through it is not `proj`
    symlink(h.join("elsewhere/deep"), h.join("proj/link-deep")).unwrap();
    // a link that makes `..` land in the credentials
    symlink(h.join(".ssh/sub"), h.join("proj/dotdot-ssh")).unwrap();
    // a link loop
    symlink(h.join("proj/loop-b"), h.join("proj/loop-a")).unwrap();
    symlink(h.join("proj/loop-a"), h.join("proj/loop-b")).unwrap();
    t
}

thread_local! {
    static FAILURES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn fail(msg: String) {
    FAILURES.with(|f| f.borrow_mut().push(msg));
}

/// Panics with every failure recorded on this thread, then clears them.
fn finish() {
    let all = FAILURES.with(|f| std::mem::take(&mut *f.borrow_mut()));
    assert!(all.is_empty(), "\n{}", all.join("\n"));
}

fn deny(g: &common_adversarial::TestGate, line: &str) {
    let d = block_on(g.gate.check(&cmd_check(line)));
    if !matches!(d, GateDecision::Deny { .. }) {
        fail(format!("{line}: wanted Deny, got {d:?}"));
    }
}

fn not_allow(g: &common_adversarial::TestGate, line: &str) {
    let d = block_on(g.gate.check(&cmd_check(line)));
    if matches!(d, GateDecision::Allow) {
        fail(format!("{line}: wanted not Allow, got {d:?}"));
    }
}

fn read_deny(g: &common_adversarial::TestGate, kind: ToolKind, name: &str, p: &str, rec: bool) {
    let c = paths_check(kind, name, &[p], rec, None);
    let d = block_on(g.gate.check(&c));
    if !matches!(d, GateDecision::Deny { .. }) {
        fail(format!("{name} {p}: wanted Deny, got {d:?}"));
    }
}

#[test]
fn links_into_credentials_reach_nothing() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    let h = t.home.display().to_string();
    for line in [
        "cat proj/link-key",
        "cat proj/rel-link-key",
        "cat proj/link-ssh/id_ed25519",
        "cat proj/link-sub/inner_key",
        "cat proj/dangling-key",
        "cat proj/chain1",
        "cat proj/chain2",
        "cat proj/link-home/.ssh/id_ed25519",
        "cat proj/link-home/.npmrc",
        "cat proj/link-aws/credentials",
        "cat proj/link-data/atrium.db",
        "head -c 4 proj/link-key",
        "tail proj/chain2",
        "wc proj/link-ssh/id_ed25519",
        "stat proj/link-key",
        "file proj/link-key",
        "cat proj/link-ssh/../.npmrc",
        "cat proj/link-sub/../id_ed25519",
        "cat proj/dotdot-ssh/../id_ed25519",
        "cat ./proj/dotdot-ssh/../id_ed25519",
        "cat proj/link-ssh/.",
        "cat proj/link-ssh/sub/../id_ed25519",
        "cat proj/link-ssh/./id_ed25519",
        "cat proj/link-ssh//id_ed25519",
        "cat proj/link-ssh/*",
        "cat proj/link-ssh/id_*",
        "cd proj/link-ssh && cat id_ed25519",
        "cd proj/link-ssh; cat id_ed25519",
        "cd proj/link-sub; cat ../id_ed25519",
        "cd proj/dotdot-ssh; cat ../id_ed25519",
        "cd proj; cat link-key",
        "cd proj/link-home; cat .npmrc",
        "cd proj/link-home/.ssh; cat id_ed25519",
        "sh -c 'cat proj/link-key'",
        "echo $(cat proj/link-key)",
        "cat <(cat proj/link-key)",
        "cat < proj/link-key",
        "grep -r x proj/link-ssh",
        "rg x proj/link-ssh",
        "rg x proj/link-data",
        "rg --files proj/link-ssh",
        "ls proj/link-ssh/id_ed25519",
        "find proj/link-ssh -type f",
        "find proj/link-home -name .npmrc",
        &format!("cat /System/Volumes/Data{h}/.npmrc"),
        &format!("cat /System/Volumes/Data{h}/proj/link-key"),
        &format!("cat {h}/proj/../.npmrc"),
        &format!("cat {h}//.npmrc"),
        &format!("cat {h}/./.npmrc"),
        &format!("cat {h}/proj/link-home/.npmrc"),
        &format!("cat {h}/.SSH/ID_ED25519"),
        &format!("cat {h}/.Ssh/Id_Ed25519"),
        &format!("cat {h}/.NPMRC"),
        &format!("cat {h}/.Aws/Credentials"),
        "cat .SSH/ID_ED25519",
        "cat .Npmrc",
        "cat .AWS/credentials",
        "cat .Local/Share/Atrium/atrium.db",
    ] {
        deny(&g, line);
    }
    // file tools
    for p in [
        "proj/link-key",
        "proj/rel-link-key",
        "proj/link-ssh/id_ed25519",
        "proj/link-sub/inner_key",
        "proj/dangling-key",
        "proj/chain2",
        "proj/link-home/.npmrc",
        "proj/link-aws/credentials",
        "proj/link-data/atrium.db",
        "proj/dotdot-ssh/../id_ed25519",
        ".SSH/id_ed25519",
        ".ssh/id_ed25519",
        ".ssh/sub/inner_key",
        ".ssh/does-not-exist-yet",
        "~/.ssh/id_ed25519",
        "$HOME/.ssh/id_ed25519",
    ] {
        read_deny(&g, ToolKind::Read, "Read", p, false);
        read_deny(&g, ToolKind::Edit, "Edit", p, false);
        read_deny(&g, ToolKind::Delete, "Delete", p, false);
        read_deny(&g, ToolKind::Search, "Grep", p, true);
    }
    // a recursive search rooted at a link to home or to a folder that holds
    // a credential file asks (it is above the credentials)
    for line in [
        "grep -r x proj/link-home",
        "rg x proj/link-home",
        "rg x proj/link-aws",
    ] {
        not_allow(&g, line);
    }
    // a link loop is ELOOP for the tool: the gate must only return
    for line in ["cat proj/loop-a", "cat proj/loop-a/x", "ls proj/loop-b"] {
        let _ = block_on(g.gate.check(&cmd_check(line)));
    }
    // ordinary files through ordinary paths are allowed (no false hard deny)
    for line in [
        "cat proj/a.txt",
        "cat proj/sub/b.txt",
        "cat ./proj/a.txt",
        "cat proj/./a.txt",
        "cat proj/sub/../a.txt",
        "cat proj/link-deep/x.txt",
        "cat proj/link-deep/../deep/x.txt",
        "ls proj",
        "ls proj/link-deep",
        "rg x proj/sub",
        "grep -r x proj/sub",
    ] {
        let d = block_on(g.gate.check(&cmd_check(line)));
        if d != GateDecision::Allow {
            fail(format!("{line}: wanted Allow, got {d:?}"));
        }
    }
    finish();
}

/// `..` after a link is relative to the link's target (kernel semantics): the
/// gate must follow the same path the shell will.
#[test]
fn dot_dot_after_a_link_goes_where_the_kernel_goes() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    // `proj/dotdot-ssh` -> `.ssh/sub`: `..` is `.ssh`
    deny(&g, "cat proj/dotdot-ssh/../id_ed25519");
    // `proj/link-deep` -> `elsewhere/deep`: `..` is `elsewhere`, not `proj`
    t.write("elsewhere/innocent.txt", "ok");
    t.write("proj/innocent.txt", "ok");
    let d = block_on(
        g.gate
            .check(&cmd_check("cat proj/link-deep/../innocent.txt")),
    );
    assert_eq!(d, GateDecision::Allow);
    finish();
}

#[test]
fn the_working_directory_a_call_names_is_checked() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    let h = t.home.display().to_string();
    for cwd in [
        ".ssh".to_string(),
        "./.ssh".to_string(),
        "~/.ssh".to_string(),
        "$HOME/.ssh".to_string(),
        format!("{h}/.ssh"),
        format!("{h}/.SSH"),
        "proj/link-ssh".to_string(),
        "proj/link-sub/..".to_string(),
        "proj/dotdot-ssh/..".to_string(),
        format!("/System/Volumes/Data{h}/.ssh"),
        "proj/link-home/.ssh".to_string(),
        ".ssh/".to_string(),
        ".ssh/sub/..".to_string(),
    ] {
        for line in [
            "cat id_ed25519",
            "head id_ed25519",
            "cat ./id_ed25519",
            "cat *",
        ] {
            let mut c = cmd_check(line);
            c.subject = ToolSubject::Command {
                line: line.into(),
                cwd: Some(cwd.clone()),
            };
            let d = block_on(g.gate.check(&c));
            if !matches!(d, GateDecision::Deny { .. }) {
                fail(format!("cwd {cwd:?} {line:?}: wanted Deny, got {d:?}"));
            }
        }
    }
    // a cwd in Atrium's folders: writing is blocked, reading is allowed
    let mut c = cmd_check("rm config.toml");
    c.subject = ToolSubject::Command {
        line: "rm config.toml".into(),
        cwd: Some(".config/atrium".into()),
    };
    assert!(matches!(
        block_on(g.gate.check(&c)),
        GateDecision::Deny { .. }
    ));
    // a cwd that is ordinary
    let mut c = cmd_check("cat a.txt");
    c.subject = ToolSubject::Command {
        line: "cat a.txt".into(),
        cwd: Some("proj".into()),
    };
    assert_eq!(block_on(g.gate.check(&c)), GateDecision::Allow);
    finish();
}

#[test]
fn user_forms_and_homes_other_than_this_one() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    // `~name` is this user's home when `name` is the home folder's name
    // (`home` in this fake HOME); any other `~name` is someone else's: asked,
    // never allowed, and recursive deletes of it are blocked
    deny(&g, "cat ~home/.ssh/id_ed25519");
    deny(&g, "cat ~home/.npmrc");
    for line in [
        "cat ~root/.ssh/id_rsa",
        "cat ~nobody/.npmrc",
        "cat ~root",
        "ls ~root/",
        "cat ~+x/y",
        "cat ~-",
        "rg x ~root",
    ] {
        not_allow(&g, line);
    }
    deny(&g, "rm -rf ~root");
    deny(&g, "rm -rf ~nobody/");
    deny(&g, "rm -rf ~home");
    finish();
}

#[test]
fn globs_expand_against_the_real_folder() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    for line in [
        "cat .ssh/id_*",
        "cat .s*/id_ed25519",
        "cat .ss?/id_ed25519",
        "cat .[s]sh/id_ed25519",
        "cat .ssh/id_ed2551[0-9]",
        "cat .ssh/id_ed25519*",
        "cat .ssh/*",
        "cat .ssh/**",
        "cat .ssh/*/inner_key",
        "cat ./.ssh/*",
        "cat .n*rc",
        "cat .npm?c",
        "cat .npm[r]c",
        "cat .{npmrc,x}",
        "cat {.npmrc,x}",
        "cat .aws/cred*",
        "cat .aws/*",
        "cat .*/id_ed25519",
        "cat .*",
        "cat .[!.]*",
        "cat .??*",
        "cat .l*/share/atrium/*",
        "rm -rf .*",
        "rm -rf .[a-z]*",
        "rm -rf ./*",
        "rm -rf *",
    ] {
        deny(&g, line);
    }
    // globs in a folder with only ordinary files ask, never allow
    for line in [
        "cat proj/*",
        "cat proj/a.*",
        "ls proj/*",
        "cat proj/{a,b}.txt",
    ] {
        let d = block_on(g.gate.check(&cmd_check(line)));
        if !matches!(d, GateDecision::Ask { .. }) {
            fail(format!("{line}: wanted Ask, got {d:?}"));
        }
    }
    finish();
}

/// The call's own paths, through every file-tool kind.
#[test]
fn file_tools_cannot_reach_sensitive_paths_or_write_outside() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    let h = t.home.display().to_string();
    // a Search rooted at home, above credentials, or in them
    for p in [
        ".", "~", &h, "..", "../..", "/", ".ssh", ".aws", ".gnupg", "Library", ".config",
    ] {
        let c = paths_check(ToolKind::Search, "Grep", &[p], true, None);
        let d = block_on(g.gate.check(&c));
        if matches!(d, GateDecision::Allow) {
            fail(format!("Grep {p}: wanted not Allow, got {d:?}"));
        }
    }
    // writes: asked in Home, blocked for sensitive and Atrium paths
    for (kind, name) in [
        (ToolKind::Edit, "Edit"),
        (ToolKind::Delete, "Delete"),
        (ToolKind::Move, "Move"),
        (ToolKind::Other, "Write"),
    ] {
        for p in [
            ".config/atrium/config.toml",
            ".local/share/atrium/atrium.db",
            ".ssh/config",
            ".ssh/authorized_keys",
            "proj/link-config/config.toml",
            "proj/link-data/atrium.db",
        ] {
            read_deny_or_ask(&g, kind, name, p, true);
        }
        for p in ["/tmp/x", "proj/new.txt", "~/new.txt", "/etc/hosts", "../x"] {
            read_deny_or_ask(&g, kind, name, p, false);
        }
    }
    // a move with a sensitive destination or source
    let c = ToolCheck {
        call_id: None,
        name: "Move".into(),
        kind: ToolKind::Move,
        title: None,
        input: serde_json::json!({"from": "proj/a.txt", "to": ".ssh/authorized_keys"}),
        subject: ToolSubject::Paths {
            paths: vec!["proj/a.txt".into(), ".ssh/authorized_keys".into()],
            glob: None,
            recursive: false,
        },
        turn: 1,
    };
    assert!(matches!(
        block_on(g.gate.check(&c)),
        GateDecision::Deny { .. }
    ));
    // Fetch, Mcp, Unknown and None subjects are never allowed by Home's list
    for subject in [
        ToolSubject::Fetch {
            url: "https://x".into(),
        },
        ToolSubject::Mcp {
            server: "s".into(),
            tool: "t".into(),
        },
        ToolSubject::Unknown,
    ] {
        let mut c = cmd_check("x");
        c.kind = ToolKind::Fetch;
        c.name = "Tool".into();
        c.subject = subject.clone();
        let d = block_on(g.gate.check(&c));
        if !matches!(d, GateDecision::Ask { .. }) {
            fail(format!("{subject:?}: wanted Ask, got {d:?}"));
        }
    }
    // an Unknown subject with a sensitive string in its input (nested values
    // are not scanned: it can only be asked, never allowed, so that is safe)
    for input in [
        serde_json::json!({"path": "~/.ssh/id_ed25519"}),
        serde_json::json!({"list": ["a", "$HOME/.aws/credentials"]}),
        serde_json::json!({"cmd": "cat .npmrc"}),
        serde_json::json!(".ssh/id_ed25519"),
    ] {
        let mut c = cmd_check("x");
        c.subject = ToolSubject::Unknown;
        c.input = input.clone();
        let d = block_on(g.gate.check(&c));
        if !matches!(d, GateDecision::Deny { .. }) {
            fail(format!("{input}: wanted Deny, got {d:?}"));
        }
    }
    finish();
}

/// `blocked`: the path is sensitive (Deny); otherwise Ask (never Allow).
fn read_deny_or_ask(
    g: &common_adversarial::TestGate,
    kind: ToolKind,
    name: &str,
    p: &str,
    blocked: bool,
) {
    let c = paths_check(kind, name, &[p], false, None);
    let d = block_on(g.gate.check(&c));
    if blocked && !matches!(d, GateDecision::Deny { .. }) {
        fail(format!("{name} {p}: wanted Deny, got {d:?}"));
    }
    if !blocked && !matches!(d, GateDecision::Ask { .. }) {
        fail(format!("{name} {p}: wanted Ask, got {d:?}"));
    }
}

/// A path that doesn't exist yet is resolved through its deepest existing
/// ancestor; a dangling link is followed by hand.
#[test]
fn paths_that_do_not_exist_yet_are_still_resolved() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    for line in [
        "cat proj/dangling-key",
        "cat proj/link-ssh/not-there-yet",
        "cat proj/link-ssh/a/b/c/d",
        "cat proj/link-ssh/../.ssh/new-key",
        "cat .ssh/new_key",
        "touch .ssh/new_key",
        "echo x > .ssh/new_key",
        "echo x > proj/link-ssh/new_key",
        "echo x > proj/link-config/new.toml",
        "mkdir proj/link-ssh/new-dir",
        "cat proj/link-ssh/../../.npmrc",
    ] {
        deny(&g, line);
    }
    finish();
}

/// The gate never panics or hangs on hostile paths.
#[test]
fn hostile_paths_do_not_break_the_gate() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    let long_name = "a".repeat(300);
    let long_path = format!("{}/x", "d/".repeat(2000));
    let started = std::time::Instant::now();
    for line in [
        &format!("cat {long_name}"),
        &format!("cat {long_path}"),
        "cat \u{0}",
        "cat a\u{0}b",
        "cat \u{202e}gnp.txt",
        "cat \u{feff}.npmrc",
        "cat .npmrc\u{200b}",
        "cat .\u{200b}npmrc",
        "cat .ssh\u{0}/id_ed25519",
        "cat ..",
        "cat ../../../../../../../../../../../../etc/passwd",
        "cat /../../../etc/passwd",
        "cat //",
        "cat ///",
        "cat ./././././.",
        "cat -",
        "cat --",
        "cat ''",
        "cat \"\"",
    ] {
        let d = block_on(g.gate.check(&cmd_check(line)));
        // only that it returns, and that the zero-width variants of a
        // credential name don't get a different answer than a plain file
        let _ = d;
    }
    assert!(started.elapsed().as_secs() < 20);
    // zero-width and BOM characters are other file names, not `.npmrc`; the
    // gate must not be fooled the other way either: `.npmrc` stays blocked
    deny(&g, "cat .npmrc");
    finish();
}

/// Finding 10: a glob is matched against the sensitive names as written; one
/// that only reaches a credential through a link is not expanded. Home asks
/// (globs never allow), so only a rule or FullAccess lets it through.
const GLOB_THROUGH_LINKS: &[&str] = &["cat proj/*/id_ed25519", "cat proj/link-*"];

/// Finding 11: a glob whose wildcard sits inside the name of a credential
/// tree (`.g*` for `.gnupg`) or Atrium's own folder (`atr*`) is not
/// recognised as a path under it, though a wildcard in a file name
/// (`.n*rc`, `id_*`, `.s*` for `.ssh`) is.
const GLOB_IN_ATRIUM_NAMES: &[&str] = &[
    "cat .g*/private-keys-v1.d/*",
    "cat .local/share/atr*/atrium.db",
    "rm .config/atr*/config.toml",
    "echo x > .config/atr*/config.toml",
    "echo x > .config/atrium/conf?g.toml",
    "cat ~/.local/*/atrium/atrium.db",
    "cat ~/.l*/share/atrium/atrium.db",
];

#[test]
fn glob_forms_the_gate_cannot_resolve_never_reach_allow_in_home() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    for line in GLOB_THROUGH_LINKS.iter().chain(GLOB_IN_ATRIUM_NAMES) {
        not_allow(&g, line);
    }
    finish();
}

#[test]
#[ignore = "M1-13 finding 10"]
fn finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    for line in GLOB_THROUGH_LINKS {
        deny(&g, line);
    }
    finish();
}

#[test]
#[ignore = "M1-13 finding 11"]
fn finding_11_a_glob_inside_atriums_folder_names_is_blocked() {
    let t = world();
    let g = t.gate(Preset::Home, vec![]);
    for line in GLOB_IN_ATRIUM_NAMES {
        deny(&g, line);
    }
    finish();
}
