//! The subjects `tool_check` builds for `Glob` and `Grep`, end to end through
//! the real policy (`atrium_core::policy` `SessionGate` + `DefaultPolicy`), in
//! a temp layout with a home of `<tmp>/home/ana` and a cwd of
//! `<tmp>/home/ana/work`.
//!
//! Fix round 2 (item 1) and the escalation (D-071, round-3 review): the
//! glob grammar, credential globs that stay inside their base, and an
//! exhaustive check over short patterns.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use atrium_agents::claude::tool_check;
use atrium_agents::{ApprovalTicket, BoxFuture, GateDecision, ToolGate};
use atrium_core::policy::{DefaultPolicy, RulesSource, SessionGate, SessionGateConfig};
use atrium_core::{ApprovalLedger, Binding, LedgerError, RequestKey};
use atrium_protocol::agent::{ApprovalDecision, Provider};
use atrium_protocol::policy::{Preset, RememberScope, Rule, RuleId, ToolSubject};
use atrium_protocol::thread::ThreadId;
use serde_json::{Value, json};

/// A ledger that only opens approvals (every `Ask` becomes ticket `ASK`).
struct AskLedger;

impl ApprovalLedger for AskLedger {
    fn open_approval(
        &self,
        _: &ThreadId,
        _: u32,
        _: Binding,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>> {
        Box::pin(async { Ok(ApprovalTicket::new("ASK".into(), false)) })
    }
    fn open_question(
        &self,
        _: &ThreadId,
        _: u32,
        _: Option<u32>,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>> {
        Box::pin(async { Err(LedgerError::Unknown) })
    }
    fn resolve(
        &self,
        _: &RequestKey,
        _: &ApprovalDecision,
        _: Option<RememberScope>,
    ) -> BoxFuture<'_, Result<Option<RuleId>, LedgerError>> {
        Box::pin(async { Err(LedgerError::Unknown) })
    }
    fn answer(&self, _: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Err(LedgerError::Unknown) })
    }
    fn redeem(
        &self,
        _: &RequestKey,
        _: &atrium_agents::ToolCheck,
    ) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Err(LedgerError::Unknown) })
    }
    fn withdraw(&self, _: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Ok(()) })
    }
    fn expire_turn(&self, _: &ThreadId, _: u32, _: u32) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }
    fn expire_incarnation(&self, _: &ThreadId, _: u32) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }
    fn expire_all(&self) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }
}

struct NoRules;

impl RulesSource for NoRules {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>> {
        Box::pin(async { Some((0, vec![])) })
    }
}

/// The credential files of the test layout, relative to the home.
const CREDENTIALS: &[&str] = &[
    ".ssh/id_ed25519",
    ".ssh/id_rsa",
    ".aws/credentials",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".claude.json",
    ".kube/config",
    ".docker/config.json",
    ".config/gh/hosts.yml",
];

struct Layout {
    root: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
}

