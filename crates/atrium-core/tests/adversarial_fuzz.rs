//! M1-13: fuzz-style tests of the policy. Random command lines from a hostile
//! alphabet (no panic, no hang, nothing sensitive reaches Allow), and random
//! spellings of sensitive paths inside random command shapes (always a hard
//! deny). Deterministic (fixed seeds; `ATRIUM_FUZZ_ITERS` raises the count),
//! bounded to well under a minute.
//!
//! No `proptest` dependency: the crate's `Cargo.toml` isn't this task's, so a
//! small xorshift generator does the same job.

mod common_adversarial;

use std::time::Instant;

use atrium_core::PolicyVerdict;
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{Preset, RuleDecision, RuleMatcher, RuleScope, ToolSubject};
use common_adversarial::{Env, classify, cmd_check, rule};

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

fn iters(default: usize) -> usize {
    std::env::var("ATRIUM_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn allow_everything() -> Vec<atrium_protocol::policy::Rule> {
    vec![
        rule(
            RuleScope::Global,
            ToolKind::Execute,
            RuleMatcher::Any,
            RuleDecision::Allow,
            "any",
        ),
        rule(
            RuleScope::Home,
            ToolKind::Execute,
            RuleMatcher::Any,
            RuleDecision::Allow,
            "any-home",
        ),
    ]
}

// ---------------------------------------------------------------------------
// 1. Unstructured: hostile alphabet
// ---------------------------------------------------------------------------

const ALPHABET: &[&str] = &[
    "rm",
    "-rf",
    "-r",
    "-f",
    " ",
    " ",
    " ",
    "~",
    "/",
    "$",
    "(",
    ")",
    "`",
    "'",
    "\"",
    "\\",
    "{",
    "}",
    ",",
    "*",
    "?",
    "[",
    "]",
    ";",
    "&",
    "&&",
    "||",
    "|",
    "<",
    ">",
    ">>",
    "\n",
    "=",
    ".",
    "..",
    "sh",
    "-c",
    "bash",
    "curl",
    "wget",
    "$(",
    "<(",
    ">(",
    "<<",
    "<<<",
    "EOF",
    "cd",
    "cd -",
    "pushd",
    "popd",
    "git",
    "push",
    "-f",
    "--force",
    "é",
    "日本",
    "\0",
    "!",
    "2>&1",
    ".ssh",
    "id_rsa",
    ".npmrc",
    ".aws",
    "credentials",
    ".gnupg",
    "atrium",
    "find",
    "-delete",
    "-exec",
    "+",
    "sudo",
    "env",
    "A=1",
    "-",
    "\r",
    "\u{1}",
    "\u{2028}",
    "-é",
    "~u",
    "~+",
    "$HOME",
    "${HOME}",
    "$PWD",
    "cat",
    "ls",
    "head",
    "echo",
    "printf",
    "xargs",
    "eval",
    "source",
    "nohup",
    "timeout 5",
    "caffeinate",
    "arch",
    "flock x",
    "chroot /",
    "at now",
    "crontab -",
    "make -f -",
    "$SHELL",
    "mksh",
    "yash",
    "dd",
    "of=/dev/disk0",
    "tee",
    "~/",
    "~/.",
    "./",
    "x",
    "proj",
    "a b",
    "*.rs",
    "{a,b}",
    "\\\n",
    "$'",
    "\\x2e",
    "\u{202e}",
    "\u{200b}",
    "\u{ff0e}",
    "\u{fe0f}",
];

#[test]
fn random_hostile_lines_never_panic_and_nothing_sensitive_is_allowed() {
    let env = Env::home();
    let any = allow_everything();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let started = Instant::now();
    let n = iters(40_000);
    let mut allowed = 0usize;
    for i in 0..n {
        let len = rng.below(16) + 1;
        let line: String = (0..len).map(|_| *rng.pick(ALPHABET)).collect();
        // Home, no rules
        let v = env.eval_check(&cmd_check(&line), &[], Preset::Home);
        if matches!(v, PolicyVerdict::Allow { .. }) {
            allowed += 1;
            assert!(
                benign_text(&line),
                "iteration {i}: {line:?} reached Allow in Home but touches something sensitive"
            );
        }
        // a different subject kind, and an allow-everything rule: no panic;
        // and a hard-deny-by-text line stays a hard deny
        let mut c = cmd_check(&line);
        c.subject = ToolSubject::Unknown;
        c.input = serde_json::json!({ "x": line });
        let u = env.eval_check(&c, &any, Preset::Home);
        assert!(
            !matches!(u, PolicyVerdict::Allow { .. }),
            "iteration {i}: an Unknown subject reached Allow for {line:?}"
        );
        let w = env.eval_check(&cmd_check(&line), &any, Preset::Home);
        if matches!(v, PolicyVerdict::HardDeny { .. }) {
            assert!(
                matches!(w, PolicyVerdict::HardDeny { .. }),
                "iteration {i}: {line:?}: hard deny without rules, {} under an allow rule",
                classify(&w)
            );
        }
        // parse_command is the rules' view: it must not panic either
        let _ = atrium_core::policy::parse_command(&line, std::path::Path::new("/h/u"));
    }
    assert!(
        started.elapsed().as_secs() < 40,
        "too slow: {:?}",
        started.elapsed()
    );
    // the alphabet reaches Allow now and then (plain `ls`, `cat x`): the
    // oracle above was exercised
    assert!(
        allowed > 0,
        "no line was allowed: the generator is too hostile"
    );
}

/// The Allow is acceptable only if the line has no word that could name a
/// sensitive path or run something destructive. A comment (`#`) hides text
/// from the shell, so lines with one are not judged.
fn benign_text(line: &str) -> bool {
    if line.contains('#') {
        return true;
    }
    let l = line.to_lowercase();
    // `id_rsa.pub` and `.ssh` folder listing are allowed by the contract
    let l = l.replace("id_rsa.pub", "");
    for bad in [
        ".npmrc",
        ".netrc",
        ".aws",
        "credentials",
        ".gnupg",
        "atrium",
        "authorized_keys",
        "sudo",
        "rm ",
        "/dev/disk",
        "keychain",
    ] {
        // as a word of its own or after a slash (`é.gnupgx` is another name)
        if l.starts_with(bad) || l.contains(&format!(" {bad}")) || l.contains(&format!("/{bad}")) {
            return false;
        }
    }
    // a hard-denied thing spelled with `$HOME` and friends is not judged by
    // text; only the explicit names above are
    true
}

// ---------------------------------------------------------------------------
// 2. Structured: spellings of a sensitive path in random shapes
// ---------------------------------------------------------------------------

/// (path as written, components the shell reads). Every spelling below names
/// the same file for a shell whose cwd is `/h/u`.
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
    // the base: how the home is written
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
        // case folding (APFS)
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
        // quote part of the segment (only the middle, so `~` stays unquoted)
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
        // a wildcard for one character (still matches only this name)
        if seg.len() > 3 && rng.chance(15) && !seg.contains(['\\', '\'', '"']) {
            let at = 1 + rng.below(seg.len() - 2);
            if seg.is_char_boundary(at) && seg.is_char_boundary(at + 1) {
                let mut s = seg.clone();
                s.replace_range(at..at + 1, ["?", "*"][rng.below(2)]);
                seg = s;
            }
        }
        // a detour: `x/..`, `./`, or a doubled slash
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

fn noise(rng: &mut Rng) -> &'static str {
    rng.pick(&[
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
    ])
}

