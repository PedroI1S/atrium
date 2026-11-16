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

    /// Representative sensitive paths a glob could be hitting.
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
            v.push(format!("{d}/atrium.db"));
        }
        v
    }

    /// A word with glob characters: does the pattern reach a sensitive path?
    fn glob_hit(&self, pattern: &Path, role: Role) -> Option<&'static str> {
        let pl = lower(pattern);
        // literal directory prefix
        let mut prefix = PathBuf::new();
        for c in Path::new(&pl).components() {
            let s = c.as_os_str().to_string_lossy();
            if s.contains(['*', '?', '[']) {
                break;
            }
            prefix.push(c.as_os_str());
        }
        if let Some(h) = self.hit(&prefix, role)
            && prefix.as_os_str() != "/"
        {
            return Some(h);
        }
        let glob = ShellGlob::new(&pl);
        let matches_dir_list = |dirs: &[String]| {
            dirs.iter().any(|d| {
                glob.matches_tree(d)
                    || glob.matches_exact(d)
                    || glob.matches_exact(&format!("{d}/x"))
                    || glob.matches_exact(&format!("{d}/atrium.db"))
            })
        };
        if matches_dir_list(&self.data) || (role != Role::Read && matches_dir_list(&self.protected))
        {
            return Some("atrium-self");
        }
        let home_tails = glob.tails_after(&self.home);
        if home_tails.is_empty() {
            return None;
        }
        for f in CRED_FILES {
            if home_tails.iter().any(|t| t.matches_exact(f)) {
                return Some("credential-read");
            }
        }
        for tree in CRED_TREES {
            if home_tails.iter().any(|t| {
                t.matches_tree(tree)
                    || t.matches_exact(tree)
                    || t.matches_exact(&format!("{tree}/x"))
            }) {
                return Some("credential-read");
            }
        }
        if home_tails.iter().any(|t| t.matches_ssh_secret(role))
            || SSH_PROBES.iter().any(|n| {
                let rel = format!(".ssh/{n}");
                home_tails.iter().any(|t| t.matches_exact(&rel))
            })
        {
            return Some("credential-read");
        }
        if let Some(last) = glob.parts.last() {
            let last_is_browser =
                matches!(last, Comp::GlobStar) || BROWSER_FILES.iter().any(|f| last.matches(f));
            if last_is_browser {
                for b in BROWSER_ROOTS {
                    if home_tails.iter().any(|t| {
                        t.matches_strictly_under(b)
                            || BROWSER_FILES.iter().any(|f| {
                                t.matches_exact(&format!("{b}/{f}"))
                                    || t.matches_exact(&format!("{b}/default/{f}"))
                            })
                    }) {
                        return Some("credential-read");
                    }
                }
            }
        }
        None
    }

    pub fn is_ssh_dir(&self, p: &Path) -> bool {
        let pl = lower(p);
        if pl == format!("{}/.ssh", self.home) {
            return true;
        }
        if !pl.contains(['*', '?', '[']) {
            return false;
        }
        let glob = ShellGlob::new(&pl);
        glob.tails_after(&self.home)
            .iter()
            .any(|t| t.matches_exact(".ssh"))
    }

    /// A credential store that asks (CR-12): `p` is one, or is inside one
    /// of its folders.
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
        if !pl.contains(['*', '?', '[']) {
            return false;
        }
        let glob = ShellGlob::new(&pl);
        let home_tails = glob.tails_after(&self.home);
        if home_tails.is_empty() {
            return false;
        }
        if ASK_FILES.iter().any(|f| {
            let fl = f.to_lowercase();
            home_tails.iter().any(|t| t.matches_exact(&fl))
        }) {
            return true;
        }
        if ASK_TREES.iter().any(|tr| {
            home_tails.iter().any(|t| {
                t.matches_tree(tr)
                    || t.matches_exact(tr)
                    || t.matches_exact(&format!("{tr}/x.json"))
            })
        }) {
            return true;
        }
        ASK_NAME_PREFIXES.iter().any(|(dir, pre)| {
            home_tails
                .iter()
                .any(|t| t.matches_dir_and_name_prefix(dir, pre))
        })
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
        // Prefix-named stores (`~/.azure/msal_token_cache.*`) with any extension.
        for (dir, pre) in ASK_NAME_PREFIXES {
            let dir_abs = format!("{}/{dir}", self.home);
            for rl in &root_lowers {
                let Some(dir_rel) = under(&dir_abs, rl) else {
                    continue;
                };
                for norm in variants {
                    if dir_rel.is_empty() {
                        if !norm.contains('/') {
                            if matches_ask_name_prefix_pat(norm, pre) {
                                return true;
                            }
                        } else if let Some((dir_pat, file_pat)) = norm.rsplit_once('/')
                            && (dir_pat == "**" || dir_pat == dir_abs)
                            && matches_ask_name_prefix_pat(file_pat, pre)
                        {
                            return true;
                        }
                        continue;
                    }
                    if let Some((dir_pat, file_pat)) = norm.rsplit_once('/') {
                        let dir_match = !dir_pat.is_empty()
                            && GlobBuilder::new(dir_pat)
                                .literal_separator(false)
                                .build()
                                .map_or(true, |g| {
                                    let m = g.compile_matcher();
                                    m.is_match(dir_rel) || m.is_match(&dir_abs)
                                });
                        if dir_match && matches_ask_name_prefix_pat(file_pat, pre) {
                            return true;
                        }
                    }
                }
            }
        }
        // Any search of an ASK_TREE with a filename filter (`.zsh_sessions/*.session`).
        // A generic `**` / `*` directory wildcard (`**/*.rs`) asks only when its
        // later components could match a file in an ask tree (`**/*.session`, `**/*`).
        const ASK_TREE_FILES: &[(&str, &[&str])] = &[
            (
                ".aws/sso/cache",
                &[
                    "0.json",
                    "a.json",
                    "x.json",
                    "cache.json",
                    "token.json",
                    "0",
                    "a",
                    "x",
                ],
            ),
            (
                ".zsh_sessions",
                &[
                    "0.session",
                    "a.session",
                    "x.session",
                    "0.history",
                    "a.history",
                    "x.history",
                    "0.historynew",
                    "0.zsh",
                    "0",
                    "a",
                    "x",
                ],
            ),
            (
                ".bash_sessions",
                &[
                    "0.history",
                    "a.history",
                    "x.history",
                    "0.historynew",
                    "0.session",
                    "0",
                    "a",
                    "x",
                ],
            ),
        ];
        let mut tree_targets: Vec<(&str, String, String)> = vec![];
        for t in ASK_TREES {
            let t_abs = format!("{}/{t}", self.home);
            for rl in &root_lowers {
                if under(rl, &t_abs).is_some() {
                    return true;
                }
                if let Some(tree_rel) = under(&t_abs, rl) {
                    tree_targets.push((*t, tree_rel.to_string(), t_abs.clone()));
                }
            }
        }
        if !tree_targets.is_empty() {
            for norm in variants {
                for (idx, _) in norm.match_indices('/') {
                    if idx == 0 {
                        continue;
                    }
                    let dir_pat = &norm[..idx];
                    let rest = &norm[idx + 1..];
                    let Ok(g) = GlobBuilder::new(dir_pat).literal_separator(true).build() else {
                        return true;
                    };
                    let m = g.compile_matcher();
                    let generic_dir = matches!(dir_pat.rsplit('/').next(), Some("**" | "*"));
                    let rest_matcher = if generic_dir && !rest.is_empty() {
                        match GlobBuilder::new(rest).literal_separator(false).build() {
                            Ok(rg) => Some(rg.compile_matcher()),
                            Err(_) => return true,
                        }
                    } else {
                        None
                    };
                    for (tree, tree_rel, t_abs) in &tree_targets {
                        if !(m.is_match(tree_rel) || m.is_match(t_abs)) {
                            continue;
                        }
                        if !generic_dir || rest.is_empty() {
                            return true;
                        }
                        let rm = rest_matcher.as_ref().unwrap();
                        let files = ASK_TREE_FILES
                            .iter()
                            .find(|(k, _)| k == tree)
                            .map_or(&[][..], |(_, f)| *f);
                        if files.iter().any(|f| rm.is_match(f)) {
                            return true;
                        }
                    }
                }
            }
        }
        false
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

