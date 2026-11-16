//! Tests for M1-29-cr17-stores: CR-17 credential stores, path-scope semantics,
//! and derived-unit / inline-code coverage under prefix rules.

mod common_adversarial;

use std::os::unix::fs::symlink;

use atrium_agents::{GateDecision, ToolGate};
use atrium_core::PolicyVerdict;
use atrium_core::policy::{DefaultPolicy, parse_command};
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope, ToolSubject};
use common_adversarial::*;

fn scoped_rule(
    scope: RuleScope,
    kind: ToolKind,
    matcher: RuleMatcher,
    decision: RuleDecision,
    path_scope: Option<&str>,
    id: &str,
) -> Rule {
    let mut r = rule(scope, kind, matcher, decision, id);
    r.path_scope = path_scope.map(str::to_string);
    r
}

// ---------------------------------------------------------------------------
// 1. CR-17: four more credential stores that ask in Home (never hard-denied)
// ---------------------------------------------------------------------------

#[test]
fn cr17_stores_ask_in_home_and_keep_allow_under_rules_and_full_access() {
    let env = Env::home();
    let any_exec = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-exec",
    )];
    let any_read = vec![rule(
        RuleScope::Global,
        ToolKind::Read,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-read",
    )];
    let any_search = vec![rule(
        RuleScope::Global,
        ToolKind::Search,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-search",
    )];

    let cmd_lines = [
        // Reading (home-anchored, $HOME, and relative in Home)
        "cat ~/.gem/credentials",
        "cat $HOME/.gem/credentials",
        "cat .gem/credentials",
        "cat ./.gem/credentials",
        "head -n 5 ~/.terraform.d/credentials.tfrc.json",
        "cat .terraform.d/credentials.tfrc.json",
        "cat ~/.config/op/config",
        "cat .config/op/config",
        "cat ~/.yarnrc.yml",
        "cat .yarnrc.yml",
        // Copying
        "cp ~/.gem/credentials /tmp/out",
        "cp ~/.terraform.d/credentials.tfrc.json /tmp/out",
        "cp ~/.config/op/config /tmp/out",
        "cp ~/.yarnrc.yml /tmp/out",
        // Searching
        "rg token ~/.gem/credentials",
        "rg token ~/.gem",
        "rg token ~/.terraform.d/credentials.tfrc.json",
        "rg token ~/.terraform.d",
        "rg token ~/.config/op/config",
        "rg token ~/.config/op",
        "rg token ~/.yarnrc.yml",
        "grep -r token ~/.gem",
        "grep -r token ~/.terraform.d",
        "grep -r token ~/.config/op",
        // git show / cat-file
        "git show HEAD:.gem/credentials",
        "git show HEAD:.terraform.d/credentials.tfrc.json",
        "git show HEAD:.config/op/config",
        "git show HEAD:.yarnrc.yml",
        "git cat-file -p HEAD:.gem/credentials",
        "git cat-file -p HEAD:.yarnrc.yml",
    ];
    for line in cmd_lines {
        let c = cmd_check(line);
        let home = env.eval_check(&c, &[], Preset::Home);
        assert_eq!(classify(&home), "Ask", "{line} [home]");
        assert!(
            matches!(
                env.eval_check(&c, &any_exec, Preset::Home),
                PolicyVerdict::Allow { .. }
            ),
            "{line} [home+any]"
        );
        assert!(
            matches!(
                env.eval_check(&c, &[], Preset::FullAccess),
                PolicyVerdict::Allow { .. }
            ),
            "{line} [full]"
        );
    }

    // File tools: Read and Search
    for p in [
        "~/.gem/credentials",
        ".gem/credentials",
        "~/.terraform.d/credentials.tfrc.json",
        ".terraform.d/credentials.tfrc.json",
        "~/.config/op/config",
        ".config/op/config",
        "~/.yarnrc.yml",
        ".yarnrc.yml",
    ] {
        let read = paths_check(ToolKind::Read, "Read", &[p], false, None);
        assert_eq!(
            classify(&env.eval_check(&read, &[], Preset::Home)),
            "Ask",
            "Read {p} [home]"
        );
        assert!(
            matches!(
                env.eval_check(&read, &any_read, Preset::Home),
                PolicyVerdict::Allow { .. }
            ),
            "Read {p} [home+any]"
        );
        assert!(
            matches!(
                env.eval_check(&read, &[], Preset::FullAccess),
                PolicyVerdict::Allow { .. }
            ),
            "Read {p} [full]"
        );
    }

    for dir in ["~/.gem", "~/.terraform.d", "~/.config/op"] {
        let search = paths_check(ToolKind::Search, "Grep", &[dir], true, None);
        assert_eq!(
            classify(&env.eval_check(&search, &[], Preset::Home)),
            "Ask",
            "Search {dir} [home]"
        );
        assert!(
            matches!(
                env.eval_check(&search, &any_search, Preset::Home),
                PolicyVerdict::Allow { .. }
            ),
            "Search {dir} [home+any]"
        );
        assert!(
            matches!(
                env.eval_check(&search, &[], Preset::FullAccess),
                PolicyVerdict::Allow { .. }
            ),
            "Search {dir} [full]"
        );
    }
}

