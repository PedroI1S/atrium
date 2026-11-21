//! The hard-deny list (`HARD_DENY_DEFAULTS`, D-050): pure matching over what
//! a call names. Applies to every tool kind, on every command segment
//! (through assignments and wrappers), inside substitutions and payloads, and
//! on the raw text.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use atrium_agents::ToolCheck;
use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{HARD_DENY_DEFAULTS, ToolSubject};
use globset::GlobBuilder;
use serde_json::Value;

use super::shell::{self, Unit, base, is_shell};
use super::words::{self, CwdState, Role, Use, Walk, lexical_resolve};
use crate::PolicyInput;

const MAX_BYTES: usize = 128 * 1024;
use super::shell::MAX_UNITS;

/// What the engine knows about a call, computed once.
pub struct Analysis {
    pub units: Vec<Unit>,
    /// Why rules and Home's read-only list must not allow a command line.
    pub reasons: Vec<String>,
    pub uses: Vec<Use>,
    /// Per `uses` entry: the gate's resolution, when its list lines up with
    /// ours.
    pub aligned: Option<Vec<Vec<PathBuf>>>,
    /// `(unit, word)` → indexes into `uses`.
    pub use_index: HashMap<(usize, String), Vec<usize>>,
    pub unit_cwd: Vec<CwdState>,
    pub cwd_unknown: bool,
    /// Some command may run in more than one directory, or in one the line
    /// doesn't name: no rule but `Any` covers the line, and Home asks.
    pub cwd_uncertain: bool,
    /// Bigger than the checker takes on (line bytes, commands, words, paths).
    pub overflow: bool,
    /// An unknown `env`/`command` invocation whose readings could not be
    /// bounded and whose words may be dangerous (M1-18, K1 fail-closed).
    pub unbounded_unknown_dangerous: bool,
    /// Text the raw-string checks run on.
    pub raw: Vec<String>,
    /// The starting directory of a command.
    pub start: PathBuf,
    /// Cached `glob_escape` result for `ToolSubject::Paths`.
    pub glob_escape: Option<GlobEscape>,
    /// Cached `(variants, unbounded)` for `search_glob_hit` and `ask_search_glob_hit`.
    pub glob_search: Option<(Vec<String>, bool)>,
}

fn unknown_tok_dangerous(tok: &str, cwd: &Path, home: &Path, s: &Sensitive, piped: bool) -> bool {
    if tok.is_empty() {
        return false;
    }
    if tok.contains(['$', '`', '*', '?', '[', '{', ';', '|', '&', '\n'])
        || tok.starts_with(['/', '~'])
        || tok.contains("..")
        || tok.contains("<(")
        || tok.contains(">(")
    {
        return true;
    }
    if !tok.starts_with('-') {
        let p = lexical_resolve(tok, cwd, home);
        if !p.starts_with(cwd)
            || s.hit(&p, Role::Write).is_some()
            || s.ask_hit(&p)
            || s.is_or_contains(&p)
        {
            return true;
        }
    }
    if shell::maybe_shell(tok) || (piped && is_interpreter(tok)) {
        return true;
    }
    let b = base(tok);
    matches!(
        b,
        "eval"
            | "source"
            | "."
            | "exec"
            | "command"
            | "env"
            | "xargs"
            | "find"
            | "osascript"
            | "crontab"
            | "security"
            | "secret-tool"
            | "rm"
            | "dd"
            | "shred"
            | "wipefs"
            | "blkdiscard"
            | "mke2fs"
            | "diskutil"
            | "curl"
            | "wget"
            | "fetch"
            | "git"
    ) || PRIV.contains(&b)
        || b.starts_with("mkfs")
        || b.starts_with("newfs")
        || words::is_known_writer(&[tok.to_string()])
}

fn unknown_words_could_be_dangerous(
    invocations: &[shell::UnboundedUnknown],
    cwd: &Path,
    home: &Path,
    s: &Sensitive,
) -> bool {
    invocations.iter().any(|inv| {
        let words = &inv.words;
        let is_script_or_flock = matches!(words.first().map(|w| base(w)), Some("script" | "flock"));
        (1..words.len()).any(|i| {
            let w = &words[i];
            if unknown_tok_dangerous(w, cwd, home, s, inv.piped) {
                return true;
            }
            if let Some((_, val)) = w.split_once('=')
                && val
                    .split_whitespace()
                    .any(|t| unknown_tok_dangerous(t, cwd, home, s, inv.piped))
            {
                return true;
            }
            if w.starts_with('-') && !w.starts_with("--") {
                for flag in ['C', 'S'] {
                    if let Some(pos) = w[1..].find(flag) {
                        let after = &w[1 + pos + 1..];
                        if after
                            .split_whitespace()
                            .any(|t| unknown_tok_dangerous(t, cwd, home, s, inv.piped))
                        {
                            return true;
                        }
                    }
                }
            }
            // Separate argument word after -S/--split-string, -C/--chdir, or script/flock -c/--command.
            let takes_split_arg = (w.starts_with("--")
                && !w.contains('=')
                && w.len() >= 3
                && ("--split-string".starts_with(w)
                    || "--chdir".starts_with(w)
                    || (is_script_or_flock && "--command".starts_with(w))))
                || (w.starts_with('-')
                    && !w.starts_with("--")
                    && (w.ends_with(['S', 'C']) || (is_script_or_flock && w.ends_with('c'))));
            if takes_split_arg
                && let Some(next) = words.get(i + 1)
                && next
                    .split_whitespace()
                    .any(|t| unknown_tok_dangerous(t, cwd, home, s, inv.piped))
            {
                return true;
            }
            false
        })
    })
}

fn strings_of(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|x| strings_of(x, out)),
        Value::Object(m) => {
            for (k, x) in m {
                out.push(k.clone());
                strings_of(x, out);
            }
        }
        _ => {}
    }
}

/// A string that looks like a single path (no whitespace, bounded).
fn path_like(s: &str) -> bool {
    !s.is_empty()
        && s.len() < 4096
        && !s.contains(char::is_whitespace)
        && !s.contains('\0')
        && (s.starts_with('/')
            || s.starts_with('~')
            || s.starts_with('$')
            || s.contains('/')
            || s.starts_with('.'))
}

pub fn analyze(input: &PolicyInput<'_>) -> Analysis {
    analyze_with(input, &lexical_resolve)
}

/// `analyze` with the path resolver chosen by the caller: the engine resolves
/// lexically; the gate resolves against the filesystem.
pub fn analyze_with(input: &PolicyInput<'_>, resolve: words::Resolver<'_>) -> Analysis {
    let check: &ToolCheck = input.check;
    let home = input.home;
    let mut a = Analysis {
        units: vec![],
        reasons: vec![],
        uses: vec![],
        aligned: None,
        use_index: HashMap::new(),
        unit_cwd: vec![],
        cwd_unknown: false,
        cwd_uncertain: false,
        overflow: false,
        unbounded_unknown_dangerous: false,
        raw: vec![],
        start: input.cwd.to_path_buf(),
        glob_escape: None,
        glob_search: None,
    };
    let s_for_unknown = Sensitive::new(input);
    let lex_cmd = |a: &mut Analysis, line: &str, start: &Path| {
        if line.len() > MAX_BYTES {
            a.overflow = true;
            return;
        }
        let l = shell::lex(line);
        a.units = l.units;
        a.reasons = l.reasons;
        if !l.unbounded_unknown_words.is_empty() {
            a.unbounded_unknown_dangerous = unknown_words_could_be_dangerous(
                &l.unbounded_unknown_words,
                start,
                home,
                &s_for_unknown,
            );
        }
        if l.overflow || a.units.len() > MAX_UNITS {
            a.overflow = true;
            return;
        }
        let walk: Walk = words::walk(&a.units, start, home, resolve);
        a.overflow |= walk.overflow;
        a.unit_cwd = walk.unit_cwd.clone();
        a.cwd_unknown = walk.unknown;
        a.uses = walk.uses;
    };
    let pseudo = |a: &mut Analysis, word: &str, role: Role| {
        a.uses.push(Use {
            word: word.to_string(),
            role,
            unit: usize::MAX,
            paths: vec![resolve(word, input.cwd, home)],
        });
    };
    match &check.subject {
        ToolSubject::Command { line, cwd } => {
            let start = match cwd {
                Some(c) => resolve(c, input.cwd, home),
                None => input.cwd.to_path_buf(),
            };
            a.start = start.clone();
            a.raw.push(line.clone());
            lex_cmd(&mut a, line, &start);
        }
        ToolSubject::Paths { paths, glob, .. } => {
            let role = match check.kind {
                ToolKind::Read | ToolKind::Search => Role::Read,
                _ => Role::Write,
            };
            if paths.len() > words::MAX_USES {
                a.overflow = true;
            }
            for p in paths.iter().take(words::MAX_USES) {
                pseudo(&mut a, p, role);
            }
            if let Some(g) = glob {
                let roots: Vec<PathBuf> = if paths.is_empty() {
                    vec![input.cwd.to_path_buf()]
                } else {
                    paths
                        .iter()
                        .take(words::MAX_USES)
                        .map(|p| lexical_resolve(p, input.cwd, home))
                        .collect()
                };
                let cands = expand_glob_candidates(g, input.cwd, home);
                a.glob_search = Some(search_variants_from_cands(&roots, input.cwd, home, &cands));
                a.glob_escape = Some(escape_from_cands(&roots, input.cwd, home, &cands));
            }
        }
        ToolSubject::Fetch { url } => {
            if let Some(p) = url.strip_prefix("file://") {
                let p = p.strip_prefix("localhost").unwrap_or(p);
                pseudo(&mut a, p, Role::Read);
            }
        }
        ToolSubject::Mcp { .. } => {
            let mut s = vec![];
            strings_of(&check.input, &mut s);
            if s.len() > words::MAX_USES {
                a.overflow = true;
            }
            for x in s.iter().filter(|x| path_like(x)).take(words::MAX_USES) {
                pseudo(&mut a, x, Role::Read);
            }
        }
        ToolSubject::None => {}
        ToolSubject::Unknown => {
            let mut s = vec![];
            strings_of(&check.input, &mut s);
            if s.iter().map(String::len).sum::<usize>() > MAX_BYTES || s.len() > words::MAX_USES {
                a.overflow = true;
                return a;
            }
            let mut units = vec![];
            for x in &s {
                let l = shell::lex(x);
                a.overflow |= l.overflow;
                units.extend(l.units);
                a.raw.push(x.clone());
                if a.overflow || units.len() > MAX_UNITS {
                    a.overflow = true;
                    return a;
                }
            }
            a.units = units;
            if a.units.len() > MAX_UNITS {
                a.overflow = true;
                return a;
            }
            let walk = words::walk(&a.units, input.cwd, home, resolve);
            a.overflow |= walk.overflow;
            a.unit_cwd = walk.unit_cwd.clone();
            a.cwd_unknown = walk.unknown;
            a.uses = walk.uses;
            a.reasons.push("unknown tool input".into());
        }
    }
    a.cwd_uncertain = a.cwd_unknown
        || a.unit_cwd.iter().any(|s| {
            let c = s.candidates();
            c.iter().any(|p| *p != c[0])
        });
    for (idx, u) in a.uses.iter().enumerate() {
        a.use_index
            .entry((u.unit, u.word.clone()))
            .or_default()
            .push(idx);
    }
    // Line the gate's resolutions up with ours, word for word.
    let total: usize = a.uses.iter().map(|u| u.paths.len()).sum();
    if total > 0 && input.resolved_paths.len() == total {
        let mut it = input.resolved_paths.iter();
        a.aligned = Some(
            a.uses
                .iter()
                .map(|u| it.by_ref().take(u.paths.len()).cloned().collect())
                .collect(),
        );
    }
    a
}

impl Analysis {
    /// Our resolution and the gate's, for one use.
    pub fn all_paths(&self, idx: usize) -> Vec<&PathBuf> {
        let mut v: Vec<&PathBuf> = self.uses[idx].paths.iter().collect();
        if let Some(al) = &self.aligned {
            v.extend(al[idx].iter());
        }
        v
    }
}

// ---------------------------------------------------------------------------
// Sensitive paths
// ---------------------------------------------------------------------------

fn lower(p: &Path) -> String {
    // APFS folds ſ (U+017F) to s; `to_lowercase` already maps K (U+212A) to k.
    let s = p.to_string_lossy().to_lowercase().replace('ſ', "s");
    let s = s.trim_end_matches('/');
    if s.is_empty() {
        "/".into()
    } else {
        s.to_string()
    }
}

/// `Some(rest)` when `p` is `base` or below it (`rest` is "" for equal).
fn under<'a>(p: &'a str, base: &str) -> Option<&'a str> {
    if p == base {
        return Some("");
    }
    let b = if base == "/" { "" } else { base };
    p.strip_prefix(b).and_then(|r| r.strip_prefix('/'))
}

