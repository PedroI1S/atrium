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
            r"find . -ok rm {} \;",
            r"find . -okdir rm {} \;",
            "find . -exec echo {} +",
            "find . -exec sh -c 'rm -rf build' ;",
            r"find . -exec sh -c 'rm -rf build' \;",
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
    assert_eq!(
        classify(&env.cmd_rules(r"find . -ok echo {} \;", &find_and_echo)),
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

// ---------------------------------------------------------------------------
// 4. Rework round 3: review findings & CR-17 test gaps
// ---------------------------------------------------------------------------

#[test]
fn round3_finding1_path_scope_retains_bare_and_whitespace_operands() {
    let home = Env::home();
    let mut in_work = Env::home();
    in_work.cwd = format!("{HOME}/work").into();

    let allow_cp_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["cp"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-cp-work",
    )];
    let deny_rm_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["rm"]),
        RuleDecision::Deny,
        Some("/h/u/work/**"),
        "deny-rm-work",
    )];

    // Exact reviewer cases: bare destination outside scope must Ask; bare filename in work under Deny must Deny
    assert_eq!(
        classify(&home.cmd_rules("cp /h/u/work/a.txt b.txt", &allow_cp_work)),
        "Ask"
    );
    assert!(matches!(
        in_work.cmd_rules("rm a.txt", &deny_rm_work),
        PolicyVerdict::Deny { .. }
    ));

    // Whitespace-containing operands (quoted) are retained and checked against scope
    assert_eq!(
        classify(&home.cmd_rules("cp /h/u/work/a.txt \"my file.txt\"", &allow_cp_work)),
        "Ask"
    );
    assert_eq!(
        classify(&home.cmd_rules(
            "cp /h/u/work/a.txt \"/h/u/work/my file.txt\"",
            &allow_cp_work
        )),
        "Allow"
    );
    assert!(matches!(
        in_work.cmd_rules("rm \"my file.txt\"", &deny_rm_work),
        PolicyVerdict::Deny { .. }
    ));

    // Resolved symlink identities for bare filenames are retained by SessionGate
    let t = Tmp::new();
    t.write("work/a.txt", "inside");
    t.write("outside.txt", "outside");
    std::fs::create_dir_all(t.home.join("work")).unwrap();
    symlink(t.home.join("outside.txt"), t.home.join("work/link.txt")).unwrap();
    let work_glob = format!("{}/work/**", t.home.display());
    let g_allow = t.gate(
        Preset::Home,
        vec![scoped_rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["cp"]),
            RuleDecision::Allow,
            Some(&work_glob),
            "cp-work",
        )],
    );
    assert!(matches!(
        block_on(g_allow.gate.check(&cmd_check("cp work/a.txt out.txt"))),
        GateDecision::Ask { .. }
    ));
    assert!(matches!(
        block_on(
            g_allow
                .gate
                .check(&cmd_check("cp work/a.txt work/link.txt"))
        ),
        GateDecision::Ask { .. }
    ));
    let g_in_work = t.gate_with(
        "t1",
        1,
        &t.home.join("work"),
        Preset::Home,
        vec![scoped_rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["rm"]),
            RuleDecision::Deny,
            Some(&work_glob),
            "rm-work",
        )],
        None,
    );
    assert!(matches!(
        block_on(g_in_work.gate.check(&cmd_check("rm a.txt"))),
        GateDecision::Deny { .. }
    ));
    let mut work_env = Env::home();
    work_env.cwd = format!("{HOME}/work").into();
    // `rm -rf` flag letters don't masquerade as paths, while operand `a.txt` is checked
    assert_eq!(
        classify(&home.cmd_rules(
            "rm -rf ~/work/a.txt",
            &[scoped_rule(
                RuleScope::Global,
                ToolKind::Execute,
                prefix(&["rm"]),
                RuleDecision::Allow,
                Some("/h/u/work/**"),
                "allow-rm-work",
            )]
        )),
        "Allow"
    );
    assert_eq!(
        classify(&work_env.cmd_rules(
            "head -n 5 a.txt",
            &[scoped_rule(
                RuleScope::Global,
                ToolKind::Execute,
                prefix(&["head"]),
                RuleDecision::Allow,
                Some("/h/u/work/**"),
                "allow-head-work",
            )]
        )),
        "Allow"
    );
}