fn shape(rng: &mut Rng, path: &str, parts: &[&str]) -> String {
    let reader = *rng.pick(&[
        "cat", "head -n1", "tail", "wc -c", "stat", "file", "less", "xxd", "od", "strings",
    ]);
    match rng.below(10) {
        0 => format!("{} {path}", reader),
        1 => format!("{reader} -- {path}"),
        2 => format!("{reader} < {path}"),
        3 => format!("< {path} {reader}"),
        4 => format!("{reader} x {path} y"),
        5 => format!("echo $({reader} {path})"),
        6 => format!("echo `{reader} {path}`"),
        7 => format!("cat <({reader} {path})"),
        8 => {
            // `cd` to the folder, then name the file bare
            let (dir, file) = parts.split_at(parts.len() - 1);
            let d = spell(rng, dir);
            let f = file[0];
            if dir.is_empty() {
                format!("{reader} {path}")
            } else {
                format!("cd {d}; {reader} {f}")
            }
        }
        _ => format!("{reader} {path} > /dev/null"),
    }
}

#[test]
#[ignore = "M1-13 finding 11"]
fn random_spellings_of_sensitive_paths_are_always_hard_denied() {
    let env = Env::home();
    let any = allow_everything();
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let started = Instant::now();
    let n = iters(30_000);
    for i in 0..n {
        let parts = *rng.pick(TARGETS);
        let path = spell(&mut rng, parts);
        let core = shape(&mut rng, &path, parts);
        let line = format!(
            "{}{}{}",
            noise(&mut rng),
            core,
            ["", " ", ";", "\n"][rng.below(4)]
        );
        // two extra shells around the whole thing, sometimes
        let line = match rng.below(8) {
            0 if !line.contains('\'') => format!("sh -c '{line}'"),
            1 if !line.contains('"') && !line.contains(['$', '`', '\\']) => {
                format!("bash -c \"{line}\"")
            }
            2 => format!("echo $({line})"),
            _ => line,
        };
        let h = env.eval_check(&cmd_check(&line), &[], Preset::Home);
        let r = env.eval_check(&cmd_check(&line), &any, Preset::Home);
        let f = env.eval_check(&cmd_check(&line), &[], Preset::FullAccess);
        for (label, v) in [("home", &h), ("any-rule", &r), ("full-access", &f)] {
            assert!(
                matches!(v, PolicyVerdict::HardDeny { .. }),
                "iteration {i} [{label}]: {line:?} -> {} (spelling of {parts:?})",
                classify(v)
            );
        }
    }
    assert!(
        started.elapsed().as_secs() < 40,
        "too slow: {:?}",
        started.elapsed()
    );
}