fn first_class_end(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.first().copied() != Some(b'[') {
        return None;
    }
    let mut j = 1;
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
    (j < bytes.len() && bytes[j] == b']').then_some(j)
}

fn next_class_char(s: &str) -> (char, bool, &str) {
    let mut it = s.chars();
    let c = it.next().unwrap();
    if c == '\\'
        && let Some(next) = it.next()
    {
        return (next, true, it.as_str());
    }
    (c, false, it.as_str())
}

fn class_matches_char(class: &str, ch: char) -> bool {
    let inner = &class[1..class.len() - 1];
    let (neg, mut rest) = if let Some(r) = inner.strip_prefix(['!', '^']) {
        (true, r)
    } else {
        (false, inner)
    };
    let mut hit = false;
    while !rest.is_empty() {
        let (a, esc, after_a) = next_class_char(rest);
        if !esc
            && let Some(after_dash) = after_a.strip_prefix('-')
            && !after_dash.is_empty()
        {
            let (b, _, after_b) = next_class_char(after_dash);
            if a <= ch && ch <= b {
                hit = true;
            }
            rest = after_b;
        } else {
            if a == ch {
                hit = true;
            }
            rest = after_a;
        }
    }
    hit ^ neg
}

fn pat_starts_with_dot(p: &str) -> bool {
    p.starts_with('.')
        || (p.starts_with('[')
            && first_class_end(p).is_some_and(|end| class_matches_char(&p[..=end], '.')))
}