#[test]
fn round3_finding2_per_command_find_derived_guard_and_folded_spelling() {
    let env = Env::home();
    let rules = vec![
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["find"]),
            RuleDecision::Allow,
            "allow-find",
        ),
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["/usr/bin/FIND"]),
            RuleDecision::Allow,
            "allow-upper-find",
        ),
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["echo"]),
            RuleDecision::Allow,
            "allow-echo",
        ),
    ];

    // Reviewer's exact case: first `find` supplies a covered `echo` derived unit,
    // second `/usr/bin/FIND` runs unextracted `-exec rm {} +` -> must Ask.
    assert_eq!(
        classify(&env.cmd_rules(
            "find ./safe -exec echo {} + ; /usr/bin/FIND ./safe -exec rm {} +",
            &rules
        )),
        "Ask"
    );
    // Mixed-case `-EXEC` on second `find` also fails closed -> must Ask.
    assert_eq!(
        classify(&env.cmd_rules(
            "find ./safe -exec echo {} + ; find ./safe -EXEC rm {} +",
            &rules
        )),
        "Ask"
    );
    // Both extracted and covered -> Allow.
    assert_eq!(
        classify(&env.cmd_rules(
            "find ./safe -exec echo a {} + ; find ./safe -exec echo b {} +",
            &rules
        )),
        "Allow"
    );
}

#[test]
fn round3_finding3_git_prefix_does_not_cover_cli_shell_aliases() {
    let env = Env::home();
    let git_prefix = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["git"]),
        RuleDecision::Allow,
        "allow-git",
    )];

    // Reviewer's exact case: command-line `-c alias.clean='!rm -f victim' clean` must Ask
    assert_eq!(
        classify(&env.cmd_rules("git -c alias.clean='!rm -f victim' clean", &git_prefix)),
        "Ask"
    );
    assert_eq!(
        classify(&env.cmd_rules("git -c core.fsmonitor='rm -rf .' status", &git_prefix)),
        "Ask"
    );
    assert_eq!(
        classify(&env.cmd_rules("git --exec-path=/tmp/bin status", &git_prefix)),
        "Ask"
    );
    assert_eq!(
        classify(&env.cmd_rules("git --config-env=core.pager=MY_PAGER log", &git_prefix)),
        "Ask"
    );
    // Ordinary git commands (including `git commit -c HEAD` where `-c` is `--reedit-message`) stay Allow
    assert_eq!(classify(&env.cmd_rules("git status", &git_prefix)), "Allow");
    assert_eq!(
        classify(&env.cmd_rules("git commit -c HEAD", &git_prefix)),
        "Allow"
    );
}

#[test]
fn round3_finding4_interpreter_option_values_not_mistaken_for_scripts() {
    let env = Env::home();
    let rules = vec![
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            exact(&["python3", "-W", "ignore"]),
            RuleDecision::Allow,
            "exact-py-w",
        ),
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["echo"]),
            RuleDecision::Allow,
            "allow-echo",
        ),
    ];

    // Reviewer's exact case: `ignore` is `-W`'s value, not a script -> pipeline must Ask
    assert_eq!(
        classify(&env.cmd_rules("echo 'print(1)' | python3 -W ignore", &rules)),
        "Ask"
    );

    // Valid exact rule with `-W ignore` AND a named script still Allows
    let valid_exact = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        exact(&["python3", "-W", "ignore", "build.py"]),
        RuleDecision::Allow,
        "exact-py-w-script",
    )];
    assert_eq!(
        classify(&env.cmd_rules("python3 -W ignore build.py", &valid_exact)),
        "Allow"
    );
}

#[test]
fn round3_finding5_execute_path_glob_cannot_allow_commands() {
    let env = Env::home();
    let bad_rule = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::PathGlob {
            glob: "/h/u/work/**".into(),
        },
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "bad-exec-path",
    )];

    // Reviewer's exact case: invalid Execute + PathGlob rule must not allow commands
    assert_eq!(
        classify(&env.cmd_rules("python3 -c pass /h/u/work/input", &bad_rule)),
        "Ask"
    );
}