const CRED_FILES: &[&str] = &[
    ".aws/credentials",
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".docker/config.json",
    ".kube/config",
    ".config/gh/hosts.yml",
    ".claude/.credentials.json",
    ".claude.json",
    ".codex/auth.json",
];
const CRED_TREES: &[&str] = &[".gnupg", ".config/gcloud", ".gemini", "library/keychains"];
const BROWSER_ROOTS: &[&str] = &[
    "library/application support/google/chrome",
    "library/application support/chromium",
    "library/application support/microsoft edge",
    "library/application support/bravesoftware",
    "library/application support/firefox",
    "library/application support/arc",
    "library/application support/vivaldi",
    "library/cookies",
    ".config/google-chrome",
    ".config/chromium",
    ".config/bravesoftware",
    ".config/microsoft-edge",
    ".config/vivaldi",
    ".mozilla",
];
const BROWSER_FILES: &[&str] = &[
    "login data",
    "login data for account",
    "cookies",
    "cookies.binarycookies",
    "cookies.sqlite",
    "logins.json",
    "key4.db",
    "key3.db",
    "web data",
    "local state",
];
const SSH_PROBES: &[&str] = &["id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", "x"];

/// Credential stores that ask (contract §9 rev 3.1, CR-12, D-082): not
/// hard-denied, but Home never auto-allows a call that reads, searches or
/// names them. Relative to home (compared in lower case). (`~/.git-credentials` is on the
/// hard-deny list and stays there.)
const ASK_FILES: &[&str] = &[
    ".cargo/credentials.toml",
    ".cargo/credentials",
    ".pypirc",
    ".pgpass",
    ".vault-token",
    ".azure/accessTokens.json",
    ".config/git/credentials",
    ".env",
    // shell history files
    ".bash_history",
    ".zsh_history",
    ".zhistory",
    ".sh_history",
    ".ksh_history",
    ".history",
    ".local/share/fish/fish_history",
    ".config/fish/fish_history",
];
/// Folders whose every file asks: `~/.aws/sso/cache/`, and macOS
/// Terminal's per-session shell histories.
const ASK_TREES: &[&str] = &[".aws/sso/cache", ".zsh_sessions", ".bash_sessions"];
/// `~/.azure/msal_token_cache.*`: (folder, file name prefix).
const ASK_NAME_PREFIXES: &[(&str, &str)] = &[(".azure", "msal_token_cache.")];

pub struct Sensitive {
    home: String,
    data: Vec<String>,
    protected: Vec<String>,
    raw_forms: Vec<(String, &'static str)>,
    /// The glob matcher's sensitive shapes, built on first use (D-131).
    specs: std::sync::OnceLock<std::sync::Arc<Specs>>,
    /// One shared work budget for all glob checks on this `Sensitive` (D-134).
    budget: std::sync::atomic::AtomicU64,
}

impl Sensitive {
    pub fn new(input: &PolicyInput<'_>) -> Self {
        let home = lower(input.home);
        let data: Vec<String> = input.atrium_data_paths.iter().map(|p| lower(p)).collect();
        let protected: Vec<String> = input
            .atrium_protected_paths
            .iter()
            .map(|p| lower(p))
            .collect();
        let mut raw_forms = vec![];
        for rel in CRED_FILES.iter().chain(CRED_TREES.iter()) {
            for anchor in ["~/", "$home/", "${home}/"] {
                raw_forms.push((format!("{anchor}{rel}"), "credential-read"));
            }
            raw_forms.push((format!("{home}/{rel}"), "credential-read"));
        }
        for d in &data {
            raw_forms.push((d.clone(), "atrium-self"));
        }
        Self {
            home,
            data,
            protected,
            raw_forms,
            specs: std::sync::OnceLock::new(),
            budget: std::sync::atomic::AtomicU64::new(MATCH_BUDGET),
        }
    }

    /// The hard-deny entry hit by reading (or, for `Write`, writing) `p`.
    pub fn hit(&self, p: &Path, role: Role) -> Option<&'static str> {
        let pl = lower(p);
        for d in &self.data {
            if under(&pl, d).is_some() {
                return Some("atrium-self");
            }
        }
        if role != Role::Read {
            for d in &self.protected {
                if under(&pl, d).is_some() {
                    return Some("atrium-self");
                }
            }
        }
        let rest = under(&pl, &self.home)?;
        if CRED_FILES.contains(&rest) || CRED_TREES.iter().any(|t| under(rest, t).is_some()) {
            return Some("credential-read");
        }
        if let Some(r) = under(rest, ".ssh")
            && !r.is_empty()
        {
            let name = r.rsplit('/').next().unwrap_or(r);
            let public = name.ends_with(".pub") || name.starts_with("known_hosts");
            let readable_config = r == "config" || r == "authorized_keys";
            if !(public || (readable_config && role != Role::Write)) {
                return Some("credential-read");
            }
        }
        for b in BROWSER_ROOTS {
            if let Some(r) = under(rest, b) {
                let name = r.rsplit('/').next().unwrap_or(r);
                if BROWSER_FILES.contains(&name) {
                    return Some("credential-read");
                }
            }
        }
        None
    }

    /// Whether `p` is a sensitive location or one of its ancestors (a
    /// recursive search from here could read it).
    pub fn is_or_contains(&self, p: &Path) -> bool {
        let pl = lower(p);
        let abs = |rel: &str| format!("{}/{rel}", self.home);
        let mut all: Vec<String> = self.data.clone();
        all.extend(self.protected.iter().cloned());
        for r in CRED_FILES.iter().chain(CRED_TREES).chain(BROWSER_ROOTS) {
            all.push(abs(r));
        }
        all.push(abs(".ssh"));
        // the credential stores that ask (CR-12): a recursive search of a
        // folder holding one asks
        for r in ASK_FILES.iter().chain(ASK_TREES) {
            all.push(abs(&r.to_lowercase()));
        }
        for (dir, pre) in ASK_NAME_PREFIXES {
            all.push(abs(&format!("{dir}/{pre}")));
        }
        all.iter().any(|s| under(s, &pl).is_some())
    }

    /// Atrium's data files (`lifecycle.rs`: the store and its WAL/SHM/journal,
    /// the development app identity). The one list both the probes and the
    /// glob matcher read.
    const DATA_FILES: &[&str] = &[
        "atrium.db",
        "atrium.db-wal",
        "atrium.db-shm",
        "atrium.db-journal",
        "app-identity.json",
    ];
    /// Atrium's data subfolders (`lifecycle.rs`: snapshots, logs).
    const DATA_SUBTREES: &[&str] = &["snapshots", "logs"];
    /// Atrium's runtime and config files in a protected folder
    /// (`lifecycle.rs`: `daemon.sock`, `daemon.lock`; the config file).
    const PROTECTED_FILES: &[&str] = &[
        "daemon.sock",
        "daemon.lock",
        "config.toml",
        "atrium.sock",
        "atrium.lock",
    ];

    /// Representative sensitive paths a glob could be hitting (`dev`'s
    /// sample set: the floor every glob check keeps).
    fn probes(&self) -> Vec<String> {
        let abs = |rel: &str| format!("{}/{rel}", self.home);
        let mut v = vec![];
        for f in CRED_FILES {
            v.push(abs(f));
        }
        for t in CRED_TREES {
            v.push(abs(t));
            v.push(format!("{}/x", abs(t)));
            if *t == ".gnupg" {
                v.push(format!("{}/private-keys-v1.d/x", abs(t)));
            }
        }
        for n in SSH_PROBES {
            v.push(abs(&format!(".ssh/{n}")));
        }
        for b in BROWSER_ROOTS {
            for f in BROWSER_FILES {
                v.push(format!("{}/{f}", abs(b)));
                v.push(format!("{}/default/{f}", abs(b)));
            }
        }
        for d in &self.data {
            v.push(d.clone());
            v.push(format!("{d}/x"));
            for f in Self::DATA_FILES {
                v.push(format!("{d}/{f}"));
            }
            for st in Self::DATA_SUBTREES {
                v.push(format!("{d}/{st}/x"));
            }
        }
        v
    }

    /// The sensitive shapes, built once per `(home, data, protected)` and
    /// cached per thread.
    fn specs(&self) -> &Specs {
        self.specs.get_or_init(|| {
            SPECS_CACHE.with(|c| {
                let mut cache = c.borrow_mut();
                if let Some(entry) = cache.iter().find(|e| {
                    e.home == self.home && e.data == self.data && e.protected == self.protected
                }) {
                    return std::sync::Arc::clone(&entry.specs);
                }
                let built = std::sync::Arc::new(Specs::build(self));
                if cache.len() >= 8 {
                    cache.remove(0);
                }
                cache.push(SpecsCacheEntry {
                    home: self.home.clone(),
                    data: self.data.clone(),
                    protected: self.protected.clone(),
                    specs: std::sync::Arc::clone(&built),
                });
                built
            })
        })
    }