#[test]
fn cr17_ordinary_siblings_keep_dev_verdict() {
    let env = Env::home();
    let ordinary_allow = [
        "cat ~/.yarnrc",
        "cat .yarnrc",
        "git show HEAD:.yarnrc",
        "cat ~/.gem/specs/index",
        "rg pat ~/.gem/specs",
        "git show HEAD:.gem/specs/index",
        "cat ~/.terraform.d/plugins/lock",
        "rg pat ~/.terraform.d/plugins",
        "git show HEAD:.terraform.d/plugins/lock",
    ];
    for line in ordinary_allow {
        assert_eq!(
            classify(&env.cmd(line)),
            "Allow",
            "{line} should stay Allow in Home"
        );
    }

    for p in [
        "~/.yarnrc",
        "~/.gem/specs/index",
        "~/.terraform.d/plugins/lock",
    ] {
        let read = paths_check(ToolKind::Read, "Read", &[p], false, None);
        assert_eq!(
            classify(&env.eval_check(&read, &[], Preset::Home)),
            "Allow",
            "Read {p}"
        );
    }
    for dir in ["~/.gem/specs", "~/.terraform.d/plugins"] {
        let search = paths_check(ToolKind::Search, "Grep", &[dir], true, None);
        assert_eq!(
            classify(&env.eval_check(&search, &[], Preset::Home)),
            "Allow",
            "Search {dir}"
        );
    }
}

#[test]
fn cr17_gate_catches_symlinks_and_hardlinks_to_new_stores() {
    let t = Tmp::new();
    t.write(".gem/credentials", "gem-secret");
    t.write(".terraform.d/credentials.tfrc.json", "tf-secret");
    t.write(".config/op/config", "op-secret");
    t.write(".yarnrc.yml", "yarn-secret");
    t.write("proj/safe.txt", "safe");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();

    symlink(t.home.join(".gem/credentials"), t.home.join("proj/gem_sym")).unwrap();
    symlink(t.home.join(".yarnrc.yml"), t.home.join("proj/yarn_sym")).unwrap();
    std::fs::hard_link(
        t.home.join(".terraform.d/credentials.tfrc.json"),
        t.home.join("proj/tf_hard"),
    )
    .unwrap();
    std::fs::hard_link(
        t.home.join(".config/op/config"),
        t.home.join("proj/op_hard"),
    )
    .unwrap();

    let g = t.gate(Preset::Home, vec![]);
    for rel in [
        "proj/gem_sym",
        "proj/yarn_sym",
        "proj/tf_hard",
        "proj/op_hard",
    ] {
        let cmd = format!("cat {rel}");
        assert!(
            matches!(
                block_on(g.gate.check(&cmd_check(&cmd))),
                GateDecision::Ask { .. }
            ),
            "{cmd}"
        );
        let read = paths_check(ToolKind::Read, "Read", &[rel], false, None);
        assert!(
            matches!(block_on(g.gate.check(&read)), GateDecision::Ask { .. }),
            "Read {rel}"
        );
    }
    assert_eq!(
        block_on(g.gate.check(&cmd_check("cat proj/safe.txt"))),
        GateDecision::Allow
    );
}

// ---------------------------------------------------------------------------
// 2. Path-scope semantics (D-121 / M1-23 escalation)
// ---------------------------------------------------------------------------

