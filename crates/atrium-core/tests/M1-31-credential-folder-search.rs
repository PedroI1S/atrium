#![allow(non_snake_case)]

mod common_adversarial;

use atrium_agents::{GateDecision, ToolCheck, ToolGate};
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope, ToolSubject};
use common_adversarial::{
    Env, HOME, Tmp, block_on, classify, cmd_check, paths_check, prefix, rule,
};

const CR: &str = "credential-read";
const AS: &str = "atrium-self";

fn eval_all_settings(env: &Env, check: &ToolCheck) -> [String; 4] {
    let any_exec = rule(
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-exec",
    );
    let any_search = rule(
        RuleScope::Global,
        ToolKind::Search,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-search",
    );
    let prefix_rules = [
        "rg", "grep", "egrep", "fgrep", "rgrep", "find", "fd", "fdfind", "ag", "ack", "git", "ls",
    ]
    .into_iter()
    .enumerate()
    .map(|(i, cmd)| {
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            prefix(&[cmd]),
            RuleDecision::Allow,
            &format!("p-{i}"),
        )
    })
    .collect::<Vec<Rule>>();
    let any_rules = [any_exec, any_search];
    [
        classify(&env.eval_check(check, &[], Preset::Home)),
        classify(&env.eval_check(check, &prefix_rules, Preset::Home)),
        classify(&env.eval_check(check, &any_rules, Preset::Home)),
        classify(&env.eval_check(check, &[], Preset::FullAccess)),
    ]
}

fn assert_hard_all(env: &Env, lines: &[&str], expected_id: &str) {
    let want = format!("Hard({expected_id})");
    for line in lines {
        let got = eval_all_settings(env, &cmd_check(line));
        assert_eq!(
            got,
            [want.clone(), want.clone(), want.clone(), want.clone()],
            "line: {line}"
        );
    }
}