    /// A word with glob characters: does the pattern reach a sensitive path?
    fn glob_hit(&self, pattern: &Path, role: Role) -> Option<&'static str> {
        let pl = lower(pattern);
        // literal directory prefix
        let mut prefix = PathBuf::new();
        for c in Path::new(&pl).components() {
            let s = c.as_os_str().to_string_lossy();
            if s.contains(['*', '?', '[', '{']) {
                break;
            }
            prefix.push(c.as_os_str());
        }
        if prefix.as_os_str() != "/"
            && !prefix.as_os_str().is_empty()
            && let Some(h) = self.hit(&prefix, role)
        {
            return Some(h);
        }
        match self.glob_str_hit_with(&pl, role, false) {
            GlobHit::Hit(h) => Some(h),
            GlobHit::Undecided | GlobHit::Miss => None,
        }
    }

    /// The shared glob check (D-131) for a lower-case absolute pattern:
    /// `dev`'s probes, then the keyed sensitive shapes. `search`: `globset`
    /// search-filter semantics, where `*` and `?` match a leading dot.
    fn glob_str_hit_with(&self, pl: &str, role: Role, search: bool) -> GlobHit {
        if !pl.starts_with('/') {
            return GlobHit::Miss;
        }
        let specs = self.specs();
        let mut guard = BudgetGuard::new(&self.budget);
        let Some(parsed) = PathPat::parse_all(pl, search, &mut guard.budget) else {
            return GlobHit::Undecided;
        };
        let mut q = Query::new(&mut guard.budget);
        let mut undecided = false;
        for pp in &parsed.pats {
            let floor = specs.floor.iter().map(|(c, id)| (c, *id, false));
            let hard = specs
                .hard
                .iter()
                .filter(|s| role != Role::Read || !s.write_only)
                .map(|s| (&s.comps, s.id, parsed.keyed));
            for (comps, id, keyed) in floor.chain(hard) {
                match path_meets(pp, comps, role, keyed, &mut q) {
                    Reach::Yes => return GlobHit::Hit(id),
                    Reach::Undecided => undecided = true,
                    Reach::No => {}
                }
            }
        }
        if undecided {
            GlobHit::Undecided
        } else {
            GlobHit::Miss
        }
    }

    /// The hard deny a search filter's anchored forms reach (D-131):
    /// `~/.g*/private-keys-v1.d/k.key` names a file no probe has.
    fn search_filter_hit(&self, variants: &[String]) -> Option<&'static str> {
        variants
            .iter()
            .find_map(|v| match self.glob_str_hit_with(v, Role::Read, true) {
                GlobHit::Hit(h) => Some(h),
                _ => None,
            })
    }

    pub fn is_ssh_dir(&self, p: &Path) -> bool {
        let pl = lower(p);
        if under(&pl, &self.home) == Some(".ssh") {
            return true;
        }
        if !pl.contains(['*', '?', '[', '{']) {
            return false;
        }
        let specs = self.specs();
        let mut guard = BudgetGuard::new(&self.budget);
        let Some(parsed) = PathPat::parse_all(&pl, false, &mut guard.budget) else {
            return false;
        };
        let mut q = Query::new(&mut guard.budget);
        parsed.pats.iter().any(|pp| {
            path_meets(pp, &specs.ssh_dir, Role::Read, parsed.keyed, &mut q) == Reach::Yes
        })
    }

    /// A credential store that asks (CR-12): `p` is one, or is inside one
    /// of its folders. A glob the matcher can't decide asks too.
    pub fn ask_hit(&self, p: &Path) -> bool {
        let pl = lower(p);
        if let Some(rest) = under(&pl, &self.home)
            && (ASK_FILES.iter().any(|f| f.to_lowercase() == rest)
                || ASK_TREES.iter().any(|t| under(rest, t).is_some())
                || ASK_NAME_PREFIXES.iter().any(|(dir, pre)| {
                    under(rest, dir).is_some_and(|r| !r.contains('/') && r.starts_with(pre))
                }))
        {
            return true;
        }
        if !pl.contains(['*', '?', '[', '{']) {
            return false;
        }
        self.ask_glob_str_hit_with(&pl, false)
    }

    /// A glob that can name a store that asks, or that the matcher can't
    /// decide (out of budget: Ask, never a hard deny).
    fn ask_glob_str_hit_with(&self, pl: &str, search: bool) -> bool {
        if !pl.starts_with('/') {
            return false;
        }
        let specs = self.specs();
        let mut guard = BudgetGuard::new(&self.budget);
        let Some(parsed) = PathPat::parse_all(pl, search, &mut guard.budget) else {
            return true;
        };
        let mut q = Query::new(&mut guard.budget);
        for pp in &parsed.pats {
            for s in &specs.ask {
                if path_meets(pp, &s.comps, Role::Read, parsed.keyed, &mut q) != Reach::No {
                    return true;
                }
            }
        }
        // If the hard-deny matcher couldn't decide this pattern within budget,
        // Ask rather than Allow (never Allow on doubt).
        for pp in &parsed.pats {
            if path_meets(pp, &specs.ssh_dir, Role::Read, parsed.keyed, &mut q) == Reach::Undecided
            {
                return true;
            }
            let floor = specs.floor.iter().map(|(c, _)| (c, false));
            let hard = specs
                .hard
                .iter()
                .filter(|s| !s.write_only)
                .map(|s| (&s.comps, parsed.keyed));
            for (comps, keyed) in floor.chain(hard) {
                if path_meets(pp, comps, Role::Read, keyed, &mut q) == Reach::Undecided {
                    return true;
                }
            }
        }
        false
    }

    /// Representative paths of the credential stores that ask.
    fn ask_probes(&self) -> Vec<String> {
        let abs = |rel: &str| format!("{}/{rel}", self.home);
        let mut v: Vec<String> = ASK_FILES.iter().map(|f| abs(&f.to_lowercase())).collect();
        for t in ASK_TREES {
            v.push(abs(t));
            v.push(format!("{}/x.json", abs(t)));
        }
        for (dir, pre) in ASK_NAME_PREFIXES {
            v.push(abs(&format!("{dir}/{pre}json")));
            v.push(abs(&format!("{dir}/{pre}bin")));
        }
        v
    }

    /// A search over `roots` scoped by `glob` (matched against file names,
    /// paths relative to a root, or whole paths) that can reach a credential
    /// store that asks.
    pub fn ask_search_glob_hit(
        &self,
        roots: &[PathBuf],
        glob: &str,
        cached: Option<&(Vec<String>, bool)>,
    ) -> bool {
        let owned;
        let (variants, unbounded) = match cached {
            Some((v, u)) => (v.as_slice(), *u),
            None => {
                let home_path = Path::new(&self.home);
                owned = glob_search_variants(roots, glob, home_path, home_path);
                (owned.0.as_slice(), owned.1)
            }
        };
        if unbounded {
            return true;
        }
        let mut matchers = Vec::with_capacity(variants.len());
        for norm in variants {
            let Ok(g) = GlobBuilder::new(norm).literal_separator(false).build() else {
                return true;
            };
            matchers.push(g.compile_matcher());
        }
        let root_lowers: Vec<String> = roots.iter().map(|r| lower(r)).collect();
        if self.ask_probes().iter().any(|probe| {
            let name = probe.rsplit('/').next().unwrap_or(probe);
            root_lowers.iter().any(|rl| {
                let Some(rel) = under(probe, rl) else {
                    return false;
                };
                matchers.iter().any(|g| {
                    g.is_match(name) || g.is_match(probe) || (!rel.is_empty() && g.is_match(rel))
                })
            })
        }) {
            return true;
        }
        // A search rooted inside a folder whose every file asks.
        for t in ASK_TREES {
            let t_abs = format!("{}/{t}", self.home);
            if root_lowers.iter().any(|rl| under(rl, &t_abs).is_some()) {
                return true;
            }
        }
        // The anchored forms of the filter, through the shared matcher (K8).
        variants.iter().any(|v| self.ask_glob_str_hit_with(v, true))
    }

    /// Raw-text check: home-anchored credential paths and Atrium's data.
    fn raw_hit(&self, text: &str) -> Option<&'static str> {
        let t: String = text
            .to_lowercase()
            .chars()
            .filter(|c| !matches!(c, '"' | '\'' | '\\'))
            .collect();
        for (form, id) in &self.raw_forms {
            if t.contains(form.as_str()) {
                return Some(id);
            }
        }
        for anchor in [
            "~/.ssh/",
            "$home/.ssh/",
            "${home}/.ssh/",
            &format!("{}/.ssh/", self.home),
        ] {
            let mut rest = t.as_str();
            while let Some(i) = rest.find(anchor) {
                rest = &rest[i + anchor.len()..];
                let name: String = rest
                    .chars()
                    .take_while(|c| !c.is_whitespace() && !matches!(c, ';' | '|' | '&' | ')' | '`'))
                    .collect();
                if !name.is_empty()
                    && !(name.ends_with(".pub")
                        || name.starts_with("known_hosts")
                        || name == "config"
                        || name == "authorized_keys")
                {
                    return Some("credential-read");
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// The bounded glob-component matcher (D-131)
// ---------------------------------------------------------------------------
//
// One matcher decides whether a path pattern (`*`, `?`, `[...]` with POSIX
// classes and ranges, `**`, braces expanded within a budget) can name a
// sensitive path. Matching is over bytes: a literal is its UTF-8 bytes, and
// `?`, a negated class, an unknown class or a class naming a non-ASCII
// character matches any non-ASCII byte run (a superset of one character).
//
// Sensitive paths are *shapes*: lists of components, each a literal name, a
// name glob (`*.session`), a set of names, any one name, any number of
// names, an SSH name that isn't public, or a store name the pattern must pin
// (`msal_token_cache.*`). Both levels are dynamic programs over (pattern
// position, shape position) that charge every visited state, and every
// parsed byte, to one budget. A query that runs out answers "undecided",
// which the callers turn into Ask, never into a hard deny.
//
// *Keys* keep ordinary globs ordinary: a shape marks the components that
// name it (a tree's folder, a store's file name). A component that matches
// every name (`*`, `***`, `?*`, `*[!.]*`, `**`) only counts against a key
// when every later pattern component does too, so `~/.config/*/settings.json`
// and `~/**/*.rs` stay ordinary while `~/.g*/k.key` and `~/**/b.session`
// don't.

/// Work allowed per query: visited DP states plus parsed bytes.
const MATCH_BUDGET: u64 = 4_000_000;
/// Brace expansions read exactly; more and the pattern is widened.
const MAX_PATH_VARIANTS: usize = 64;
/// DP tables up to this many cells use a flat bitmap/vec; larger ones are hashed.
const DENSE_CELLS: usize = 4096;

struct SpecsCacheEntry {
    home: String,
    data: Vec<String>,
    protected: Vec<String>,
    specs: std::sync::Arc<Specs>,
}

thread_local! {
    static SPECS_CACHE: std::cell::RefCell<Vec<SpecsCacheEntry>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

struct Budget(u64);

impl Budget {
    fn new() -> Self {
        Self(MATCH_BUDGET)
    }
    /// Spends `n` steps; `false` once the budget is gone.
    fn take(&mut self, n: u64) -> bool {
        if self.0 < n {
            self.0 = 0;
            return false;
        }
        self.0 -= n;
        true
    }
}

/// Checks out a `Sensitive`'s shared budget into a local `u64` for tight DP
/// loops and writes the remaining budget back on drop.
struct BudgetGuard<'a> {
    cell: &'a std::sync::atomic::AtomicU64,
    budget: Budget,
}

impl<'a> BudgetGuard<'a> {
    fn new(cell: &'a std::sync::atomic::AtomicU64) -> Self {
        Self {
            cell,
            budget: Budget(cell.load(std::sync::atomic::Ordering::Relaxed)),
        }
    }
}

impl Drop for BudgetGuard<'_> {
    fn drop(&mut self) {
        self.cell
            .store(self.budget.0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// One parsed pattern's scratch state and component-match cache, sharing the
/// check's single budget.
struct Query<'a, 'b> {
    budget: &'a mut Budget,
    cache: HashMap<(usize, &'b str), Reach>,
    path_stack: Vec<(usize, usize)>,
    comp_stack: Vec<(usize, usize, bool, bool)>,
}

impl<'a, 'b> Query<'a, 'b> {
    fn new(budget: &'a mut Budget) -> Self {
        Self {
            budget,
            cache: HashMap::new(),
            path_stack: Vec::new(),
            comp_stack: Vec::new(),
        }
    }
}

/// What a query found.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Reach {
    No,
    /// Out of budget: neither shown nor ruled out.
    Undecided,
    Yes,
}

/// Visited states of a DP: stack bitset for small tables, dense vec for
/// medium ones, hashed for large ones.
enum Seen {
    Small([u64; 8]),
    Dense(Vec<bool>),
    Sparse(std::collections::HashSet<usize>),
}

impl Seen {
    fn new(cells: usize) -> Self {
        if cells <= 512 {
            Seen::Small([0; 8])
        } else if cells <= DENSE_CELLS {
            Seen::Dense(vec![false; cells])
        } else {
            Seen::Sparse(std::collections::HashSet::new())
        }
    }
    /// Marks `id`; `false` when it was already marked.
    fn insert(&mut self, id: usize) -> bool {
        match self {
            Seen::Small(bits) => {
                let slot = &mut bits[id >> 6];
                let mask = 1u64 << (id & 63);
                if (*slot & mask) != 0 {
                    return false;
                }
                *slot |= mask;
                true
            }
            Seen::Dense(v) => !std::mem::replace(&mut v[id], true),
            Seen::Sparse(s) => s.insert(id),
        }
    }
}

/// A set of bytes.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BSet([u128; 2]);

impl BSet {
    const EMPTY: BSet = BSet([0, 0]);
    /// Every byte a file name can hold (not NUL, not `/`).
    const NAME: BSet = BSet([!1u128 & !(1u128 << b'/'), !0]);
    /// The bytes of a non-ASCII character.
    const HI: BSet = BSet([0, !0]);

    fn one(b: u8) -> Self {
        let mut s = Self::EMPTY;
        s.add(b);
        s
    }
    fn add(&mut self, b: u8) {
        self.0[usize::from(b >> 7)] |= 1u128 << (b & 0x7f);
    }
    fn contains(&self, b: u8) -> bool {
        (self.0[usize::from(b >> 7)] & (1u128 << (b & 0x7f))) != 0
    }
    fn and(self, o: BSet) -> BSet {
        BSet([self.0[0] & o.0[0], self.0[1] & o.0[1]])
    }
    fn or(self, o: BSet) -> BSet {
        BSet([self.0[0] | o.0[0], self.0[1] | o.0[1]])
    }
    fn minus(self, o: BSet) -> BSet {
        BSet([self.0[0] & !o.0[0], self.0[1] & !o.0[1]])
    }
    fn is_empty(&self) -> bool {
        self.0 == [0, 0]
    }
    fn single(&self) -> Option<u8> {
        let n = self.0[0].count_ones() + self.0[1].count_ones();
        if n != 1 {
            return None;
        }
        Some(if self.0[0] != 0 {
            self.0[0].trailing_zeros() as u8
        } else {
            0x80 | self.0[1].trailing_zeros() as u8
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Atom {
    /// `*`: any bytes of a name.
    Star,
    /// One byte from the set; `dot`: it may be a name's leading `.` (a
    /// literal or a class may; `?` may not, as in the shell).
    One(BSet, bool),
    /// The rest of a non-ASCII character: zero or more non-ASCII bytes.
    Tail,
}

const fn ascii_range(lo: u8, hi: u8) -> u128 {
    ((1u128 << (hi - lo + 1)) - 1) << lo
}

fn posix_class_bset(name: &str) -> Option<BSet> {
    const LOWER: u128 = ascii_range(b'a', b'z');
    const UPPER: u128 = ascii_range(b'A', b'Z');
    const ALPHA: u128 = LOWER | UPPER;
    const DIGIT: u128 = ascii_range(b'0', b'9');
    const ALNUM: u128 = ALPHA | DIGIT;
    const XDIGIT: u128 = DIGIT | ascii_range(b'a', b'f') | ascii_range(b'A', b'F');
    const PUNCT: u128 = ascii_range(0x21, 0x2F)
        | ascii_range(0x3A, 0x40)
        | ascii_range(0x5B, 0x60)
        | ascii_range(0x7B, 0x7E);
    const GRAPH: u128 = ascii_range(0x21, 0x7E);
    const PRINT: u128 = ascii_range(0x20, 0x7E);
    const SPACE: u128 = (1u128 << b' ') | ascii_range(0x09, 0x0D);
    const BLANK: u128 = (1u128 << b' ') | (1u128 << b'\t');
    const CNTRL: u128 = ascii_range(0x01, 0x1F) | (1u128 << 0x7F);
    const WORD: u128 = ALNUM | (1u128 << b'_');
    let lo = match name {
        "alpha" => ALPHA,
        "alnum" => ALNUM,
        "lower" => LOWER,
        "upper" => UPPER,
        "digit" => DIGIT,
        "xdigit" => XDIGIT,
        "punct" => PUNCT,
        "graph" => GRAPH,
        "print" => PRINT,
        "space" => SPACE,
        "blank" => BLANK,
        "cntrl" => CNTRL,
        "word" => WORD,
        _ => return None,
    };
    Some(BSet([lo, 0]))
}

/// A class body (without brackets and negation): its ASCII bytes, and
/// whether it names a non-ASCII character. `None`: an unknown `[:class:]`
/// or unsupported POSIX bracket expression (`[=a=]`, `[.ch.]`).
fn class_body(body: &str) -> Option<(BSet, bool)> {
    let cs: Vec<char> = body.chars().collect();
    let mut set = BSet::EMPTY;
    let mut hi = false;
    let add = |lo: char, h: char, set: &mut BSet| -> bool {
        if lo > h {
            return false;
        }
        if lo == h {
            if lo.is_ascii() {
                let b = lo as u8;
                if b != 0 {
                    set.add(b);
                }
                return false;
            }
            return true;
        }
        let lo_u = lo as u32;
        let h_u = h as u32;
        if lo_u < 0x80 {
            let start = (lo_u as u8).max(1);
            let end = h_u.min(0x7f) as u8;
            set.0[0] |= ascii_range(start, end);
        }
        !h.is_ascii()
    };
    let mut k = 0;
    while k < cs.len() {
        if cs[k] == '[' && matches!(cs.get(k + 1), Some(':' | '=' | '.')) {
            let tag = cs[k + 1];
            if tag == ':' {
                let mut m = k + 2;
                let mut name_buf = [0u8; 16];
                let mut nlen = 0usize;
                while m < cs.len()
                    && (cs[m].is_ascii_alphanumeric() || cs[m] == '_' || cs[m] == '-')
                {
                    if nlen < name_buf.len() {
                        name_buf[nlen] = cs[m] as u8;
                    }
                    nlen += 1;
                    m += 1;
                }
                if m > k + 2 && cs.get(m) == Some(&':') && cs.get(m + 1) == Some(&']') {
                    if nlen > name_buf.len() {
                        return None;
                    }
                    let name = std::str::from_utf8(&name_buf[..nlen]).ok()?;
                    set = set.or(posix_class_bset(name)?);
                    k = m + 2;
                    continue;
                }
            } else {
                let mut m = k + 2;
                let limit = (k + 18).min(cs.len());
                while m < limit && cs[m] != '[' && cs[m] != ']' && cs[m] != tag {
                    m += 1;
                }
                if m > k + 2 && cs.get(m) == Some(&tag) && cs.get(m + 1) == Some(&']') {
                    return None;
                }
            }
        }
        let (lo, nk) = if cs[k] == '\\' && k + 1 < cs.len() {
            (cs[k + 1], k + 2)
        } else {
            (cs[k], k + 1)
        };
        if nk + 1 < cs.len() && cs[nk] == '-' {
            let (h, nk2) = if cs[nk + 1] == '\\' && nk + 2 < cs.len() {
                (cs[nk + 2], nk + 3)
            } else {
                (cs[nk + 1], nk + 2)
            };
            hi |= add(lo, h, &mut set);
            k = nk2;
        } else {
            hi |= add(lo, lo, &mut set);
            k = nk;
        }
    }
    Some((set.and(BSet::NAME), hi))
}

/// Advance past a POSIX bracket item (`[:name:]`, `[=x=]`, `[.x.]`) starting
/// at `b[j] == b'['`, returning the next index and bytes scanned.
fn posix_item_end(b: &[u8], j: usize) -> Option<(usize, u64)> {
    let tag = *b.get(j + 1)?;
    if !matches!(tag, b':' | b'=' | b'.') {
        return None;
    }
    let mut k = j + 2;
    let mut steps = 0u64;
    if tag == b':' {
        while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'_' || b[k] == b'-') {
            k += 1;
            steps += 1;
        }
    } else {
        let limit = (j + 18).min(b.len());
        while k < limit && b[k] != b'[' && b[k] != b']' && b[k] != tag {
            k += 1;
            steps += 1;
        }
    }
    if k > j + 2 && b.get(k) == Some(&tag) && b.get(k + 1) == Some(&b']') {
        Some((k + 2, steps))
    } else {
        None
    }
}

/// The byte index of the `]` closing the class opened at `start`, charging
/// each scanned byte and memoizing positions that never reach a closing `]`
/// so unclosed openers stay strictly linear. `Err` when the budget ran out.
fn class_end(
    b: &[u8],
    start: usize,
    dead: &mut Option<Vec<bool>>,
    budget: &mut Budget,
) -> Result<Option<usize>, ()> {
    let mut j = start + 1;
    if matches!(b.get(j), Some(b'!' | b'^')) {
        j += 1;
    }
    if b.get(j) == Some(&b']') {
        j += 1;
    }
    let first_j = j;
    let mut visited: Vec<usize> = Vec::new();
    let mut steps = 1u64;
    let found = loop {
        if j >= b.len() {
            break None;
        }
        if dead.as_ref().is_some_and(|d| d[j]) {
            break None;
        }
        steps += 1;
        if dead.is_some() {
            visited.push(j);
        }
        match b[j] {
            b'[' if matches!(b.get(j + 1), Some(b':' | b'=' | b'.')) => {
                if let Some((next, extra)) = posix_item_end(b, j) {
                    steps += extra;
                    j = next;
                } else {
                    j += 1;
                }
            }
            b'\\' => j += 2,
            b']' => break Some(j),
            _ => j += 1,
        }
    };
    if !budget.take(steps) {
        return Err(());
    }
    if found.is_none() {
        if let Some(d) = dead {
            for idx in visited {
                d[idx] = true;
            }
        } else {
            let mut d = vec![false; b.len()];
            let mut k = first_j;
            while k < b.len() {
                d[k] = true;
                match b[k] {
                    b'[' if matches!(b.get(k + 1), Some(b':' | b'=' | b'.')) => {
                        if let Some((next, _)) = posix_item_end(b, k) {
                            k = next;
                        } else {
                            k += 1;
                        }
                    }
                    b'\\' => k += 2,
                    b']' => break,
                    _ => k += 1,
                }
            }
            *dead = Some(d);
        }
    }
    Ok(found)
}

fn push_char(out: &mut Vec<Atom>, c: char) {
    let mut buf = [0u8; 4];
    for &b in c.encode_utf8(&mut buf).as_bytes() {
        out.push(Atom::One(BSet::one(b), true));
    }
}

/// A name pattern's atoms. `None` when the budget ran out.
fn parse_atoms(pat: &str, budget: &mut Budget) -> Option<Vec<Atom>> {
    // Charge 4 steps per byte so many brace variants of a huge component hit
    // the budget before allocating excessive memory.
    if !budget.take(pat.len().saturating_mul(4) as u64 + 1) {
        return None;
    }
    let bytes = pat.as_bytes();
    // With no `]` after it, an opener is a literal: no scan needed.
    let last_close = bytes.iter().rposition(|&b| b == b']');
    let mut dead: Option<Vec<bool>> = None;
    let mut out = Vec::with_capacity(pat.len());
    let mut it = pat.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        match c {
            '*' => {
                if out.last() != Some(&Atom::Star) {
                    out.push(Atom::Star);
                }
            }
            '?' => {
                out.push(Atom::One(BSet::NAME, false));
                out.push(Atom::Tail);
            }
            '\\' => match it.next() {
                Some((_, e)) => push_char(&mut out, e),
                None => push_char(&mut out, '\\'),
            },
            '[' => {
                let end = match last_close {
                    Some(lc) if lc > i => class_end(bytes, i, &mut dead, budget).ok()?,
                    _ => None,
                };
                let Some(end) = end else {
                    push_char(&mut out, '[');
                    continue;
                };
                let inner = &pat[i + 1..end];
                let (neg, body) = match inner.strip_prefix(['!', '^']) {
                    Some(b) => (true, b),
                    None => (false, inner),
                };
                // A negated or unknown class, or one naming a non-ASCII
                // character, matches every non-ASCII character.
                let (set, hi) = match class_body(body) {
                    Some((set, _)) if neg => (BSet::NAME.minus(set).or(BSet::HI), true),
                    Some((set, hi)) => (if hi { set.or(BSet::HI) } else { set }, hi),
                    None => (BSet::NAME, true),
                };
                out.push(Atom::One(set, true));
                if hi {
                    out.push(Atom::Tail);
                }
                while it.peek().is_some_and(|(k, _)| *k <= end) {
                    it.next();
                }
            }
            _ => push_char(&mut out, c),
        }
    }
    Some(out)
}

fn lit_atoms_bytes(name: &[u8]) -> Vec<Atom> {
    name.iter()
        .map(|&b| Atom::One(BSet::one(b), true))
        .collect()
}

/// Whether two name patterns share a non-empty name. `loose`: `*` and `?`
/// in `a` may match a leading dot (`dev`'s globset reading of patterns
/// holding `**`, and `Search` filters). `stem`: the first `stem` atoms of `b`
/// are a store's own name, and a match counts only when a non-wildcard atom of
/// `a` takes one of their bytes (the pattern pins part of the name).
fn comp_intersect(
    a: &[Atom],
    b: &[Atom],
    loose: bool,
    stem: Option<usize>,
    stack: &mut Vec<(usize, usize, bool, bool)>,
    budget: &mut Budget,
) -> Reach {
    if a.is_empty() || b.is_empty() {
        return Reach::No;
    }
    if let (Atom::One(sa, da), Atom::One(sb, db)) = (a[0], b[0]) {
        let mut meet = sa.and(sb).and(BSet::NAME);
        if !((da || loose) && db) {
            meet = meet.minus(BSet::one(b'.'));
        }
        if meet.is_empty() {
            return if budget.take(1) {
                Reach::No
            } else {
                Reach::Undecided
            };
        }
    }
    if let (Some(Atom::One(sa, _)), Some(Atom::One(sb, _))) = (a.last(), b.last())
        && sa.and(*sb).and(BSet::NAME).is_empty()
    {
        return if budget.take(1) {
            Reach::No
        } else {
            Reach::Undecided
        };
    }
    let (la, lb) = (a.len(), b.len());
    let w = lb + 1;
    let cells = (la + 1).saturating_mul(w).saturating_mul(4);
    let mut seen = Seen::new(cells);
    stack.clear();
    stack.push((0usize, 0usize, false, false));
    while let Some((i, j, started, named)) = stack.pop() {
        let id = ((i * w + j) * 2 + usize::from(started)) * 2 + usize::from(named);
        if !seen.insert(id) {
            continue;
        }
        if !budget.take(1) {
            return Reach::Undecided;
        }
        if i == la && j == lb && started && (named || stem.is_none()) {
            return Reach::Yes;
        }
        if i < la && matches!(a[i], Atom::Star | Atom::Tail) {
            stack.push((i + 1, j, started, named));
        }
        if j < lb && matches!(b[j], Atom::Star | Atom::Tail) {
            stack.push((i, j + 1, started, named));
        }
        if i >= la || j >= lb {
            continue;
        }
        let (xa, xdot, ni, astay) = match a[i] {
            Atom::Star => (BSet::NAME, loose, i, true),
            Atom::Tail => (BSet::HI, false, i, true),
            Atom::One(s, d) => (s, d || loose, i + 1, false),
        };
        let (yb, ydot, nj, bstay) = match b[j] {
            Atom::Star => (BSet::NAME, false, j, true),
            Atom::Tail => (BSet::HI, false, j, true),
            Atom::One(s, d) => (s, d, j + 1, false),
        };
        if astay && bstay && started {
            continue;
        }
        let mut meet = xa.and(yb).and(BSet::NAME);
        if !started && !(xdot && ydot) {
            meet = meet.minus(BSet::one(b'.'));
        }
        if !meet.is_empty() {
            let pins = !astay && !bstay && stem.is_some_and(|k| j < k);
            stack.push((ni, nj, true, named || pins));
        }
    }
    Reach::No
}

/// Fast path for matching pattern atoms against a literal name without heap
/// allocation for common lengths.
fn comp_meets_lit(
    atoms: &[Atom],
    name: &[u8],
    loose: bool,
    stack: &mut Vec<(usize, usize, bool, bool)>,
    budget: &mut Budget,
) -> Reach {
    if atoms.is_empty() || name.is_empty() {
        return Reach::No;
    }
    if let Atom::One(sa, da) = atoms[0] {
        let b0 = name[0];
        if !sa.contains(b0) || (b0 == b'.' && !(da || loose)) {
            return if budget.take(1) {
                Reach::No
            } else {
                Reach::Undecided
            };
        }
    }
    if let Some(Atom::One(sa, _)) = atoms.last()
        && !sa.contains(*name.last().unwrap())
    {
        return if budget.take(1) {
            Reach::No
        } else {
            Reach::Undecided
        };
    }
    if name.len() <= 64 {
        let mut buf = [Atom::Star; 64];
        for (dst, &b) in buf.iter_mut().zip(name) {
            *dst = Atom::One(BSet::one(b), true);
        }
        comp_intersect(atoms, &buf[..name.len()], loose, None, stack, budget)
    } else {
        let b_atoms = lit_atoms_bytes(name);
        comp_intersect(atoms, &b_atoms, loose, None, stack, budget)
    }
}

/// A component that matches every name a directory listing gives (`*`,
/// `***`, `?*`, `*[!.]*`): it doesn't single anything out.
fn atoms_catchall(atoms: &[Atom]) -> bool {
    let mut stars = 0;
    let mut ones = 0;
    let all_but_dot = BSet::NAME.minus(BSet::one(b'.'));
    for a in atoms {
        match a {
            Atom::Star => stars += 1,
            Atom::Tail => {}
            Atom::One(s, _) if all_but_dot.minus(*s).is_empty() => ones += 1,
            Atom::One(..) => return false,
        }
    }
    stars > 0 && ones <= 1
}

/// Whether a name pattern can match an SSH file that isn't public
/// (`*.pub`, `known_hosts*`, and at the top `config`/`authorized_keys`
/// unless written).
fn atoms_may_be_secret(atoms: &[Atom], top: bool, role: Role) -> bool {
    let single = |a: &Atom| match a {
        Atom::One(s, _) => s.single(),
        _ => None,
    };
    let prefix: Vec<u8> = atoms.iter().map_while(single).collect();
    let mut suffix: Vec<u8> = atoms.iter().rev().map_while(single).collect();
    suffix.reverse();
    if suffix.ends_with(b".pub") || prefix.starts_with(b"known_hosts") {
        return false;
    }
    let literal = prefix.len() == atoms.len();
    !(top
        && role != Role::Write
        && literal
        && (prefix == b"config" || prefix == b"authorized_keys"))
}

/// A pattern path component.
enum PComp {
    GlobStar,
    Pat {
        atoms: Vec<Atom>,
        catchall: bool,
        /// The name, when the component has no wildcard.
        lit: Option<String>,
    },
}

struct PathPat {
    comps: Vec<PComp>,
    /// `tail_catchall[i]`: every component from `i` on matches everything.
    tail_catchall: Vec<bool>,
    globstar: bool,
    /// The spelling holds `**` anywhere (`dev` then matched it with
    /// globset, where `*` takes a leading dot).
    dstar: bool,
}

/// Widens a pattern whose braces expand too far: each outermost brace
/// group becomes `*`, or the rest of the path `/**` when one holds a `/`.
fn widen_braces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    let mut slash = false;
    for c in s.chars() {
        match c {
            '{' => {
                if depth == 0 {
                    slash = false;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    if slash {
                        let cut = out.rfind('/').unwrap_or(0);
                        out.truncate(cut);
                        out.push_str("/**");
                        return out;
                    }
                    out.push('*');
                }
            }
            '/' if depth > 0 => slash = true,
            _ if depth > 0 => {}
            _ => out.push(c),
        }
    }
    if depth > 0 {
        let cut = out.rfind('/').unwrap_or(0);
        out.truncate(cut);
        out.push_str("/**");
    }
    out
}

/// A pattern read for matching: its brace variants and whether keys apply
/// (`false` once the braces were widened: then nothing is ordinary).
struct Parsed {
    pats: Vec<PathPat>,
    keyed: bool,
}

impl PathPat {
    /// `None` when the budget ran out while reading. `loose_dots`: `true` for
    /// `globset` search filters (where `*` and `?` always match leading dots).
    fn parse_all(s: &str, loose_dots: bool, budget: &mut Budget) -> Option<Parsed> {
        if !budget.take(s.len() as u64) {
            return None;
        }
        let (variants, keyed) = match words::brace_expand(s) {
            Some(v) if v.len() <= MAX_PATH_VARIANTS => (v, true),
            _ => (vec![widen_braces(s)], false),
        };
        let base_dstar = loose_dots || !keyed || s.contains("**");
        let mut pats = Vec::with_capacity(variants.len());
        for v in &variants {
            let dstar = base_dstar || v.contains("**");
            pats.push(PathPat::parse(v, dstar, budget)?);
        }
        Some(Parsed { pats, keyed })
    }

    fn parse(s: &str, dstar: bool, budget: &mut Budget) -> Option<Self> {
        let mut comps: Vec<PComp> = vec![];
        for part in s.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    if !matches!(comps.last(), Some(PComp::GlobStar) | None) {
                        comps.pop();
                    }
                }
                "**" => {
                    if !matches!(comps.last(), Some(PComp::GlobStar)) {
                        comps.push(PComp::GlobStar);
                    }
                }
                _ => {
                    let atoms = parse_atoms(part, budget)?;
                    let lit = (!part.contains(['*', '?', '[', '\\'])).then(|| part.to_string());
                    let catchall = atoms_catchall(&atoms);
                    comps.push(PComp::Pat {
                        atoms,
                        catchall,
                        lit,
                    });
                }
            }
        }
        let mut tail_catchall = vec![true; comps.len() + 1];
        for i in (0..comps.len()).rev() {
            let c = match &comps[i] {
                PComp::GlobStar => true,
                PComp::Pat { catchall, .. } => *catchall,
            };
            tail_catchall[i] = c && tail_catchall[i + 1];
        }
        let globstar = comps.iter().any(|c| matches!(c, PComp::GlobStar));
        Some(Self {
            comps,
            tail_catchall,
            globstar,
            dstar,
        })
    }
}

/// A component of a sensitive shape.
#[derive(Debug)]
enum SComp {
    Lit(String),
    /// A name glob (`*.session`).
    Glob(Vec<Atom>),
    /// One of these names.
    Names(&'static [&'static str]),
    /// Any one name.
    One,
    /// Any number of names (zero included).
    Any,
    /// An SSH file name that isn't public (`true`: directly in `~/.ssh`).
    Secret(bool),
    /// `<stem>.*`, reached only by a name pattern that pins part of the
    /// stem (`ms?l_*.custom`, `*token_cache.x`), not by one whose wildcard
    /// covers the whole stem (`*.rs`).
    Pinned(Vec<Atom>, usize),
}

struct Shape {
    comps: Vec<(SComp, bool)>,
    id: &'static str,
    /// Only a write touches it (Atrium's protected folders).
    write_only: bool,
}

/// The sensitive shapes, from the same lists `Sensitive::hit` reads.
struct Specs {
    /// `dev`'s probes, matched without keys.
    floor: Vec<(Vec<(SComp, bool)>, &'static str)>,
    hard: Vec<Shape>,
    ask: Vec<Shape>,
    ssh_dir: Vec<(SComp, bool)>,
}

fn lits(path: &str) -> Vec<(SComp, bool)> {
    path.split('/')
        .filter(|c| !c.is_empty())
        .map(|c| (SComp::Lit(c.to_string()), false))
        .collect()
}

/// `path` and everything below it; its last name is the key.
fn tree_shape(path: &str) -> Vec<(SComp, bool)> {
    let mut v = lits(path);
    if let Some(l) = v.last_mut() {
        l.1 = true;
    }
    v.push((SComp::Any, false));
    v
}

/// The file `path`; its name is the key.
fn file_shape(path: &str) -> Vec<(SComp, bool)> {
    let mut v = lits(path);
    if let Some(l) = v.last_mut() {
        l.1 = true;
    }
    v
}

/// A shape's own name glob (internal text, read without a budget limit).
fn shape_glob(g: &str) -> Vec<Atom> {
    parse_atoms(g, &mut Budget::new()).unwrap_or_else(|| vec![Atom::Star])
}

impl Specs {
    fn build(s: &Sensitive) -> Self {
        let home = &s.home;
        let abs = |rel: &str| format!("{home}/{rel}");
        let floor = s
            .probes()
            .into_iter()
            .filter_map(|p| {
                let id = s.hit(Path::new(&p), Role::Read)?;
                Some((lits(&p), id))
            })
            .collect();
        let shape = |comps, id, write_only| Shape {
            comps,
            id,
            write_only,
        };
        let mut hard = vec![];
        for d in &s.data {
            hard.push(shape(tree_shape(d), "atrium-self", false));
            for f in Sensitive::DATA_FILES {
                hard.push(shape(file_shape(&format!("{d}/{f}")), "atrium-self", false));
            }
            for st in Sensitive::DATA_SUBTREES {
                hard.push(shape(
                    tree_shape(&format!("{d}/{st}")),
                    "atrium-self",
                    false,
                ));
            }
        }
        for p in &s.protected {
            hard.push(shape(tree_shape(p), "atrium-self", true));
            for f in Sensitive::PROTECTED_FILES {
                hard.push(shape(file_shape(&format!("{p}/{f}")), "atrium-self", true));
            }
        }
        const CR: &str = "credential-read";
        for f in CRED_FILES {
            hard.push(shape(file_shape(&abs(f)), CR, false));
        }
        for t in CRED_TREES {
            hard.push(shape(tree_shape(&abs(t)), CR, false));
        }
        let ssh_dir = file_shape(&abs(".ssh"));
        let mut top = file_shape(&abs(".ssh"));
        top.push((SComp::Secret(true), false));
        hard.push(shape(top, CR, false));
        let mut deep = file_shape(&abs(".ssh"));
        deep.extend([
            (SComp::One, false),
            (SComp::Any, false),
            (SComp::Secret(false), false),
        ]);
        hard.push(shape(deep, CR, false));
        for n in SSH_PROBES.iter().filter(|n| **n != "x") {
            hard.push(shape(file_shape(&abs(&format!(".ssh/{n}"))), CR, false));
        }
        for b in BROWSER_ROOTS {
            let mut v = lits(&abs(b));
            v.push((SComp::Any, false));
            v.push((SComp::Names(BROWSER_FILES), true));
            hard.push(shape(v, CR, false));
        }
        let mut ask = vec![];
        for f in ASK_FILES {
            ask.push(shape(file_shape(&abs(&f.to_lowercase())), "ask", false));
        }
        for t in ASK_TREES {
            ask.push(shape(tree_shape(&abs(t)), "ask", false));
        }
        for (t, sufs) in ASK_TREE_SUFFIXES {
            for suf in *sufs {
                // The folder's files at any depth below it.
                let mut v = lits(&abs(t));
                v.push((SComp::Any, false));
                v.push((SComp::Glob(shape_glob(&format!("*{suf}"))), true));
                ask.push(shape(v, "ask", false));
            }
        }
        for (dir, pre) in ASK_NAME_PREFIXES {
            // Named folder: any `msal_token_cache.*`.
            let mut v = file_shape(&abs(dir));
            v.push((SComp::Glob(shape_glob(&format!("{pre}*"))), true));
            ask.push(shape(v, "ask", false));
            // Past the folder by a `**`: a file name that pins the stem.
            let stem = pre.trim_end_matches('.');
            let mut v = lits(&abs(dir));
            v.push((
                SComp::Pinned(shape_glob(&format!("{pre}*")), stem.len()),
                true,
            ));
            ask.push(shape(v, "ask", false));
        }
        Self {
            floor,
            hard,
            ask,
            ssh_dir,
        }
    }
}

/// The file-name families of the folders whose every file asks: a `**`
/// that passes the folder still asks when its file name can be one.
const ASK_TREE_SUFFIXES: &[(&str, &[&str])] = &[
    (".aws/sso/cache", &[".json"]),
    (
        ".zsh_sessions",
        &[".session", ".history", ".historynew", ".zsh"],
    ),
    (".bash_sessions", &[".history", ".historynew", ".session"]),
];

/// Whether pattern component `p` can match shape component `s`.
fn comp_meets<'b>(
    p: &PComp,
    s: &'b SComp,
    role: Role,
    loose: bool,
    q: &mut Query<'_, 'b>,
) -> Reach {
    let PComp::Pat { atoms, lit, .. } = p else {
        return Reach::Yes;
    };
    let key = p as *const PComp as usize;
    let against = |name: &'b str, q: &mut Query<'_, 'b>| -> Reach {
        if let Some(l) = lit {
            return if l == name { Reach::Yes } else { Reach::No };
        }
        if let Some(r) = q.cache.get(&(key, name)) {
            return *r;
        }
        let r = comp_meets_lit(atoms, name.as_bytes(), loose, &mut q.comp_stack, q.budget);
        if r != Reach::Undecided {
            q.cache.insert((key, name), r);
        }
        r
    };
    match s {
        SComp::One | SComp::Any => Reach::Yes,
        SComp::Lit(name) => against(name.as_str(), q),
        SComp::Names(names) => {
            let mut r = Reach::No;
            for &n in *names {
                match against(n, q) {
                    Reach::Yes => return Reach::Yes,
                    Reach::Undecided => r = Reach::Undecided,
                    Reach::No => {}
                }
            }
            r
        }
        SComp::Glob(g) => comp_intersect(atoms, g, loose, None, &mut q.comp_stack, q.budget),
        SComp::Pinned(g, stem) => {
            comp_intersect(atoms, g, loose, Some(*stem), &mut q.comp_stack, q.budget)
        }
        SComp::Secret(top) => {
            if atoms_may_be_secret(atoms, *top, role) {
                Reach::Yes
            } else {
                Reach::No
            }
        }
    }
}

/// Whether pattern `p` can name a path of shape `s`. `keyed`: a component
/// matching every name only counts against a key when all later ones do too.
fn path_meets<'b>(
    p: &PathPat,
    s: &'b [(SComp, bool)],
    role: Role,
    keyed: bool,
    q: &mut Query<'_, 'b>,
) -> Reach {
    let (n, m) = (p.comps.len(), s.len());
    let s_any = s.iter().any(|c| matches!(c.0, SComp::Any));
    if !p.globstar && !s_any && n != m {
        return Reach::No;
    }
    let mut start = 0usize;
    while start < n && start < m {
        match (&p.comps[start], &s[start].0) {
            (PComp::Pat { lit: Some(pl), .. }, SComp::Lit(sl)) => {
                if pl != sl {
                    return if q.budget.take(start as u64 + 1) {
                        Reach::No
                    } else {
                        Reach::Undecided
                    };
                }
                start += 1;
            }
            _ => break,
        }
    }
    if start == n && start == m {
        return if q.budget.take(start as u64 + 1) {
            Reach::Yes
        } else {
            Reach::Undecided
        };
    }
    if start > 0 && !q.budget.take(start as u64) {
        return Reach::Undecided;
    }
    let loose = p.dstar;
    let w = m - start + 1;
    let cells = (n - start + 1).saturating_mul(w);
    let mut seen = Seen::new(cells);
    q.path_stack.clear();
    q.path_stack.push((start, start));
    let mut undecided = false;
    while let Some((i, j)) = q.path_stack.pop() {
        if !seen.insert((i - start) * w + (j - start)) {
            continue;
        }
        if !q.budget.take(1) {
            return Reach::Undecided;
        }
        if i == n && j == m {
            return Reach::Yes;
        }
        if i < n && matches!(p.comps[i], PComp::GlobStar) {
            q.path_stack.push((i + 1, j));
        }
        if j < m && matches!(s[j].0, SComp::Any) {
            q.path_stack.push((i, j + 1));
        }
        if i >= n || j >= m {
            continue;
        }
        let (ni, pstay, pcatch) = match &p.comps[i] {
            PComp::GlobStar => (i, true, true),
            PComp::Pat { catchall, .. } => (i + 1, false, *catchall),
        };
        let (nj, sstay) = match s[j].0 {
            SComp::Any => (j, true),
            _ => (j + 1, false),
        };
        if pstay && sstay {
            continue;
        }
        if keyed && s[j].1 && pcatch && !p.tail_catchall[i + 1] {
            continue;
        }
        match comp_meets(&p.comps[i], &s[j].0, role, loose, q) {
            Reach::Yes => q.path_stack.push((ni, nj)),
            Reach::Undecided => undecided = true,
            Reach::No => {}
        }
    }
    if undecided {
        Reach::Undecided
    } else {
        Reach::No
    }
}