#[test]
fn round3_finding8_and_cr17_test_gaps() {
    let env = Env::home();
    let any_exec = [rule(
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-exec",
    )];

    // Finding 8: `~/.git-credentials` is HardDeny(credential-read), even with Allow Any
    for rules in [&[][..], &any_exec[..]] {
        assert!(matches!(
            env.cmd_rules("cat ~/.git-credentials", rules),
            PolicyVerdict::HardDeny {
                id: "credential-read"
            }
        ));
    }

    // CR-17 `..` traversal spellings and glob spellings ask in Home
    for line in [
        "cat ~/proj/../.gem/credentials",
        "cat .terraform.d/../.terraform.d/credentials.tfrc.json",
        "cat ~/.config/op/../op/config",
        "cat ~/proj/../.yarnrc.yml",
        "cat ~/.gem/*",
        "cat ~/.config/op/*",
    ] {
        assert_eq!(classify(&env.cmd(line)), "Ask", "{line}");
    }
    for p in [
        "proj/../.gem/credentials",
        "proj/../.terraform.d/credentials.tfrc.json",
        "proj/../.config/op/config",
        "proj/../.yarnrc.yml",
    ] {
        let read = paths_check(ToolKind::Read, "Read", &[p], false, None);
        assert_eq!(
            classify(&env.eval_check(&read, &[], Preset::Home)),
            "Ask",
            "Read {p}"
        );
    }
    for (dir, pat) in [
        ("~/.gem", "*credentials*"),
        ("~/.terraform.d", "credentials*"),
        ("~/.config/op", "*"),
        ("~", ".yarnrc*"),
    ] {
        let search = paths_check(ToolKind::Search, "Glob", &[dir], true, Some(pat));
        assert_eq!(
            classify(&env.eval_check(&search, &[], Preset::Home)),
            "Ask",
            "Search {dir} {pat}"
        );
    }
    // Recursive `*.yml` search from `~` reaches `~/.config/gh/hosts.yml` (hard-denied in `CRED_FILES`).
    let yml_search = paths_check(ToolKind::Search, "Glob", &["~"], true, Some("*.yml"));
    assert!(matches!(
        env.eval_check(&yml_search, &[], Preset::Home),
        PolicyVerdict::HardDeny {
            id: "credential-read"
        }
    ));

    // CR-17 linked Search calls and remembered rules through symlinks in SessionGate
    let t = Tmp::new();
    t.write(".gem/credentials", "gem-secret");
    t.write(".terraform.d/credentials.tfrc.json", "tf-secret");
    t.write(".config/op/config", "op-secret");
    t.write(".yarnrc.yml", "yarn-secret");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    symlink(t.home.join(".gem"), t.home.join("proj/gem_dir_sym")).unwrap();
    symlink(t.home.join(".config/op"), t.home.join("proj/op_dir_sym")).unwrap();
    symlink(t.home.join(".gem/credentials"), t.home.join("proj/gem_sym")).unwrap();
    symlink(t.home.join(".yarnrc.yml"), t.home.join("proj/yarn_sym")).unwrap();
    std::fs::hard_link(
        t.home.join(".terraform.d/credentials.tfrc.json"),
        t.home.join("proj/tf_hard"),
    )
    .unwrap();

    let g_home = t.gate(Preset::Home, vec![]);
    for dir in ["proj/gem_dir_sym", "proj/op_dir_sym"] {
        let search = paths_check(ToolKind::Search, "Grep", &[dir], true, None);
        assert!(
            matches!(
                block_on(g_home.gate.check(&search)),
                GateDecision::Ask { .. }
            ),
            "Search {dir}"
        );
        let rg_cmd = cmd_check(&format!("rg secret {dir}"));
        assert!(
            matches!(
                block_on(g_home.gate.check(&rg_cmd)),
                GateDecision::Ask { .. }
            ),
            "rg {dir}"
        );
    }

    // Remembered rules allow CR-17 symlinked reads & searches
    let gem_glob = t
        .home
        .join(".gem/credentials")
        .to_string_lossy()
        .to_string();
    let g_rules = t.gate(
        Preset::Home,
        vec![
            rule(
                RuleScope::Global,
                ToolKind::Read,
                RuleMatcher::PathGlob { glob: gem_glob },
                RuleDecision::Allow,
                "allow-gem-read",
            ),
            rule(
                RuleScope::Global,
                ToolKind::Read,
                RuleMatcher::Any,
                RuleDecision::Allow,
                "allow-any-read",
            ),
            rule(
                RuleScope::Global,
                ToolKind::Search,
                RuleMatcher::Any,
                RuleDecision::Allow,
                "allow-any-search",
            ),
        ],
    );
    assert_eq!(
        block_on(g_rules.gate.check(&paths_check(
            ToolKind::Read,
            "Read",
            &["proj/gem_sym"],
            false,
            None
        ))),
        GateDecision::Allow
    );
    assert_eq!(
        block_on(g_rules.gate.check(&paths_check(
            ToolKind::Read,
            "Read",
            &["proj/yarn_sym"],
            false,
            None
        ))),
        GateDecision::Allow
    );
    assert_eq!(
        block_on(g_rules.gate.check(&paths_check(
            ToolKind::Search,
            "Grep",
            &["proj/gem_dir_sym"],
            true,
            None
        ))),
        GateDecision::Allow
    );

    // DefaultPolicy allows hard-linked CR-17 store once mapped to its canonical ask-store path
    // (and SessionGate::check also preserves Allow for positively identified ask-store aliases
    // under explicit rules — see `round5_finding4_gate_hardlink_remembered_rule`).
    let read_hard = paths_check(ToolKind::Read, "Read", &["proj/tf_hard"], false, None);
    let mapped_input = atrium_core::PolicyInput {
        check: &read_hard,
        thread: &env.thread,
        provider: atrium_protocol::agent::Provider::Claude,
        cwd: &env.cwd,
        home: &env.home,
        atrium_data_paths: &env.data,
        atrium_protected_paths: &env.prot,
        resolved_paths: &[env.home.join(".terraform.d/credentials.tfrc.json")],
        preset: Preset::Home,
    };
    let read_any = vec![rule(
        RuleScope::Global,
        ToolKind::Read,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "allow-any-read",
    )];
    assert!(matches!(
        atrium_core::PolicyEngine::evaluate(&DefaultPolicy::new(), &mapped_input, &read_any),
        PolicyVerdict::Allow { .. }
    ));
}

