//! M1-13, third part, step 5: the table of ordinary lines.
//!
//! Lines taken from the security review
//! (`docs/agents/reviews/M1-13-adversarial.md`), the brief and everyday Home
//! use, each with the verdict it has on `dev` (the contract's behaviour that
//! the branch must keep: "no stricter on ordinary lines"; a false hard deny
//! can't be overridden). The verdicts were obtained by running this very
//! table against `dev`'s `crates/atrium-core/src` (see the handoff), not by
//! guessing.
//!
//! Each row is `[home, prefix, any, full]`, the review's four settings:
//! preset Home with no rules; Home with global `CommandPrefix` allow rules for
//! `cd`, `cat`, `pushd`, `popd`, `ls`; Home with one global `Any` allow rule
//! for Execute; preset FullAccess. Verdicts: `Allow`, `Ask`, `Deny` (a Deny
//! with a reason, such as "too large to check") or `Hard(<id>)`.
//!
//! The tests fail where the branch is stricter than `dev` (that is the
//! review's finding 6, 10 and 12) and are not ignored. Nothing is executed.

mod common_adversarial;

use atrium_agents::{GateDecision, ToolGate};
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope};
use common_adversarial::*;
use std::os::unix::fs::symlink;

type Row = (&'static str, [&'static str; 4]);

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

fn settings() -> Vec<(&'static str, Vec<Rule>, Preset)> {
    vec![
        ("home", vec![], Preset::Home),
        ("prefix", prefix_rules(), Preset::Home),
        ("any", vec![any_rule()], Preset::Home),
        ("full", vec![], Preset::FullAccess),
    ]
}

fn code(v: &atrium_core::PolicyVerdict) -> String {
    match v {
        atrium_core::PolicyVerdict::Deny { .. } => "Deny".into(),
        v => classify(v),
    }
}