/// What the glob check found for a pattern.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GlobHit {
    Hit(&'static str),
    /// Out of budget: the caller asks.
    Undecided,
    Miss,
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

const DOWNLOADERS: &[&str] = &["curl", "wget", "fetch"];
const PRIV: &[&str] = &["sudo", "sudoedit", "su", "doas", "pkexec", "run0"];

fn is_interpreter(cmd: &str) -> bool {
    let b = base(cmd);
    is_shell(b)
        || b.starts_with("python")
        || matches!(
            b,
            "perl" | "ruby" | "node" | "deno" | "bun" | "php" | "lua" | "osascript" | "pwsh"
        )
}

pub(crate) fn reads_stdin_as_code(eff: &[String]) -> bool {
    reads_stdin_as_code_at(eff, 0)
}

fn reads_stdin_as_code_at(eff: &[String], depth: u8) -> bool {
    let Some(first) = eff.first() else {
        return false;
    };
    if depth > 4 {
        return true;
    }
    let b = base(first);
    let args = &eff[1..];
    if shell::reads_stdin_as_script(eff) {
        return true;
    }
    if matches!(b, "source" | ".") {
        return args
            .first()
            .is_some_and(|a| matches!(a.as_str(), "/dev/stdin" | "/dev/fd/0" | "-"));
    }
    // `| s$(echo h)`: a word built by running something may name a shell;
    // `| $X -s`: a command named by a variable, told to read a script
    // a word built by a substitution that names a program plainly is read
    // as that program (`$(which python3)`: an interpreter, round 6 item 6)
    if !is_interpreter(b) && shell::has_command_substitution(first) {
        return match shell::substituted_program(first) {
            None => true,
            Some(name) => {
                // then the wrappers in front of what it names (`$(which env)
                // sh`, `$(which timeout) 5 bash`: round 7 item 5)
                let mut e = eff.to_vec();
                e[0] = name;
                let (u, unknown) = shell::unwrap_checked(&e);
                unknown || (!u.is_empty() && u != eff && reads_stdin_as_code_at(&u, depth + 1))
            }
        };
    }
    if !is_interpreter(b) && shell::maybe_shell(first) {
        return args.iter().any(|a| a == "-" || a == "-s");
    }
    if b == "eval" || !is_interpreter(b) {
        return false;
    }
    if args.iter().any(|a| a == "-" || a == "-s") {
        return true;
    }
    // For a shell only `-c` means code follows; `-e`, `-E`, `-m` are options.
    let shell = is_shell(b);
    let code_flag = args.iter().any(|a| {
        a.starts_with('-')
            && !a.starts_with("--")
            && a.len() > 1
            && a[1..]
                .chars()
                .any(|c| c == 'c' || (!shell && matches!(c, 'e' | 'E' | 'm')))
            || (!shell && matches!(a.as_str(), "--eval" | "--command" | "--print"))
    });
    if code_flag {
        // `sh -c bash`, `script -c bash f`, `flock -c bash l` (unwrapped to
        // `sh -c …`): the code given may itself read the script on stdin
        // (round 8 blocker 3)
        return shell
            && shell::shell_c_codes(eff).iter().any(|code| {
                shell::lex(code)
                    .units
                    .iter()
                    .any(|u| u.readings().any(|e| reads_stdin_as_code_at(e, depth + 1)))
            });
    }
    // `bash -o pipefail`: the option's value isn't a script
    let mut positional = false;
    let mut k = 0;
    while k < args.len() {
        let a = args[k].as_str();
        if shell && matches!(a, "-o" | "+o" | "-O" | "+O") {
            k += 2;
            continue;
        }
        if !a.starts_with('-') && !a.starts_with('+') {
            positional = true;
        }
        k += 1;
    }
    !positional
}

fn is_executor(eff: &[String]) -> bool {
    eff.first().is_some_and(|f| {
        let b = base(f);
        is_interpreter(b) || matches!(b, "eval" | "source" | ".")
    })
}

fn contains_downloader(body: &str) -> bool {
    shell::lex(body).units.iter().any(|u| {
        u.eff
            .first()
            .is_some_and(|f| DOWNLOADERS.contains(&base(f)))
    })
}

fn force_push(eff: &[String]) -> bool {
    if eff.first().map(|f| base(f)) != Some("git") {
        return false;
    }
    // skip git's own options up to the subcommand
    let mut i = 1;
    while i < eff.len() {
        let w = eff[i].as_str();
        if matches!(
            w,
            "-C" | "-c"
                | "--git-dir"
                | "--work-tree"
                | "--namespace"
                | "--super-prefix"
                | "--config-env"
                | "--exec-path"
        ) {
            i += 2;
        } else if w.starts_with('-') {
            i += 1;
        } else {
            break;
        }
    }
    if eff.get(i).map(String::as_str) != Some("push") {
        return false;
    }
    let mut force = false;
    let mut positional: Vec<&str> = vec![];
    let mut implicit = false;
    let mut after_dd = false;
    let mut j = i + 1;
    while j < eff.len() {
        let w = eff[j].as_str();
        j += 1;
        if after_dd || !w.starts_with('-') || w == "-" {
            positional.push(w);
            continue;
        }
        if w == "--" {
            after_dd = true;
        } else if w.starts_with("--") {
            let name = w.split('=').next().unwrap_or(w);
            // git takes unique prefixes of long options (`--force-w`, `--mirr`)
            let pfx = |full: &str| name.len() >= 4 && full.starts_with(name);
            if [
                "--force",
                "--force-with-lease",
                "--force-if-includes",
                "--mirror",
            ]
            .iter()
            .any(|f| pfx(f))
            {
                force = true;
            } else if ["--all", "--tags", "--branches", "--prune"]
                .iter()
                .any(|f| pfx(f))
            {
                implicit = true;
            } else if matches!(name, "--push-option" | "--receive-pack" | "--exec")
                && !w.contains('=')
            {
                j += 1;
            }
        } else {
            if w[1..].contains('f') {
                force = true;
            }
            if w == "-o" {
                j += 1;
            }
        }
    }
    let refspecs: &[&str] = if positional.len() > 1 {
        &positional[1..]
    } else {
        &[]
    };
    let check = |spec: &str| -> bool {
        // true = blocked
        let s = spec.strip_prefix('+').unwrap_or(spec);
        let dst = match s.split_once(':') {
            Some((_, d)) => d,
            None => s,
        };
        if dst.is_empty() || dst == "HEAD" || dst.contains(['*', '?', '[']) {
            return true;
        }
        let branch = match dst.strip_prefix("refs/heads/") {
            Some(b) => b,
            None if dst.starts_with("refs/") => return true,
            None => dst,
        };
        matches!(branch, "main" | "master" | "dev" | "develop" | "trunk")
            || branch.starts_with("release/")
    };
    if force {
        return implicit || refspecs.is_empty() || refspecs.iter().any(|s| check(s));
    }
    refspecs.iter().any(|s| s.starts_with('+') && check(s))
}

fn rm_recursive(eff: &[String]) -> Option<Vec<String>> {
    if eff.first().map(|f| base(f)) != Some("rm") {
        return None;
    }
    let mut rec = false;
    let mut targets = vec![];
    let mut dd = false;
    for w in &eff[1..] {
        if dd || !w.starts_with('-') || w == "-" {
            targets.push(w.clone());
        } else if w == "--" {
            dd = true;
        } else if (w.starts_with("--")
            && w.len() >= 3
            && "--recursive".starts_with(w.split('=').next().unwrap_or(w)))
            || (!w.starts_with("--") && w[1..].contains(['r', 'R']))
        {
            rec = true;
        }
    }
    rec.then_some(targets)
}

/// `find … -delete`: its roots, and whether the expression has any filter.
fn find_delete(eff: &[String]) -> Option<(Vec<String>, bool)> {
    if eff.first().map(|f| base(f)) != Some("find") || !eff.iter().any(|a| a == "-delete") {
        return None;
    }
    let (roots, j) = shell::find_roots(eff);
    let mut filtered = false;
    let mut k = j;
    while k < eff.len() {
        match eff[k].as_str() {
            "-delete" | "-depth" => {}
            "-maxdepth" | "-mindepth" => k += 1,
            _ => filtered = true,
        }
        k += 1;
    }
    Some((roots, filtered))
}

struct Ctx<'a> {
    input: &'a PolicyInput<'a>,
    a: &'a Analysis,
    s: Sensitive,
}