// ---------------------------------------------------------------------------
// 3. Structured: deletes outside the folder, random wrappers
// ---------------------------------------------------------------------------

#[test]
fn random_wrapped_recursive_deletes_of_home_are_hard_denied() {
    let env = Env::home();
    let any = allow_everything();
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let n = iters(10_000);
    let rm_flags = [
        "-rf",
        "-fr",
        "-Rf",
        "-r -f",
        "-f -r",
        "--recursive --force",
        "-rfv",
        "-r",
        "-R",
    ];
    let victims = [
        "~", "$HOME", "${HOME}", "/h/u", "~/", "/h/u/", "/", "/*", "~/*", "*", ".", "..", "~/.",
        "/h",
    ];
    let wrappers = [
        "",
        "env ",
        "command ",
        "nohup ",
        "nice -n 5 ",
        "timeout 5 ",
        "time ",
        "exec ",
        "stdbuf -o0 ",
        "caffeinate -i ",
        "arch -arm64 ",
        "flock /tmp/l ",
        "sandbox-exec -n x ",
        "env -i ",
        "env -S ",
        "xargs ",
        "setsid ",
        "busybox ",
        "ionice -c 3 ",
        "chroot / ",
    ];
    for i in 0..n {
        let rm = *rng.pick(&["rm", "/bin/rm", "\\rm", "'rm'", "r\\m", "r\"\"m"]);
        let core = format!(
            "{}{rm} {} {}",
            rng.pick(&wrappers),
            rng.pick(&rm_flags),
            rng.pick(&victims)
        );
        let core = if core.contains("env -S ") {
            // env -S takes one string
            let (a, b) = core.split_once("env -S ").unwrap();
            format!("{a}env -S '{b}'")
        } else {
            core
        };
        let line = match rng.below(9) {
            0 => format!("sh -c '{}'", core.replace('\'', "")),
            1 => format!("echo x; {core}"),
            2 => format!("true && {core}"),
            3 => format!("{{ {core}; }}"),
            4 => format!("({core})"),
            5 => format!("if true; then {core}; fi"),
            6 => format!("echo $({core})"),
            7 => format!("bash <<EOF\n{core}\nEOF"),
            _ => core,
        };
        // some of these (`env -S 'rm …'` wrapped in `sh -c '…'` loses quotes)
        // are not valid shell: judge only lines whose quotes balance
        if line.matches('\'').count() % 2 != 0 {
            continue;
        }
        for (label, v) in [
            ("home", env.eval_check(&cmd_check(&line), &[], Preset::Home)),
            (
                "any-rule",
                env.eval_check(&cmd_check(&line), &any, Preset::Home),
            ),
        ] {
            assert!(
                matches!(v, PolicyVerdict::HardDeny { .. }),
                "iteration {i} [{label}]: {line:?} -> {}",
                classify(&v)
            );
        }
    }
}