// ---------------------------------------------------------------------------
// 6. Rework round 5: reproducing cases for review-round2.md findings 1-4 & 6
// ---------------------------------------------------------------------------

#[test]
fn round5_finding1_path_filtering_retains_script_and_git_pathspec_operands() {
    let home = Env::home();
    let mut in_work = Env::home();
    in_work.cwd = format!("{HOME}/work").into();

    let allow_sed_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["sed"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-sed-work",
    )];
    // `sed 'w /h/u/out.txt' /h/u/work/input.txt`: script writes outside scope -> Ask
    assert_eq!(
        classify(&home.cmd_rules("sed 'w /h/u/out.txt' /h/u/work/input.txt", &allow_sed_work)),
        "Ask"
    );
    assert_eq!(
        classify(&home.cmd_rules(
            "sed -e 'w /h/u/out.txt' /h/u/work/input.txt",
            &allow_sed_work
        )),
        "Ask"
    );
    assert_eq!(
        classify(&home.cmd_rules("sed 'w out.txt' /h/u/work/input.txt", &allow_sed_work)),
        "Ask"
    );
    // Ordinary sed scripts and in-scope script writes keep Allow
    assert_eq!(
        classify(&home.cmd_rules(
            "sed 'w /h/u/work/out.txt' /h/u/work/input.txt",
            &allow_sed_work
        )),
        "Allow"
    );
    assert_eq!(
        classify(&home.cmd_rules("sed 's/foo/bar/' /h/u/work/input.txt", &allow_sed_work)),
        "Allow"
    );
    assert_eq!(
        classify(&home.cmd_rules("sed '/foo/d' /h/u/work/input.txt", &allow_sed_work)),
        "Allow"
    );

    let allow_awk_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["awk"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-awk-work",
    )];
    assert_eq!(
        classify(&home.cmd_rules(
            "awk '{ print > \"/h/u/out.txt\" }' /h/u/work/input.txt",
            &allow_awk_work
        )),
        "Ask"
    );

    // `diff -u` is a boolean flag, not a value-taking flag
    let allow_diff_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["diff"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-diff-work",
    )];
    assert_eq!(
        classify(&home.cmd_rules("diff -u /h/u/other/a.txt /h/u/work/b.txt", &allow_diff_work)),
        "Ask"
    );
    let deny_diff_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["diff"]),
        RuleDecision::Deny,
        Some("/h/u/work/**"),
        "deny-diff-work",
    )];
    assert!(matches!(
        home.cmd_rules("diff -u /h/u/work/a.txt /h/u/other/b.txt", &deny_diff_work),
        PolicyVerdict::Deny { .. }
    ));

    // Git pathspecs in `cwd=/h/u/work` under Deny prefix `["git"]` scoped `/h/u/work/**`
    let deny_git_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["git"]),
        RuleDecision::Deny,
        Some("/h/u/work/**"),
        "deny-git-work",
    )];
    for cmd in [
        "git status a.txt",
        "git diff a.txt",
        "git log a.txt",
        "git checkout a.txt",
    ] {
        assert!(
            matches!(
                in_work.cmd_rules(cmd, &deny_git_work),
                PolicyVerdict::Deny { .. }
            ),
            "expected Deny for {cmd}"
        );
    }
    // Non-path ref/remote arguments for `git push` stay Ask under scoped Deny
    assert_eq!(
        classify(&in_work.cmd_rules("git push origin main", &deny_git_work)),
        "Ask"
    );

    // Scoped Allow `git` fails closed on pathspec/unknown operands outside scope
    let allow_git_work = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["git"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-git-work",
    )];
    assert_eq!(
        classify(&home.cmd_rules("git -C ~/work/repo status a.txt", &allow_git_work)),
        "Ask"
    );
    assert_eq!(
        classify(&home.cmd_rules("git -C ~/work/repo push origin main", &allow_git_work)),
        "Allow"
    );
}