#[test]
fn path_scope_allow_and_deny_for_commands_and_file_tools() {
    let home = Env::home();
    let mut in_work = Env::home();
    in_work.cwd = format!("{HOME}/work").into();

    let allow_rm_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["rm"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-rm-work",
    )];
    let allow_cargo_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["cargo", "test"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-cargo-work",
    )];
    let allow_git_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["git"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-git-work",
    )];
    let allow_cp_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["cp"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-cp-work",
    )];

    // Scoped allow matches in Home when every named path is inside the scope
    assert_eq!(
        classify(&home.cmd_rules("rm ~/work/a.txt", &allow_rm_work)),
        "Allow"
    );
    assert_eq!(
        classify(&home.cmd_rules(
            "cargo test --manifest-path ~/work/Cargo.toml",
            &allow_cargo_work
        )),
        "Allow"
    );
    assert_eq!(
        classify(&home.cmd_rules("git -C ~/work/repo push", &allow_git_work)),
        "Allow"
    );
    // And in another cwd when relative paths resolve into scope
    assert_eq!(
        classify(&in_work.cmd_rules("rm ./a.txt", &allow_rm_work)),
        "Allow"
    );
    // And via `ToolSubject::Command { cwd: Some(...) }`
    let cmd_with_cwd = atrium_agents::ToolCheck {
        call_id: None,
        name: "Bash".into(),
        kind: ToolKind::Execute,
        title: None,
        input: serde_json::json!({ "command": "rm ./a.txt", "cwd": "/h/u/work" }),
        subject: ToolSubject::Command {
            line: "rm ./a.txt".into(),
            cwd: Some("/h/u/work".into()),
        },
        turn: 1,
    };
    assert_eq!(
        classify(&home.eval_check(&cmd_with_cwd, &allow_rm_work, Preset::Home)),
        "Allow"
    );
    // Relative path in Home resolves outside ~/work/** -> Ask
    assert_eq!(
        classify(&home.cmd_rules("rm ./a.txt", &allow_rm_work)),
        "Ask"
    );
    // Multi-path command with one path outside scope -> Ask
    assert_eq!(
        classify(&home.cmd_rules("cp ~/work/a.txt ~/other/b.txt", &allow_cp_work)),
        "Ask"
    );
    assert_eq!(
        classify(&home.cmd_rules("cp ~/work/a.txt ~/work/b.txt", &allow_cp_work)),
        "Allow"
    );
    // A command that names NO path is never covered by a scoped allow rule (in Home or in ~/work)
    assert_eq!(
        classify(&home.cmd_rules("cargo test", &allow_cargo_work)),
        "Ask"
    );
    assert_eq!(
        classify(&in_work.cmd_rules("cargo test", &allow_cargo_work)),
        "Ask"
    );

    // Scoped deny rule matches if ANY named path is inside scope (fail closed)
    let deny_cp_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["cp"]),
        RuleDecision::Deny,
        Some("/h/u/work/**"),
        "deny-cp-work",
    )];
    let deny_git_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        exact(&["git", "push"]),
        RuleDecision::Deny,
        Some("/h/u/work/**"),
        "deny-git-work",
    )];
    assert!(matches!(
        home.cmd_rules("cp ~/work/a.txt ~/other/b.txt", &deny_cp_work),
        PolicyVerdict::Deny { .. }
    ));
    assert!(matches!(
        home.cmd_rules("cp ~/other/a.txt ~/work/b.txt", &deny_cp_work),
        PolicyVerdict::Deny { .. }
    ));
    assert_eq!(
        classify(&home.cmd_rules("cp ~/other/a.txt ~/other/b.txt", &deny_cp_work)),
        "Ask"
    );
    assert!(matches!(
        home.cmd_rules("cd ~/work/repo && git push", &deny_git_work),
        PolicyVerdict::Deny { .. }
    ));
    assert_eq!(
        classify(&in_work.cmd_rules("git push", &deny_git_work)),
        "Ask"
    );

    // File tools: single & multi-path, allow vs deny
    let allow_edit_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Edit,
        RuleMatcher::Any,
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-edit-work",
    )];
    let deny_edit_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Edit,
        RuleMatcher::Any,
        RuleDecision::Deny,
        Some("/h/u/work/**"),
        "deny-edit-work",
    )];
    let inside = paths_check(ToolKind::Edit, "Edit", &["~/work/a.rs"], false, None);
    let multi = paths_check(
        ToolKind::Edit,
        "Edit",
        &["~/work/a.rs", "~/other/b.rs"],
        false,
        None,
    );
    let outside = paths_check(ToolKind::Edit, "Edit", &["~/other/b.rs"], false, None);
    let empty = paths_check(ToolKind::Edit, "Edit", &[], false, None);

    assert_eq!(
        classify(&home.eval_check(&inside, &allow_edit_work, Preset::Home)),
        "Allow"
    );
    assert_eq!(
        classify(&home.eval_check(&multi, &allow_edit_work, Preset::Home)),
        "Ask"
    );
    assert_eq!(
        classify(&home.eval_check(&empty, &allow_edit_work, Preset::Home)),
        "Ask"
    );
    assert!(matches!(
        home.eval_check(&inside, &deny_edit_work, Preset::Home),
        PolicyVerdict::Deny { .. }
    ));
    assert!(matches!(
        home.eval_check(&multi, &deny_edit_work, Preset::Home),
        PolicyVerdict::Deny { .. }
    ));
    assert_eq!(
        classify(&home.eval_check(&outside, &deny_edit_work, Preset::Home)),
        "Ask"
    );
}

