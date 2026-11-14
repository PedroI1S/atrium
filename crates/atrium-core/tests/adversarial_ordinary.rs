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

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope};
use common_adversarial::*;

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