#[test]
fn round5_finding2_pwsh_option_values_and_inline_code() {
    let env = Env::home();
    let bad_pwsh_rule = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["pwsh", "-WorkingDirectory", "work.ps1"]),
        RuleDecision::Allow,
        "allow-pwsh-wd",
    )];
    assert_eq!(
        classify(&env.cmd_rules(
            "pwsh -WorkingDirectory work.ps1 -Command 'Remove-Item victim'",
            &bad_pwsh_rule
        )),
        "Ask"
    );

    let bad_pwsh_ep = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["pwsh", "-ExecutionPolicy", "Bypass"]),
        RuleDecision::Allow,
        "allow-pwsh-ep",
    )];
    assert_eq!(
        classify(&env.cmd_rules(
            "pwsh -ExecutionPolicy Bypass -Command 'Remove-Item victim'",
            &bad_pwsh_ep
        )),
        "Ask"
    );

    // Valid pinned script rule still allows script invocation
    let good_pwsh_rule = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&[
            "pwsh",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            "build.ps1",
        ]),
        RuleDecision::Allow,
        "allow-pwsh-build",
    )];
    assert_eq!(
        classify(&env.cmd_rules(
            "pwsh -NoProfile -ExecutionPolicy Bypass -File build.ps1",
            &good_pwsh_rule
        )),
        "Allow"
    );
}

#[test]
fn round5_finding3_stdin_alias_normalization() {
    let env = Env::home();
    let rules = vec![
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["echo"]),
            RuleDecision::Allow,
            "allow-echo",
        ),
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["python3", "/dev/./stdin"]),
            RuleDecision::Allow,
            "allow-py-dot-stdin",
        ),
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["node", "/dev//fd/0"]),
            RuleDecision::Allow,
            "allow-node-fd0",
        ),
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&["perl", "/proc/self/./fd/0"]),
            RuleDecision::Allow,
            "allow-perl-proc-fd0",
        ),
    ];
    for line in [
        "echo 'print(1)' | python3 /dev/./stdin",
        "echo 'console.log(1)' | node /dev//fd/0",
        "echo 'print 1' | perl /proc/self/./fd/0",
    ] {
        assert_eq!(classify(&env.cmd_rules(line, &rules)), "Ask", "{line}");
    }
}