impl Ctx<'_> {
    fn home(&self) -> &Path {
        self.input.home
    }

    fn is_home_or_ancestor(&self, p: &Path) -> bool {
        let pl = lower(p);
        under(&lower(self.home()), &pl).is_some()
    }

    /// Resolutions of `word` as used by unit `i` (ours and the gate's).
    fn target_paths(&self, i: usize, word: &str) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut found = false;
        if let Some(idxs) = self.a.use_index.get(&(i, word.to_string())) {
            found = true;
            for &idx in idxs {
                out.extend(self.a.all_paths(idx).into_iter().cloned());
            }
        }
        if !found {
            let cwd = self
                .a
                .unit_cwd
                .get(i)
                .cloned()
                .unwrap_or_else(|| CwdState::Known(vec![self.a.start.clone()]));
            for c in cwd.candidates() {
                out.push(lexical_resolve(word, c, self.home()));
            }
        }
        out
    }

    fn delete_blocked(&self, i: usize, targets: &[String], filtered_find: Option<bool>) -> bool {
        let cwd = self.input.cwd;
        let unknown = self.a.unit_cwd.get(i).is_some_and(CwdState::is_unknown);
        let start_l = lower(&self.a.start);
        let mut seen = std::collections::HashSet::new();
        for t in targets {
            if !seen.insert(t.as_str()) {
                continue;
            }
            let relative = {
                let e = words::expand_home(t, self.home(), cwd);
                !Path::new(&e).is_absolute()
            };
            if unknown && relative {
                return true;
            }
            // `~name` for anyone but the user: can't tell where it points
            if words::expand_home(t, self.home(), cwd).starts_with('~') {
                return true;
            }
            let globby = t.contains(['*', '?', '[']);
            for p in self.target_paths(i, t) {
                let pl = lower(&p);
                if pl == "/" || self.is_home_or_ancestor(&p) {
                    return true;
                }
                if globby {
                    let parent = p.parent().unwrap_or(Path::new("/"));
                    if self.is_home_or_ancestor(parent) {
                        return true;
                    }
                    if p.file_name().is_some_and(|n| n == ".*") {
                        return true;
                    }
                }
                // outside the folder, an ancestor of it, or the folder itself
                let cwd_l = lower(cwd);
                match under(&pl, &cwd_l) {
                    None => return true,
                    Some("") => {
                        // the folder itself; `find … -name x -delete` only
                        // removes matches
                        if filtered_find != Some(true) {
                            return true;
                        }
                    }
                    Some(_) => {}
                }
                let _ = &start_l;
            }
        }
        false
    }

    fn check(&self) -> Vec<&'static str> {
        let mut hits: Vec<&'static str> = vec![];
        let a = self.a;

        // paths, per use
        for (idx, u) in a.uses.iter().enumerate() {
            let globby = u.word.contains(['*', '?', '[']);
            for p in a.all_paths(idx) {
                if let Some(h) = self.s.hit(p, u.role) {
                    hits.push(h);
                }
                if self.s.is_ssh_dir(p) && self.ssh_dir_hit(u) {
                    hits.push("credential-read");
                }
                if globby && let Some(h) = self.s.glob_hit(p, u.role) {
                    hits.push(h);
                }
            }
        }
        // the gate's flat list (symlinks resolved), when it didn't line up
        // (which word each path belongs to is lost, so each is checked with
        // the strongest role any word of the call has)
        if a.aligned.is_none() {
            let role = if a.uses.iter().any(|u| u.role == Role::Write) {
                Role::Write
            } else if a.uses.iter().any(|u| u.role == Role::MaybeWrite) {
                Role::MaybeWrite
            } else {
                Role::Read
            };
            for p in self.input.resolved_paths {
                if let Some(h) = self.s.hit(p, role) {
                    hits.push(h);
                }
            }
        }
        // raw text
        for t in &a.raw {
            if let Some(h) = self.s.raw_hit(t) {
                hits.push(h);
            }
            if raw_pipe_to_shell(t) {
                hits.push("pipe-to-shell");
            }
        }
        // search globs that name credential files
        if let ToolSubject::Paths {
            paths,
            glob: Some(g),
            recursive,
            ..
        } = &self.input.check.subject
        {
            hits.extend(self.search_glob_hit(paths, g, *recursive));
        }
        // a search glob that leaves its roots (M1-18, P1): each expansion
        // that leaves is checked as a path pattern of its own
        if let Some(esc) = &self.a.glob_escape {
            for p in &esc.patterns {
                let globby = p.to_string_lossy().contains(['*', '?', '[']);
                hits.extend(self.s.hit(p, Role::Read));
                if globby {
                    hits.extend(self.s.glob_hit(p, Role::Read));
                }
                if self.s.is_ssh_dir(p) {
                    hits.push("credential-read");
                }
            }
        }

        for (i, u) in a.units.iter().enumerate() {
            let eff = &u.eff;
            let Some(first) = eff.first() else { continue };
            let b = base(first);
            // privilege escalation
            if PRIV.contains(&b)
                || (b == "osascript"
                    && eff
                        .iter()
                        .any(|w| w.to_lowercase().contains("administrator privileges")))
            {
                hits.push("privilege-escalation");
            }
            // disk wipe
            if b.starts_with("mkfs") || b == "mke2fs" {
                hits.push("disk-wipe");
            }
            if b == "dd"
                && eff
                    .iter()
                    .any(|w| w.strip_prefix("of=").is_some_and(|v| self.is_device(i, v)))
            {
                hits.push("disk-wipe");
            }
            if matches!(b, "shred" | "wipefs" | "blkdiscard")
                && eff.iter().skip(1).any(|w| self.is_device(i, w))
            {
                hits.push("disk-wipe");
            }
            if b.starts_with("newfs") {
                hits.push("disk-wipe");
            }
            if b == "diskutil"
                && matches!(
                    eff.get(1).map(|s| s.to_lowercase()).as_deref(),
                    Some("apfs" | "cs" | "corestorage")
                )
                && eff.get(2).is_some_and(|v| {
                    let v = v.to_lowercase();
                    [
                        "delete", "erase", "reformat", "zero", "secure", "resize", "split", "merge",
                    ]
                    .iter()
                    .any(|p| v.starts_with(p))
                })
            {
                hits.push("disk-wipe");
            }
            if words::is_known_writer(eff)
                && !matches!(b, "chmod" | "chown" | "chgrp")
                && eff.iter().skip(1).any(|w| self.is_device(i, w))
            {
                hits.push("disk-wipe");
            }
            if b == "diskutil"
                && eff.get(1).is_some_and(|s| {
                    let s = s.to_lowercase();
                    s.starts_with("erase")
                        || s.starts_with("reformat")
                        || s.starts_with("partitiondisk")
                        || s.starts_with("secureerase")
                        || s.starts_with("zerodisk")
                        || s.starts_with("randomdisk")
                })
            {
                hits.push("disk-wipe");
            }
            for r in &u.seg.redirects {
                if r.op != super::RedirectOp::Read
                    && r.op != super::RedirectOp::Dup
                    && self.is_device(i, &r.target)
                {
                    hits.push("disk-wipe");
                }
            }
            // credential commands
            if b == "security"
                && eff.iter().skip(1).any(|w| {
                    // `security` takes unique subcommand prefixes
                    w.len() >= 6
                        && [
                            "find-generic-password",
                            "find-internet-password",
                            "dump-keychain",
                        ]
                        .iter()
                        .any(|f| f.starts_with(w.as_str()))
                })
            {
                hits.push("credential-read");
            }
            if b == "secret-tool" && eff.get(1).is_some_and(|s| s == "lookup") {
                hits.push("credential-read");
            }
            // force push
            if force_push(eff) {
                hits.push("force-push-protected");
            }
            // recursive delete
            if let Some(mut targets) = rm_recursive(eff) {
                if let Some(roots) = &u.exec_roots {
                    targets = targets
                        .into_iter()
                        .flat_map(|t| if t == "{}" { roots.clone() } else { vec![t] })
                        .collect();
                }
                if self.delete_blocked(i, &targets, None) {
                    hits.push("recursive-delete-outside-cwd");
                }
            }
            if let Some((roots, filtered)) = find_delete(eff)
                && self.delete_blocked(i, &roots, Some(filtered))
            {
                hits.push("recursive-delete-outside-cwd");
            }
            // pipe-to-shell
            if DOWNLOADERS.contains(&b) && u.pipe != 0 {
                // in any reading of its wrappers (M1-18, K1)
                let downstream = a.units.iter().enumerate().any(|(j, v)| {
                    j > i
                        && v.pipe == u.pipe
                        && v.seg.piped
                        && v.readings().any(reads_stdin_as_code)
                });
                if downstream {
                    hits.push("pipe-to-shell");
                }
            }
            if u.readings().any(is_executor) && u.subs.iter().any(|s| contains_downloader(s)) {
                hits.push("pipe-to-shell");
            }
        }
        hits
    }

    /// `~/.ssh` itself: listing it is fine; searching, copying or writing into
    /// it is not.
    fn ssh_dir_hit(&self, u: &Use) -> bool {
        const LISTERS: &[&str] = &[
            "ls", "stat", "cd", "pushd", "popd", "pwd", "test", "[", "du", "dir", "readlink",
            "realpath", "basename", "dirname", "echo", "printf", "chmod", "chown", "chgrp",
            "mkdir", "rmdir",
        ];
        match self.a.units.get(u.unit) {
            Some(unit) => unit
                .eff
                .first()
                .map(|f| base(f))
                .is_some_and(|c| !LISTERS.contains(&c)),
            None => {
                let k = self.input.check.kind;
                u.role == Role::Write
                    || (k == ToolKind::Search
                        && matches!(
                            &self.input.check.subject,
                            ToolSubject::Paths {
                                recursive: true,
                                ..
                            }
                        ))
            }
        }
    }

    fn is_device(&self, i: usize, word: &str) -> bool {
        self.target_paths(i, word).iter().any(|p| {
            let s = lower(p);
            s.starts_with("/dev/")
                && !matches!(
                    s.as_str(),
                    "/dev/null"
                        | "/dev/tty"
                        | "/dev/stdout"
                        | "/dev/stderr"
                        | "/dev/stdin"
                        | "/dev/zero"
                )
                && !s.starts_with("/dev/fd/")
        })
    }

    fn search_glob_hit(
        &self,
        roots: &[String],
        glob: &str,
        recursive: bool,
    ) -> Option<&'static str> {
        let owned;
        let (variants, unbounded) = match &self.a.glob_search {
            Some((v, u)) => (v.as_slice(), *u),
            None => {
                let resolved_roots: Vec<PathBuf> = if roots.is_empty() {
                    vec![self.input.cwd.to_path_buf()]
                } else {
                    roots
                        .iter()
                        .take(words::MAX_USES)
                        .map(|r| lexical_resolve(r, self.input.cwd, self.home()))
                        .collect()
                };
                owned = glob_search_variants(&resolved_roots, glob, self.input.cwd, self.home());
                (owned.0.as_slice(), owned.1)
            }
        };
        if unbounded {
            return None;
        }
        let matchers: Vec<globset::GlobMatcher> = variants
            .iter()
            .filter_map(|norm| {
                GlobBuilder::new(norm)
                    .literal_separator(false)
                    .build()
                    .ok()
                    .map(|g| g.compile_matcher())
            })
            .collect();
        if matchers.is_empty() {
            return None;
        }
        let root_lowers: Vec<String> = if roots.is_empty() {
            vec![lower(self.input.cwd)]
        } else {
            roots
                .iter()
                .map(|r| lower(&lexical_resolve(r, self.input.cwd, self.home())))
                .collect()
        };
        for probe in self.s.probes() {
            let pp = Path::new(&probe);
            let name = pp.file_name()?.to_string_lossy().to_string();
            let Some(hit) = self.s.hit(pp, Role::Read) else {
                continue;
            };
            for rl in &root_lowers {
                let Some(rel) = under(&probe, rl) else {
                    continue;
                };
                let name_allowed = recursive || !rel.contains('/');
                if matchers.iter().any(|g| {
                    (name_allowed && g.is_match(&name))
                        || g.is_match(&probe)
                        || (!rel.is_empty() && g.is_match(rel))
                }) {
                    return Some(hit);
                }
            }
        }
        // D-131 (owner M1-31 to confirm): the filter through the shared matcher.
        self.s.search_filter_hit(variants)
    }
}

