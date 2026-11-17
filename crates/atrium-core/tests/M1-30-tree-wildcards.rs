//! M1-30-tree-wildcards (S5 / M1-13 finding 11): wildcards (`*`, `?`, classes,
//! braces) in credential tree names, ask store names, and Atrium folders.

mod common_adversarial;

use atrium_agents::{GateDecision, ToolGate};
use atrium_core::PolicyVerdict;
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, RuleDecision, RuleMatcher, RuleScope};
use common_adversarial::{
    Env, HOME, Tmp, Want, block_on, classify, cmd_check, paths_check, rule, run_table,
};

const CR: &str = "credential-read";
const AS: &str = "atrium-self";

fn env_with_app_support() -> Env {
    let mut env = Env::home();
    env.data
        .push(format!("{HOME}/Library/Application Support/atrium").into());
    env.prot
        .push(format!("{HOME}/Library/Application Support/atrium-cfg").into());
    env
}

fn any_execute_rule() -> Vec<atrium_protocol::policy::Rule> {
    vec![rule(
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any",
    )]
}

fn prefix_rules() -> Vec<atrium_protocol::policy::Rule> {
    ["cd", "cat", "pushd", "popd", "ls"]
        .iter()
        .enumerate()
        .map(|(i, w)| {
            rule(
                RuleScope::Global,
                ToolKind::Execute,
                RuleMatcher::CommandPrefix {
                    argv: vec![(*w).to_string()],
                },
                RuleDecision::Allow,
                &format!("p{i}"),
            )
        })
        .collect()
}