#[test]
fn round5_finding4_gate_hardlink_remembered_rule() {
    let t = Tmp::new();
    t.write(".terraform.d/credentials.tfrc.json", "tf-secret");
    t.write("other/plain.txt", "plain");
    std::fs::create_dir_all(t.home.join("proj")).unwrap();
    std::fs::hard_link(
        t.home.join(".terraform.d/credentials.tfrc.json"),
        t.home.join("proj/tf_hard"),
    )
    .unwrap();
    std::fs::hard_link(
        t.home.join("other/plain.txt"),
        t.home.join("proj/unknown_hard"),
    )
    .unwrap();

    // Without rules in Home: Ask
    let g_no_rules = t.gate(Preset::Home, vec![]);
    assert!(matches!(
        block_on(g_no_rules.gate.check(&paths_check(
            ToolKind::Read,
            "Read",
            &["proj/tf_hard"],
            false,
            None
        ))),
        GateDecision::Ask { .. }
    ));

    // With Read Allow Any rule in Home: positively identified ask-store alias -> Allow,
    // including the original store once multiply-linked; unknown hard link stays Ask.
    let g_rules = t.gate(
        Preset::Home,
        vec![
            rule(
                RuleScope::Global,
                ToolKind::Read,
                RuleMatcher::Any,
                RuleDecision::Allow,
                "allow-any-read",
            ),
            rule(
                RuleScope::Global,
                ToolKind::Execute,
                RuleMatcher::Any,
                RuleDecision::Allow,
                "allow-any-exec",
            ),
        ],
    );
    assert_eq!(
        block_on(g_rules.gate.check(&paths_check(
            ToolKind::Read,
            "Read",
            &["proj/tf_hard"],
            false,
            None
        ))),
        GateDecision::Allow
    );
    assert_eq!(
        block_on(g_rules.gate.check(&paths_check(
            ToolKind::Read,
            "Read",
            &[".terraform.d/credentials.tfrc.json"],
            false,
            None
        ))),
        GateDecision::Allow
    );
    // A shell command through the hard link keeps dev's answer: it asks
    // even under an Execute Any rule (adversarial_m1_18 `cat asks2/hl`).
    assert!(matches!(
        block_on(g_rules.gate.check(&cmd_check("cat proj/tf_hard"))),
        GateDecision::Ask { .. }
    ));
    assert!(matches!(
        block_on(g_rules.gate.check(&paths_check(
            ToolKind::Read,
            "Read",
            &["proj/unknown_hard"],
            false,
            None
        ))),
        GateDecision::Ask { .. }
    ));
}

#[test]
fn round5_finding6_exact_git_safe_config_allow() {
    let env = Env::home();
    let exact_safe = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        exact(&["git", "-c", "color.ui=false", "status"]),
        RuleDecision::Allow,
        "allow-git-exact-safe",
    )];
    assert_eq!(
        classify(&env.cmd_rules("git -c color.ui=false status", &exact_safe)),
        "Allow"
    );

    let prefix_git = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["git"]),
        RuleDecision::Allow,
        "allow-git-prefix",
    )];
    assert_eq!(
        classify(&env.cmd_rules("git -c color.ui=false status", &prefix_git)),
        "Ask"
    );

    let exact_dangerous = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        exact(&["git", "-c", "alias.clean=!rm -f victim", "clean"]),
        RuleDecision::Allow,
        "allow-git-exact-danger",
    )];
    assert_eq!(
        classify(&env.cmd_rules("git -c alias.clean='!rm -f victim' clean", &exact_dangerous)),
        "Ask"
    );
}

/// D-129: a stdin name after the script (`python3 x.py -`) is refused, as on
/// dev's editor; the Rust check and the editor agree.
#[test]
fn escalation_stdin_name_after_script_is_refused() {
    use atrium_core::policy::command_allow_risk;
    let argv = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
    for c in [
        "python3 x.py -",
        "python3 x.py /dev/./stdin",
        "node x.js /dev/fd/0",
    ] {
        assert!(command_allow_risk(&argv(c), true).is_err(), "{c}");
        assert!(command_allow_risk(&argv(c), false).is_err(), "{c}");
    }
    for c in ["python3 x.py --flag", "python3 scripts/build.py"] {
        assert!(command_allow_risk(&argv(c), true).is_ok(), "{c}");
    }
}

// ---------------------------------------------------------------------------
// Escalation round 2 (review 3, D-129)
// ---------------------------------------------------------------------------