/// What a search subject's `glob` can name outside every path the subject
/// gives (M1-18, P1). `patterns`: each brace expansion that leaves, made
/// absolute (`/h/u/.ssh/id_*`); `unknown`: a form whose reach can't be read
/// (a backslash, `~user`, a component made of classes and `?` that can be
/// `.` or `..`, `$VAR`, too many expansions).
#[derive(Debug, Default)]
pub struct GlobEscape {
    pub patterns: Vec<PathBuf>,
    pub unknown: bool,
    pub unbounded: bool,
}

impl GlobEscape {
    pub fn leaves(&self) -> bool {
        self.unbounded || self.unknown || !self.patterns.is_empty()
    }
}

/// A pattern component that can match `.` or `..` without being written
/// so (`[.][.]`, `.[.]`, `??`, `[.][.]*`). Plain `*` / `**` are left out:
/// a directory walk never lists `.` or `..` for `*`.
fn class_may_match_dot(class: &str) -> bool {
    let inner = &class[1..class.len() - 1];
    if !inner.contains(['!', '^', '.', '\\'])
        && !inner
            .as_bytes()
            .windows(3)
            .any(|w| w[1] == b'-' && w[0] <= b'.' && b'.' <= w[2])
    {
        return false;
    }
    GlobBuilder::new(class)
        .literal_separator(true)
        .build()
        .map_or(true, |g| g.compile_matcher().is_match("."))
}