#[test]
fn recursive_search_tools_block_every_hard_denied_folder_and_ancestors() {
    let env = Env::home();

    let cred_folders = [
        ".aws",
        "~/.aws",
        "~/.aws/",
        "./.aws",
        "$HOME/.aws",
        "\"$HOME\"/.aws",
        "~/proj/../.aws",
        "/h/u/.aws",
        "~/.AWS",
        ".docker",
        "~/.docker",
        ".kube",
        "~/.kube",
        ".config/gh",
        "~/.config/gh",
        ".claude",
        "~/.claude",
        ".codex",
        "~/.codex",
        ".ssh",
        "~/.ssh",
        ".gnupg",
        "~/.gnupg",
        ".config/gcloud",
        "~/.config/gcloud",
        ".gemini",
        "~/.gemini",
        "Library/Keychains",
        "~/Library/Keychains",
        "Library",
        "~/Library",
        "Library/Application Support",
        "~/Library/Application Support",
        "Library/Application Support/Google",
        "Library/Application Support/Google/Chrome",
        "~/Library/Application Support/Google/Chrome/Default",
        "Library/Cookies",
        ".config/google-chrome",
        "~/.config/chromium",
        ".mozilla",
        "~/.mozilla",
        ".mozilla/firefox",
    ];

    for dir in cred_folders {
        let quoted = if dir.contains(' ') {
            format!("'{dir}'")
        } else {
            dir.to_string()
        };
        let cmds = [
            format!("rg pat {quoted}"),
            format!("rg -e pat {quoted}"),
            format!("rg --files {quoted}"),
            format!("rg -f patfile {quoted}"),
            format!("rg --hidden pat {quoted}"),
            format!("rg -g '*.json' pat {quoted}"),
            format!("rg -- {quoted} {quoted}"),
            format!("grep -r pat {quoted}"),
            format!("grep -R pat {quoted}"),
            format!("grep -rn pat {quoted}"),
            format!("grep --recursive pat {quoted}"),
            format!("grep --rec pat {quoted}"),
            format!("grep -d recurse pat {quoted}"),
            format!("grep -drecurse pat {quoted}"),
            format!("grep --directories=recurse pat {quoted}"),
            format!("grep --dir=recurse pat {quoted}"),
            format!("grep -rf patfile {quoted}"),
            format!("egrep -r pat {quoted}"),
            format!("fgrep -r pat {quoted}"),
            format!("rgrep pat {quoted}"),
            format!("find {quoted}"),
            format!("find {quoted} -name '*.json'"),
            format!("find -L {quoted} -type f"),
            format!("fd pat {quoted}"),
            format!("fd -H pat {quoted}"),
            format!("fd --search-path {quoted} pat"),
            format!("fd --base-directory {quoted} pat"),
            format!("fdfind pat {quoted}"),
            format!("ag pat {quoted}"),
            format!("ag -g pat {quoted}"),
            format!("ag -C 3 pat {quoted}"),
            format!("ack pat {quoted}"),
            format!("ack -f {quoted}"),
            format!("ack -g pat {quoted}"),
            format!("git grep pat -- {quoted}"),
            format!("git grep -n pat -- {quoted}"),
            format!("git grep --no-index pat {quoted}"),
            format!("git grep -e pat {quoted}"),
            format!("git -C {quoted} grep pat"),
        ];
        let refs: Vec<&str> = cmds.iter().map(String::as_str).collect();
        assert_hard_all(&env, &refs, CR);
    }

    // Atrium data paths and their ancestors inside `~` -> atrium-self
    for dir in [
        ".local/share/atrium",
        "~/.local/share/atrium",
        ".local/share",
        "~/.local/share",
        ".local",
        "~/.local",
    ] {
        let cmds = [
            format!("rg pat {dir}"),
            format!("grep -r pat {dir}"),
            format!("find {dir}"),
            format!("fd pat {dir}"),
            format!("ag pat {dir}"),
            format!("ack pat {dir}"),
            format!("git grep pat -- {dir}"),
        ];
        let refs: Vec<&str> = cmds.iter().map(String::as_str).collect();
        assert_hard_all(&env, &refs, AS);
    }
}

#[test]
fn special_spellings_wrappers_brace_expansion_and_implicit_cwd() {
    let env = Env::home();
    assert_hard_all(
        &env,
        &[
            "rg pat ~/{.aws,src}",
            "grep -r pat {.docker,src}",
            "find ~/{.kube,src} -type f",
            "fd pat ~/{.claude,src}",
            "ag pat ~/{.codex,src}",
            "ack pat ~/{.config/gh,src}",
            "git grep pat -- ~/{.aws,src}",
            "git -C ~ grep pat -- .aws",
            "git --work-tree ~/.aws grep pat",
            "git grep pat -- ':(top).aws'",
            "git grep pat -- ':/.aws'",
            "cd ~/.aws && rg pat",
            "cd ~/.aws && rg pat .",
            "cd ~/.aws && grep -r pat",
            "cd ~/.aws && find",
            "cd ~/.aws && fd pat",
            "cd ~/.aws && ag pat",
            "cd ~/.aws && ack pat",
            "cd ~/.aws && git grep pat",
            "(cd ~/.aws; rg pat)",
            "cd ~/.aws; rg pat",
            "env rg pat ~/.aws",
            "env -C ~/.aws rg pat",
            "sh -c 'grep -r pat ~/.aws'",
            "echo $(fd pat ~/.aws)",
            "find /tmp -exec ag pat ~/.aws \\;",
            "rg --some-unknown-flag pat ~/.aws",
            "rg -ne pat ~/.aws",
            "rg -nepat ~/.aws",
        ],
        CR,
    );

    let mut c = cmd_check("rg pat");
    c.subject = ToolSubject::Command {
        line: "rg pat".into(),
        cwd: Some(".aws".into()),
    };
    assert_eq!(eval_all_settings(&env, &c), ["Hard(credential-read)"; 4]);
}