/// Review 3, finding 1: a quoted Git pathspec glob that can match a
/// credential store asks; an ordinary glob keeps dev's verdict.
#[test]
fn review3_finding1_git_pathspec_globs_reach_credentials() {
    let mut repo = Env::home();
    repo.cwd = format!("{HOME}/repo").into();
    for line in [
        "git show HEAD -- '.gem/cred*'",
        "git show HEAD -- .gem/cred*",
        "git show HEAD -- '.terraform.d/credentials*'",
        "git show HEAD -- '.config/op/*'",
        "git show HEAD -- '.yarnrc.*'",
        "git log -p -- '.gem/*'",
        "git diff HEAD~1 -- '*/.gem/credentials'",
        "git show HEAD -- ':(glob).gem/cred*'",
        "git show HEAD -- '.ya?nrc.yml'",
        "git show HEAD -- '.gem/[c]redentials'",
        "git show HEAD -- '*'",
        "git show HEAD -- 'sub/*'",
    ] {
        assert_eq!(classify(&repo.cmd(line)), "Ask", "{line}");
    }
    // ordinary: dev's verdict
    for line in [
        "git show HEAD -- '*.rs'",
        "git show HEAD -- 'src/*.rs'",
        "git log -- 'docs/*.md'",
    ] {
        assert_eq!(classify(&repo.cmd(line)), "Allow", "{line}");
    }
}

/// Review 3, finding 2: any `s` delimiter, attached `w` filenames and the
/// `w`/`r` commands are read as paths for path scopes.
#[test]
fn review3_finding2_sed_delimiters_and_path_scopes() {
    let home = Env::home();
    let allow = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["sed"]),
        RuleDecision::Allow,
        Some("/h/u/work/**"),
        "allow-sed-work",
    )];
    let deny = vec![scoped_rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["sed"]),
        RuleDecision::Deny,
        Some("/h/u/work/**"),
        "deny-sed-work",
    )];
    for line in [
        "sed 's:a:b:w /h/u/out.txt' /h/u/work/input.txt",
        "sed 's|a|b|w /h/u/out.txt' /h/u/work/input.txt",
        "sed 's#a#b#gw /h/u/out.txt' /h/u/work/input.txt",
        "sed 's,a,b,2pw /h/u/out.txt' /h/u/work/input.txt",
        "sed 's:a:b:w/h/u/out.txt' /h/u/work/input.txt",
        "sed -e 's@a@b@w /h/u/out.txt' /h/u/work/input.txt",
        "sed '1w /h/u/out.txt' /h/u/work/input.txt",
        "sed '/x/w /h/u/out.txt' /h/u/work/input.txt",
        "sed '\\%x%w /h/u/out.txt' /h/u/work/input.txt",
        "sed '$r /h/u/secret.txt' /h/u/work/input.txt",
        "sed 's:a:b:' -e 's:c:d:w /h/u/out.txt' /h/u/work/input.txt",
    ] {
        assert_eq!(classify(&home.cmd_rules(line, &allow)), "Ask", "{line}");
    }
    for line in [
        "sed 's:a:b:w /h/u/work/out.txt' /h/u/other/input.txt",
        "sed 's|a|b|w /h/u/work/out.txt' /h/u/other/input.txt",
        "sed 's:a:b:w/h/u/work/out.txt' /h/u/other/input.txt",
    ] {
        assert!(
            matches!(home.cmd_rules(line, &deny), PolicyVerdict::Deny { .. }),
            "{line}"
        );
    }
    // inside the scope: still covered
    for line in [
        "sed 's:a:b:w /h/u/work/out.txt' /h/u/work/input.txt",
        "sed 's:a:b:g' /h/u/work/input.txt",
        "sed -n '/x/p' /h/u/work/input.txt",
    ] {
        assert_eq!(classify(&home.cmd_rules(line, &allow)), "Allow", "{line}");
    }
}