fn may_be_dot_component(c: &str) -> bool {
    if c == "*" || c == "**" || !c.contains(['[', '?']) {
        return false;
    }
    if !c.contains('?')
        && c.starts_with('[')
        && c.ends_with(']')
        && !c[1..c.len() - 1].contains(['[', ']'])
        && !class_may_match_dot(c)
    {
        return false;
    }
    let Ok(g) = GlobBuilder::new(c).literal_separator(true).build() else {
        return true;
    };
    let m = g.compile_matcher();
    m.is_match(".") || m.is_match("..")
}

fn unescape_glob_backslashes(g: &str) -> String {
    let mut out = String::with_capacity(g.len());
    let mut chars = g.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(next) = chars.next() {
                out.push(next);
            } else {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Normalizes `./`, `//`, and `literal/..` steps in a glob pattern while
/// keeping `**` and wildcard components intact.
const MAX_GLOB_VARIANTS: usize = 256;

/// Single normalizer for both `glob_escape` and `search_glob_hit` /
/// `ask_search_glob_hit`: collapses curdir, empty segments, and `literal/..`,
/// and treats `..` after a single-component wildcard as possibly popping that
/// segment (emitting both popped and as-written forms).
fn apply_dotdot<'a>(
    mut st: Vec<&'a str>,
    abs: bool,
    out: &mut Vec<Vec<&'a str>>,
    cap: usize,
) -> bool {
    if out.len() > cap.saturating_mul(4).max(16) {
        out.sort_unstable();
        out.dedup();
        if out.len() > cap {
            return false;
        }
    }
    match st.last().copied() {
        None => {
            if !abs {
                st.push("..");
            }
            out.push(st);
        }
        Some("..") => {
            st.push("..");
            out.push(st);
        }
        Some("**") => {
            // `**` matches >= 2 segments: absorbs `..` and stays `**`.
            out.push(st.clone());
            // `**` matches 1 segment: cancelled by `..`.
            st.pop();
            out.push(st.clone());
            // `**` matches 0 segments: `..` acts on what preceded `**`.
            if !apply_dotdot(st, abs, out, cap) {
                return false;
            }
        }
        Some(last) if !last.contains(['*', '?', '[']) => {
            st.pop();
            out.push(st);
        }
        Some(_) => {
            let mut popped = st.clone();
            popped.pop();
            out.push(popped);
            st.push("..");
            out.push(st);
        }
    }
    true
}

fn normalize_glob_variants(g: &str, cap: usize) -> Option<Vec<String>> {
    let abs = g.starts_with('/');
    let mut states: Vec<Vec<&str>> = vec![vec![]];
    for c in g.split('/') {
        match c {
            "" | "." => {}
            "**" => {
                for st in &mut states {
                    if st.last().copied() != Some("**") {
                        st.push(c);
                    }
                }
                states.sort_unstable();
                states.dedup();
            }
            ".." => {
                let mut next = Vec::with_capacity(states.len());
                for st in states {
                    if !apply_dotdot(st, abs, &mut next, cap) {
                        return None;
                    }
                }
                next.sort_unstable();
                next.dedup();
                if next.len() > cap {
                    return None;
                }
                states = next;
            }
            _ => {
                for st in &mut states {
                    st.push(c);
                }
            }
        }
    }
    let mut out = Vec::with_capacity(states.len());
    for st in states {
        let s = if abs {
            format!("/{}", st.join("/"))
        } else if st.is_empty() {
            ".".to_string()
        } else {
            st.join("/")
        };
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Some(out)
}

/// Widens `?` and any character class `[...]` that can match `.` to `.`.
fn widen_pattern_dots(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let ch = s[i..].chars().next().unwrap();
        let len = ch.len_utf8();
        if ch == '?' {
            out.push('.');
            i += len;
        } else if ch == '[' {
            let mut j = i + 1;
            if j < bytes.len() && (bytes[j] == b'!' || bytes[j] == b'^') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b']' {
                j += 1;
            }
            while j < bytes.len() && bytes[j] != b']' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() && bytes[j] == b']' {
                let class = &s[i..=j];
                if class_may_match_dot(class) {
                    out.push('.');
                } else {
                    out.push_str(class);
                }
                i = j + 1;
            } else {
                out.push(ch);
                i += len;
            }
        } else {
            out.push(ch);
            i += len;
        }
    }
    out
}

/// If a `[...]` class can contain or span `/`, returns a variant replacing it
/// with `/`.
fn widen_pattern_slashes(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    let mut i = 0;
    while i < bytes.len() {
        let ch = s[i..].chars().next().unwrap();
        let len = ch.len_utf8();
        if ch == '[' {
            let mut j = i + 1;
            if j < bytes.len() && (bytes[j] == b'!' || bytes[j] == b'^') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b']' {
                j += 1;
            }
            while j < bytes.len() && bytes[j] != b']' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() && bytes[j] == b']' {
                let class = &s[i..=j];
                let inner = &s[i + 1..j];
                let has_slash = inner.contains('/')
                    || inner
                        .as_bytes()
                        .windows(3)
                        .any(|w| w[1] == b'-' && w[0] <= b'/' && b'/' <= w[2]);
                if has_slash {
                    out.push('/');
                    changed = true;
                } else {
                    out.push_str(class);
                }
                i = j + 1;
                continue;
            }
        }
        out.push(ch);
        i += len;
    }
    changed.then_some(out)
}

/// Widens `$VAR` / `${VAR}` in a glob as possibly an empty string, `**`, or
/// any absolute path `/**`.
fn expand_glob_vars(s: &str) -> Vec<String> {
    if !s.contains('$') {
        return vec![];
    }
    let mut as_globstar = String::new();
    let mut as_empty = String::new();
    let mut first_abs: Option<String> = None;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' {
            let end = if i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                s[i + 2..].find('}').map_or(bytes.len(), |k| i + 2 + k + 1)
            } else {
                let mut k = i + 1;
                while k < bytes.len() && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_') {
                    k += 1;
                }
                k
            };
            as_globstar.push_str("**");
            if first_abs.is_none() {
                let rest = &s[end..];
                first_abs = Some(if rest.starts_with('/') || rest.is_empty() {
                    format!("/**{rest}")
                } else {
                    format!("/**/{rest}")
                });
            }
            i = end;
        } else {
            let ch = s[i..].chars().next().unwrap();
            as_globstar.push(ch);
            as_empty.push(ch);
            i += ch.len_utf8();
        }
    }
    let mut out = vec![as_globstar, as_empty];
    if let Some(a) = first_abs {
        out.push(a);
    }
    out
}