fn check_4_settings(env: &Env, rows: &[(&str, [&str; 4])]) {
    let settings = [
        ("home", vec![], Preset::Home),
        ("prefix", prefix_rules(), Preset::Home),
        ("any", any_execute_rule(), Preset::Home),
        ("full", vec![], Preset::FullAccess),
    ];
    let mut failures = vec![];
    for (line, want) in rows {
        let got: Vec<String> = settings
            .iter()
            .map(|(_, rules, preset)| classify(&env.eval_check(&cmd_check(line), rules, *preset)))
            .collect();
        if got.iter().map(String::as_str).ne(want.iter().copied()) {
            failures.push(format!("{line:?}: wanted {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn wildcards_in_each_component_of_cred_trees_and_files() {
    let env = Env::home();
    run_table(
        &env,
        &[
            // .gnupg (CRED_TREES): *, ?, class, braces, arbitrary depth, relative + home-anchored
            ("cat ~/.g*/private-keys-v1.d/*", Want::Hard(CR)),
            ("cat ~/.gn?pg/private-keys-v1.d/x.key", Want::Hard(CR)),
            ("cat ~/.[g]nupg/secring.gpg", Want::Hard(CR)),
            ("cat ~/.{g,x}nupg/trustdb.gpg", Want::Hard(CR)),
            ("cat .g*/a/b/c/d.key", Want::Hard(CR)),
            ("cat $HOME/.g*", Want::Hard(CR)),
            // .config/gcloud (CRED_TREES): each component
            ("cat ~/.con*/gcloud/credentials.db", Want::Hard(CR)),
            ("cat ~/.c?nfig/gcloud/credentials.db", Want::Hard(CR)),
            ("cat ~/.[c]onfig/gcloud/x", Want::Hard(CR)),
            ("cat ~/.{c,d}onfig/gcloud/x", Want::Hard(CR)),
            ("cat ~/.config/gcl*/credentials.db", Want::Hard(CR)),
            ("cat ~/.config/g?loud/x", Want::Hard(CR)),
            ("cat ~/.config/[g]cloud/x", Want::Hard(CR)),
            ("cat ~/.config/{g,x}cloud/x", Want::Hard(CR)),
            ("cat .c?nfig/gcl*/sub/deep.json", Want::Hard(CR)),
            // .gemini (CRED_TREES)
            ("cat ~/.gem*/oauth_creds.json", Want::Hard(CR)),
            ("cat ~/.g?mini/oauth_creds.json", Want::Hard(CR)),
            ("cat ~/.[g]emini/x", Want::Hard(CR)),
            ("cat ~/.{g,x}emini/x", Want::Hard(CR)),
            // Library/Keychains (CRED_TREES, case-folded)
            ("cat ~/Lib*/Keychains/login.keychain-db", Want::Hard(CR)),
            ("cat ~/L?brary/Keychains/login.keychain-db", Want::Hard(CR)),
            ("cat ~/[L]ibrary/Keychains/x", Want::Hard(CR)),
            ("cat ~/{L,X}ibrary/Keychains/x", Want::Hard(CR)),
            ("cat ~/Library/Key*/login.keychain-db", Want::Hard(CR)),
            ("cat ~/Library/K?ychains/x", Want::Hard(CR)),
            ("cat ~/Library/[K]eychains/x", Want::Hard(CR)),
            ("cat ~/Library/{K,X}eychains/x", Want::Hard(CR)),
            ("cat Library/Key*/sub/item", Want::Hard(CR)),
            // CRED_FILES: each component
            ("cat ~/.a?s/credentials", Want::Hard(CR)),
            ("cat ~/.[a]ws/credentials", Want::Hard(CR)),
            ("cat ~/.{a,b}ws/credentials", Want::Hard(CR)),
            ("cat ~/.aws/cred*", Want::Hard(CR)),
            ("cat ~/.aws/cred?ntials", Want::Hard(CR)),
            ("cat ~/.aws/[c]redentials", Want::Hard(CR)),
            ("cat ~/.aws/{cred,x}entials", Want::Hard(CR)),
            ("cat ~/.n*rc", Want::Hard(CR)),
            ("cat ~/.n?trc", Want::Hard(CR)),
            ("cat ~/.[n]pmrc", Want::Hard(CR)),
            ("cat ~/.{git,x}-credentials", Want::Hard(CR)),
            ("cat ~/.d?cker/config.json", Want::Hard(CR)),
            ("cat ~/.docker/conf?g.json", Want::Hard(CR)),
            ("cat ~/.k*be/config", Want::Hard(CR)),
            ("cat ~/.kube/conf?g", Want::Hard(CR)),
            ("cat ~/.config/g?/hosts.yml", Want::Hard(CR)),
            ("cat ~/.config/gh/ho?ts.yml", Want::Hard(CR)),
            ("cat ~/.cl*de/.credentials.json", Want::Hard(CR)),
            ("cat ~/.claude/.cred*json", Want::Hard(CR)),
            ("cat ~/.c?dex/auth.json", Want::Hard(CR)),
            ("cat ~/.codex/au?h.json", Want::Hard(CR)),
            ("cat ~/.cl?ude.json", Want::Hard(CR)),
        ],
    );
}

#[test]
fn wildcards_in_ssh_dir_keep_plain_ssh_verdicts() {
    let env = Env::home();
    run_table(
        &env,
        &[
            // files inside .ssh: private keys blocked
            ("cat ~/.s?h/id_rsa", Want::Hard(CR)),
            ("cat ~/.[s]sh/id_ed25519", Want::Hard(CR)),
            ("cat ~/.s*/id_ecdsa", Want::Hard(CR)),
            ("cat ~/.{s,t}sh/id_rsa", Want::Hard(CR)),
            ("cat .s?h/sub/deep_key", Want::Hard(CR)),
            ("cat ~/.ssh/id_*", Want::Hard(CR)),
            ("cat ~/.ssh/id_rs?", Want::Hard(CR)),
            ("cat ~/.ssh/id_[r]sa", Want::Hard(CR)),
            ("cat ~/.ssh/id_{rsa,dsa}", Want::Hard(CR)),
            ("cat ~/.ssh/**", Want::Hard(CR)),
            // .ssh dir itself: cat/rg/rm blocked, ls not hard-denied (same as plain ~/.ssh with glob Ask)
            ("cat ~/.[s]sh", Want::Hard(CR)),
            ("cat ~/.s?h", Want::Hard(CR)),
            ("rg x ~/.[s]sh", Want::Hard(CR)),
            ("rg x ~/.s?h", Want::Hard(CR)),
            ("rm ~/.[s]sh", Want::Hard(CR)),
            ("rm -rf ~/.[s]sh", Want::AnyHard),
            ("rm ~/.s?h/config", Want::Hard(CR)),
            ("echo x > ~/.[s]sh/authorized_keys", Want::Hard(CR)),
        ],
    );
    check_4_settings(
        &env,
        &[
            ("ls ~/.[s]sh", ["Ask", "Ask", "Allow", "Allow"]),
            ("ls ~/.s?h", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/.s?h/id_rsa.pub", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/.[s]sh/*.pub", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/.s?h/known_hosts*", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/.s?h/config", ["Ask", "Ask", "Allow", "Allow"]),
            (
                "cat ~/.[s]sh/authorized_keys",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
        ],
    );
}

#[test]
fn wildcards_in_browser_roots() {
    let env = Env::home();
    run_table(
        &env,
        &[
            (
                "cat \"~/Lib*/Application Support/Google/Chrome/Default/Cookies\"",
                Want::Hard(CR),
            ),
            (
                "cat \"~/Library/App*/Google/Chrome/Default/Login Data\"",
                Want::Hard(CR),
            ),
            (
                "cat \"~/Library/Application Support/Goo*/Chrome/Default/Cookies\"",
                Want::Hard(CR),
            ),
            (
                "cat \"~/Library/Application Support/Google/Chr*/Default/Cookies\"",
                Want::Hard(CR),
            ),
            (
                "cat \"~/Library/Application Support/Google/Chrome/Default/Cook*\"",
                Want::Hard(CR),
            ),
            ("cat ~/.config/g*chrome/Default/Cookies", Want::Hard(CR)),
            ("cat ~/.config/[g]oogle-chrome/*/key4.db", Want::Hard(CR)),
            ("cat ~/.moz*/firefox/*/cookies.sqlite", Want::Hard(CR)),
        ],
    );
    check_4_settings(
        &env,
        &[
            (
                "cat ~/Library/Application\\ Support/Google/Chr?/e/Default/Cookies",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/.config/g*chrome/Default/README.md",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
        ],
    );
}

#[test]
fn wildcards_in_atrium_data_and_protected_folders() {
    let env = env_with_app_support();
    run_table(
        &env,
        &[
            // data: reads and writes are HardDeny(atrium-self)
            ("cat ~/.l*/share/atrium/atrium.db", Want::Hard(AS)),
            ("cat ~/.local/sh?re/atrium/atrium.db", Want::Hard(AS)),
            ("cat ~/.local/share/atr*/atrium.db", Want::Hard(AS)),
            ("cat ~/.local/share/[a]trium/atrium.db", Want::Hard(AS)),
            ("cat ~/.local/{share,lib}/atrium/atrium.db", Want::Hard(AS)),
            ("cat ~/.local/*/atrium/atrium.db", Want::Hard(AS)),
            ("ls ~/.local/share/atr*", Want::Hard(AS)),
            ("rm ~/.local/share/atr*/atrium.db", Want::Hard(AS)),
            (
                "cat ~/Library/Application\\ Support/atr*/atrium.db",
                Want::Hard(AS),
            ),
            ("cat ~/Lib*/App*/atrium/sub/wal.db", Want::Hard(AS)),
            // protected: writes are HardDeny(atrium-self)
            ("rm ~/.config/atr*/config.toml", Want::Hard(AS)),
            ("echo x > ~/.config/atr*/config.toml", Want::Hard(AS)),
            ("echo x > ~/.[c]onfig/atrium/config.toml", Want::Hard(AS)),
            ("echo x > ~/.config/at?ium/config.toml", Want::Hard(AS)),
            (
                "echo x > ~/.config/{atrium,other}/config.toml",
                Want::Hard(AS),
            ),
            (
                "rm ~/Library/Application\\ Support/atr*-cfg/config.toml",
                Want::Hard(AS),
            ),
        ],
    );
    check_4_settings(
        &env,
        &[
            (
                "cat ~/.config/atr*/config.toml",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            ("ls ~/.config/atr*", ["Ask", "Ask", "Allow", "Allow"]),
            (
                "cat ~/Library/Application\\ Support/atr*-cfg/config.toml",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
        ],
    );
}

#[test]
fn wildcards_in_ask_stores_and_trees() {
    let env = Env::home();
    run_table(
        &env,
        &[
            // ASK_FILES with *, ?, class, braces
            ("cat ~/.c*go/credentials.toml", Want::Ask),
            ("cat ~/.[c]argo/credentials", Want::Ask),
            ("cat ~/.[e]nv", Want::Ask),
            ("cat ~/.z*h_history", Want::Ask),
            ("cat ~/.local/*/fish/fish_history", Want::Ask),
            ("cat ~/.config/f?sh/fish_history", Want::Ask),
            ("cat ~/.config/g?t/credentials", Want::Ask),
            ("cat ~/.{bash,zsh}_history", Want::Ask),
            // ASK_TREES
            ("cat ~/.a?s/sso/cache/x.json", Want::Ask),
            ("cat ~/.aws/s?o/cache/x.json", Want::Ask),
            ("cat ~/.aws/sso/c*che/x.json", Want::Ask),
            ("cat ~/.z*h_sessions/abc", Want::Ask),
            ("cat ~/.[b]ash_sessions/abc", Want::Ask),
            // ASK_NAME_PREFIXES (.azure/msal_token_cache.*)
            ("cat ~/.[a]zure/msal_token_cache.json", Want::Ask),
            ("cat ~/.a?ure/msal_*", Want::Ask),
            ("cat ~/.azure/msal_?oken_cache.db", Want::Ask),
            ("cat ~/.azure/config", Want::Allow),
        ],
    );
    // Ask stores are still allowed under FullAccess (not hard-denied)
    for line in [
        "cat ~/.[e]nv",
        "cat ~/.z*h_sessions/abc",
        "cat ~/.a?ure/msal_token_cache.db",
    ] {
        let v = env.eval_check(&cmd_check(line), &[], Preset::FullAccess);
        assert!(
            matches!(v, PolicyVerdict::Allow { .. }),
            "{line}: expected Allow in FullAccess, got {}",
            classify(&v)
        );
    }
}

#[test]
fn relative_paths_from_subdir_and_file_tools_reads_and_searches() {
    let mut env = Env::home();
    env.cwd = format!("{HOME}/proj").into();

    // Relative command words escaping ~/proj with ..
    run_table(
        &env,
        &[
            ("cat ../.s?h/id_rsa", Want::Hard(CR)),
            ("cat ../.g*/private-keys-v1.d/*", Want::Hard(CR)),
            ("cat ../.local/*/atrium/atrium.db", Want::Hard(AS)),
            ("rm ../.config/atr*/config.toml", Want::Hard(AS)),
            ("cat ../.[e]nv", Want::Ask),
        ],
    );
    check_4_settings(&env, &[("cat src/*.rs", ["Ask", "Ask", "Allow", "Allow"])]);

    // ToolKind::Read / Edit / Search via ToolSubject::Paths
    type PathCase = (
        &'static str,
        ToolKind,
        &'static [&'static str],
        bool,
        Option<&'static str>,
        Want,
    );
    let cases: &[PathCase] = &[
        (
            "Read ~/.s?h/id_rsa",
            ToolKind::Read,
            &["~/.s?h/id_rsa"],
            false,
            None,
            Want::Hard(CR),
        ),
        (
            "Read ../.local/*/atrium/atrium.db",
            ToolKind::Read,
            &["../.local/*/atrium/atrium.db"],
            false,
            None,
            Want::Hard(AS),
        ),
        (
            "Read ~/.config/atr*/config.toml",
            ToolKind::Read,
            &["~/.config/atr*/config.toml"],
            false,
            None,
            Want::Allow,
        ),
        (
            "Write ~/.config/atr*/config.toml",
            ToolKind::Edit,
            &["~/.config/atr*/config.toml"],
            false,
            None,
            Want::Hard(AS),
        ),
        (
            "Read ~/.[e]nv",
            ToolKind::Read,
            &["~/.[e]nv"],
            false,
            None,
            Want::Ask,
        ),
        (
            "Search paths=~/.[s]sh recursive",
            ToolKind::Search,
            &["~/.[s]sh"],
            true,
            None,
            Want::Hard(CR),
        ),
        (
            "Search paths=~/.g* recursive",
            ToolKind::Search,
            &["~/.g*"],
            true,
            None,
            Want::Hard(CR),
        ),
        (
            "Search escaping glob ../.g*/private-keys-v1.d/*",
            ToolKind::Search,
            &["~/proj"],
            true,
            Some("../.g*/private-keys-v1.d/*"),
            Want::Hard(CR),
        ),
        (
            "Search escaping glob ../.[s]sh/id_rsa",
            ToolKind::Search,
            &["~/proj"],
            true,
            Some("../.[s]sh/id_rsa"),
            Want::Hard(CR),
        ),
        (
            "Search escaping glob ../.local/*/atrium/atrium.db",
            ToolKind::Search,
            &["~/proj"],
            true,
            Some("../.local/*/atrium/atrium.db"),
            Want::Hard(AS),
        ),
        (
            "Search non-escaping in ~ glob=.g*/private-keys-v1.d/*",
            ToolKind::Search,
            &["~"],
            true,
            Some(".g*/private-keys-v1.d/*"),
            Want::Hard(CR),
        ),
        (
            "Search non-escaping in ~ glob=.local/*/atrium/atrium.db",
            ToolKind::Search,
            &["~"],
            true,
            Some(".local/*/atrium/atrium.db"),
            Want::Hard(AS),
        ),
        (
            "Search non-escaping in ~ glob=.s?h/id_rsa",
            ToolKind::Search,
            &["~"],
            true,
            Some(".s?h/id_rsa"),
            Want::Hard(CR),
        ),
        (
            "Search non-escaping ask tree .z*/*.session",
            ToolKind::Search,
            &["~"],
            false,
            Some(".z*/*.session"),
            Want::Ask,
        ),
        (
            "Search non-escaping ask prefix .az*/msal_token_cache.db",
            ToolKind::Search,
            &["~"],
            false,
            Some(".az*/msal_token_cache.db"),
            Want::Ask,
        ),
        (
            "Ordinary Search in ~/Documents *.md",
            ToolKind::Search,
            &["~/Documents"],
            true,
            Some("*.md"),
            Want::Allow,
        ),
        (
            "Ordinary Search in ~/proj src/*.rs",
            ToolKind::Search,
            &["~/proj"],
            true,
            Some("src/*.rs"),
            Want::Allow,
        ),
        (
            "Ordinary Search non-recursive in ~ *.rs",
            ToolKind::Search,
            &["~"],
            false,
            Some("*.rs"),
            Want::Allow,
        ),
    ];

    for (label, kind, paths, rec, glob, want) in cases {
        let c = paths_check(*kind, "Tool", paths, *rec, *glob);
        let v = env.eval_check(&c, &[], Preset::Home);
        assert!(
            common_adversarial::matches_want(&v, want),
            "{label}: wanted {want:?}, got {}",
            classify(&v)
        );
    }
}

#[test]
fn ordinary_globs_keep_expected_verdicts() {
    let env = Env::home();
    check_4_settings(
        &env,
        &[
            ("cat ~/Documents/*.md", ["Ask", "Ask", "Allow", "Allow"]),
            ("ls ~/Documents/*.md", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat src/*.rs", ["Ask", "Ask", "Allow", "Allow"]),
            ("ls src/*.rs", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/*", ["Ask", "Ask", "Allow", "Allow"]),
            ("ls ~/*", ["Ask", "Ask", "Allow", "Allow"]),
            (
                "cat ~/.config/nvim/*.json",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/.config/app/settings.json",
                ["Allow", "Allow", "Allow", "Allow"],
            ),
            (
                "cat ~/Documents/[A-Z]*.md",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            // Ordinary globs inside ~/.config, ~/.local/share, ~/Library, ~, src must keep `dev`'s verdict.
            (
                "cat ~/.config/*/settings.json",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/.config/*/README.md",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/.local/share/*/notes.md",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            ("cat ~/Library/*/notes.md", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/*/*/notes.md", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat **/*.rs", ["Ask", "Ask", "Allow", "Allow"]),
            ("ls **/*.rs", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat src/**/mod.rs", ["Ask", "Ask", "Allow", "Allow"]),
            ("ls src/**/mod.rs", ["Ask", "Ask", "Allow", "Allow"]),
        ],
    );
}

#[test]
fn non_cd_fuzz_spellings_of_sensitive_paths_are_always_hard_denied() {
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
        fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len())]
        }
        fn chance(&mut self, percent: u64) -> bool {
            self.next() % 100 < percent
        }
    }

    const TARGETS: &[&[&str]] = &[
        &[".ssh", "id_rsa"],
        &[".ssh", "id_ed25519"],
        &[".npmrc"],
        &[".netrc"],
        &[".aws", "credentials"],
        &[".kube", "config"],
        &[".docker", "config.json"],
        &[".gnupg", "private-keys-v1.d", "x.key"],
        &[".config", "gh", "hosts.yml"],
        &[".config", "gcloud", "credentials.db"],
        &[".claude", ".credentials.json"],
        &[".codex", "auth.json"],
        &[".local", "share", "atrium", "atrium.db"],
        &["Library", "Keychains", "login.keychain-db"],
    ];

    fn spell(rng: &mut Rng, parts: &[&str]) -> String {
        let mut out = match rng.below(6) {
            0 => "~/".to_string(),
            1 => "$HOME/".to_string(),
            2 => "${HOME}/".to_string(),
            3 => "/h/u/".to_string(),
            4 => String::new(),
            _ => "./".to_string(),
        };
        if out.is_empty() && rng.chance(20) {
            out.push_str("./");
        }
        for (i, part) in parts.iter().enumerate() {
            let mut seg = (*part).to_string();
            if rng.chance(20) {
                seg = seg.to_uppercase();
            } else if rng.chance(15) {
                seg = seg
                    .chars()
                    .enumerate()
                    .map(|(k, c)| {
                        if k % 2 == 0 {
                            c.to_ascii_uppercase()
                        } else {
                            c
                        }
                    })
                    .collect();
            }
            if seg.len() > 2 && rng.chance(25) {
                let at = 1 + rng.below(seg.len() - 1);
                if seg.is_char_boundary(at) {
                    let (a, b) = seg.split_at(at);
                    seg = match rng.below(3) {
                        0 => format!("{a}\"\"{b}"),
                        1 => format!("{a}''{b}"),
                        _ => format!("{a}\\{b}"),
                    };
                }
            }
            if seg.len() > 3 && rng.chance(15) && !seg.contains(['\\', '\'', '"']) {
                let at = 1 + rng.below(seg.len() - 2);
                if seg.is_char_boundary(at) && seg.is_char_boundary(at + 1) {
                    let mut s = seg.clone();
                    s.replace_range(at..at + 1, ["?", "*"][rng.below(2)]);
                    seg = s;
                }
            }
            if rng.chance(15) {
                out.push_str(["x/../", "./", "proj/../"][rng.below(3)]);
            }
            out.push_str(&seg);
            if i + 1 < parts.len() {
                out.push_str(["/", "/", "//", "/./"][rng.below(4)]);
            }
        }
        out
    }

    let env = Env::home();
    let any = any_execute_rule();
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for i in 0..30_000 {
        let parts = *rng.pick(TARGETS);
        let path = spell(&mut rng, parts);
        let reader = *rng.pick(&[
            "cat", "head -n1", "tail", "wc -c", "stat", "file", "less", "xxd", "od", "strings",
        ]);
        let shape_pick = rng.below(10);
        let mut cd_glob = false;
        let core = match shape_pick {
            0 => format!("{reader} {path}"),
            1 => format!("{reader} -- {path}"),
            2 => format!("{reader} < {path}"),
            3 => format!("< {path} {reader}"),
            4 => format!("{reader} x {path} y"),
            5 => format!("echo $({reader} {path})"),
            6 => format!("echo `{reader} {path}`"),
            7 => format!("cat <({reader} {path})"),
            8 => {
                let (dir, file) = parts.split_at(parts.len() - 1);
                let d = spell(&mut rng, dir);
                let f = file[0];
                if dir.is_empty() {
                    format!("{reader} {path}")
                } else {
                    cd_glob = d.contains(['*', '?']);
                    format!("cd {d}; {reader} {f}")
                }
            }
            _ => format!("{reader} {path} > /dev/null"),
        };
        let noise = *rng.pick(&[
            "",
            "",
            "",
            "true; ",
            "ls && ",
            "echo x | ",
            "false || ",
            "true\n",
            "ls ;",
            "pwd; pwd; ",
            "{ ",
            "! ",
            "FOO=1 ",
            "env ",
            "command ",
            "time ",
            "nohup ",
            "timeout 5 ",
        ]);
        let line = format!("{noise}{core}{}", ["", " ", ";", "\n"][rng.below(4)]);
        let line = match rng.below(8) {
            0 if !line.contains('\'') => format!("sh -c '{line}'"),
            1 if !line.contains('"') && !line.contains(['$', '`', '\\']) => {
                format!("bash -c \"{line}\"")
            }
            2 => format!("echo $({line})"),
            _ => line,
        };
        // `cd <glob>; <reader> <file>` requires `words.rs:cd_effect` (outside M1-30 owned paths).
        if cd_glob {
            continue;
        }
        for (label, rules, preset) in [
            ("home", &[][..], Preset::Home),
            ("any", any.as_slice(), Preset::Home),
            ("full", &[][..], Preset::FullAccess),
        ] {
            let v = env.eval_check(&cmd_check(&line), rules, preset);
            assert!(
                matches!(v, PolicyVerdict::HardDeny { .. }),
                "iteration {i} [{label}]: {line:?} -> {}",
                classify(&v)
            );
        }
    }
}

#[test]
fn globstar_tree_leaf_rules_preserve_ordinary_globs_and_block_sensitive_trees() {
    let env = Env::home();
    // Ordinary `**` globs must not falsely match `**` as the leaf of a sensitive tree.
    check_4_settings(
        &env,
        &[
            ("cat ~/**/*.rs", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/**/notes.txt", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/.local/**/*.rs", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/Library/**/*.md", ["Ask", "Ask", "Allow", "Allow"]),
            ("cat ~/.config/**/*.rs", ["Ask", "Ask", "Allow", "Allow"]),
        ],
    );
    for path in ["~/**/*.rs", "~/**/*.md", "~/.azure/*.rs"] {
        let read_ordinary = paths_check(ToolKind::Read, "Read", &[path], false, None);
        assert!(
            matches!(
                env.eval_check(&read_ordinary, &[], Preset::Home),
                PolicyVerdict::Allow { .. }
            ),
            "Read {path} should Allow"
        );
    }

    // `**` ending at or inside a sensitive tree (or matching files inside one) stays blocked.
    run_table(
        &env,
        &[
            ("cat ~/**", Want::AnyHard),
            ("cat ~/.local/**", Want::Hard(AS)),
            ("cat ~/.local/share/**", Want::Hard(AS)),
            ("cat ~/.local/share/atr*/**", Want::Hard(AS)),
            ("cat ~/.local/**/atrium.db", Want::Hard(AS)),
            ("cat ~/**/atrium.db", Want::Hard(AS)),
            ("cat ~/.g*/**", Want::Hard(CR)),
            ("cat ~/.gnupg/**", Want::Hard(CR)),
            ("cat ~/**/.gnupg/secring.gpg", Want::Hard(CR)),
            ("cat ~/**/.g*/secring.gpg", Want::Hard(CR)),
            ("cat ~/**/.local/share/atr*/atrium.db", Want::Hard(AS)),
            ("cat ~/.s?h/**", Want::Hard(CR)),
            ("cat ~/**/.s?h/id_rsa", Want::Hard(CR)),
            ("cat ~/.config/g*chrome/**", Want::Hard(CR)),
        ],
    );
    for path in [
        "~/.z*/**",
        "~/**/.z*/*.session",
        "~/**/msal_token_cache.*",
        "~/.[a]zure/*",
        "~/.[a]zure/*.json",
    ] {
        let c = paths_check(ToolKind::Read, "Read", &[path], false, None);
        let v = env.eval_check(&c, &[], Preset::Home);
        assert!(
            matches!(v, PolicyVerdict::Ask),
            "Read {path}: expected Ask, got {}",
            classify(&v)
        );
    }
}

#[test]
fn k8_home_globstar_search_asks_only_when_suffix_can_match_ask_tree() {
    let env = Env::home();
    let any_search = vec![rule(
        RuleScope::Global,
        ToolKind::Search,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-search",
    )];
    let check_search = |paths: &[&str], rec: bool, glob: &str, want: [&str; 3]| {
        let c = paths_check(ToolKind::Search, "Search", paths, rec, Some(glob));
        let got: Vec<String> = [
            (&[][..], Preset::Home),
            (any_search.as_slice(), Preset::Home),
            (&[][..], Preset::FullAccess),
        ]
        .into_iter()
        .map(|(rules, preset)| classify(&env.eval_check(&c, rules, preset)))
        .collect();
        assert!(
            got.iter().map(String::as_str).eq(want.iter().copied()),
            "Search {paths:?} rec={rec} glob={glob:?}: wanted {want:?}, got {got:?}"
        );
    };

    // Home-wide `**` searches whose later components cannot match any ask-tree/ask-file: Allow.
    for glob in [
        "**/*.rs",
        "**/*.md",
        "**/*/*.rs",
        "*/*.rs",
        "src/**/*.rs",
        "**/src/*.rs",
        "**/*.{rs,md}",
        "**/m*.rs",
        ".azure/*.rs",
    ] {
        check_search(&["~"], false, glob, ["Allow", "Allow", "Allow"]);
    }
    check_search(&["~/.azure"], false, "*.rs", ["Allow", "Allow", "Allow"]);

    // Searches whose later components can match an ask-tree / ask-file / ask-prefix: Ask in Home.
    for glob in [
        "**/*.session",
        "**/*/*.session",
        "**/*.history",
        "**/token.json",
        "**/cache.json",
        "**/*.cache",
        // `.cargo/credentials.toml` is in ASK_FILES (CR-12), so `**/*.toml` must Ask.
        "**/*.toml",
        "**/*.{rs,toml}",
        "**/*.{rs,session}",
        "**/[0-9]*.session",
        "**/*token*",
        "**/msal_token_cache.db",
        "**/.zsh_sessions/*.txt",
        ".zsh_sessions/*.session",
        ".bash_sessions/*.history",
        ".aws/sso/cache/*.txt",
        ".z*/*.session",
        ".az*/msal_token_cache.db",
    ] {
        check_search(&["~"], false, glob, ["Ask", "Allow", "Allow"]);
    }
    // Catch-all globs or `**/*.json` reach hard-denied CRED_FILES (`.docker/config.json`, `.netrc`).
    for glob in ["**/*.json", "**/*", "**", "**/"] {
        check_search(
            &["~"],
            false,
            glob,
            [
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
            ],
        );
    }
    check_search(
        &["~/.zsh_sessions"],
        false,
        "*.rs",
        ["Ask", "Allow", "Allow"],
    );
    check_search(
        &["~/.azure"],
        false,
        "msal_token_cache.db",
        ["Ask", "Allow", "Allow"],
    );

    // Recursive searches rooted at `~` still ask in Home (engine root_ok rejects `~`).
    check_search(&["~"], true, "**/*.rs", ["Ask", "Allow", "Allow"]);
    check_4_settings(
        &env,
        &[
            ("rg --glob '**/*.rs' x ~", ["Ask", "Ask", "Allow", "Allow"]),
            (
                "rg --glob '**/*.rs' x ~/proj",
                ["Allow", "Allow", "Allow", "Allow"],
            ),
        ],
    );
}

#[test]
fn session_gate_blocks_wildcarded_credential_and_atrium_trees_when_absent_on_disk() {
    // Pins S5 through the real filesystem gate when target dirs/files do not exist on disk
    // (so `gate.rs:expand_glob` returns empty and `deny.rs:glob_hit` must block lexically).
    let t = Tmp::new();
    let g = t.gate(Preset::Home, vec![]);
    for line in [
        "cat ~/.g*/nonexistent",
        "cat ~/.gem*/oauth_creds.json",
        "cat ~/.con*/gcloud/credentials.db",
        "cat ~/.local/share/atr*/missing.db",
        "rm ~/.config/atr*/new.toml",
        "echo x > ~/.config/atr*/new.toml",
        "cat ~/.s?h/nonexistent",
    ] {
        let d = block_on(g.gate.check(&cmd_check(line)));
        assert!(
            matches!(d, GateDecision::Deny { .. }),
            "{line}: wanted Deny, got {d:?}"
        );
    }
}

#[test]
#[ignore = "escalated: requires changing Sensitive::search_glob_hit (owned by M1-31)"]
fn rework_round2_finding_1_non_escaping_search_globs_escalated() {
    let env = Env::home();
    let any_search = vec![rule(
        RuleScope::Global,
        ToolKind::Search,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any-search",
    )];
    for (glob, want_id) in [
        (".g*/private-keys-v1.d/k.key", CR),
        (".local/share/atr*/missing.db", AS),
    ] {
        let c = paths_check(ToolKind::Search, "Search", &["~"], false, Some(glob));
        for (label, rules, preset) in [
            ("home", &[][..], Preset::Home),
            ("any", any_search.as_slice(), Preset::Home),
            ("full", &[][..], Preset::FullAccess),
        ] {
            let v = env.eval_check(&c, rules, preset);
            assert!(
                matches!(v, PolicyVerdict::HardDeny { id, .. } if id == want_id),
                "Search [~] {glob:?} [{label}]: expected Hard({want_id}), got {}",
                classify(&v)
            );
        }
    }
}

#[test]
fn rework_round2_regressions() {
    let env = Env::home();
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

    // Finding 3: POSIX named character classes.
    run_table(
        &env,
        &[
            ("cat ~/.s[[:alpha:]]h/id_rsa", Want::Hard(CR)),
            ("cat ~/[[:punct:]]ssh/id_rsa", Want::Hard(CR)),
            ("cat ~/.g[[:alpha:]]upg/secring.gpg", Want::Hard(CR)),
            (
                "cat ~/.config/[[:alpha:]]cloud/credentials.db",
                Want::Hard(CR),
            ),
            (
                "cat ~/.local/share/[[:alpha:]]trium/atrium.db",
                Want::Hard(AS),
            ),
            ("cat ~/.s[![:digit:]]h/id_rsa", Want::Hard(CR)),
        ],
    );
    check_4_settings(
        &env,
        &[
            (
                "cat ~/.s[[:digit:]]h/id_rsa",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/.[[:punct:]]sh/id_rsa",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
        ],
    );

    // Finding 4: `**` leaf exception must cover Atrium data artifacts including WAL, SHM, logs, snapshots.
    run_table(
        &env,
        &[
            ("cat ~/.local/**/atrium.db-wal", Want::Hard(AS)),
            ("cat ~/.local/**/atrium.db-shm", Want::Hard(AS)),
            ("cat ~/.local/**/atrium.db-journal", Want::Hard(AS)),
            ("cat ~/**/atrium.db-wal", Want::Hard(AS)),
            ("cat ~/.local/**/snapshots/t1.json", Want::Hard(AS)),
            ("cat ~/.local/**/logs/daemon.log", Want::Hard(AS)),
            ("rm ~/.config/**/atrium.sock", Want::Hard(AS)),
            ("rm ~/.config/**/config.toml", Want::Hard(AS)),
        ],
    );
    check_4_settings(
        &env,
        &[
            (
                "cat ~/.config/**/config.toml",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            ("cat ~/.local/**/*.rs", ["Ask", "Ask", "Allow", "Allow"]),
        ],
    );

    // Finding 5 & 6: K8 filename families and Azure prefix matching across arbitrary extensions.
    let check_paths_3 =
        |kind: ToolKind, name: &str, paths: &[&str], glob: Option<&str>, want: [&str; 3]| {
            let c = paths_check(kind, name, paths, false, glob);
            let any_rules = match kind {
                ToolKind::Read => any_read.as_slice(),
                ToolKind::Search => any_search.as_slice(),
                _ => &[],
            };
            let got: Vec<String> = [
                (&[][..], Preset::Home),
                (any_rules, Preset::Home),
                (&[][..], Preset::FullAccess),
            ]
            .into_iter()
            .map(|(rules, preset)| classify(&env.eval_check(&c, rules, preset)))
            .collect();
            assert!(
                got.iter().map(String::as_str).eq(want.iter().copied()),
                "{name} {paths:?} glob={glob:?}: wanted {want:?}, got {got:?}"
            );
        };

    // Finding 5: valid session-file searches with arbitrary stems must Ask in Home.
    for glob in [
        "**/b.session",
        "**/b.history",
        "**/foo.historynew",
        "**/my.zsh",
        "**/abc.json",
    ] {
        check_paths_3(
            ToolKind::Search,
            "Search",
            &["~"],
            Some(glob),
            ["Ask", "Allow", "Allow"],
        );
    }
    for glob in ["**/b.rs", "**/a.session.bak", "**/.DS_Store"] {
        check_paths_3(
            ToolKind::Search,
            "Search",
            &["~"],
            Some(glob),
            ["Allow", "Allow", "Allow"],
        );
    }

    // Finding 6: Azure `msal_token_cache.*` with arbitrary extensions.
    for path in [
        "~/.a?ure/*token_cache.custom",
        "~/.a?ure/msal_token_cache.custom",
        "~/.a?ure/msal_*.custom",
        "~/.a?ure/*.custom",
        "~/.a?ure/[m]sal_token_cache.xyz",
        "~/.a?ure/*token_cache.rs",
    ] {
        check_paths_3(
            ToolKind::Read,
            "Read",
            &[path],
            None,
            ["Ask", "Allow", "Allow"],
        );
    }
    for glob in [".a?ure/*token_cache.custom", "**/*token_cache.custom"] {
        check_paths_3(
            ToolKind::Search,
            "Search",
            &["~"],
            Some(glob),
            ["Ask", "Allow", "Allow"],
        );
    }

    // Finding 7: bare `*` at tree leaf preserves ordinary globs while keeping `*` / `*/*` / `g*` blocked.
    run_table(
        &env,
        &[
            ("cat ~/.config/*", Want::Hard(CR)),
            ("cat ~/.config/*/*", Want::Hard(CR)),
            ("cat ~/.config/g*/settings.json", Want::Hard(CR)),
        ],
    );

    // Finding 8: singleton class spellings of exempt SSH files stay Allow/Ask; broader patterns HardDeny.
    for path in [
        "~/.s?h/confi[g]",
        "~/.s?h/[c]onfi[g]",
        "~/.s?h/{confi[g],[c]onfig}",
        "~/.s?h/[a]uthorized_keys",
        "~/.s?h/id_rsa.pu[b]",
        "~/.s?h/known_host[s]",
    ] {
        check_paths_3(
            ToolKind::Read,
            "Read",
            &[path],
            None,
            ["Allow", "Allow", "Allow"],
        );
    }
    check_4_settings(
        &env,
        &[("cat ~/.s?h/confi[g]", ["Ask", "Ask", "Allow", "Allow"])],
    );
    run_table(&env, &[("rm ~/.s?h/confi[g]", Want::Hard(CR))]);
    for path in [
        "~/.s?h/confi[gh]",
        "~/.s?h/confi?",
        "~/.s?h/confi*",
        "~/.s?h/{config,id_rsa}",
        "~/.s?h/id_rsa.pu[bc]",
    ] {
        check_paths_3(
            ToolKind::Read,
            "Read",
            &[path],
            None,
            [
                "Hard(credential-read)",
                "Hard(credential-read)",
                "Hard(credential-read)",
            ],
        );
    }
}

#[test]
fn posix_named_classes_match_real_character_sets() {
    let env = Env::home();
    // Each row: (class_name, component matching `.ssh`, component NOT matching `.ssh`).
    // `.ssh` consists of `.` (punct/graph/print), `s`, `s`, `h` (lower/alpha/alnum/word/graph/print).
    const CLASSES: &[(&str, &str, &str)] = &[
        ("alpha", ".s[[:alpha:]]h", "[[:alpha:]]ssh"),
        ("alnum", ".s[[:alnum:]]h", "[[:alnum:]]ssh"),
        ("lower", ".s[[:lower:]]h", "[[:lower:]]ssh"),
        ("upper", ".s[![:upper:]]h", ".s[[:upper:]]h"),
        ("punct", "[[:punct:]]ssh", ".[[:punct:]]sh"),
        ("print", ".s[[:print:]]h", ".s[![:print:]]h"),
        ("graph", ".s[[:graph:]]h", ".s[![:graph:]]h"),
        ("space", ".s[![:space:]]h", ".s[[:space:]]h"),
        ("digit", ".s[![:digit:]]h", ".s[[:digit:]]h"),
        ("xdigit", ".s[![:xdigit:]]h", ".s[[:xdigit:]]h"),
        ("word", ".s[[:word:]]h", "[[:word:]]ssh"),
        ("blank", ".s[![:blank:]]h", ".s[[:blank:]]h"),
        ("cntrl", ".s[![:cntrl:]]h", ".s[[:cntrl:]]h"),
    ];

    for (class_name, matching_comp, non_matching_comp) in CLASSES {
        let match_cmd = format!("cat ~/{matching_comp}/id_rsa");
        let v = env.cmd(&match_cmd);
        assert!(
            matches!(v, PolicyVerdict::HardDeny { id } if id == CR),
            "POSIX class {class_name}: expected {match_cmd:?} to HardDeny({CR}), got {}",
            classify(&v)
        );

        let non_match_cmd = format!("cat ~/{non_matching_comp}/id_rsa");
        check_4_settings(&env, &[(&non_match_cmd, ["Ask", "Ask", "Allow", "Allow"])]);
    }

    // Cross-checks verifying membership boundaries for `.` and `s` and `^` negation.
    run_table(
        &env,
        &[
            ("cat ~/[[:print:]]ssh/id_rsa", Want::Hard(CR)),
            ("cat ~/[[:graph:]]ssh/id_rsa", Want::Hard(CR)),
            ("cat ~/.s[^[:digit:]]h/id_rsa", Want::Hard(CR)),
            ("cat ~/.[[:xdigit:]]ws/credentials", Want::Hard(CR)),
        ],
    );
    check_4_settings(
        &env,
        &[
            (
                "cat ~/.s[![:alpha:]]h/id_rsa",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/.s[![:alnum:]]h/id_rsa",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/.s[![:lower:]]h/id_rsa",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/.s[![:word:]]h/id_rsa",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
            (
                "cat ~/[![:punct:]]ssh/id_rsa",
                ["Ask", "Ask", "Allow", "Allow"],
            ),
        ],
    );
}