/// Review 3, finding 3: Git options and subcommands that run code are never
/// covered by a prefix rule on `git`, and the rule check refuses them.
#[test]
fn review3_finding3_git_execution_payloads() {
    use atrium_core::policy::command_allow_risk;
    let mut repo = Env::home();
    repo.cwd = format!("{HOME}/repo").into();
    let git = vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["git"]),
        RuleDecision::Allow,
        "allow-git",
    )];
    let runs_code = [
        "git rebase --exec 'touch victim' HEAD~1",
        "git rebase --exec='touch victim' HEAD~1",
        "git rebase -x 'touch victim' HEAD~1",
        "git rebase -x'touch victim' HEAD~1",
        "git rebase -i -x 'touch victim' HEAD~1",
        "git submodule foreach 'touch victim'",
        "git submodule --quiet foreach 'touch victim'",
        "git bisect run ./t.sh",
        "git difftool -x 'touch victim'",
        "git difftool --extcmd='touch victim'",
        "git grep -O'touch victim' x",
        "git grep --open-files-in-pager=vi x",
        "git filter-branch --tree-filter 'touch victim'",
        "git fetch --upload-pack='touch victim' origin",
        "git clone -u 'touch victim' x y",
        "git push --receive-pack='touch victim' origin",
        "git archive --remote=x --exec='touch victim' HEAD",
        "git ls-remote --upload-pack 'touch victim' origin",
        "git config core.fsmonitor 'touch victim'",
        "git config --global alias.st '!touch victim'",
        "git config set core.pager 'touch victim'",
        "git -c core.fsmonitor='touch victim' status",
        "git -c alias.x='!touch victim' x",
        "git --exec-path=/tmp status",
    ];
    let argv = |s: &str| match parse_command(s, repo.cwd.as_path()) {
        atrium_core::policy::ParsedCommand::Segments(segs) => segs[0].argv.clone(),
        other => panic!("{s}: {other:?}"),
    };
    for line in runs_code {
        assert_eq!(classify(&repo.cmd_rules(line, &git)), "Ask", "{line}");
        let a = argv(line);
        assert!(command_allow_risk(&a, false).is_err(), "exact: {line}");
        assert!(command_allow_risk(&a, true).is_err(), "prefix: {line}");
    }
    // ordinary git lines keep their verdict under the rule
    for line in [
        "git status",
        "git rebase HEAD~1",
        "git cherry-pick -x abc",
        "git commit -c HEAD",
        "git grep -n x",
        "git fetch -u origin",
        "git config user.name",
        "git config --get core.editor",
        "git log --oneline",
    ] {
        assert_eq!(classify(&repo.cmd_rules(line, &git)), "Allow", "{line}");
    }
}

/// Review 3, finding 4: a relative script path that resolves to standard
/// input is refused and not covered.
#[test]
fn review3_finding4_relative_stdin_paths() {
    use atrium_core::policy::command_allow_risk;
    let mut in_dev = Env::home();
    in_dev.cwd = "/dev".into();
    let mut in_fd = Env::home();
    in_fd.cwd = "/dev/fd".into();
    let mut in_proc = Env::home();
    in_proc.cwd = "/proc".into();
    let echo = rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["echo"]),
        RuleDecision::Allow,
        "allow-echo",
    );
    for (env, script, line) in [
        (&in_dev, "./stdin", "echo 'print(1)' | python3 ./stdin"),
        (&in_dev, "stdin", "echo 'print(1)' | python3 stdin"),
        (&in_dev, "fd/0", "echo 'print(1)' | python3 fd/0"),
        (&in_fd, "0", "echo 'print(1)' | python3 0"),
        (&in_fd, "./0", "echo 'print(1)' | python3 ./0"),
        (&in_proc, "self/fd/0", "echo 'print(1)' | python3 self/fd/0"),
        (&in_dev, "../dev/stdin", "echo 'print(1)' | python3 ../dev/stdin"),
        (&in_dev, "x/../stdin", "echo 'print(1)' | python3 x/../stdin"),
    ] {
        let rules = vec![
            echo.clone(),
            rule(
                RuleScope::Global,
                ToolKind::Execute,
                prefix(&["python3", script]),
                RuleDecision::Allow,
                "allow-py",
            ),
        ];
        assert_eq!(classify(&env.cmd_rules(line, &rules)), "Ask", "{line}");
        let a = vec!["python3".to_string(), script.to_string()];
        assert!(command_allow_risk(&a, true).is_err(), "{script}");
        assert!(command_allow_risk(&a, false).is_err(), "{script}");
    }
    for ok in ["build.py", "./scripts/build.py", "tools/stdin.py", "x.py 0", "x.py stdin"] {
        let a: Vec<String> = std::iter::once("python3").chain(ok.split(' ')).map(String::from).collect();
        assert!(command_allow_risk(&a, true).is_ok(), "{ok}");
    }
}