#[test]
fn paths_subject_recursive_blocks_hard_denied_folders_and_keeps_non_recursive() {
    let env = Env::home();
    for dir in [
        ".aws",
        "~/.aws",
        ".docker",
        ".kube",
        ".config/gh",
        ".claude",
        ".codex",
        ".ssh",
        ".gnupg",
        ".config/gcloud",
        ".gemini",
        "Library",
        "Library/Keychains",
        "Library/Application Support/Google/Chrome",
        ".mozilla",
    ] {
        let rec = paths_check(ToolKind::Search, "Grep", &[dir], true, None);
        assert_eq!(
            eval_all_settings(&env, &rec),
            ["Hard(credential-read)"; 4],
            "recursive Grep on {dir}"
        );
        let rec_glob = paths_check(ToolKind::Search, "Grep", &[dir], true, Some("*.json"));
        assert_eq!(
            eval_all_settings(&env, &rec_glob),
            ["Hard(credential-read)"; 4],
            "recursive Grep with glob on {dir}"
        );
    }

    // Atrium data ancestors
    for dir in [".local", ".local/share", ".local/share/atrium"] {
        let rec = paths_check(ToolKind::Search, "Grep", &[dir], true, None);
        assert_eq!(
            eval_all_settings(&env, &rec),
            ["Hard(atrium-self)"; 4],
            "recursive Grep on {dir}"
        );
    }

    // Empty paths with cwd inside ~/.aws
    let mut aws_env = Env::home();
    aws_env.cwd = format!("{HOME}/.aws").into();
    let rec_cwd = paths_check(ToolKind::Search, "Grep", &[], true, None);
    assert_eq!(
        eval_all_settings(&aws_env, &rec_cwd),
        ["Hard(credential-read)"; 4]
    );

    // Non-recursive Paths search on ~/.aws keeps dev's verdict (Allow in Home)
    let non_rec = paths_check(ToolKind::Search, "Glob", &["~/.aws"], false, None);
    assert_eq!(
        eval_all_settings(&env, &non_rec),
        ["Allow", "Allow", "Allow", "Allow"]
    );
}

#[test]
fn cr12_stores_keep_asking_in_home() {
    let env = Env::home();
    // CR-12 (D-082): Ask in Home with no rules, Allow under a rule or FullAccess.
    let cr12_verdict = [
        "Ask".to_string(),
        "Allow".to_string(),
        "Allow".to_string(),
        "Allow".to_string(),
    ];
    for dir in [
        "~/.cargo",
        "~/.aws/sso",
        "~/.aws/sso/cache",
        "~/.azure",
        "~/.local/share/fish",
        "~/.config/git",
        "~/.config/fish",
        "~/.zsh_sessions",
        "~/.bash_sessions",
    ] {
        for line in [
            format!("rg token {dir}"),
            format!("grep -r token {dir}"),
            format!("find {dir}"),
            format!("fd token {dir}"),
            format!("ag token {dir}"),
            format!("ack token {dir}"),
            format!("git grep token -- {dir}"),
        ] {
            assert_eq!(
                eval_all_settings(&env, &cmd_check(&line)),
                cr12_verdict,
                "CR-12 line: {line}"
            );
        }
        let rec = paths_check(ToolKind::Search, "Grep", &[dir], true, None);
        assert_eq!(
            eval_all_settings(&env, &rec),
            [
                "Ask".to_string(),
                "Ask".to_string(),
                "Allow".to_string(),
                "Allow".to_string(),
            ],
            "CR-12 Paths on {dir}"
        );
    }
}