fn brace_branch_dot_ok(c: &str) -> bool {
    let Some(exps) = words::brace_expand(c) else {
        return true;
    };
    exps.iter().any(|e| pat_starts_with_dot(e))
}

enum GlobAtom<'a> {
    Lit(char),
    Any,
    Star,
    Class(&'a str),
}

fn next_glob_atom(pat: &str) -> Option<(GlobAtom<'_>, usize)> {
    let ch = pat.chars().next()?;
    let len = ch.len_utf8();
    if ch == '\\' {
        let lit = pat[len..].chars().next()?;
        return Some((GlobAtom::Lit(lit), len + lit.len_utf8()));
    }
    if ch == '*' {
        return Some((GlobAtom::Star, len));
    }
    if ch == '?' {
        return Some((GlobAtom::Any, len));
    }
    if ch == '['
        && let Some(rel_end) = first_class_end(pat)
    {
        return Some((GlobAtom::Class(&pat[..=rel_end]), rel_end + 1));
    }
    Some((GlobAtom::Lit(ch), len))
}

fn comp_glob_matches_impl(pat: &str, tgt: &str, prefix: bool) -> bool {
    if pat.contains('{') {
        let Some(exps) = words::brace_expand(pat) else {
            return true;
        };
        return exps.iter().any(|e| comp_glob_dp(e, tgt, prefix));
    }
    comp_glob_dp(pat, tgt, prefix)
}

fn comp_glob_dp(pat: &str, tgt: &str, prefix: bool) -> bool {
    if pat == tgt {
        return true;
    }
    if tgt.starts_with('.') && !pat_starts_with_dot(pat) {
        return false;
    }
    let tc: Vec<char> = tgt.chars().collect();
    let m = tc.len();
    if prefix && m == 0 {
        return true;
    }
    let (mut dp_stack, mut next_stack) = ([false; 64], [false; 64]);
    let (mut dp_heap, mut next_heap) = (Vec::new(), Vec::new());
    let (mut dp, mut next): (&mut [bool], &mut [bool]) = if m < 64 {
        (&mut dp_stack[..=m], &mut next_stack[..=m])
    } else {
        dp_heap.resize(m + 1, false);
        next_heap.resize(m + 1, false);
        (&mut dp_heap, &mut next_heap)
    };
    dp[0] = true;
    let mut i = 0;
    while i < pat.len() {
        let Some((atom, step)) = next_glob_atom(&pat[i..]) else {
            return false;
        };
        i += step;
        match atom {
            GlobAtom::Star => {
                if let Some(first_true) = (0..=m).find(|&k| dp[k]) {
                    dp[first_true..=m].fill(true);
                }
            }
            atom => {
                next.fill(false);
                for k in 0..m {
                    if dp[k] {
                        let ok = match atom {
                            GlobAtom::Any => true,
                            GlobAtom::Lit(lit) => tc[k] == lit,
                            GlobAtom::Class(cls) => class_matches_char(cls, tc[k]),
                            GlobAtom::Star => unreachable!(),
                        };
                        if ok {
                            next[k + 1] = true;
                        }
                    }
                }
                std::mem::swap(&mut dp, &mut next);
            }
        }
        if prefix && dp[m] {
            return true;
        }
        if !dp.iter().any(|&b| b) {
            return false;
        }
    }
    dp[m]
}