// ---------------------------------------------------------------------------
// 3. Derived units, `find -delete`, `xargs`, and inline code under prefix rules
// ---------------------------------------------------------------------------

#[test]
fn find_prefix_rules_require_derived_units_to_be_covered_and_reject_delete() {
    let env = Env::home();
    let mut proj = Env::home();
    proj.cwd = format!("{HOME}/proj").into();
    let find_rule = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["find"]),
        RuleDecision::Allow,
        "allow-find",
    )];
    let find_dot_rule = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["find", "."]),
        RuleDecision::Allow,
        "allow-find-dot",
    )];

    // Ordinary `find` lines keep their Allow verdict
    for rules in [&find_rule, &find_dot_rule] {
        assert_eq!(
            classify(&env.cmd_rules("find . -name '*.rs'", rules)),
            "Allow"
        );
        // Filtered `-delete` in a project folder is inside cwd (not hard-denied),
        // yet never covered by a prefix rule on `find` or `find .`.
        assert_eq!(
            classify(&proj.cmd_rules("find . -name '*.o' -delete", rules)),
            "Ask"
        );
        // `-exec` / `-execdir` / `-ok` / `-okdir` ask without a covering rule
        for exec_cmd in [
            "find . -exec rm {} +",
            "find . -execdir rm {} +",
            "find . -ok rm {} ;",
            "find . -okdir rm {} ;",
            "find . -exec echo {} +",
            "find . -exec sh -c 'rm -rf build' ;",
        ] {
            assert_eq!(
                classify(&env.cmd_rules(exec_cmd, rules)),
                "Ask",
                "{exec_cmd}"
            );
        }
    }

    // `-delete` targeting a subfolder strictly inside the working folder is not
    // hard-denied, yet never covered by a `find` prefix rule (even unfiltered).
    assert_eq!(
        classify(&env.cmd_rules("find build -delete", &find_rule)),
        "Ask"
    );
    assert_eq!(
        classify(&env.cmd_rules("find build -name '*.o' -delete", &find_rule)),
        "Ask"
    );
    assert_eq!(
        classify(&proj.cmd_rules("find build -delete", &find_rule)),
        "Ask"
    );

    // Deleting `~` or unfiltered `.` keeps the existing safety hard deny.
    for (ctx, line) in [
        (&env, "find . -delete"),
        (&env, "find . -name '*.o' -delete"),
        (&proj, "find . -delete"),
    ] {
        assert!(
            matches!(
                ctx.cmd_rules(line, &find_rule),
                PolicyVerdict::HardDeny {
                    id: "recursive-delete-outside-cwd"
                }
            ),
            "{line}"
        );
    }

    // Uppercase `FIND . -EXEC` also fails closed
    let upper_find = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["FIND"]),
        RuleDecision::Allow,
        "allow-FIND",
    )];
    assert_eq!(
        classify(&env.cmd_rules("FIND . -EXEC rm {} +", &upper_find)),
        "Ask"
    );

    // When the derived unit IS covered by another allow rule, `-exec` is allowed
    let find_and_echo = vec![
        find_rule[0].clone(),
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["echo"]),
            RuleDecision::Allow,
            "allow-echo",
        ),
    ];
    assert_eq!(
        classify(&env.cmd_rules("find . -exec echo {} +", &find_and_echo)),
        "Allow"
    );

    // A lower-ranked Deny rule on the derived command still denies even if top-level `find` is Thread-allowed
    let thread_find_global_deny_rm = vec![
        rule(
            RuleScope::Thread {
                thread_id: env.thread.clone(),
            },
            ToolKind::Execute,
            prefix(&["find"]),
            RuleDecision::Allow,
            "t-find",
        ),
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["rm"]),
            RuleDecision::Deny,
            "g-deny-rm",
        ),
    ];
    assert!(matches!(
        env.cmd_rules("find . -exec rm {} +", &thread_find_global_deny_rm),
        PolicyVerdict::Deny { .. }
    ));

    // Exact rule on `-delete` inside the working folder keeps working
    let exact_delete = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        exact(&["find", "build", "-delete"]),
        RuleDecision::Allow,
        "exact-del",
    )];
    assert_eq!(
        classify(&env.cmd_rules("find build -delete", &exact_delete)),
        "Allow"
    );
    let exact_proj_delete = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        exact(&["find", ".", "-name", "*.o", "-delete"]),
        RuleDecision::Allow,
        "exact-proj-del",
    )];
    assert_eq!(
        classify(&proj.cmd_rules("find . -name '*.o' -delete", &exact_proj_delete)),
        "Allow"
    );
}