#[test]
fn non_recursive_reads_and_ordinary_searches_keep_devs_verdict() {
    let env = Env::home();
    let allow4 = [
        "Allow".to_string(),
        "Allow".to_string(),
        "Allow".to_string(),
        "Allow".to_string(),
    ];
    let ask_home_allow_prefix = [
        "Ask".to_string(),
        "Allow".to_string(),
        "Allow".to_string(),
        "Allow".to_string(),
    ];
    let ask_home_allow_broad = [
        "Ask".to_string(),
        "Ask".to_string(),
        "Allow".to_string(),
        "Allow".to_string(),
    ];

    // Non-recursive folder listings and non-recursive grep keep dev's verdict
    for line in [
        "ls ~/.aws",
        "ls -la ~/.aws",
        "ls ~/.docker",
        "ls ~/.kube",
        "ls ~/.config/gh",
        "ls ~/.claude",
        "ls ~/.codex",
        "ls ~/.ssh",
        "ls ~/.local/share",
        "ls ~/Library",
        "cat ~/.aws/config",
        "rg token ~/.aws/config",
        "grep pat ~/.aws",
        "rg x src",
        "rg x ~/proj",
        "rg .aws proj",
        "grep -r .aws proj",
        "grep -r x src",
        "grep -r --exclude-dir=.aws x proj",
        "rg x proj/.aws",
        "rg x proj/.ssh",
    ] {
        assert_eq!(
            eval_all_settings(&env, &cmd_check(line)),
            allow4,
            "ordinary Allow line: {line}"
        );
    }

    // Home-wide search of `~` itself (and `.` when cwd=`~`), `~/.config`, and
    // tools not on Home's read-only list keep Ask in Home, Allow under prefix/Any/FullAccess
    for line in [
        "rg x ~",
        "rg x ~/",
        "rg x .",
        "rg x",
        "grep -r x ~",
        "grep -r x .",
        "grep -r x",
        "rg token ~/.config",
        "grep -r token ~/.config",
        "find ~/.config/atrium -name x",
        "grep -r pat -",
        "grep -d skip pat ~/.aws",
        "find src -name '*.rs'",
        "fd pat src",
        "fd .aws proj",
        "ag pat src",
        "ag .aws proj",
        "ag -C 3 .aws proj",
        "ack pat src",
        "ack .aws proj",
        "git grep pat -- src",
        "git grep .aws -- proj",
        "git grep .aws",
        "git grep pat -- ':!.aws'",
        "git grep pat -- ':(exclude).aws'",
        "git -C ~/proj grep pat",
    ] {
        assert_eq!(
            eval_all_settings(&env, &cmd_check(line)),
            ask_home_allow_prefix,
            "ordinary Ask-in-Home line: {line}"
        );
    }

    // Recursive Paths search on `~` itself and `~/.config` keeps Ask in Home + prefix
    for dir in ["~", "~/.config", "~/proj"] {
        let want = if dir == "~/proj" {
            &allow4
        } else {
            &ask_home_allow_broad
        };
        let rec = paths_check(ToolKind::Search, "Grep", &[dir], true, None);
        assert_eq!(&eval_all_settings(&env, &rec), want, "Paths on {dir}");
    }
}

#[cfg(unix)]
#[test]
fn gate_blocks_symlinks_into_credential_folders() {
    use std::os::unix::fs::symlink;

    let t = Tmp::new();
    t.write(".aws/credentials", "secret");
    t.write(".docker/config.json", "{}");
    t.write("proj/src/lib.rs", "fn main() {}");
    symlink(t.home.join(".aws"), t.home.join("proj/link-aws")).unwrap();
    symlink(t.home.join(".docker"), t.home.join("proj/link-docker")).unwrap();

    for preset in [Preset::Home, Preset::FullAccess] {
        let g = t.gate(preset, vec![]);
        for line in [
            "rg pat proj/link-aws",
            "grep -r pat proj/link-aws",
            "find proj/link-aws",
            "fd pat proj/link-aws",
            "ag pat proj/link-docker",
            "ack pat proj/link-docker",
            "git grep pat -- proj/link-aws",
            "git -C proj/link-aws grep pat",
            "cd proj/link-aws && rg pat",
        ] {
            let d = block_on(g.gate.check(&cmd_check(line)));
            assert!(
                matches!(d, GateDecision::Deny { .. }),
                "{preset:?} {line}: expected Deny, got {d:?}"
            );
        }
        let c = paths_check(ToolKind::Search, "Grep", &["proj/link-aws"], true, None);
        let d = block_on(g.gate.check(&c));
        assert!(
            matches!(d, GateDecision::Deny { .. }),
            "{preset:?} Grep proj/link-aws: expected Deny, got {d:?}"
        );
    }

    // Ordinary project folder and non-recursive ls through link keep dev's verdict
    let g = t.gate(Preset::Home, vec![]);
    for line in ["rg main proj/src", "ls proj/link-aws"] {
        let d = block_on(g.gate.check(&cmd_check(line)));
        assert_eq!(d, GateDecision::Allow, "{line}: got {d:?}");
    }
}