impl Layout {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("atrium-e2e-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let home = root.join("home/ana");
        let cwd = home.join("work");
        let files = CREDENTIALS.iter().map(|f| (*f, "secret")).chain([
            ("work/src/mod.rs", "fn main() {}"),
            ("work/src/a/mod.rs", "fn main() {}"),
            ("work/a.ts", "x"),
            ("work/b.tsx", "x"),
            ("work/x.c", "x"),
            ("work/file-1.rs", "x"),
        ]);
        for (file, text) in files {
            let p = home.join(file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        Self { root, home, cwd }
    }

    fn gate(&self, preset: Preset) -> SessionGate {
        self.gate_at(preset, &self.cwd)
    }

    fn gate_at(&self, preset: Preset, cwd: &Path) -> SessionGate {
        SessionGate::new(
            SessionGateConfig {
                thread: ThreadId("t1".into()),
                incarnation: 1,
                cwd: cwd.to_path_buf(),
                home: self.home.clone(),
                atrium_data_paths: vec![self.home.join(".local/share/atrium")],
                atrium_protected_paths: vec![self.home.join(".config/atrium")],
                preset,
                provider: Provider::Claude,
            },
            Arc::new(DefaultPolicy::new()),
            Arc::new(AskLedger),
            Arc::new(NoRules),
        )
    }

    /// The input, with `{home}` and `{cwd}` filled in.
    fn input(&self, raw: &str) -> Value {
        let text = raw
            .replace("{home}", &self.home.display().to_string())
            .replace("{cwd}", &self.cwd.display().to_string());
        serde_json::from_str(&text).unwrap()
    }
}

impl Drop for Layout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn verdict(
    l: &Layout,
    preset: Preset,
    tool: &str,
    input: Value,
) -> (GateDecision, ToolSubject) {
    let check = tool_check(Some("t1"), tool, &input, 1);
    let subject = check.subject.clone();
    (l.gate(preset).check(&check).await, subject)
}

fn hard_denied(d: &GateDecision) -> bool {
    matches!(d, GateDecision::Deny { reason } if reason.contains("Always blocked"))
}

/// The same input as a `Glob` and as a `Grep` whose `glob` filter is the
/// `Glob`'s pattern.
fn as_glob_and_grep(glob_input: &Value) -> [(&'static str, Value); 2] {
    let mut grep = glob_input.clone();
    let pattern = grep["pattern"].take();
    grep["glob"] = pattern;
    grep["pattern"] = json!("secret");
    [("Glob", glob_input.clone()), ("Grep", grep)]
}

/// Patterns that must never be allowed, in Home; the credential ones must be
/// hard-denied in Home and in FullAccess.
const ESCAPES: &[(&str, bool)] = &[
    (r#"{"path":"{cwd}","pattern":"../.aws/cred*"}"#, true),
    (r#"{"path":"{cwd}","pattern":"../.ss*/id_ed25519"}"#, true),
    (r#"{"path":"{cwd}","pattern":"~/.ss*/id_ed25519"}"#, true),
    (r#"{"path":"{cwd}","pattern":"~/.aws/*"}"#, true),
    (
        r#"{"path":"{cwd}","pattern":"{home}/.ssh/id_ed25519"}"#,
        true,
    ),
    (r#"{"pattern":"{home}/.ssh/id_ed25519"}"#, true),
    (r#"{"path":"{cwd}","pattern":"../../ana/.ssh/**"}"#, true),
    (r#"{"path":"{cwd}","pattern":"~/.ssh/*"}"#, true),
    (r#"{"path":"{cwd}","pattern":"/etc/pass*"}"#, false),
];

/// Spelled so that we can't tell where they point: `Unknown`, which the
/// policy asks about and never allows. Since the escalation (D-071) this
/// holds every brace group outside the grammar (an empty alternative, a `.`
/// or `..` alternative, a nested group, anything but letters, digits, `_`,
/// `-`, `.` and `,`), so round 2's brace escapes are here now, not in
/// [`ESCAPES`].
const NOT_UNDERSTOOD: &[&str] = &[
    r#"{"path":"{cwd}","pattern":"[.][.]/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"[!a][!a]/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":".[.]/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"[/]etc/passwd"}"#,
    r#"{"path":"{cwd}","pattern":"[+-0]etc/passwd"}"#,
    r#"{"path":"{cwd}","pattern":"a\\b"}"#,
    r#"{"path":"{cwd}","pattern":"{a,{b,c}"}"#,
    r#"{"path":"{cwd}","pattern":"**/../../etc/passwd"}"#,
    r#"{"path":"{cwd}","pattern":"[a"}"#,
    // Round 2's brace escapes.
    r#"{"path":"{cwd}","pattern":"{a,}{home}/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":".{.,.}/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"{a,}/etc/passwd"}"#,
    r#"{"path":"{cwd}","pattern":"{..,x}/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"{x,~}/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":".{,}./.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"src/{a,{..,b}}/../.ssh/*"}"#,
    // Round 3, blocker 1: classes and braces interleaved.
    r#"{"path":"{cwd}","pattern":"[{a],.}][{a],.}]/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"[{a],/}]/etc/passwd"}"#,
    r#"{"path":"{cwd}","pattern":"[{a],~}]/.ssh/id_ed25519"}"#,
    // Round 3, blocker 2: a tilde that is not the user's home.
    r#"{"path":"{cwd}","pattern":"~bob/*"}"#,
    r#"{"path":"~bob","pattern":"*.rs"}"#,
    r#"{"path":"{cwd}","pattern":"~a*/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"~?na/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"~ana/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"~*"}"#,
    r#"{"path":"~*","pattern":"*"}"#,
    // Round 3, minor 4: a range that spans `~`.
    "{\"path\":\"{cwd}\",\"pattern\":\"[}-\u{7f}]/.ssh/id_ed25519\"}",
];

#[tokio::test]
async fn patterns_we_cannot_bound_are_unknown_and_never_allowed() {
    let l = Layout::new("unknown");
    for raw in NOT_UNDERSTOOD {
        for (tool, input) in as_glob_and_grep(&l.input(raw)) {
            for preset in [Preset::Home, Preset::FullAccess] {
                let (d, subject) = verdict(&l, preset, tool, input.clone()).await;
                assert_eq!(subject, ToolSubject::Unknown, "{tool} {raw}");
                assert!(
                    !matches!(d, GateDecision::Allow),
                    "{preset:?} allowed {tool} {raw}"
                );
            }
        }
    }
}

#[tokio::test]
async fn glob_escapes_are_never_allowed_and_credentials_are_hard_denied() {
    let l = Layout::new("glob");
    for (raw, credential) in ESCAPES {
        for (tool, input) in as_glob_and_grep(&l.input(raw)) {
            let (home, subject) = verdict(&l, Preset::Home, tool, input.clone()).await;
            assert!(
                !matches!(home, GateDecision::Allow),
                "Home allowed {tool} {raw}: {subject:?}"
            );
            if *credential {
                assert!(
                    hard_denied(&home),
                    "Home did not hard-deny {tool} {raw}: {home:?} {subject:?}"
                );
                let (full, subject) = verdict(&l, Preset::FullAccess, tool, input).await;
                assert!(
                    hard_denied(&full),
                    "FullAccess did not hard-deny {tool} {raw}: {full:?} {subject:?}"
                );
            }
        }
    }
}

/// Round 3, major 3: a credential glob of more than one component that stays
/// inside its base (the base is a parent of the credential) is hard-denied.
#[tokio::test]
async fn r3_credential_globs_that_stay_inside_their_base_are_hard_denied() {
    let l = Layout::new("inside");
    let cases = [
        r#"{"path":"..","pattern":".ssh/id_ed25519"}"#,
        r#"{"path":"..","pattern":".aws/cred*"}"#,
        r#"{"path":"{home}","pattern":".aws/cred*"}"#,
        r#"{"path":"{home}","pattern":".ssh/id_*"}"#,
        r#"{"path":"{home}/.config","pattern":"gh/hosts.yml"}"#,
        r#"{"path":"{home}/.config","pattern":"gh/*.yml"}"#,
        r#"{"path":"{home}","pattern":".{ssh,x}/id_ed25519"}"#,
        r#"{"path":"{home}","pattern":".[a-z]sh/id_ed25519"}"#,
    ];
    for raw in cases {
        for (tool, input) in as_glob_and_grep(&l.input(raw)) {
            for preset in [Preset::Home, Preset::FullAccess] {
                let (d, subject) = verdict(&l, preset, tool, input.clone()).await;
                assert!(
                    hard_denied(&d),
                    "{preset:?} did not hard-deny {tool} {raw}: {d:?} {subject:?}"
                );
            }
        }
    }
    // With the home as the cwd and no `path`.
    for raw in [
        r#"{"pattern":".ssh/id_ed25519"}"#,
        r#"{"pattern":".aws/cred*"}"#,
    ] {
        for (tool, input) in as_glob_and_grep(&l.input(raw)) {
            for preset in [Preset::Home, Preset::FullAccess] {
                let check = tool_check(Some("t1"), tool, &input, 1);
                let d = l.gate_at(preset, &l.home).check(&check).await;
                assert!(
                    hard_denied(&d),
                    "{preset:?} (cwd = home) did not hard-deny {tool} {raw}: {d:?} {:?}",
                    check.subject
                );
            }
        }
    }
}

/// Item 2 of the escalation (round 3, minor 4): ordinary patterns are still
/// understood and allowed at home.
#[tokio::test]
async fn ordinary_patterns_keep_their_plain_subject_and_are_allowed_at_home() {
    let l = Layout::new("plain");
    let base = l.cwd.display().to_string();
    for pattern in [
        "**/*.rs",
        "src/**/mod.rs",
        "*.{ts,tsx}",
        "src/{a,b}/mod.rs",
        "*.[ch]",
        "*.[a-z]",
        "file-[0-9].rs",
        "*.{d.ts,ts}",
        "[A-Z_]*.md",
    ] {
        for (tool, input) in as_glob_and_grep(&json!({"path": base, "pattern": pattern})) {
            let (d, subject) = verdict(&l, Preset::Home, tool, input).await;
            match &subject {
                ToolSubject::Paths {
                    paths,
                    glob,
                    recursive: true,
                } => {
                    assert_eq!(paths[0], base, "{pattern}");
                    assert_eq!(glob.as_deref(), Some(pattern));
                }
                other => panic!("{tool} {pattern}: {other:?}"),
            }
            assert!(matches!(d, GateDecision::Allow), "{tool} {pattern}: {d:?}");
        }
    }
    // The same without a `path` (the base is the cwd).
    let (d, subject) = verdict(&l, Preset::Home, "Glob", json!({"pattern": "**/*.rs"})).await;
    assert!(matches!(subject, ToolSubject::Paths { .. }));
    assert!(matches!(d, GateDecision::Allow), "{d:?}");
}

// ---- item 3 of the escalation: every short pattern ----

/// What a glob's braces stand for, with no limit and no grammar: the oracle's
/// own expansion (an unbalanced brace is left as it is).
fn oracle_expand(glob: &str) -> Vec<String> {
    let Some(open) = glob.find('{') else {
        return vec![glob.to_owned()];
    };
    let mut depth = 0usize;
    let mut close = None;
    for (i, c) in glob[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(close) = close else {
        return vec![glob.to_owned()];
    };
    let inner = &glob[open + 1..close];
    let mut alts = vec![];
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                alts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    alts.push(&inner[start..]);
    let (head, tail) = (&glob[..open], &glob[close + 1..]);
    alts.iter()
        .flat_map(|a| oracle_expand(&format!("{head}{a}{tail}")))
        .collect()
}

/// `.` and empty segments dropped, `..` climbs.
fn normalize(path: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    format!("/{}", out.join("/"))
}

/// The probes `glob`, searched from `base` (absolute), reaches. Matched as
/// the policy does, with `globset` (where a `*` matches a leading dot): the
/// pattern joined to the base against the whole path, and a pattern without
/// `/` against the name of any file under the base. A trailing `/` is
/// dropped, which can only make the match wider.
fn reached<'p>(glob: &str, base: &str, home: &str, probes: &'p [String]) -> Vec<&'p str> {
    let mut hits = vec![];
    let compile = |p: &str| {
        globset::GlobBuilder::new(p)
            .literal_separator(true)
            .build()
            .ok()
            .map(|g| g.compile_matcher())
    };
    for e in oracle_expand(glob) {
        let joined = if e == "~" {
            home.to_owned()
        } else if let Some(rest) = e.strip_prefix("~/") {
            format!("{home}/{rest}")
        } else if e.starts_with('/') {
            e.clone()
        } else {
            format!("{base}/{e}")
        };
        let joined = normalize(&joined);
        // Only what starts with the pattern's literal prefix can match it.
        let literal = &joined[..joined.find(['*', '?', '[', '{']).unwrap_or(joined.len())];
        let candidates: Vec<&String> = probes.iter().filter(|p| p.starts_with(literal)).collect();
        if !candidates.is_empty()
            && let Some(m) = compile(&joined)
        {
            hits.extend(
                candidates
                    .iter()
                    .filter(|p| m.is_match(p.as_str()))
                    .map(|p| p.as_str()),
            );
        }
        let under: Vec<&String> = probes
            .iter()
            .filter(|p| p.starts_with(&format!("{base}/")))
            .collect();
        if !e.contains('/')
            && !under.is_empty()
            && let Some(m) = compile(&e)
        {
            hits.extend(
                under
                    .iter()
                    .filter(|p| m.is_match(Path::new(p.as_str()).file_name().unwrap()))
                    .map(|p| p.as_str()),
            );
        }
    }
    hits.sort_unstable();
    hits.dedup();
    hits
}

/// Every pattern of 1 to `len` characters over `alphabet`.
fn all_patterns(alphabet: &[char], len: usize) -> Vec<String> {
    let mut all = vec![];
    let mut level = vec![String::new()];
    for _ in 0..len {
        let mut next = Vec::with_capacity(level.len() * alphabet.len());
        for p in &level {
            for c in alphabet {
                let mut q = p.clone();
                q.push(*c);
                next.push(q);
            }
        }
        all.extend(next.iter().cloned());
        level = next;
    }
    all
}

/// Every pattern up to 5 characters over `{ } [ ] , . / ~ * a -`, alone and
/// followed by `/.ssh/id_ed25519`, searched from the cwd and from `..`: no
/// pattern that reaches a credential file of the layout is allowed, in Home
/// or in FullAccess.
///
/// That is "every allowed pattern reaches no credential", turned around so
/// the policy is asked only where it matters: the oracle ([`reached`]) is
/// cheap and runs on every understood pattern; each one that reaches a
/// credential goes through the real `SessionGate` in both presets. (Asking
/// the policy about all ~71,000 understood patterns took 39 to 49 s in a
/// debug build, nearly all in the policy's own glob compiling; review of
/// the escalation.)
///
/// A subject the adapter can't bound is `Unknown`, which the policy never
/// allows (`patterns_we_cannot_bound_are_unknown_and_never_allowed`, and
/// checked here on a sample).
#[test]
fn every_short_pattern_the_policy_allows_reaches_no_credential() {
    let started = Instant::now();
    let l = Layout::new("exhaustive");
    let home = l.home.display().to_string();
    let cwd = l.cwd.display().to_string();
    let probes: Vec<String> = CREDENTIALS.iter().map(|c| format!("{home}/{c}")).collect();
    let alphabet = ['{', '}', '[', ']', ',', '.', '/', '~', '*', 'a', '-'];
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let gates = Arc::new([l.gate(Preset::Home), l.gate(Preset::FullAccess)]);

    let mut inputs = vec![];
    for p in all_patterns(&alphabet, 5) {
        for suffix in ["", "/.ssh/id_ed25519"] {
            let pattern = format!("{p}{suffix}");
            for (path, base) in [(cwd.as_str(), cwd.as_str()), ("..", home.as_str())] {
                inputs.push((pattern.clone(), path.to_owned(), base.to_owned()));
            }
        }
    }
    let total = inputs.len();

    let mut known = vec![];
    let mut unknown_sample = vec![];
    for (i, (pattern, path, base)) in inputs.into_iter().enumerate() {
        let input = json!({"path": path, "pattern": pattern});
        let check = tool_check(Some("t1"), "Glob", &input, 1);
        if check.subject == ToolSubject::Unknown {
            if i % 997 == 0 {
                unknown_sample.push(check);
            }
        } else {
            known.push((pattern, base, check));
        }
    }
    let unknown = total - known.len();
    let subjects = started.elapsed();

    // The oracle on every understood pattern, one thread per core.
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = known.len().div_ceil(workers).max(1);
    let reaching: Vec<(atrium_agents::ToolCheck, Vec<String>)> = std::thread::scope(|s| {
        let handles: Vec<_> = known
            .chunks(chunk)
            .map(|part| {
                let (probes, home) = (&probes, &home);
                s.spawn(move || {
                    part.iter()
                        .filter_map(|(pattern, base, check)| {
                            let hits = reached(pattern, base, home, probes);
                            (!hits.is_empty()).then(|| {
                                (check.clone(), hits.into_iter().map(str::to_owned).collect())
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect()
    });
    let oracle = started.elapsed() - subjects;

    // Through the gate: every reaching pattern, and the Unknown sample.
    let reaching_count = reaching.len();
    let (sampled, mut leaks) = rt.block_on(async {
        let mut set = tokio::task::JoinSet::new();
        for check in unknown_sample {
            let gates = gates.clone();
            set.spawn(async move {
                for g in gates.iter() {
                    let d = g.check(&check).await;
                    assert!(
                        !matches!(d, GateDecision::Allow),
                        "an Unknown subject was allowed: {:?}",
                        check.input
                    );
                }
                vec![]
            });
        }
        let sampled = set.len();
        for (check, hits) in reaching {
            let gates = gates.clone();
            set.spawn(async move {
                let mut leaks = vec![];
                for (g, preset) in gates.iter().zip(["Home", "FullAccess"]) {
                    if matches!(g.check(&check).await, GateDecision::Allow) {
                        for probe in &hits {
                            leaks.push(format!("{preset} allows {} reaching {probe}", check.input));
                        }
                    }
                }
                leaks
            });
        }
        let mut leaks = vec![];
        while let Some(r) = set.join_next().await {
            leaks.extend(r.unwrap());
        }
        (sampled, leaks)
    });
    leaks.sort();

    eprintln!(
        "exhaustive: {total} patterns ({unknown} Unknown, {sampled} of them through the gate; \
         {} understood, {reaching_count} of them reach a credential and went through the gate); \
         subjects in {subjects:?}, oracle in {oracle:?}, all in {:?}",
        total - unknown,
        started.elapsed()
    );
    assert!(
        leaks.is_empty(),
        "{} leaks:\n{}",
        leaks.len(),
        leaks.join("\n")
    );
}