#[test]
fn xargs_and_inline_code_and_shells_never_bypass_prefix_rules() {
    let env = Env::home();
    let echo = rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["echo"]),
        RuleDecision::Allow,
        "allow-echo",
    );
    let xargs = rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["xargs"]),
        RuleDecision::Allow,
        "allow-xargs",
    );
    let rm = rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["rm"]),
        RuleDecision::Allow,
        "allow-rm",
    );

    // `xargs` target must also be covered
    assert_eq!(
        classify(&env.cmd_rules("echo a | xargs rm", &[echo.clone(), xargs.clone()])),
        "Ask"
    );
    assert_eq!(
        classify(&env.cmd_rules("echo a | xargs", &[echo.clone(), xargs.clone()])),
        "Ask"
    );
    assert_eq!(
        classify(&env.cmd_rules("echo a | xargs rm", &[echo.clone(), xargs.clone(), rm])),
        "Allow"
    );

    // Shells / eval never allow via CommandPrefix
    for sh_cmd in [
        ("sh", "sh -c 'rm -rf build'"),
        ("bash", "bash -c 'echo $HOME'"),
        ("eval", "eval 'rm -rf build'"),
    ] {
        let r = vec![
            rule(
                RuleScope::Global,
                ToolKind::Execute,
                prefix(&[sh_cmd.0]),
                RuleDecision::Allow,
                "sh",
            ),
            echo.clone(),
        ];
        assert_eq!(
            classify(&env.cmd_rules(sh_cmd.1, &r)),
            "Ask",
            "{}",
            sh_cmd.1
        );
    }

    // Interpreters & package runners with inline code or stdin are never covered by a prefix rule
    for (bin, bad_lines, good_lines) in [
        (
            "python3",
            &[
                "python3 -c 'import os'",
                "python3 -Ic 'import os'",
                "python3 -u -c 'import os'",
                "python3 -",
                "python3 /dev/stdin",
                "python3",
            ][..],
            &["python3 scripts/build.py"][..],
        ),
        (
            "node",
            &["node -e '1'", "node --eval=1", "node -p 1", "node"][..],
            &["node build.js"][..],
        ),
        (
            "perl",
            &["perl -e '1'", "perl -0e '1'", "perl"][..],
            &["perl script.pl"][..],
        ),
        (
            "ruby",
            &["ruby -e '1'", "ruby -0e '1'", "ruby"][..],
            &["ruby script.rb"][..],
        ),
        (
            "php",
            &["php -r '1;'", "php --run '1;'", "php"][..],
            &["php script.php"][..],
        ),
        ("lua", &["lua -e '1'", "lua"][..], &["lua script.lua"][..]),
        (
            "deno",
            &["deno eval '1'", "deno -q eval '1'", "deno run", "deno"][..],
            &["deno run x.ts"][..],
        ),
        (
            "bun",
            &["bun exec 'rm'", "bun eval '1'", "bun -e '1'", "bun run"][..],
            &["bun run build"][..],
        ),
        (
            "pwsh",
            &["pwsh -c 'ls'", "pwsh -Command 'ls'", "pwsh -ec AA", "pwsh"][..],
            &["pwsh -File build.ps1"][..],
        ),
        (
            "npx",
            &["npx -c 'sh'", "npx -yc 'sh'", "npx --call=sh", "npx"][..],
            &["npx prettier --write"][..],
        ),
    ] {
        let rules = vec![rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&[bin]),
            RuleDecision::Allow,
            "interp",
        )];
        for bad in bad_lines {
            assert_eq!(classify(&env.cmd_rules(bad, &rules)), "Ask", "{bad}");
        }
        for good in good_lines {
            assert_eq!(classify(&env.cmd_rules(good, &rules)), "Allow", "{good}");
        }
    }

    // A pinned script prefix allows flags (even `-c`) passed to the script afterwards
    let pinned_py = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["python3", "scripts/build.py"]),
        RuleDecision::Allow,
        "pinned-py",
    )];
    assert_eq!(
        classify(&env.cmd_rules("python3 scripts/build.py -c prod.toml", &pinned_py)),
        "Allow"
    );

    // Check `parse_command` contract wasn't changed
    let parsed = parse_command("find . -name '*.rs'", env.home.as_path());
    assert!(matches!(
        parsed,
        atrium_core::policy::ParsedCommand::Segments(_)
    ));
    let _ = DefaultPolicy::new();
}
