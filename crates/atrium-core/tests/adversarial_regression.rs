//! M1-13, third part, step 5: the regression table of the security review
//! (`docs/agents/reviews/M1-13-adversarial.md`).
//!
//! Every input the review names, with the verdict the review says it must
//! get. The tests are expected to FAIL on the branch where the review says
//! the branch is wrong, and stay red until the fixer closes the finding.
//! Nothing here is ignored.
//!
//! Engine tests run `DefaultPolicy::evaluate` (virtual home `/h/u`, nothing on
//! disk) under the review's four settings: **home** (preset Home, no rules),
//! **prefix** (global `CommandPrefix` allow rules for `cd`, `cat`, `pushd`,
//! `popd`, `ls`), **any** (one global `RuleMatcher::Any` allow rule for
//! Execute) and **full** (preset FullAccess). Gate tests run `SessionGate`
//! over a temp folder with a fake HOME (home, any, full). Nothing is
//! executed and no real credential file is read.

mod common_adversarial;

use Want::NotAllow;
use atrium_agents::{GateDecision, ToolGate};
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope};
use common_adversarial::*;
use std::os::unix::fs::symlink;

const RD: &str = "recursive-delete-outside-cwd";
const CR: &str = "credential-read";

fn any_rule() -> Rule {
    rule(
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any",
    )
}

fn prefix_rules() -> Vec<Rule> {
    ["cd", "cat", "pushd", "popd", "ls"]
        .iter()
        .enumerate()
        .map(|(i, w)| {
            rule(
                RuleScope::Global,
                ToolKind::Execute,
                prefix(&[w]),
                RuleDecision::Allow,
                &format!("p{i}"),
            )
        })
        .collect()
}

/// The review's four engine settings.
fn settings() -> Vec<(&'static str, Vec<Rule>, Preset)> {
    vec![
        ("home", vec![], Preset::Home),
        ("prefix", prefix_rules(), Preset::Home),
        ("any", vec![any_rule()], Preset::Home),
        ("full", vec![], Preset::FullAccess),
    ]
}