fn check_table(table: &[Row]) {
    let env = Env::home();
    let mut failures = vec![];
    for (line, want) in table {
        let got: Vec<String> = settings()
            .iter()
            .map(|(_, rules, preset)| code(&env.eval_check(&cmd_check(line), rules, *preset)))
            .collect();
        if got.iter().map(String::as_str).ne(want.iter().copied()) {
            failures.push(format!("{line:?}: wanted {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// After a `cd` that works (review finding 6): `dev` asks (allows with the
/// `cd` and `cat` rules). The branch hard-denies a dot-file read, a glob delete
/// and `..` paths that no cwd could give.
#[test]
fn ordinary_lines_after_a_working_cd() {
    check_table(&[
        (
            "cd proj && cat README.md",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        ("cd proj && rm -rf build", ["Ask", "Ask", "Allow", "Allow"]),
        ("cd proj && cat .npmrc", ["Ask", "Allow", "Allow", "Allow"]),
        (
            "cd ~/proj && cat .npmrc",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        ("cd /tmp && cat .npmrc", ["Ask", "Allow", "Allow", "Allow"]),
        ("cd proj && cat .netrc", ["Ask", "Allow", "Allow", "Allow"]),
        (
            "cd infra && cat .kube/config",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd app && cat .docker/config.json",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd proj && cat .claude.json",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd proj && grep registry .npmrc",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd proj && ls -la .gnupg",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd packages/app && cat ../.npmrc",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd a/b/c && cat ../../.npmrc",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        ("cd build && rm -rf *", ["Ask", "Ask", "Allow", "Allow"]),
        ("cd dist && rm -rf ./*", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "cd proj && rm -rf ../proj-old",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd build && find . -delete",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && find . -name '*.o' -delete",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("cd build && make", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "cd proj && rm -rf node_modules",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && cat .gitignore",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        ("cd src && ls", ["Ask", "Allow", "Allow", "Allow"]),
        ("cd .. && ls", ["Ask", "Allow", "Allow", "Allow"]),
        ("cd ~ && ls", ["Ask", "Allow", "Allow", "Allow"]),
        ("cd /tmp && ls", ["Ask", "Allow", "Allow", "Allow"]),
        ("cd proj && git status", ["Ask", "Allow", "Allow", "Allow"]),
        ("cd proj && cargo build", ["Ask", "Ask", "Allow", "Allow"]),
        ("cd proj && npm install", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "cd proj && rm -rf target dist",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && find . -name '*.tmp' -delete",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && cat .env.example",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd proj && cat .eslintrc.json",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd proj && cat .gitconfig",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd docs && cat ../README.md",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
    ]);
}

/// Third-part corrections (the fixer's, see the handoff): after `;` or a
/// newline a `cd` may have failed, so the shell may still be in `~`. These
/// three lines then read `~/.npmrc` or run `rm -rf ~/*`, the same shape as
/// the review's held hard denies (`cd nope; cat .ssh/id_rsa`, `cd nope; rm
/// -rf .`). The table had dev's verdicts (Ask; Allow with rules), which is
/// finding 2. Written with `&&` (above) they keep dev's verdicts.
#[test]
fn after_a_cd_that_may_fail_the_old_directory_counts() {
    check_table(&[
        (
            "cd proj; cat .npmrc",
            [
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
            ],
        ),
        (
            "cd proj\ncat .npmrc",
            [
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
            ],
        ),
        (
            "cd build; rm -rf *",
            [
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
            ],
        ),
        // an uncertain cwd asks under rules (brief item 1(2), contract §9);
        // `Any` and FullAccess allow what isn't hard-denied (dev: Allow with
        // the prefix rules)
        ("cd proj; ls", ["Ask", "Ask", "Allow", "Allow"]),
        ("cd proj; cat README.md", ["Ask", "Ask", "Allow", "Allow"]),
        // `cd X || exit`: past it the shell is in X
        (
            "cd build || exit 1; rm -rf *",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("(cd sub && make)", ["Ask", "Ask", "Allow", "Allow"]),
    ]);
}

/// Review finding 12: nested `cd`s. dev never refuses them as "too large".
/// Third-part correction: seven `cd`s joined by `;` may each fail, so the
/// shell may be in any of 128 directories: too many to follow, the cwd is
/// unknown and the prefix rules no longer cover the line (dev: Allow). Joined
/// by `&&` they keep dev's verdicts.
#[test]
fn ordinary_nested_cds() {
    check_table(&[
        (
            "cd a && cd b && cd c && cd d && cd e && cd f && cd g && ls",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd a && cd b && cd c && cd d && cd e && cd f && ls",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd a && cd b && cd c && ls",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd a; cd b; cd c; cd d; cd e; cd f; cd g; ls",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("cd a/b/c/d/e/f/g && ls", ["Ask", "Allow", "Allow", "Allow"]),
        (
            "cd a && cd b && cd c && cd d && cd e && cd f && cd g && cat README.md",
            ["Ask", "Allow", "Allow", "Allow"],
        ),
        (
            "cd a && cd b && cd c && cd d && cd e && cd f && cd g && rm -rf build",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
    ]);
}

/// Review finding 10 and "tried and held": pipes and `$VAR` command words,
/// here-docs of plain text (round 6 finding 1).
#[test]
fn ordinary_pipes_variables_and_heredocs() {
    check_table(&[
        (
            "curl -s https://example.com/notes.txt | $PAGER",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "echo \"sudo apt install foo\" | $CLIP",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("git log | $PAGER", ["Ask", "Ask", "Allow", "Allow"]),
        ("echo done | cat", ["Ask", "Ask", "Allow", "Allow"]),
        ("printf '%s\\n' x | sort", ["Ask", "Ask", "Allow", "Allow"]),
        ("$CC -c foo.c -o foo.o", ["Ask", "Ask", "Allow", "Allow"]),
        ("echo y | sh install.sh", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "git commit -F - <<EOF\nfix $(git rev-parse --short HEAD)\nEOF",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cat <<EOF > out.txt\nhello\nEOF",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("echo hello | tee out.txt", ["Ask", "Ask", "Allow", "Allow"]),
        ("ls | wc -l", ["Allow", "Allow", "Allow", "Allow"]),
        ("git diff | less", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "cat package.json | jq .name",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("docker ps | grep web", ["Ask", "Ask", "Allow", "Allow"]),
        ("ps aux | grep node", ["Ask", "Ask", "Allow", "Allow"]),
        ("sort data.txt | uniq -c", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "echo $PATH | tr ':' '\\n'",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("$EDITOR notes.md", ["Ask", "Ask", "Allow", "Allow"]),
        ("echo hello", ["Ask", "Ask", "Allow", "Allow"]),
    ]);
}

/// Everyday Home lines: reads, searches, builds, git, plain deletes.
#[test]
fn ordinary_everyday_home_lines() {
    check_table(&[
        ("ls", ["Allow", "Allow", "Allow", "Allow"]),
        ("ls -la", ["Allow", "Allow", "Allow", "Allow"]),
        ("pwd", ["Allow", "Allow", "Allow", "Allow"]),
        ("git status", ["Allow", "Allow", "Allow", "Allow"]),
        ("git diff", ["Allow", "Allow", "Allow", "Allow"]),
        ("git log --oneline", ["Allow", "Allow", "Allow", "Allow"]),
        ("cat README.md", ["Allow", "Allow", "Allow", "Allow"]),
        ("cat src/main.rs", ["Allow", "Allow", "Allow", "Allow"]),
        ("head -n 20 README.md", ["Allow", "Allow", "Allow", "Allow"]),
        ("wc -l src/main.rs", ["Allow", "Allow", "Allow", "Allow"]),
        ("rg foo src", ["Allow", "Allow", "Allow", "Allow"]),
        ("grep -rn TODO src", ["Allow", "Allow", "Allow", "Allow"]),
        ("find . -name '*.rs'", ["Ask", "Ask", "Allow", "Allow"]),
        ("find . -name '*.o'", ["Ask", "Ask", "Allow", "Allow"]),
        ("cargo build", ["Ask", "Ask", "Allow", "Allow"]),
        ("cargo test", ["Ask", "Ask", "Allow", "Allow"]),
        ("npm install", ["Ask", "Ask", "Allow", "Allow"]),
        ("npm run build", ["Ask", "Ask", "Allow", "Allow"]),
        ("make -j4", ["Ask", "Ask", "Allow", "Allow"]),
        ("rm -rf target", ["Ask", "Ask", "Allow", "Allow"]),
        ("rm -rf ./build", ["Ask", "Ask", "Allow", "Allow"]),
        ("rm -rf node_modules", ["Ask", "Ask", "Allow", "Allow"]),
        ("rm build/out.o", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "mkdir -p out && cp a.txt out/",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("mv old.txt new.txt", ["Ask", "Ask", "Allow", "Allow"]),
        ("touch notes.md", ["Ask", "Ask", "Allow", "Allow"]),
        ("git commit -m \"fix\"", ["Ask", "Ask", "Allow", "Allow"]),
        ("git add -A", ["Ask", "Ask", "Allow", "Allow"]),
        ("ls ~", ["Allow", "Allow", "Allow", "Allow"]),
        ("ls -la ~", ["Allow", "Allow", "Allow", "Allow"]),
        ("cat ~/.zshrc", ["Allow", "Allow", "Allow", "Allow"]),
        ("cat ~/notes.txt", ["Allow", "Allow", "Allow", "Allow"]),
        ("ls ~/proj", ["Allow", "Allow", "Allow", "Allow"]),
        (
            "cat .npmrc",
            [
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
            ],
        ),
        ("cat .gitignore", ["Allow", "Allow", "Allow", "Allow"]),
        ("cat .env.example", ["Allow", "Allow", "Allow", "Allow"]),
        ("ls .config", ["Allow", "Allow", "Allow", "Allow"]),
        (
            "curl https://example.com | head",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("docker ps", ["Ask", "Ask", "Allow", "Allow"]),
        ("python3 script.py", ["Ask", "Ask", "Allow", "Allow"]),
        ("node index.js", ["Ask", "Ask", "Allow", "Allow"]),
        ("echo \"a b\" > out.txt", ["Ask", "Ask", "Allow", "Allow"]),
        ("ls /tmp", ["Allow", "Allow", "Allow", "Allow"]),
    ]);
}

/// Round 6 finding 1, fixed by part 1 and to stay fixed: `$(…)` in an
/// unquoted here-doc body is not shell input. These two are the one place
/// where `dev` is wrong (it hard-denies `sudo make install` inside the body),
/// so the expected verdicts are the branch's, not `dev`'s.
#[test]
fn ordinary_heredoc_text_with_a_substitution_is_not_a_false_hard_deny() {
    check_table(&[
        (
            "cat > notes-$(date).md <<EOF\nBuilt $(date)\nsudo make install\nEOF",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cat > notes.md <<EOF\nBuilt $(date)\nsudo make install\nEOF",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        // a substitution in the arguments of an ordinary owner stays text
        // (only a command that runs another command reads it: finding 5)
        (
            "tee notes-$(date).md <<EOF\nsudo make install\nEOF",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
    ]);
}

/// Fourth round, re-review majors 1 and 2: a substitution, `-exec` or `-c`
/// payload inside an `&&` chain must not bring the directory before the
/// `cd` back; `cd X && Y` inside `sh -c`, `eval` or `env -C` follows the
/// `&&` rule. Verdicts are dev's (run on dev's `src`, see the handoff).
/// `env -C DIR CMD` is a hard deny on dev too (dev reads `CMD` in the start
/// directory), so those rows keep dev's verdict.
#[test]
fn round_4_majors_derived_units_keep_their_own_chain() {
    let ask = ["Ask", "Ask", "Allow", "Allow"];
    check_table(&[
        ("cd proj && echo $(date) && cat .npmrc", ask),
        ("cd build && TAG=$(date +%F) && rm -rf *", ask),
        (
            "cd proj && find . -name '*.o' -exec true {} + && cat .npmrc",
            ask,
        ),
        ("cd proj && sh -c 'true' && cat .npmrc", ask),
        ("sh -c 'cd proj && cat .npmrc'", ask),
        ("sh -c 'cd build && rm -rf *'", ask),
        ("sh -c \"cd proj && find . -name '*.o' -delete\"", ask),
        ("eval 'cd proj && cat .npmrc'", ask),
        (
            "env -C proj cat .npmrc",
            [
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
            ],
        ),
        (
            "env -C build rm -rf *",
            [
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
            ],
        ),
    ]);
}

/// Fifth round, re-review major: what the last command of an `&&` chain
/// runs (a `sh -c` or `eval` payload, a substitution, a `find -exec`) runs
/// only if the chain got that far, so the directory before the `cd` doesn't
/// come back for it; and an `eval` that ended its own chain hands on where
/// it ended. Plus the re-review's trade-off: a pipe into a word built by a
/// substitution whose program is known and isn't a shell. Verdicts are
/// dev's (run on dev's `src`, see the handoff).
#[test]
fn round_5_major_the_last_command_of_a_chain_and_named_pipe_programs() {
    let ask = ["Ask", "Ask", "Allow", "Allow"];
    check_table(&[
        ("cd proj && sh -c 'cat .npmrc'", ask),
        ("cd build && sh -c 'rm -rf *'", ask),
        ("cd proj && eval 'cat .npmrc'", ask),
        ("cd build && eval 'rm -rf *'", ask),
        ("cd proj && echo $(cat .npmrc)", ask),
        ("cd proj && find . -name '*.o' -exec cat .npmrc \\;", ask),
        ("cd build && find . -name '*.o' -exec rm -rf * \\;", ask),
        ("eval 'cd proj && echo hi' && cat .npmrc", ask),
        ("cd proj && bash -c 'cat .npmrc'", ask),
        ("cd proj && make && sh -c 'cat .npmrc'", ask),
        ("cd a && cd b && sh -c 'cat .npmrc'", ask),
        ("cd proj && time sh -c 'cat .npmrc'", ask),
        ("eval 'cd proj && true' && eval 'cat .npmrc'", ask),
        ("cd proj && source /dev/stdin <<<'cat .npmrc'", ask),
        ("cd proj && echo `cat .npmrc`", ask),
        ("cd proj && nohup sh -c 'cat .npmrc'", ask),
        (
            "curl -s https://example.com/data.json | $(brew --prefix)/bin/jq .",
            ask,
        ),
        ("cat data.json | $(brew --prefix)/bin/jq .", ask),
        ("curl -s https://example.com/x | $(which less)", ask),
        ("echo hi | $(brew --prefix)/bin/jq .", ask),
        (
            "curl -s https://example.com/x | \"$(brew --prefix)/bin/jq\" .",
            ask,
        ),
        ("curl -s https://example.com/x | $HOME/bin/jq .", ask),
    ]);
}

/// Compound commands written with `X` for their body (round 6).
const COMPOUNDS: &[&str] = &[
    "if true; then X; fi",
    "if false; then X; else X; fi",
    "while read l; do X; done < f",
    "until false; do X; done",
    "for x in a b; do X; done",
    "select x in a; do X; done",
    "case a in a) X;; esac",
    "case a in (a) X;; esac",
    "{ X; }",
    "{ X; X; }",
    "(X)",
    "(X; X)",
    "! { X; }",
    "time { X; }",
    "f() { X; }",
    "function f { X; }",
    "if true; then { X; }; fi",
    "while false; do if true; then X; fi; done",
    "{ (X); }",
    "if true; then (X); fi",
];

/// Round 6 major (the round-5 review's major): a compound command inside a
/// `cd X && … && Y` chain keeps the chain: its internal `;` and `)` don't end
/// it, so `Y` runs only after the `cd` worked and gets dev's verdict. Every
/// compound, alone and twice, before three kinds of tail.
#[test]
fn round_6_major_generated_compounds_inside_a_cd_chain() {
    let tails = [
        "cd proj && C && cat .npmrc",
        "cd build && C && rm -rf *",
        "cd proj && C && sh -c 'cat .npmrc'",
        "cd proj && C && C && cat .npmrc",
    ];
    let mut lines: Vec<String> = vec![];
    for t in tails {
        for c in COMPOUNDS {
            lines.push(t.replace('C', &c.replace('X', "echo ok")));
        }
    }
    let ask = ["Ask", "Ask", "Allow", "Allow"];
    let lines: &'static [String] = Box::leak(lines.into_boxed_slice());
    let table: Vec<Row> = lines.iter().map(|l| (l.as_str(), ask)).collect();
    check_table(&table);
}

/// Round 6: ordinary lines (wrappers with ordinary options, compounds in
/// chains, substituted programs, recursive readers) at dev's verdict.
#[test]
fn round_6_ordinary_lines_keep_devs_verdict() {
    check_table(&[
        ("nice -n 10 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("nice -10 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("nice make", ["Ask", "Ask", "Allow", "Allow"]),
        ("xargs -I {} echo {}", ["Ask", "Ask", "Allow", "Allow"]),
        ("xargs -0 -n 1 echo", ["Ask", "Ask", "Allow", "Allow"]),
        ("xargs -tI {} echo {}", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "find . -name '*.rs' | xargs -I{} wc -l {}",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("timeout 5 cargo test", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "timeout -s KILL 5 cargo test",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "timeout --foreground 5 cargo test",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("find -L . -name '*.rs'", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "find . -type f -name '*.o' -delete",
            [
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
                "Hard(recursive-delete-outside-cwd)",
            ],
        ),
        ("find src -name '*.rs'", ["Ask", "Ask", "Allow", "Allow"]),
        ("find -H src -type d", ["Ask", "Ask", "Allow", "Allow"]),
        ("stdbuf -oL make", ["Ask", "Ask", "Allow", "Allow"]),
        ("stdbuf -o0 tail -f log", ["Ask", "Ask", "Allow", "Allow"]),
        ("ionice -c 3 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("ionice -c3 -n7 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("caffeinate -i make", ["Ask", "Ask", "Allow", "Allow"]),
        ("caffeinate -dims make", ["Ask", "Ask", "Allow", "Allow"]),
        ("exec make", ["Ask", "Ask", "Allow", "Allow"]),
        ("exec -a foo make", ["Ask", "Ask", "Allow", "Allow"]),
        ("env FOO=1 make", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "env -i PATH=/usr/bin make",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("env -u FOO make", ["Ask", "Ask", "Allow", "Allow"]),
        ("nohup make", ["Ask", "Ask", "Allow", "Allow"]),
        ("nohup ./server &", ["Ask", "Ask", "Allow", "Allow"]),
        ("command -v cargo", ["Ask", "Ask", "Allow", "Allow"]),
        ("command ls", ["Ask", "Ask", "Allow", "Allow"]),
        ("time make", ["Ask", "Ask", "Allow", "Allow"]),
        ("time -p make", ["Ask", "Ask", "Allow", "Allow"]),
        ("flock /tmp/l make", ["Ask", "Ask", "Allow", "Allow"]),
        ("flock -n /tmp/l make", ["Ask", "Ask", "Allow", "Allow"]),
        ("flock -w 10 /tmp/l make", ["Ask", "Ask", "Allow", "Allow"]),
        ("flock /tmp/l -c 'make'", ["Ask", "Ask", "Allow", "Allow"]),
        ("script -q /dev/null make", ["Ask", "Ask", "Allow", "Allow"]),
        ("setsid make", ["Ask", "Ask", "Allow", "Allow"]),
        ("chrt -f 10 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("taskset -c 0 make", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "sandbox-exec -n no-network make",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("arch -arm64 make", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "cd proj && make && make test",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && if [ -f x ]; then make; fi && make test",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && for f in *.rs; do echo $f; done && make",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && while read l; do echo \"$l\"; done < list && make",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && { make; make test; } && echo done",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && (make) && echo done",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd proj && case x in x) make;; esac && echo ok",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("cd proj && make || true", ["Ask", "Ask", "Allow", "Allow"]),
        ("cd proj; make", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "if true; then echo ok; fi",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "for f in a b; do echo $f; done",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "while false; do echo x; done",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("f() { echo hi; }; f", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "function f { echo hi; }; f",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "{ echo a; echo b; } | sort",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("(cd src && ls)", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "git log | while read l; do echo \"$l\"; done",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "ls | for f in a b; do echo \"$f\"; done",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "echo hi | if true; then cat; fi",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cat file | $(brew --prefix)/bin/jq .",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "curl -s https://example.com/data.json | $(brew --prefix)/bin/jq .",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "curl -s https://example.com/x | $(which less)",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "curl -s https://example.com/x | $(which jq) .",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "git log | $(git var GIT_PAGER)",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("ls | $(which wc) -l", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "nohup $(which node) server.js",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "eval \"$(ssh-agent -s)\" && echo ok",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "eval \"$(direnv export bash)\"",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("echo \"${HOME}/x\"", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "echo \"${x:-default}\" ; ls",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("echo \"${x:-$(date)}\"", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "echo \"$(git rev-parse --short HEAD)\"",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "echo \"${x:-\"a b\"}\" && ls",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("rg TODO src", ["Allow", "Allow", "Allow", "Allow"]),
        ("rg -n TODO", ["Ask", "Ask", "Allow", "Allow"]),
        ("grep -rn TODO src", ["Allow", "Allow", "Allow", "Allow"]),
        ("ag TODO src", ["Ask", "Ask", "Allow", "Allow"]),
        ("ack TODO src", ["Ask", "Ask", "Allow", "Allow"]),
        ("cp -r src /tmp/x", ["Ask", "Ask", "Allow", "Allow"]),
        ("tar czf /tmp/x.tgz src", ["Ask", "Ask", "Allow", "Allow"]),
        ("rm -rf build", ["Ask", "Ask", "Allow", "Allow"]),
        ("mv a b", ["Ask", "Ask", "Allow", "Allow"]),
        ("crontab -l", ["Ask", "Ask", "Allow", "Allow"]),
        ("echo hi | eval 'echo ok'", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "source .venv/bin/activate && pytest",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (". ./env.sh && make", ["Ask", "Ask", "Allow", "Allow"]),
    ]);
}

/// Round 7 major: a compound (or a plain command) with a redirection or a
/// pipe after it, in a `cd proj &&` chain, keeps its place in the chain: the
/// `.npmrc` read after it is in `proj`, as dev reads it (eb0aa88 hard-denied
/// these). Generated: compounds × what follows them × what comes after.
/// Home asks, `Any` and FullAccess allow, prefix rules never hard-deny.
#[test]
fn round_7_major_generated_redirected_and_piped_compounds_in_a_cd_chain() {
    let compounds = [
        "(true)",
        "(ls; true)",
        "(make)",
        "{ true; }",
        "if true; then true; fi",
        "for f in a b; do true; done",
        "true",
        "ls",
        // a `cd` in a subshell stays a candidate, inside `proj` (dev
        // hard-denied this one)
        "(cd sub && make)",
    ];
    let after = [
        "",
        " > /dev/null",
        " 2>&1 > /dev/null",
        " >> out.log",
        " < /dev/null",
        " 2>/dev/null",
        " | cat",
        " |& cat",
        " 2>&1 | cat",
        " | cat | cat",
        " | wc -l",
        " | grep x | wc -l",
    ];
    let tails = ["&& cat .npmrc", "&& cat .npmrc && ls", "&& { cat .npmrc; }"];
    let env = Env::home();
    let mut failures = vec![];
    let mut n = 0;
    for c in compounds {
        for a in after {
            for t in tails {
                let line = format!("cd proj && {c}{a} {t}");
                n += 1;
                let got: Vec<String> = settings()
                    .iter()
                    .map(|(_, rules, preset)| {
                        code(&env.eval_check(&cmd_check(&line), rules, *preset))
                    })
                    .collect();
                let ok = got[0] == "Ask"
                    && matches!(got[1].as_str(), "Ask" | "Allow")
                    && got[2] == "Allow"
                    && got[3] == "Allow";
                if !ok {
                    failures.push(format!("{line:?}: got {got:?}"));
                }
            }
        }
    }
    assert!(n > 300, "{n}");
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Round 7: ordinary lines next to the other findings keep dev's verdict
/// (a function defined after `{` or `!`, `||` before an `eval` that
/// doesn't move, wrapper options, `ag`/`ack` context options, brace
/// sequences).
#[test]
fn round_7_ordinary_lines_keep_devs_verdict() {
    check_table(&[
        ("{ f() { true; }; } && f", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "! f() { true; } && echo ok",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("f() { true; } && f", ["Ask", "Ask", "Allow", "Allow"]),
        ("time make", ["Ask", "Ask", "Allow", "Allow"]),
        ("time -p make", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "false || eval 'echo hi' && ls",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("true || cd /tmp && ls", ["Ask", "Ask", "Allow", "Allow"]),
        ("script out.log", ["Ask", "Ask", "Allow", "Allow"]),
        ("script -q out.log make", ["Ask", "Ask", "Allow", "Allow"]),
        ("timeout -k 2 5 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("taskset -c 0-3 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("taskset 0x3 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("xargs --replace=X echo X", ["Ask", "Ask", "Allow", "Allow"]),
        ("ag --context=3 TODO src", ["Ask", "Ask", "Allow", "Allow"]),
        ("ag -C 3 TODO src", ["Ask", "Ask", "Allow", "Allow"]),
        ("ack --context=3 TODO src", ["Ask", "Ask", "Allow", "Allow"]),
        ("ack -C 3 TODO src", ["Ask", "Ask", "Allow", "Allow"]),
        ("echo {1..5}", ["Ask", "Ask", "Allow", "Allow"]),
        ("mkdir -p dir{1..3}", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "for i in {1..10}; do echo $i; done",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("cat proj/{a..c}.md", ["Ask", "Ask", "Allow", "Allow"]),
        ("echo {1..1000}", ["Ask", "Ask", "Allow", "Allow"]),
        ("(cd proj && make)", ["Ask", "Ask", "Allow", "Allow"]),
        ("(cd proj && make) > log", ["Ask", "Ask", "Allow", "Allow"]),
        ("(cd proj && ls) | wc -l", ["Ask", "Ask", "Allow", "Allow"]),
    ]);
}

/// Round 8, major 6 of the round-7 re-review: with a folder linked into
/// `~/.ssh` (`~/.ssh/work -> ~/vault/work`), searches from `~/vault` given
/// another folder keep dev's verdict (ask in Home, allow under `Any` and
/// FullAccess): a long option that gives the pattern (`ag
/// --filename-pattern`, `ack --match`) or a pattern file (`-f`, `--file`)
/// leaves every positional word a folder, and `rg -L` searches only the
/// folders it is given (ccb1b60 hard-denied the first four, asked on the
/// rest).
#[test]
fn round_8_major_6_searches_given_a_folder_beside_a_linked_one() {
    let t = Tmp::new();
    t.write("vault/work/id_work", "secret");
    t.write("vault/safe_sub/a.rs", "x");
    t.write(".ssh/known_hosts", "x");
    symlink(t.home.join("vault/work"), t.home.join(".ssh/work")).unwrap();
    let dir = t.home.join("vault");
    let settings = [
        ("home", vec![], Preset::Home),
        ("any", vec![any_rule()], Preset::Home),
        ("full", vec![], Preset::FullAccess),
    ];
    let mut failures = vec![];
    for line in [
        "ag --filename-pattern '*.rs' safe_sub",
        "ag --filename-pattern='*.rs' safe_sub",
        "ack --match pat safe_sub",
        "ack --match=pat safe_sub",
        "ag -g '*.rs' safe_sub",
        "ack -g pat safe_sub",
        "ack -f safe_sub",
        "ack -C 2 pat safe_sub",
        "ag -C 2 pat safe_sub",
        "rg -f /dev/null safe_sub",
        "grep -rf /dev/null safe_sub",
        "grep -r --file=/dev/null safe_sub",
        "grep -r --file /dev/null safe_sub",
        "rg -L pat safe_sub",
        "rg --follow pat safe_sub",
        "find safe_sub -name '*.rs'",
    ] {
        for (name, rules, preset) in &settings {
            let g = t.gate_with("t1", 1, &dir, *preset, rules.clone(), None);
            let d = block_on(g.gate.check(&cmd_check(line)));
            let ok = match *name {
                "home" => matches!(d, GateDecision::Ask { .. }),
                _ => matches!(d, GateDecision::Allow),
            };
            if !ok {
                failures.push(format!("[{name}] {line:?}: got {d:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

/// Round 8: ordinary lines next to the blockers keep dev's verdict.
#[test]
fn round_8_ordinary_lines_keep_devs_verdict() {
    check_table(&[
        (
            "timeout --signal=KILL 5 make",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "timeout --sig KILL 5 make",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("nice --adjustment=5 make", ["Ask", "Ask", "Allow", "Allow"]),
        ("env --chdir=proj make", ["Ask", "Ask", "Allow", "Allow"]),
        ("env --chd proj make", ["Ask", "Ask", "Allow", "Allow"]),
        ("xargs -I {} echo {}", ["Ask", "Ask", "Allow", "Allow"]),
        ("xargs -0 -n 1 echo", ["Ask", "Ask", "Allow", "Allow"]),
        ("flock /tmp/l make", ["Ask", "Ask", "Allow", "Allow"]),
        ("flock -c make /tmp/l", ["Ask", "Ask", "Allow", "Allow"]),
        ("script -c make /dev/null", ["Ask", "Ask", "Allow", "Allow"]),
        ("sh -c 'make test'", ["Ask", "Ask", "Allow", "Allow"]),
        ("bash -c 'cat README.md'", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "curl -s https://example.com/x | sh -c 'cat > out.txt'",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("$(which make) test", ["Ask", "Ask", "Allow", "Allow"]),
        ("$(which env) make test", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "find . -exec $(which wc) -l {} +",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        ("$(which eval) 'echo hi'", ["Ask", "Ask", "Allow", "Allow"]),
        (
            "cd proj && $(which source) .venv/bin/activate && pytest",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
    ]);
}