#[derive(Default)]
struct GlobCandidates {
    variants: Vec<String>,
    unknown: bool,
    unbounded: bool,
}

/// Shared brace-expansion, home-expansion, and dot/slash/`$VAR` widening,
/// capped at `MAX_GLOB_VARIANTS` without sampling on overflow.
fn expand_glob_candidates(glob: &str, cwd: &Path, home: &Path) -> GlobCandidates {
    let mut res = GlobCandidates::default();
    if glob.contains('\0') {
        res.unbounded = true;
        return res;
    }
    let mut raw = vec![glob.to_string()];
    if glob.contains('\\') {
        res.unknown = true;
        raw.push(unescape_glob_backslashes(glob));
    }
    let mut expansions = vec![];
    for r in raw {
        if let Some(exps) = words::brace_expand(&r) {
            expansions.extend(exps);
        } else {
            res.unknown = true;
            res.unbounded = true;
            res.variants.clear();
            return res;
        }
    }
    let mut seen = std::collections::HashSet::new();
    for e in expansions {
        if e.is_empty() {
            continue;
        }
        let h = words::expand_home(&e, home, cwd);
        if let Some(rest) = h.strip_prefix('~')
            && !rest.is_empty()
            && !rest.starts_with('/')
        {
            res.unknown = true;
            continue;
        }
        if h.split('/').any(may_be_dot_component) {
            res.unknown = true;
        }
        let mut bases = vec![h.clone()];
        if h.contains('$') {
            res.unknown = true;
            bases.extend(expand_glob_vars(&h));
        }
        let mut cands = vec![];
        for b in bases {
            if !cands.contains(&b) {
                cands.push(b.clone());
            }
            let dots = widen_pattern_dots(&b);
            if dots != b && !cands.contains(&dots) {
                cands.push(dots.clone());
            }
            if let Some(sl) = widen_pattern_slashes(&b) {
                res.unknown = true;
                if !cands.contains(&sl) {
                    cands.push(sl.clone());
                }
                let sl_dots = widen_pattern_dots(&sl);
                if sl_dots != sl && !cands.contains(&sl_dots) {
                    cands.push(sl_dots);
                }
            }
            if dots != b
                && let Some(dots_sl) = widen_pattern_slashes(&dots)
            {
                res.unknown = true;
                if !cands.contains(&dots_sl) {
                    cands.push(dots_sl);
                }
            }
        }
        for cand in cands {
            if cand.is_empty() {
                continue;
            }
            if seen.insert(cand.clone()) {
                if res.variants.len() >= MAX_GLOB_VARIANTS {
                    res.unknown = true;
                    res.unbounded = true;
                    res.variants.clear();
                    return res;
                }
                res.variants.push(cand);
            }
        }
    }
    res
}

fn search_variants_from_cands(
    roots: &[PathBuf],
    cwd: &Path,
    home: &Path,
    cands: &GlobCandidates,
) -> (Vec<String>, bool) {
    if cands.unbounded {
        return (vec![], true);
    }
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut unjoined_norm = 0usize;
    let mut joined_norm = 0usize;
    let mut push_norms = |pat: &str, total: &mut usize| -> bool {
        let remaining = MAX_GLOB_VARIANTS.saturating_sub(*total);
        if remaining == 0 {
            return false;
        }
        let Some(norms) = normalize_glob_variants(pat, remaining) else {
            return false;
        };
        *total += norms.len();
        if *total > MAX_GLOB_VARIANTS {
            return false;
        }
        for n in norms {
            let norm = n.to_lowercase().replace('ſ', "s");
            if !norm.is_empty() && seen.insert(norm.clone()) {
                out.push(norm);
            }
        }
        true
    };
    for gv in &cands.variants {
        if gv.is_empty() {
            continue;
        }
        if !push_norms(gv, &mut unjoined_norm) {
            return (vec![], true);
        }
        let anchored = gv.starts_with('/') || gv.starts_with('~');
        if !anchored {
            for r in roots {
                let joined = format!("{}/{}", r.to_string_lossy().trim_end_matches('/'), gv);
                if !push_norms(&joined, &mut joined_norm) {
                    return (vec![], true);
                }
            }
        } else {
            let expanded = words::expand_home(gv, home, cwd);
            if expanded != *gv && !push_norms(&expanded, &mut joined_norm) {
                return (vec![], true);
            }
        }
    }
    (out, false)
}

fn glob_search_variants(
    roots: &[PathBuf],
    glob: &str,
    cwd: &Path,
    home: &Path,
) -> (Vec<String>, bool) {
    let cands = expand_glob_candidates(glob, cwd, home);
    search_variants_from_cands(roots, cwd, home, &cands)
}

fn escape_from_cands(
    roots: &[PathBuf],
    cwd: &Path,
    home: &Path,
    cands: &GlobCandidates,
) -> GlobEscape {
    let mut out = GlobEscape {
        patterns: vec![],
        unknown: cands.unknown,
        unbounded: cands.unbounded,
    };
    if cands.unbounded {
        return out;
    }
    let mut total_norm = 0usize;
    for gv in &cands.variants {
        check_escape_variant(gv, roots, cwd, home, &mut total_norm, &mut out);
        if out.unbounded {
            break;
        }
    }
    out
}

fn check_escape_variant(
    variant: &str,
    roots: &[PathBuf],
    cwd: &Path,
    home: &Path,
    total_norm: &mut usize,
    out: &mut GlobEscape,
) {
    if variant.is_empty() {
        return;
    }
    let is_pat = |c: &str| c.contains(['*', '?', '[']);
    let anchored = variant.starts_with('/') || variant.starts_with('~');
    let raw_joined: Vec<String> = if anchored {
        vec![words::expand_home(variant, home, cwd)]
    } else {
        roots
            .iter()
            .map(|r| format!("{}/{}", r.to_string_lossy().trim_end_matches('/'), variant))
            .collect()
    };
    for j in raw_joined {
        let remaining = MAX_GLOB_VARIANTS.saturating_sub(*total_norm);
        if remaining == 0 {
            out.unbounded = true;
            out.unknown = true;
            return;
        }
        let Some(norms) = normalize_glob_variants(&j, remaining) else {
            out.unbounded = true;
            out.unknown = true;
            return;
        };
        *total_norm += norms.len();
        if *total_norm > MAX_GLOB_VARIANTS {
            out.unbounded = true;
            out.unknown = true;
            return;
        }
        for n in norms {
            let abs = PathBuf::from(&n);
            // the folder the walk starts from: the literal components
            let mut start = PathBuf::new();
            for c in abs.components() {
                if is_pat(&c.as_os_str().to_string_lossy()) {
                    break;
                }
                start.push(c.as_os_str());
            }
            let inside = roots.iter().any(|r| start.starts_with(r));
            if !inside && !out.patterns.contains(&abs) {
                if out.patterns.len() >= MAX_GLOB_VARIANTS {
                    out.unbounded = true;
                    out.unknown = true;
                    return;
                }
                out.patterns.push(abs);
            }
        }
    }
}

/// Words split on blanks, keeping a `$(…)`, `` `…` `` or quoted part whole
/// (`$(brew --prefix)/bin/jq` is one word), for the raw text check.
fn raw_words(t: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut tick = false;
    let mut prev = ' ';
    for c in t.chars() {
        match c {
            _ if quote.is_some() => {
                if Some(c) == quote {
                    quote = None;
                }
            }
            '\'' | '"' => quote = Some(c),
            '`' => tick = !tick,
            '(' if prev == '$' || depth > 0 => depth += 1,
            ')' if depth > 0 => depth -= 1,
            c if c.is_whitespace() && depth == 0 && !tick => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                prev = c;
                continue;
            }
            _ => {}
        }
        cur.push(c);
        prev = c;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `curl … | sh` as text, for lines the lexer got wrong.
fn raw_pipe_to_shell(t: &str) -> bool {
    let l = t.to_lowercase();
    for dl in ["curl", "wget"] {
        let mut rest = l.as_str();
        while let Some(i) = rest.find(dl) {
            rest = &rest[i + dl.len()..];
            let seg: &str = rest.split([';', '\n', '&']).next().unwrap_or("");
            if let Some((_, after)) = seg.split_once('|') {
                let w = raw_words(after);
                let eff: Vec<String> = shell::unwrap(&w);
                if reads_stdin_as_code(&eff) {
                    return true;
                }
            }
        }
    }
    false
}

/// The first `HARD_DENY_DEFAULTS` entry (in list order) that the call hits.
pub fn hard_deny(input: &PolicyInput<'_>, a: &Analysis) -> Option<&'static str> {
    let ctx = Ctx {
        input,
        a,
        s: Sensitive::new(input),
    };
    let hits = ctx.check();
    HARD_DENY_DEFAULTS
        .iter()
        .map(|d| d.id)
        .find(|id| hits.contains(id))
}

pub fn sensitive(input: &PolicyInput<'_>) -> Sensitive {
    Sensitive::new(input)
}

/// The credential stores that ask (CR-12), absolute: files and folders
/// (`~/.azure/msal_token_cache.*` are found by listing `~/.azure`).
pub fn ask_roots(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ASK_FILES
        .iter()
        .chain(ASK_TREES)
        .map(|r| home.join(r))
        .collect();
    for (dir, pre) in ASK_NAME_PREFIXES {
        if let Ok(rd) = std::fs::read_dir(home.join(dir)) {
            for e in rd.flatten().take(256) {
                if e.file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .starts_with(pre)
                {
                    v.push(e.path());
                }
            }
        }
    }
    v
}

/// Every credential, `~/.ssh`, browser and Atrium location, absolute.
pub fn sensitive_roots(home: &Path, data: &[PathBuf], protected: &[PathBuf]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = vec![home.join(".ssh")];
    for rel in CRED_FILES.iter().chain(CRED_TREES).chain(BROWSER_ROOTS) {
        v.push(home.join(rel));
    }
    v.extend(data.iter().cloned());
    v.extend(protected.iter().cloned());
    v
}