/// Every (line, want) under every setting; all failures are listed at once.
fn run_all(table: &[(&str, Want)]) {
    let env = Env::home();
    let mut failures = vec![];
    for (name, rules, preset) in settings() {
        for (line, want) in table {
            let v = env.eval_check(&cmd_check(line), &rules, preset);
            if !matches_want(&v, want) {
                failures.push(format!(
                    "[{name}] {line:?}: wanted {want:?}, got {}",
                    classify(&v)
                ));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn h(id: &'static str) -> Want {
    Want::Hard(id)
}

// ---------------------------------------------------------------------------
// Findings 2 and 3: an uncertain working directory
// ---------------------------------------------------------------------------

/// Finding 2: after failing `cd`s the shell is where the line can't say; a
/// credential reached that way is a hard deny in every setting.
#[test]
fn finding_2_failing_cds_then_a_credential() {
    run_all(&[
        ("cd .config; cd n1; cd n2; cd gh; cat hosts.yml", h(CR)),
        ("cd /h; cd n1; cd n2; cd u/.ssh; cat id_rsa", h(CR)),
        (
            "cd .config; cd nope; cd /tmp; cd -; cd gh; cat hosts.yml",
            h(CR),
        ),
        ("cd .config\ncd n1\ncd n2\ncd gh\ncat hosts.yml", h(CR)),
        ("cd /h\ncd n1\ncd n2\ncd u/.ssh\ncat id_rsa", h(CR)),
        (
            "cd .config\ncd nope\ncd /tmp\ncd -\ncd gh\ncat hosts.yml",
            h(CR),
        ),
        ("(cd .config; cd n1; cd n2; cd gh; cat hosts.yml)", h(CR)),
        ("(cd /h; cd n1; cd n2; cd u/.ssh; cat id_rsa)", h(CR)),
    ]);
}

/// Finding 2, `||` forms: the next `cd` runs when the previous one failed,
/// and each `; cat` runs wherever the shell ended up. (The review also names
/// "the same joined by `&&`": those lines are left out, because after a `cd`
/// that fails the rest of an `&&` chain doesn't run, so a hard deny there
/// could be a false one.)
#[test]
fn finding_2_failing_cds_joined_by_or() {
    run_all(&[(
        "cd /h || true; cd n1 || true; cd n2 || true; cd u/.ssh; cat id_rsa",
        h(CR),
    )]);
}

/// Finding 3: `pushd`, `popd` and `cd -` leave the cwd unknown (§9): a
/// relative path after them never reaches Allow (the review asks for a hard
/// deny after `popd` where the candidates include the credential folder; the
/// minimum, which every input here also needs, is that it does not Allow).
#[test]
fn finding_3_pushd_popd_and_cd_dash_leave_the_cwd_unknown() {
    run_all(&[
        (
            "pushd .config; pushd /tmp; pushd /var; popd; popd; cd gh; cat hosts.yml",
            NotAllow,
        ),
        ("cd -; cat id_rsa", NotAllow),
        ("pushd .ssh; popd; cat id_rsa", NotAllow),
        ("pushd /tmp; popd; cat .npmrc", NotAllow),
        ("cd /tmp; cd -; cat id_rsa", NotAllow),
        ("cd .ssh; cd /tmp; cd -; cat id_rsa", NotAllow),
    ]);
}

/// Finding 3 and the brief: `popd` after `pushd .config` ends in a folder
/// that holds a credential: the hard deny, not only an ask.
#[test]
fn finding_3_popd_back_into_a_credential_folder_is_a_hard_deny() {
    run_all(&[(
        "pushd .config; pushd /tmp; pushd /var; popd; popd; cd gh; cat hosts.yml",
        h(CR),
    )]);
}

// ---------------------------------------------------------------------------
// Finding 5: a here-doc or payload for a shell named with a substitution
// ---------------------------------------------------------------------------

/// Finding 5: dev hard-denies each of these here-docs (`rm -rf ~` is read as
/// code); the branch asks in home and allows with any and full.
#[test]
fn finding_5_heredoc_to_a_shell_named_with_a_substitution() {
    run_all(&[
        ("s$(echo h) <<EOF\nrm -rf ~\nEOF", h(RD)),
        ("ba$(printf sh) <<EOF\nrm -rf ~\nEOF", h(RD)),
        ("s`echo h` <<EOF\nrm -rf ~\nEOF", h(RD)),
        ("ssh host $(echo bash) <<EOF\nrm -rf ~\nEOF", h(RD)),
        ("nsenter -t 1 $(echo sh) <<EOF\nrm -rf ~\nEOF", h(RD)),
    ]);
}

/// Finding 5, the same hole for payloads (not read on dev either): the
/// minimum is that they don't Allow.
#[test]
fn finding_5_payload_for_a_shell_named_with_a_substitution() {
    run_all(&[
        ("s$(echo h) -c 'rm -rf ~'", NotAllow),
        ("s${E}h -c 'rm -rf ~'", NotAllow),
    ]);
}

// ---------------------------------------------------------------------------
// Finding 7: text piped to a shell is code however it is spelled
// ---------------------------------------------------------------------------

#[test]
fn finding_7_text_piped_to_a_shell_is_code_however_it_is_spelled() {
    run_all(&[
        ("echo -e 'rm -rf\\t~' | sh", NotAllow),
        ("echo 'rm -rf\\t~' | sh", NotAllow),
        ("echo -e 'rm -rf \\x7e' | sh", NotAllow),
        ("echo -e 'cat .aws/cr\\145dentials' | sh", NotAllow),
        ("printf 'rm -rf \\176' | sh", NotAllow),
        ("printf '%b' 'rm -rf \\0176' | sh", NotAllow),
        ("printf 'cat .aws/credentials%s' | sh", NotAllow),
        ("printf 'cat .aws/credentials%.0s' x | sh", NotAllow),
        ("printf 'cat .aws/%.4sentials' credXXX | sh", NotAllow),
        ("printf '%*s%s' 0 'cat .aws/cred' entials | sh", NotAllow),
        ("printf 'rm -rf ~\\n' | (sh)", NotAllow),
        ("yes 'rm -rf ~' | sh", NotAllow),
        ("sh -c \"$(echo rm -rf ~)\"", NotAllow),
        ("eval $(echo rm -rf ~)", NotAllow),
        ("bash -c \"$(cat <<'EOF'\nrm -rf ~\nEOF\n)\"", NotAllow),
    ]);
}

// ---------------------------------------------------------------------------
// Finding 9: wrappers' long options and the command's own `-c`
// ---------------------------------------------------------------------------

#[test]
fn finding_9_wrapper_long_options_and_the_commands_own_dash_c() {
    run_all(&[
        ("timeout --signal KILL 5 rm -rf ~", NotAllow),
        ("timeout --kill-after 3 5 rm -rf ~", NotAllow),
        ("nice --adjustment 5 rm -rf ~", NotAllow),
        ("ionice --class 3 rm -rf ~", NotAllow),
        ("stdbuf --output L rm -rf ~", NotAllow),
        ("arch -arch x86_64 rm -rf ~", NotAllow),
        ("flock /tmp/l sudo sh -c 'id'", NotAllow),
        ("script -q /dev/null sudo sh -c id", NotAllow),
        ("flock /tmp/l git -c x=y push -f origin main", NotAllow),
        ("env - rm -rf ~", NotAllow),
        ("env -iS 'rm -rf ~'", NotAllow),
        ("env -a foo rm -rf ~", NotAllow),
        ("env -S 'cat .aws/credentials'", NotAllow),
        ("env -C .aws cat credentials", NotAllow),
        ("xargs --max-args 1 rm -rf ~", NotAllow),
        ("time -f %e rm -rf ~", NotAllow),
    ]);
}

// ---------------------------------------------------------------------------
// Finding 12: seven nested cds are refused outright (the regression is the
// refusal; the verdicts are in adversarial_ordinary.rs)
// ---------------------------------------------------------------------------

/// Finding 12: seven `cd`s never come out as the "too large to check" Deny
/// (dev: Ask; Allow with rules). The exact dev verdicts are in the ordinary
/// table; this one states the rule: not a Deny.
#[test]
fn finding_12_seven_nested_cds_are_not_refused() {
    let env = Env::home();
    let mut failures = vec![];
    for (name, rules, preset) in settings() {
        let v = env.eval_check(
            &cmd_check("cd a && cd b && cd c && cd d && cd e && cd f && cd g && ls"),
            &rules,
            preset,
        );
        if matches!(v, atrium_core::PolicyVerdict::Deny { .. }) {
            failures.push(format!("[{name}] got {}", classify(&v)));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

// ---------------------------------------------------------------------------
// Finding 13: shell options that take a value still hide the payload
// ---------------------------------------------------------------------------

#[test]
fn finding_13_shell_options_that_take_a_value() {
    run_all(&[
        ("bash --rcfile /dev/null -c 'rm -rf ~'", NotAllow),
        ("bash --init-file /dev/null -c 'rm -rf ~'", NotAllow),
        ("zsh --emulate sh -c 'rm -rf ~'", NotAllow),
        ("fish -C true -c 'rm -rf ~'", NotAllow),
        ("fish --init-command='rm -rf ~'", NotAllow),
    ]);
}

// ---------------------------------------------------------------------------
// Finding 14: crontab bodies and the other stdin readers
// ---------------------------------------------------------------------------

#[test]
fn finding_14_crontab_bodies_and_other_stdin_readers() {
    run_all(&[
        ("echo '* * * * * rm -rf ~' | crontab -", NotAllow),
        ("echo '@reboot rm -rf ~' | crontab -", NotAllow),
        ("crontab - <<EOF\n* * * * * rm -rf ~\nEOF", NotAllow),
        ("make -sf - <<EOF\nall:\n\trm -rf ~\nEOF", NotAllow),
        ("make -f /dev/stdin <<EOF\nall:\n\trm -rf ~\nEOF", NotAllow),
        ("chroot --userspec u:g /mnt <<EOF\nrm -rf ~\nEOF", NotAllow),
        ("at now -f x <<EOF\nrm -rf ~\nEOF", NotAllow),
    ]);
}

// ---------------------------------------------------------------------------
// "Tried and held": the review's inputs that were already hard denies and
// must stay so (the fixes may only make verdicts stricter)
// ---------------------------------------------------------------------------

/// A single failing `cd`, in the forms the review tried. (The review gives
/// the families, not the exact lines; each uses the same credential read.)
#[test]
fn held_one_failing_cd_forms() {
    run_all(&[
        ("cd nope; cat .ssh/id_rsa", h(CR)),
        ("builtin cd nope; cat .ssh/id_rsa", h(CR)),
        ("command cd nope; cat .ssh/id_rsa", h(CR)),
        ("cd -L nope; cat .ssh/id_rsa", h(CR)),
        ("cd -P nope; cat .ssh/id_rsa", h(CR)),
        ("cd -- nope; cat .ssh/id_rsa", h(CR)),
        ("cd 'nope'; cat .ssh/id_rsa", h(CR)),
        ("cd no\\pe; cat .ssh/id_rsa", h(CR)),
        ("cd nope; cat ~/.ssh/id_rsa", h(CR)),
        ("cd nope; cat $HOME/.ssh/id_rsa", h(CR)),
        ("chdir nope; cat .ssh/id_rsa", h(CR)),
        ("pushd nope; cat .ssh/id_rsa", h(CR)),
        ("if cd nope; then cat .ssh/id_rsa; fi", h(CR)),
        ("{ cd nope; cat .ssh/id_rsa; }", h(CR)),
        ("cd nope & cat .ssh/id_rsa", h(CR)),
        ("bash -c 'cd nope'; cat .ssh/id_rsa", h(CR)),
        ("eval cd nope; cat .ssh/id_rsa", h(CR)),
    ]);
}

/// Deletes after a failing `cd` stay hard denies.
#[test]
fn held_deletes_after_a_failing_cd() {
    run_all(&[
        ("cd nope; rm -rf .", h(RD)),
        ("cd proj; cd nope; rm -rf ..", h(RD)),
        ("cd a/b; cd n1; cd n2; rm -rf ../..", h(RD)),
        ("cd nope; find . -delete", h(RD)),
    ]);
}

/// `sh -c` payload forms that were read and must stay read.
#[test]
fn held_sh_dash_c_forms() {
    run_all(&[
        ("sh -c -- 'rm -rf ~'", h(RD)),
        ("sh -c '--' 'rm -rf ~'", h(RD)),
        ("sh -ec 'rm -rf ~'", h(RD)),
        ("sh -c -e 'rm -rf ~'", h(RD)),
        ("sh +o posix -c 'rm -rf ~'", h(RD)),
        ("sh -o pipefail -c 'rm -rf ~'", h(RD)),
        ("sh -O extglob -c 'rm -rf ~'", h(RD)),
        ("bash --norc -c 'rm -rf ~'", h(RD)),
        ("sh -lc 'rm -rf ~'", h(RD)),
        ("env sh -c 'rm -rf ~'", h(RD)),
        ("ksh -c 'rm -rf ~' --", h(RD)),
        ("eval -- 'rm -rf ~'", h(RD)),
    ]);
}

/// `echo`/`printf` text into a shell, in the forms that were read.
#[test]
fn held_echo_and_printf_into_a_shell() {
    run_all(&[
        ("echo -n -e 'rm -rf ~' | sh", h(RD)),
        ("echo -ne 'rm -rf ~' | sh", h(RD)),
        ("echo 'rm' '-rf' '~' | sh", h(RD)),
        ("/bin/echo 'rm -rf ~' | sh", h(RD)),
        ("command echo 'rm -rf ~' | sh", h(RD)),
        ("env echo 'rm -rf ~' | sh", h(RD)),
        ("echo 'rm -rf ~' |& sh", h(RD)),
        ("echo 'rm -rf ~' | cat | sh", h(RD)),
        ("echo 'rm -rf ~' | tee x | sh", h(RD)),
        ("echo 'rm -rf ~' | xargs sh -c", h(RD)),
        ("echo 'rm -rf ~' | sh -s", h(RD)),
        ("echo 'rm -rf ~' | bash /dev/stdin", h(RD)),
        ("echo 'rm -rf ~' | source /dev/stdin", h(RD)),
        ("sh <<< 'rm -rf ~'", h(RD)),
        ("printf '%s ' rm -rf ~ | sh", h(RD)),
        ("printf -- 'rm -rf ~' | sh", h(RD)),
    ]);
}

// ---------------------------------------------------------------------------
// Gate tests: the filesystem (findings 1, 4 and 11)
// ---------------------------------------------------------------------------

/// The gate settings: home, any, full.
fn gate_settings() -> Vec<(&'static str, Vec<Rule>, Preset)> {
    vec![
        ("home", vec![], Preset::Home),
        ("any", vec![any_rule()], Preset::Home),
        ("full", vec![], Preset::FullAccess),
    ]
}

fn gate_lines(t: &Tmp, name: &str, lines: &[&str], want_deny: bool, failures: &mut Vec<String>) {
    for (setting, rules, preset) in gate_settings() {
        let g = t.gate(preset, rules);
        for line in lines {
            let d = block_on(g.gate.check(&cmd_check(line)));
            let ok = matches!(d, GateDecision::Deny { .. }) == want_deny
                && (want_deny || !matches!(d, GateDecision::Allow));
            if !ok {
                failures.push(format!("{name} [{setting}] {line:?}: got {d:?}"));
            }
        }
    }
}

/// Finding 1: a key kept elsewhere and symlinked into `~/.ssh` is still the
/// key: every name for it is a hard deny (dev: Deny for every one).
#[test]
fn finding_1_a_key_symlinked_into_ssh_is_still_a_credential() {
    let t = Tmp::new();
    t.write(".ssh/known_hosts", "x");
    t.write("keys/id_ed25519", "secret");
    t.write("dotfiles/ssh/id_rsa", "secret");
    symlink(
        t.home.join("keys/id_ed25519"),
        t.home.join(".ssh/id_ed25519"),
    )
    .unwrap();
    symlink("../dotfiles/ssh/id_rsa", t.home.join(".ssh/id_rsa")).unwrap();
    let mut failures = vec![];
    gate_lines(
        &t,
        "cmd",
        &[
            "cat keys/id_ed25519",
            "head -c 64 keys/id_ed25519",
            "cat dotfiles/ssh/id_rsa",
        ],
        true,
        &mut failures,
    );
    for (setting, rules, preset) in gate_settings() {
        let g = t.gate(preset, rules);
        for p in ["keys/id_ed25519", "dotfiles/ssh/id_rsa"] {
            let c = paths_check(ToolKind::Read, "Read", &[p], false, None);
            let d = block_on(g.gate.check(&c));
            if !matches!(d, GateDecision::Deny { .. }) {
                failures.push(format!("Read [{setting}] {p}: got {d:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Finding 1, the linked folder: `~/.ssh/work -> ~/vault/work`, a folder
/// holding `id_work`. dev denied `cat vault/work/id_work`. (The review says
/// "decide separately": M1-03 finding 5 wants a linked folder that holds
/// nothing sensitive to stay ordinary, and this one holds a key.)
#[test]
fn finding_1_a_folder_of_keys_linked_into_ssh() {
    let t = Tmp::new();
    t.write("vault/work/id_work", "secret");
    t.write(".ssh/known_hosts", "x");
    symlink(t.home.join("vault/work"), t.home.join(".ssh/work")).unwrap();
    let mut failures = vec![];
    gate_lines(&t, "cmd", &["cat vault/work/id_work"], true, &mut failures);
    for (setting, rules, preset) in gate_settings() {
        let g = t.gate(preset, rules);
        let c = paths_check(ToolKind::Read, "Read", &["vault/work/id_work"], false, None);
        let d = block_on(g.gate.check(&c));
        if !matches!(d, GateDecision::Deny { .. }) {
            failures.push(format!("Read [{setting}] vault/work/id_work: got {d:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Finding 4: a `cd` through a symlink must not lose the write check: a write
/// to Atrium's config or to `~/.ssh/authorized_keys` / `config` through a
/// link is a hard deny (dev: Deny in all three settings).
#[test]
fn finding_4_a_cd_through_a_symlink_keeps_the_write_check() {
    let t = Tmp::new();
    t.write("evil", "x");
    t.write(".ssh/authorized_keys", "k");
    t.write(".ssh/config", "c");
    t.write(".config/atrium/config.toml", "cfg");
    symlink(t.home.join(".config/atrium"), t.home.join("lnk")).unwrap();
    symlink(t.home.join(".ssh"), t.home.join("s")).unwrap();
    symlink(&t.home, t.home.join("self")).unwrap();
    let mut failures = vec![];
    gate_lines(
        &t,
        "cmd",
        &[
            "cd lnk/..; cp evil ~/lnk/config.toml",
            "cd lnk/..; tee ~/lnk/config.toml",
            "cd lnk/..; cp ~/evil ~/s/authorized_keys",
            "cd lnk/..; tee ~/s/config",
            "cd self; cp evil ~/s/authorized_keys",
            "cd self && tee ~/lnk/config.toml",
        ],
        true,
        &mut failures,
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// The same lines without a `cd` are denied on the branch and on dev: the
/// control for finding 4 (must stay green).
#[test]
fn finding_4_control_the_same_writes_without_a_cd() {
    let t = Tmp::new();
    t.write("evil", "x");
    t.write(".ssh/authorized_keys", "k");
    t.write(".ssh/config", "c");
    t.write(".config/atrium/config.toml", "cfg");
    symlink(t.home.join(".config/atrium"), t.home.join("lnk")).unwrap();
    symlink(t.home.join(".ssh"), t.home.join("s")).unwrap();
    let mut failures = vec![];
    gate_lines(
        &t,
        "cmd",
        &[
            "cp evil ~/lnk/config.toml",
            "tee ~/lnk/config.toml",
            "cp ~/evil ~/s/authorized_keys",
            "tee ~/s/config",
        ],
        true,
        &mut failures,
    );
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Finding 11 (of the branch review): a hard link under Home to a key in a
/// tree the identity map doesn't walk. A recursive search of the folder, or a
/// glob that names it, must not be auto-allowed (the review: ask when the
/// tree holds a multiply-linked file). dev allows these too, so this is
/// stricter than dev; the direct `cat proj/k2` case is already denied.
#[test]
fn finding_11_a_recursive_search_over_a_hard_linked_credential() {
    let t = Tmp::new();
    t.write(".gnupg/private-keys-v1.d/k.key", "secret");
    t.write("proj/a.txt", "x");
    std::fs::hard_link(
        t.home.join(".gnupg/private-keys-v1.d/k.key"),
        t.home.join("proj/k2"),
    )
    .unwrap();
    let mut failures = vec![];
    for (setting, rules, preset) in gate_settings() {
        let g = t.gate(preset, rules);
        for line in ["rg x proj", "grep -r x proj", "cat proj/*"] {
            let d = block_on(g.gate.check(&cmd_check(line)));
            if matches!(d, GateDecision::Allow) {
                failures.push(format!("[{setting}] {line:?}: got {d:?}"));
            }
        }
        let c = paths_check(ToolKind::Search, "Grep", &["proj"], true, None);
        let d = block_on(g.gate.check(&c));
        if matches!(d, GateDecision::Allow) {
            failures.push(format!("Grep [{setting}] proj: got {d:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Finding 11, the hard link kept outside Home (the review: "drop the
/// `starts_with(home)` test or state it"). Stricter than dev.
#[test]
fn finding_11_a_hard_link_to_a_credential_kept_outside_home() {
    let t = Tmp::new();
    t.write(".gnupg/private-keys-v1.d/k.key", "secret");
    std::fs::create_dir_all(t.root.join("outside")).unwrap();
    std::fs::hard_link(
        t.home.join(".gnupg/private-keys-v1.d/k.key"),
        t.root.join("outside/innocent.txt"),
    )
    .unwrap();
    t.write("proj/a.txt", "x");
    symlink(t.root.join("outside"), t.home.join("proj/out-link")).unwrap();
    let outside = t.root.join("outside/innocent.txt").display().to_string();
    let mut failures = vec![];
    for (setting, rules, preset) in gate_settings() {
        let g = t.gate(preset, rules);
        for line in [
            format!("cat {outside}"),
            "cat proj/out-link/innocent.txt".into(),
        ] {
            let d = block_on(g.gate.check(&cmd_check(&line)));
            if matches!(d, GateDecision::Allow) {
                failures.push(format!("[{setting}] {line:?}: got {d:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