fn eval_search_presets(env: &Env, check: &ToolCheck) -> [String; 3] {
    let any_search = rule(
        RuleScope::Global,
        ToolKind::Search,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-search",
    );
    [
        classify(&env.eval_check(check, &[], Preset::Home)),
        classify(&env.eval_check(check, &[any_search], Preset::Home)),
        classify(&env.eval_check(check, &[], Preset::FullAccess)),
    ]
}

#[test]
fn k10_400_star_dotdot_globs_stay_under_50ms_and_fail_closed_to_ask() {
    let env = Env::home();
    let ask3 = ["Ask".to_string(), "Ask".to_string(), "Ask".to_string()];

    let star_dd = |n: usize| format!("{}{}", "*/".repeat(n), "../".repeat(n));
    let star_dd_noslash = |n: usize| format!("{}..{}", "*/".repeat(n), "/..".repeat(n - 1));

    let cases = [
        ("400 */ + 400 ..", vec!["~/proj"], star_dd_noslash(400)),
        ("400 */ + 400 ../", vec!["~/proj"], star_dd(400)),
        ("500 */ + 500 ../", vec!["~"], star_dd(500)),
        ("2000 */ + 2000 ../", vec!["~/proj"], star_dd(2000)),
        (
            "20000 */ + 20000 ../ (~100 KB)",
            vec!["~/proj"],
            star_dd(20_000),
        ),
        (
            "400 */ + 400 ../.ssh/id_rsa",
            vec!["~"],
            format!("{}.ssh/id_rsa", star_dd(400)),
        ),
        (
            "500 **/ + 500 ../",
            vec!["~/proj"],
            format!("{}{}", "**/".repeat(500), "../".repeat(500)),
        ),
        (
            "200 */ + 200 ../ + 20000 a/ (work budget)",
            vec!["~/proj"],
            format!("{}{}", star_dd(200), "a/".repeat(20_000)),
        ),
        (
            "256 brace * 400 segments (work budget)",
            vec!["~/proj"],
            format!("{{1..64}}{{a,b,c,d}}/{}", "a/".repeat(400)),
        ),
        (
            "over MAX_BYTES (> 128 KiB)",
            vec!["~/proj"],
            "a".repeat(128 * 1024 + 1),
        ),
    ];

    for (label, roots, glob) in cases {
        let check = paths_check(ToolKind::Search, "Grep", &roots, true, Some(&glob));
        let start = std::time::Instant::now();
        let got = eval_search_presets(&env, &check);
        let elapsed = start.elapsed();
        assert_eq!(got, ask3, "{label}: expected Ask everywhere, got {got:?}");
        assert!(
            elapsed.as_millis() < 750,
            "{label}: took too long (>250 ms per evaluate across 3 presets): {elapsed:?}"
        );
    }

    let brackets = "[".repeat(100_000);
    let bracket_check = paths_check(ToolKind::Search, "Grep", &["~/proj"], true, Some(&brackets));
    let start = std::time::Instant::now();
    let got_brackets = eval_search_presets(&env, &bracket_check);
    let elapsed_brackets = start.elapsed();
    assert_eq!(
        got_brackets,
        ["Ask".to_string(), "Allow".to_string(), "Allow".to_string()],
        "100000 unclosed brackets: got {got_brackets:?}"
    );
    assert!(
        elapsed_brackets.as_millis() < 750,
        "100000 unclosed brackets took too long (>250 ms per evaluate across 3 presets): {elapsed_brackets:?}"
    );
}