enum Comp<'a> {
    GlobStar,
    Literal(&'a str),
    Pattern { raw: &'a str, dot_ok: bool },
}

impl<'a> Comp<'a> {
    fn from_part(c: &'a str) -> Self {
        if c == "**" {
            return Self::GlobStar;
        }
        if !c.contains(['*', '?', '[', '{']) {
            return Self::Literal(c);
        }
        let dot_ok = pat_starts_with_dot(c) || (c.starts_with('{') && brace_branch_dot_ok(c));
        Self::Pattern { raw: c, dot_ok }
    }

    fn matches(&self, target: &str) -> bool {
        match self {
            Self::GlobStar => true,
            Self::Literal(s) => *s == target,
            Self::Pattern { raw, dot_ok } => {
                (*dot_ok || !target.starts_with('.')) && comp_glob_matches_impl(raw, target, false)
            }
        }
    }

    fn is_exact_literal(&self, s: &str) -> bool {
        matches!(self, Self::Literal(l) if *l == s)
    }

    fn is_public_ssh_name(&self) -> bool {
        match self {
            Self::GlobStar => false,
            Self::Literal(s) => s.ends_with(".pub") || s.starts_with("known_hosts"),
            Self::Pattern { raw, .. } => raw.ends_with(".pub") || raw.starts_with("known_hosts"),
        }
    }

    fn matches_prefix(&self, pre: &str) -> bool {
        match self {
            Self::GlobStar => true,
            Self::Literal(s) => s.starts_with(pre),
            Self::Pattern { raw, .. } => matches_ask_name_prefix_pat(raw, pre),
        }
    }
}

const ASK_PREFIX_PROBES: &[&str] = &[
    "msal_token_cache.json",
    "msal_token_cache.bin",
    "msal_token_cache.db",
    "msal_token_cache.txt",
    "msal_token_cache.cache",
];

/// Whether `file_pat` can match an ask-store file prefixed with `pre`
/// (`msal_token_cache.*`), without falsely catching unrelated extensions like `*.rs`.
fn matches_ask_name_prefix_pat(file_pat: &str, pre: &str) -> bool {
    if file_pat == "**" {
        return true;
    }
    if file_pat.contains('{') {
        let Some(exps) = words::brace_expand(file_pat) else {
            return true;
        };
        return exps.iter().any(|e| matches_ask_name_prefix_pat(e, pre));
    }
    if !comp_glob_dp(file_pat, pre, true) {
        return false;
    }
    file_pat.starts_with(pre)
        || file_pat.starts_with("msal_")
        || file_pat.split_once('.').is_some_and(|(stem, _)| {
            !stem.contains('*') && comp_glob_dp(stem, pre.trim_end_matches('.'), false)
        })
        || ASK_PREFIX_PROBES
            .iter()
            .any(|f| comp_glob_dp(file_pat, f, false))
}

fn split_comps(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

fn reach_indices(parts: &[Comp<'_>], has_globstar: bool, tgt: &[&str]) -> Vec<usize> {
    let n = parts.len();
    let m = tgt.len();
    if !has_globstar {
        if n >= m && parts[..m].iter().zip(tgt).all(|(p, t)| p.matches(t)) {
            return vec![m];
        }
        return vec![];
    }
    let mut dp = vec![vec![false; m + 1]; n + 1];
    dp[0][0] = true;
    for i in 0..n {
        for j in 0..=m {
            if !dp[i][j] {
                continue;
            }
            match &parts[i] {
                Comp::GlobStar => {
                    dp[i + 1][j] = true;
                    if j < m {
                        dp[i][j + 1] = true;
                    }
                }
                comp => {
                    if j < m && comp.matches(tgt[j]) {
                        dp[i + 1][j + 1] = true;
                    }
                }
            }
        }
    }
    (0..=n).filter(|&i| dp[i][m]).collect()
}

struct Tail<'a> {
    parts: &'a [Comp<'a>],
    has_globstar: bool,
}

impl Tail<'_> {
    fn matches_exact(&self, target: &str) -> bool {
        let tgt = split_comps(target);
        if !self.has_globstar {
            return self.parts.len() == tgt.len()
                && self.parts.iter().zip(&tgt).all(|(p, t)| p.matches(t));
        }
        reach_indices(self.parts, true, &tgt).contains(&self.parts.len())
    }

    fn matches_tree_impl(&self, tree: &str, strictly_under: bool) -> bool {
        let tgt = split_comps(tree);
        let Some((&leaf, parents)) = tgt.split_last() else {
            return false;
        };
        if !self.has_globstar {
            let len_ok = if strictly_under {
                self.parts.len() > tgt.len()
            } else {
                self.parts.len() >= tgt.len()
            };
            return len_ok
                && self.parts[..tgt.len()]
                    .iter()
                    .zip(&tgt)
                    .all(|(p, t)| p.matches(t));
        }
        for idx in reach_indices(self.parts, true, parents) {
            if idx >= self.parts.len() {
                continue;
            }
            match &self.parts[idx] {
                Comp::GlobStar => {
                    if self.parts[idx..]
                        .iter()
                        .all(|c| matches!(c, Comp::GlobStar))
                    {
                        return true;
                    }
                }
                comp => {
                    if comp.matches(leaf) && (!strictly_under || idx + 1 < self.parts.len()) {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn matches_tree(&self, tree: &str) -> bool {
        self.matches_tree_impl(tree, false)
    }

    fn matches_strictly_under(&self, root: &str) -> bool {
        self.matches_tree_impl(root, true)
    }

    fn matches_ssh_secret(&self, role: Role) -> bool {
        for idx in 0..self.parts.len() {
            if idx > 0 && !self.has_globstar {
                break;
            }
            let prefix_ok = idx == 0
                || self.parts[..idx]
                    .iter()
                    .all(|c| matches!(c, Comp::GlobStar));
            if !prefix_ok {
                continue;
            }
            let matched_ssh = match &self.parts[idx] {
                Comp::GlobStar => self.parts[idx..]
                    .iter()
                    .all(|c| matches!(c, Comp::GlobStar)),
                comp => comp.matches(".ssh"),
            };
            if !matched_ssh {
                continue;
            }
            let sub = &self.parts[idx + 1..];
            if sub.is_empty() {
                continue;
            }
            let public = sub.last().is_some_and(Comp::is_public_ssh_name);
            let readable_config = role != Role::Write
                && sub.len() == 1
                && (sub[0].is_exact_literal("config")
                    || sub[0].is_exact_literal("authorized_keys"));
            if !(public || readable_config) {
                return true;
            }
        }
        false
    }

    fn matches_dir_and_name_prefix(&self, dir: &str, pre: &str) -> bool {
        let tgt = split_comps(dir);
        for idx in reach_indices(self.parts, self.has_globstar, &tgt) {
            let sub = &self.parts[idx..];
            if sub.is_empty() {
                continue;
            }
            if sub[..sub.len() - 1]
                .iter()
                .all(|c| matches!(c, Comp::GlobStar))
                && sub.last().is_some_and(|c| c.matches_prefix(pre))
            {
                return true;
            }
        }
        false
    }
}

/// Shell globbing: `*` and `?` don't cross `/` and don't match a leading dot.
struct ShellGlob<'a> {
    abs: bool,
    parts: Vec<Comp<'a>>,
    has_globstar: bool,
}

impl<'a> ShellGlob<'a> {
    fn new(pattern: &'a str) -> Self {
        let parts: Vec<Comp<'a>> = split_comps(pattern)
            .into_iter()
            .map(Comp::from_part)
            .collect();
        let has_globstar = parts.iter().any(|c| matches!(c, Comp::GlobStar));
        Self {
            abs: pattern.starts_with('/'),
            parts,
            has_globstar,
        }
    }

    fn as_tail(&self) -> Tail<'_> {
        Tail {
            parts: &self.parts,
            has_globstar: self.has_globstar,
        }
    }

    fn matches_exact(&self, target: &str) -> bool {
        self.abs == target.starts_with('/') && self.as_tail().matches_exact(target)
    }

    fn matches_tree(&self, tree: &str) -> bool {
        self.abs == tree.starts_with('/') && self.as_tail().matches_tree(tree)
    }

    fn tails_after(&self, prefix: &str) -> Vec<Tail<'_>> {
        if self.abs != prefix.starts_with('/') {
            return vec![];
        }
        let tgt = split_comps(prefix);
        reach_indices(&self.parts, self.has_globstar, &tgt)
            .into_iter()
            .map(|idx| {
                let parts = &self.parts[idx..];
                let has_globstar =
                    self.has_globstar && parts.iter().any(|c| matches!(c, Comp::GlobStar));
                Tail {
                    parts,
                    has_globstar,
                }
            })
            .collect()
    }
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
        None
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