#[test]
fn k10_bounded_star_dotdot_globs_preserve_verdicts_and_stay_fast() {
    let env = Env::home();
    let allow3 = [
        "Allow".to_string(),
        "Allow".to_string(),
        "Allow".to_string(),
    ];
    let ask_home3 = ["Ask".to_string(), "Allow".to_string(), "Allow".to_string()];
    let ask3 = ["Ask".to_string(), "Ask".to_string(), "Ask".to_string()];
    let cr3 = [
        "Hard(credential-read)".to_string(),
        "Hard(credential-read)".to_string(),
        "Hard(credential-read)".to_string(),
    ];

    let star_dd = |n: usize| format!("{}{}", "*/".repeat(n), "../".repeat(n));

    let t100 = std::time::Instant::now();
    let got100 = eval_search_presets(
        &env,
        &paths_check(
            ToolKind::Search,
            "Grep",
            &["~/proj"],
            true,
            Some(&star_dd(100)),
        ),
    );
    let d100 = t100.elapsed();
    assert_eq!(got100, allow3);
    assert!(
        d100.as_millis() < 750,
        "100 */ + 100 .. took too long (>250 ms per evaluate across 3 presets): {d100:?}"
    );

    let t200 = std::time::Instant::now();
    let got200 = eval_search_presets(
        &env,
        &paths_check(
            ToolKind::Search,
            "Grep",
            &["~/proj"],
            true,
            Some(&star_dd(200)),
        ),
    );
    let d200 = t200.elapsed();
    assert_eq!(got200, allow3);
    assert!(
        d200.as_millis() < 750,
        "200 */ + 200 .. took too long (>250 ms per evaluate across 3 presets): {d200:?}"
    );
    // Doubling 100 -> 200 segments must scale linearly (not quadratically like dev).
    assert!(
        d200 <= d100.mul_f64(2.5) + std::time::Duration::from_millis(20),
        "200 */ + 200 .. scaled superlinearly vs 100: d100={d100:?}, d200={d200:?}"
    );

    let t255 = std::time::Instant::now();
    assert_eq!(
        eval_search_presets(
            &env,
            &paths_check(
                ToolKind::Search,
                "Grep",
                &["~/proj"],
                true,
                Some(&star_dd(255)),
            ),
        ),
        allow3
    );
    let d255 = t255.elapsed();
    assert!(
        d255.as_millis() < 750,
        "255 */ + 255 .. took too long (>250 ms per evaluate across 3 presets): {d255:?}"
    );
    assert_eq!(
        eval_search_presets(
            &env,
            &paths_check(
                ToolKind::Search,
                "Grep",
                &["~/proj"],
                true,
                Some(&star_dd(256)),
            ),
        ),
        ask3
    );
    assert_eq!(
        eval_search_presets(
            &env,
            &paths_check(
                ToolKind::Search,
                "Grep",
                &["~"],
                true,
                Some(&format!("{}.ssh/id_rsa", star_dd(200))),
            ),
        ),
        cr3
    );
    assert_eq!(
        eval_search_presets(
            &env,
            &paths_check(
                ToolKind::Search,
                "Grep",
                &["~"],
                true,
                Some(&format!("{}.cargo/credentials.toml", star_dd(100))),
            ),
        ),
        ask_home3
    );
    assert_eq!(
        eval_search_presets(
            &env,
            &paths_check(
                ToolKind::Search,
                "Grep",
                &["~/proj"],
                true,
                Some(&format!("{}../*.rs", star_dd(100))),
            ),
        ),
        ask_home3
    );
}
