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
        }
        for n in SSH_PROBES {
            v.push(abs(&format!(".ssh/{n}")));
        }
        for b in BROWSER_ROOTS {
            for f in ["cookies", "login data", "cookies.sqlite", "logins.json"] {
                v.push(format!("{}/{f}", abs(b)));
                v.push(format!("{}/default/{f}", abs(b)));
            }
        }
        for d in &self.data {
            v.push(d.clone());
            v.push(format!("{d}/x"));
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
        for probe in self.probes() {
            if glob.is_match(&probe)
                && let Some(h) = self.hit(Path::new(&probe), Role::Read)
            {
                return Some(h);
            }
        }
        None
    }

    pub fn is_ssh_dir(&self, p: &Path) -> bool {
        lower(p) == format!("{}/.ssh", self.home)
    }

    /// A credential store that asks (CR-12): `p` is one, or is inside one
    /// of its folders.
    pub fn ask_hit(&self, p: &Path) -> bool {
        let pl = lower(p);
        let Some(rest) = under(&pl, &self.home) else {
            return false;
        };
        ASK_FILES.iter().any(|f| f.to_lowercase() == rest)
            || ASK_TREES.iter().any(|t| under(rest, t).is_some())
            || ASK_NAME_PREFIXES.iter().any(|(dir, pre)| {
                under(rest, dir).is_some_and(|r| !r.contains('/') && r.starts_with(pre))
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
        // Any search of an ASK_TREE with a filename filter (`.zsh_sessions/*.session`).
        let mut tree_targets: Vec<(String, String)> = vec![];
        for t in ASK_TREES {
            let t_abs = format!("{}/{t}", self.home);
            for rl in &root_lowers {
                if under(rl, &t_abs).is_some() {
                    return true;
                }
                if let Some(tree_rel) = under(&t_abs, rl) {
                    tree_targets.push((tree_rel.to_string(), t_abs.clone()));
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
                    let Ok(g) = GlobBuilder::new(dir_pat).literal_separator(true).build() else {
                        return true;
                    };
                    let m = g.compile_matcher();
                    if tree_targets
                        .iter()
                        .any(|(tree_rel, t_abs)| m.is_match(tree_rel) || m.is_match(t_abs))
                    {
                        return true;
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

/// Shell globbing: `*` and `?` don't cross `/` and don't match a leading dot.
struct ShellGlob {
    whole: Option<globset::GlobMatcher>,
    parts: Vec<(bool, Option<globset::GlobMatcher>)>,
}

impl ShellGlob {
    fn new(pattern: &str) -> Self {
        let compile = |p: &str| {
            GlobBuilder::new(p)
                .literal_separator(true)
                .build()
                .ok()
                .map(|g| g.compile_matcher())
        };
        if pattern.contains("**") {
            return Self {
                whole: compile(pattern),
                parts: vec![],
            };
        }
        Self {
            whole: None,
            parts: pattern
                .split('/')
                .map(|c| (c.starts_with('.'), compile(c)))
                .collect(),
        }
    }

    fn is_match(&self, probe: &str) -> bool {
        if let Some(w) = &self.whole {
            return w.is_match(probe);
        }
        if self.parts.is_empty() {
            return false;
        }
        let qc: Vec<&str> = probe.split('/').collect();
        qc.len() == self.parts.len()
            && self.parts.iter().zip(&qc).all(|((dot, m), q)| {
                (*dot || !q.starts_with('.')) && m.as_ref().is_some_and(|m| m.is_match(q))
            })
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
        // recursive search over a folder that holds a hard-denied path (S8)
        if let ToolSubject::Paths {
            paths,
            recursive: true,
            ..
        } = &self.input.check.subject
        {
            if paths.is_empty() {
                if let Some(h) = self.dir_contains_hard_deny(self.input.cwd) {
                    hits.push(h);
                }
            } else {
                for idx in 0..a.uses.len() {
                    for p in a.all_paths(idx) {
                        if let Some(h) = self.dir_contains_hard_deny(p) {
                            hits.push(h);
                        }
                    }
                }
                if a.aligned.is_none() {
                    for p in self.input.resolved_paths {
                        if let Some(h) = self.dir_contains_hard_deny(p) {
                            hits.push(h);
                        }
                    }
                }
            }
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
            // recursive search of a folder holding a hard-denied path (S8)
            for eff_r in u.readings() {
                if let Some(spec) = recursive_search_roots(eff_r) {
                    self.check_search_roots(i, &spec, &mut hits);
                }
            }
        }
        hits
    }

    fn dir_contains_hard_deny(&self, p: &Path) -> Option<&'static str> {
        if let Some(h) = self.s.hit(p, Role::Read) {
            return Some(h);
        }
        let pl = lower(p);
        let rest = under(&pl, &self.s.home)?;
        // `~` itself keeps `dev`'s verdict; `~/.config` is kept as `Ask` by
        // `policy.rs` (`whole_home_recursive_search_asks` and `hard_deny_table`).
        if rest.is_empty() || rest == ".config" {
            return None;
        }
        for d in &self.s.data {
            if under(d, &pl).is_some() {
                return Some("atrium-self");
            }
        }
        if under(".ssh", rest).is_some()
            || CRED_FILES.iter().any(|f| under(f, rest).is_some())
            || CRED_TREES.iter().any(|t| under(t, rest).is_some())
            || BROWSER_ROOTS
                .iter()
                .any(|b| under(b, rest).is_some() || under(rest, b).is_some())
        {
            return Some("credential-read");
        }
        None
    }

    fn check_search_roots(&self, i: usize, spec: &SearchRootSpec, hits: &mut Vec<&'static str>) {
        if spec.base_dirs.is_empty() {
            if spec.roots.is_empty() {
                if spec.searches_cwd_when_empty {
                    let cwd = self
                        .a
                        .unit_cwd
                        .get(i)
                        .cloned()
                        .unwrap_or_else(|| CwdState::Known(vec![self.a.start.clone()]));
                    for c in cwd.candidates() {
                        if let Some(h) = self.dir_contains_hard_deny(c) {
                            hits.push(h);
                        }
                        if let Some(al) = &self.a.aligned {
                            for (idx, u) in self.a.uses.iter().enumerate() {
                                for (k, lp) in u.paths.iter().enumerate() {
                                    if lp == c
                                        && let Some(rp) = al[idx].get(k)
                                        && let Some(h) = self.dir_contains_hard_deny(rp)
                                    {
                                        hits.push(h);
                                    }
                                }
                            }
                        }
                    }
                }
                return;
            }
            for r in &spec.roots {
                let expanded = words::brace_expand(r).unwrap_or_else(|| vec![r.clone()]);
                for er in expanded {
                    for p in self.target_paths(i, &er) {
                        if let Some(h) = self.dir_contains_hard_deny(&p) {
                            hits.push(h);
                        }
                    }
                }
            }
            return;
        }
        let mut bases: Vec<PathBuf> = vec![];
        for (idx, b) in spec.base_dirs.iter().enumerate() {
            let expanded = words::brace_expand(b).unwrap_or_else(|| vec![b.clone()]);
            if idx == 0 {
                for eb in expanded {
                    bases.extend(self.target_paths(i, &eb));
                }
            } else {
                let mut next = vec![];
                for prev in &bases {
                    for eb in &expanded {
                        next.push(lexical_resolve(eb, prev, self.home()));
                    }
                }
                bases = next;
            }
        }
        if spec.roots.is_empty() {
            if spec.searches_cwd_when_empty {
                for b in &bases {
                    if let Some(h) = self.dir_contains_hard_deny(b) {
                        hits.push(h);
                    }
                }
            }
            return;
        }
        for r in &spec.roots {
            let expanded = words::brace_expand(r).unwrap_or_else(|| vec![r.clone()]);
            for er in expanded {
                for b in &bases {
                    let p = lexical_resolve(&er, b, self.home());
                    if let Some(h) = self.dir_contains_hard_deny(&p) {
                        hits.push(h);
                    }
                }
            }
        }
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
        let probes = self.s.probes();
        let has_dotdot_probe = probes.iter().any(|p| p.split('/').any(|s| s == ".."));
        let matchers: Vec<globset::GlobMatcher> = variants
            .iter()
            .filter(|norm| has_dotdot_probe || !norm.split('/').any(|s| s == ".."))
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
        for probe in probes {
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

struct SearchRootSpec {
    roots: Vec<String>,
    base_dirs: Vec<String>,
    searches_cwd_when_empty: bool,
}

struct SearchOpts {
    val_s: &'static str,
    val_l: &'static [&'static str],
    extra_val_l: &'static [&'static str],
    pat_s: &'static str,
    pat_l: &'static [&'static str],
    opt_num_s: &'static str,
    opt_num_l: &'static [&'static str],
    eq_only_l: &'static [&'static str],
    bool_l: &'static [&'static str],
    extra_bool_l: &'static [&'static str],
    long_prefixes: bool,
    always_recursive: bool,
    check_grep_recursion: bool,
    exec_s: &'static str,
    exec_l: &'static [&'static str],
    root_val_l: &'static [&'static str],
    base_val_l: &'static [&'static str],
    skip_parens: bool,
}

fn fold(s: &str) -> String {
    if s.is_ascii() {
        s.to_ascii_lowercase()
    } else {
        s.to_lowercase().replace('ſ', "s")
    }
}

fn eq_folded(a: &str, lower_lit: &str) -> bool {
    if a.is_ascii() {
        a.eq_ignore_ascii_case(lower_lit)
    } else {
        fold(a) == lower_lit
    }
}

fn resolve_long_opt<'a>(name: &'a str, o: &SearchOpts) -> &'a str {
    if !o.long_prefixes || name.is_empty() {
        return name;
    }
    let all = o
        .val_l
        .iter()
        .chain(o.extra_val_l.iter())
        .chain(o.bool_l.iter())
        .chain(o.extra_bool_l.iter())
        .chain(o.opt_num_l.iter())
        .chain(o.eq_only_l.iter())
        .chain(o.exec_l.iter())
        .chain(o.root_val_l.iter())
        .chain(o.base_val_l.iter())
        .copied();
    let mut exact = false;
    for candidate in all.clone() {
        if candidate == name {
            exact = true;
            break;
        }
    }
    if exact || name.len() < 3 {
        return name;
    }
    let mut found: Option<&'static str> = None;
    for candidate in all {
        if candidate.starts_with(name) {
            if let Some(prev) = found {
                if prev != candidate {
                    return name;
                }
            } else {
                found = Some(candidate);
            }
        }
    }
    found.unwrap_or(name)
}

fn parse_search_opts(args: &[String], o: &SearchOpts) -> Option<SearchRootSpec> {
    let mut recursive = o.always_recursive;
    let mut have_pattern = false;
    let mut dd = false;
    let mut positional: Vec<String> = vec![];
    let mut extra_roots: Vec<String> = vec![];
    let mut base_dirs: Vec<String> = vec![];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if dd || !a.starts_with('-') || a == "-" {
            if !dd && o.skip_parens && (a == "(" || a == ")") {
                continue;
            }
            positional.push(a.to_string());
            continue;
        }
        if a == "--" {
            dd = true;
            continue;
        }
        if let Some(long) = a.strip_prefix("--") {
            let (raw_name, val_eq) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (long, None),
            };
            let name = resolve_long_opt(raw_name, o);
            if o.exec_l.contains(&name) {
                while i < args.len() && args[i] != ";" {
                    i += 1;
                }
                if i < args.len() {
                    i += 1;
                }
                continue;
            }
            if o.pat_l.contains(&name) {
                have_pattern = true;
            }
            if o.check_grep_recursion && matches!(name, "recursive" | "dereference-recursive") {
                recursive = true;
            }
            let take_long_val = |idx: &mut usize| -> Option<&str> {
                match val_eq {
                    Some(v) => Some(v),
                    None => {
                        let next = args.get(*idx).map(String::as_str);
                        if next.is_some() {
                            *idx += 1;
                        }
                        next
                    }
                }
            };
            if o.root_val_l.contains(&name) {
                if let Some(v) = take_long_val(&mut i) {
                    extra_roots.push(v.to_string());
                }
            } else if o.base_val_l.contains(&name) {
                if let Some(v) = take_long_val(&mut i) {
                    base_dirs.push(v.to_string());
                }
            } else if o.val_l.contains(&name) || o.extra_val_l.contains(&name) {
                let val = take_long_val(&mut i);
                if o.check_grep_recursion
                    && name == "directories"
                    && let Some(v) = val
                    && v.len() >= 3
                    && "recurse".starts_with(v)
                {
                    recursive = true;
                }
            } else if o.opt_num_l.contains(&name)
                && val_eq.is_none()
                && args
                    .get(i)
                    .is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit()))
            {
                i += 1;
            }
        } else {
            let cs: Vec<char> = a[1..].chars().collect();
            for (k, c) in cs.iter().enumerate() {
                let attached: String = cs[k + 1..].iter().collect();
                if o.exec_s.contains(*c) {
                    while i < args.len() && args[i] != ";" {
                        i += 1;
                    }
                    if i < args.len() {
                        i += 1;
                    }
                    break;
                }
                if o.pat_s.contains(*c) {
                    have_pattern = true;
                }
                if o.check_grep_recursion && matches!(*c, 'r' | 'R') {
                    recursive = true;
                }
                if o.val_s.contains(*c) {
                    let val = if !attached.is_empty() {
                        Some(attached.as_str())
                    } else {
                        let next = args.get(i).map(String::as_str);
                        if next.is_some() {
                            i += 1;
                        }
                        next
                    };
                    if o.check_grep_recursion
                        && *c == 'd'
                        && let Some(v) = val
                        && v.len() >= 3
                        && "recurse".starts_with(v)
                    {
                        recursive = true;
                    }
                    break;
                }
                if o.opt_num_s.contains(*c) {
                    if attached.is_empty()
                        && args
                            .get(i)
                            .is_some_and(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_digit()))
                    {
                        i += 1;
                    }
                    break;
                }
            }
        }
    }
    if !recursive {
        return None;
    }
    let skip = usize::from(!have_pattern);
    let mut roots: Vec<String> = positional.into_iter().skip(skip).collect();
    let saw_stdin = roots.iter().any(|r| r == "-");
    roots.retain(|r| r != "-");
    roots.extend(extra_roots);
    Some(SearchRootSpec {
        roots,
        base_dirs,
        searches_cwd_when_empty: !saw_stdin,
    })
}

fn strip_git_pathspec_magic(arg: &str) -> Option<String> {
    let Some(after) = arg.strip_prefix(':') else {
        return Some(arg.to_string());
    };
    let mut s = after;
    loop {
        if let Some(rest) = s.strip_prefix('(') {
            let mut depth = 1usize;
            let mut end = None;
            for (idx, ch) in rest.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(idx);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let end_idx = end?;
            let inner = &rest[..end_idx];
            if inner.split(',').any(|k| k.trim() == "exclude") {
                return None;
            }
            s = &rest[end_idx + 1..];
        } else if s.starts_with(['!', '^']) {
            return None;
        } else if let Some(rest) = s.strip_prefix(['/', ':']) {
            s = rest;
        } else {
            break;
        }
    }
    (!s.is_empty()).then(|| s.to_string())
}

fn git_grep_roots(eff: &[String]) -> Option<SearchRootSpec> {
    let first = eff.first()?;
    if !eq_folded(base(first), "git") {
        return None;
    }
    let mut base_dirs = vec![];
    let mut i = 1;
    while i < eff.len() {
        let w = eff[i].as_str();
        if w == "-C" {
            if let Some(d) = eff.get(i + 1) {
                base_dirs.push(d.clone());
            }
            i += 2;
        } else if let Some(d) = w.strip_prefix("-C")
            && !d.is_empty()
        {
            base_dirs.push(d.to_string());
            i += 1;
        } else if w == "--work-tree" {
            if let Some(d) = eff.get(i + 1) {
                base_dirs.push(d.clone());
            }
            i += 2;
        } else if let Some(d) = w.strip_prefix("--work-tree=") {
            base_dirs.push(d.to_string());
            i += 1;
        } else if matches!(
            w,
            "-c" | "--git-dir" | "--namespace" | "--super-prefix" | "--config-env"
        ) {
            i += 2;
        } else if w.starts_with('-') {
            i += 1;
        } else {
            break;
        }
    }
    if eff.get(i).is_none_or(|s| !eq_folded(s, "grep")) {
        return None;
    }
    const GIT_GREP_OPTS: SearchOpts = SearchOpts {
        val_s: "efABCm",
        val_l: &[
            "regexp",
            "file",
            "max-depth",
            "threads",
            "after-context",
            "before-context",
            "context",
            "max-count",
            "parent-basename",
        ],
        extra_val_l: &[],
        pat_s: "ef",
        pat_l: &["regexp", "file"],
        opt_num_s: "",
        opt_num_l: &[],
        eq_only_l: &["color", "open-files-in-pager"],
        bool_l: &[],
        extra_bool_l: &[],
        long_prefixes: false,
        always_recursive: true,
        check_grep_recursion: false,
        exec_s: "",
        exec_l: &[],
        root_val_l: &[],
        base_val_l: &[],
        skip_parens: true,
    };
    let args = &eff[i + 1..];
    // Before `--`, positional 0 (if no `-e`/`-f`) is `<pattern>`; everything
    // after `--` is a pathspec directly.
    if let Some(dd_idx) = args.iter().position(|a| a == "--") {
        let mut roots = vec![];
        for p in &args[dd_idx + 1..] {
            if let Some(cleaned) = strip_git_pathspec_magic(p) {
                roots.push(cleaned);
            }
        }
        return Some(SearchRootSpec {
            roots,
            base_dirs,
            searches_cwd_when_empty: true,
        });
    }
    let mut spec = parse_search_opts(args, &GIT_GREP_OPTS)?;
    spec.roots = spec
        .roots
        .into_iter()
        .filter_map(|p| strip_git_pathspec_magic(&p))
        .collect();
    spec.base_dirs = base_dirs;
    Some(spec)
}

fn recursive_search_roots(eff: &[String]) -> Option<SearchRootSpec> {
    let first = eff.first()?;
    let b_fold = fold(base(first));
    const RG_OPTS: SearchOpts = SearchOpts {
        val_s: "egtTmABCdjMErf",
        val_l: super::engine::RG_VAL_L,
        extra_val_l: &["file", "ignore-file", "pre", "pre-glob", "hostname-bin"],
        pat_s: "ef",
        pat_l: &["regexp", "file", "files", "type-list"],
        opt_num_s: "",
        opt_num_l: &[],
        eq_only_l: &[],
        bool_l: &[],
        extra_bool_l: &[],
        long_prefixes: false,
        always_recursive: true,
        check_grep_recursion: false,
        exec_s: "",
        exec_l: &[],
        root_val_l: &[],
        base_val_l: &[],
        skip_parens: false,
    };
    const GREP_OPTS: SearchOpts = SearchOpts {
        val_s: "efmABCdD",
        val_l: &[
            "regexp",
            "file",
            "max-count",
            "after-context",
            "before-context",
            "context",
            "directories",
            "devices",
            "include",
            "exclude",
            "exclude-from",
            "exclude-dir",
            "label",
            "binary-files",
            "group-separator",
        ],
        extra_val_l: &[],
        pat_s: "ef",
        pat_l: &["regexp", "file"],
        opt_num_s: "",
        opt_num_l: &[],
        eq_only_l: &["color", "colour"],
        bool_l: super::engine::GREP_BOOL_L,
        extra_bool_l: &[
            "help",
            "version",
            "unix-byte-offsets",
            "line-buffered",
            "mmap",
        ],
        long_prefixes: true,
        always_recursive: false,
        check_grep_recursion: true,
        exec_s: "",
        exec_l: &[],
        root_val_l: &[],
        base_val_l: &[],
        skip_parens: false,
    };
    const RGREP_OPTS: SearchOpts = SearchOpts {
        always_recursive: true,
        ..GREP_OPTS
    };
    const FD_OPTS: SearchOpts = SearchOpts {
        val_s: "dteEcCjSo",
        val_l: &[
            "max-depth",
            "maxdepth",
            "min-depth",
            "exact-depth",
            "type",
            "extension",
            "exclude",
            "ignore-file",
            "color",
            "threads",
            "max-results",
            "size",
            "changed-within",
            "changed-before",
            "owner",
            "path-separator",
            "max-buffer-time",
            "format",
        ],
        extra_val_l: &[],
        pat_s: "",
        pat_l: &[],
        opt_num_s: "",
        opt_num_l: &[],
        eq_only_l: &[],
        bool_l: &[],
        extra_bool_l: &[],
        long_prefixes: false,
        always_recursive: true,
        check_grep_recursion: false,
        exec_s: "xX",
        exec_l: &["exec", "exec-batch"],
        root_val_l: &["search-path"],
        base_val_l: &["base-directory"],
        skip_parens: false,
    };
    const AG_OPTS: SearchOpts = SearchOpts {
        val_s: "GgmpW",
        val_l: &[
            "ackmate-dir-filter",
            "depth",
            "file-search-regex",
            "filename-pattern",
            "ignore",
            "ignore-dir",
            "max-count",
            "pager",
            "path-to-ignore",
            "width",
            "workers",
            "color-line-number",
            "color-match",
            "color-path",
        ],
        extra_val_l: &[],
        pat_s: "g",
        pat_l: &["filename-pattern"],
        opt_num_s: "ABC",
        opt_num_l: &["after", "before", "context"],
        eq_only_l: &[],
        bool_l: &[
            "ackmate",
            "affinity",
            "noaffinity",
            "all-types",
            "all-text",
            "unrestricted",
            "hidden",
            "ignore-case",
            "smart-case",
            "case-sensitive",
            "fixed-strings",
            "literal",
            "files-with-matches",
            "files-without-match",
            "count",
            "column",
            "nocolor",
            "color",
            "color-win-ansi",
            "debug",
            "noheading",
            "heading",
            "nobreak",
            "break",
            "follow",
            "nofollow",
            "skip-vcs-ignores",
            "search-binary",
            "search-zip",
            "silent",
            "stats",
            "stats-only",
            "vimgrep",
            "word-regexp",
            "only-matching",
            "null",
            "print0",
            "one-device",
            "filename",
            "nofilename",
            "numbers",
            "nonumbers",
            "line-numbers",
            "group",
            "nogroup",
            "invert-match",
            "match",
            "mmap",
            "nommap",
            "multiline",
            "nomultiline",
            "nopager",
            "parallel",
            "passthrough",
            "passthru",
            "print-all-files",
            "print-long-lines",
            "recurse",
            "norecurse",
        ],
        extra_bool_l: &[],
        long_prefixes: true,
        always_recursive: true,
        check_grep_recursion: false,
        exec_s: "",
        exec_l: &[],
        root_val_l: &[],
        base_val_l: &[],
        skip_parens: false,
    };
    const ACK_OPTS: SearchOpts = SearchOpts {
        val_s: "ABmgt",
        val_l: &[
            "after-context",
            "before-context",
            "type",
            "ignore-dir",
            "noignore-dir",
            "match",
            "output",
            "max-count",
            "pager",
            "color-filename",
            "color-lineno",
            "color-match",
            "files-from",
        ],
        extra_val_l: &[],
        pat_s: "gf",
        pat_l: &["match"],
        opt_num_s: "C",
        opt_num_l: &["context"],
        eq_only_l: &[],
        bool_l: &[
            "ignore-case",
            "smart-case",
            "invert-match",
            "word-regexp",
            "literal",
            "files-with-matches",
            "files-without-match",
            "count",
            "no-filename",
            "with-filename",
            "column",
            "nocolor",
            "color",
            "noheading",
            "heading",
            "nobreak",
            "break",
            "follow",
            "nofollow",
            "recurse",
            "no-recurse",
            "sort-files",
            "nopager",
        ],
        extra_bool_l: &[],
        long_prefixes: true,
        always_recursive: true,
        check_grep_recursion: false,
        exec_s: "",
        exec_l: &[],
        root_val_l: &[],
        base_val_l: &[],
        skip_parens: false,
    };
    let args = &eff[1..];
    match b_fold.as_str() {
        "rg" => parse_search_opts(args, &RG_OPTS),
        "grep" | "egrep" | "fgrep" | "zgrep" | "ggrep" => parse_search_opts(args, &GREP_OPTS),
        "rgrep" => parse_search_opts(args, &RGREP_OPTS),
        "find" => Some(SearchRootSpec {
            roots: shell::find_roots(eff).0,
            base_dirs: vec![],
            searches_cwd_when_empty: false,
        }),
        "fd" | "fdfind" => parse_search_opts(args, &FD_OPTS),
        "ag" => parse_search_opts(args, &AG_OPTS),
        "ack" => parse_search_opts(args, &ACK_OPTS),
        "git" => git_grep_roots(eff),
        _ => None,
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
/// Upper bound on state transitions per glob check so pathological globs fail
/// closed to Ask in linear/bounded time (K10).
const MAX_GLOB_WORK: usize = 100_000;
/// Upper bound on total reconstructed bytes across normalized variants.
const MAX_NORM_OUTPUT_BYTES: usize = 512 * 1024;

struct NormNode {
    parent: usize,
    seg_id: u32,
    count: usize,
}

struct NormArena<'a> {
    nodes: Vec<NormNode>,
    segs: Vec<&'a str>,
    seg_ids: HashMap<&'a str, u32>,
    last_seg: (&'a str, u32),
    intern: HashMap<(usize, u32, usize), usize>,
    work: usize,
    budget: usize,
}

impl<'a> NormArena<'a> {
    fn new(budget: usize) -> Self {
        let mut seg_ids = HashMap::new();
        seg_ids.insert("", 0);
        seg_ids.insert("..", 1);
        seg_ids.insert("**", 2);
        Self {
            nodes: vec![NormNode {
                parent: 0,
                seg_id: 0,
                count: 0,
            }],
            segs: vec!["", "..", "**"],
            seg_ids,
            last_seg: ("", 0),
            intern: HashMap::new(),
            work: 0,
            budget,
        }
    }

    fn step(&mut self) -> bool {
        self.work += 1;
        self.work <= self.budget
    }

    fn intern_seg(&mut self, seg: &'a str) -> u32 {
        if seg == self.last_seg.0 {
            return self.last_seg.1;
        }
        let id = if let Some(&existing) = self.seg_ids.get(seg) {
            existing
        } else {
            let next = self.segs.len() as u32;
            self.segs.push(seg);
            self.seg_ids.insert(seg, next);
            next
        };
        self.last_seg = (seg, id);
        id
    }

    fn push_seg(&mut self, parent: usize, seg_id: u32, count: usize) -> Option<usize> {
        if !self.step() {
            return None;
        }
        if let Some(&existing) = self.intern.get(&(parent, seg_id, count)) {
            return Some(existing);
        }
        let next = self.nodes.len();
        self.nodes.push(NormNode {
            parent,
            seg_id,
            count,
        });
        self.intern.insert((parent, seg_id, count), next);
        Some(next)
    }

    fn is_dotdot(&self, id: usize) -> bool {
        self.nodes[id].seg_id == 1
    }

    fn is_globstar(&self, id: usize) -> bool {
        self.nodes[id].seg_id == 2
    }

    fn last_seg(&self, id: usize) -> Option<&'a str> {
        (id != 0).then(|| self.segs[self.nodes[id].seg_id as usize])
    }

    fn pop(&self, id: usize) -> usize {
        self.nodes[id].parent
    }
}

/// Single normalizer for both `glob_escape` and `search_glob_hit` /
/// `ask_search_glob_hit`: collapses curdir, empty segments, and `literal/..`,
/// and treats `..` after a single-component wildcard as possibly popping that
/// segment (emitting both popped and as-written forms).
fn apply_one_dotdot<'a>(
    id: usize,
    abs: bool,
    rem: usize,
    next_active: &mut Vec<usize>,
    dotdot_tails: &mut Vec<(usize, usize)>,
    arena: &mut NormArena<'a>,
) -> bool {
    if !arena.step() {
        return false;
    }
    match arena.last_seg(id) {
        None => {
            if abs {
                next_active.push(id);
            } else {
                dotdot_tails.push((id, rem));
            }
        }
        Some("..") => {
            let node = &arena.nodes[id];
            dotdot_tails.push((node.parent, node.count + rem));
        }
        Some("**") => {
            // `**` matches >= 2 segments: absorbs `..` and stays `**`.
            next_active.push(id);
            // `**` matches 1 segment: cancelled by `..`.
            let parent = arena.pop(id);
            if arena.is_dotdot(parent) {
                let node = &arena.nodes[parent];
                dotdot_tails.push((node.parent, node.count + (rem - 1)));
            } else {
                next_active.push(parent);
            }
            // `**` matches 0 segments: `..` acts on what preceded `**`.
            if !apply_one_dotdot(parent, abs, rem, next_active, dotdot_tails, arena) {
                return false;
            }
        }
        Some(last) if !last.contains(['*', '?', '[']) => {
            next_active.push(arena.pop(id));
        }
        Some(_) => {
            next_active.push(arena.pop(id));
            dotdot_tails.push((id, rem));
        }
    }
    true
}

/// Collapses a run of `run` consecutive `..` segments in linear time per
/// active state + popped depth instead of stepping `..`-ending states
/// quadratically across the run.
fn apply_dotdot_run<'a>(
    states: &[usize],
    abs: bool,
    run: usize,
    cap: usize,
    arena: &mut NormArena<'a>,
) -> Option<Vec<usize>> {
    let mut active = Vec::with_capacity(states.len());
    let mut dotdot_tails: Vec<(usize, usize)> = Vec::new();
    for &st in states {
        if arena.is_dotdot(st) {
            if !arena.step() {
                return None;
            }
            let node = &arena.nodes[st];
            dotdot_tails.push((node.parent, node.count + run));
        } else {
            active.push(st);
        }
    }
    for step_idx in 0..run {
        if active.is_empty() {
            break;
        }
        let rem = run - step_idx;
        let tails_before = dotdot_tails.len();
        let mut next_active = Vec::with_capacity(active.len());
        for &st in &active {
            if !apply_one_dotdot(st, abs, rem, &mut next_active, &mut dotdot_tails, arena) {
                return None;
            }
        }
        next_active.sort_unstable();
        next_active.dedup();
        if next_active.len() + dotdot_tails.len() > cap {
            dotdot_tails.sort_unstable();
            dotdot_tails.dedup();
            if next_active.len() + dotdot_tails.len() > cap {
                return None;
            }
        }
        if next_active == active && dotdot_tails.len() == tails_before {
            break;
        }
        active = next_active;
    }
    dotdot_tails.sort_unstable();
    dotdot_tails.dedup();
    if active.len() + dotdot_tails.len() > cap {
        return None;
    }
    for (parent, count) in dotdot_tails {
        let id = arena.push_seg(parent, 1, count)?;
        active.push(id);
    }
    active.sort_unstable();
    active.dedup();
    if active.len() > cap {
        return None;
    }
    Some(active)
}

fn normalize_glob_variants(
    g: &str,
    cap: usize,
    total_work: &mut usize,
    max_work: usize,
) -> Option<Vec<String>> {
    let remaining_work = max_work.checked_sub(*total_work)?;
    let abs = g.starts_with('/');
    let mut arena = NormArena::new(remaining_work);
    let mut states: Vec<usize> = vec![0];
    let mut iter = g.split('/').peekable();
    while let Some(c) = iter.next() {
        match c {
            "" | "." => {
                if !arena.step() {
                    *total_work = max_work;
                    return None;
                }
            }
            "**" => {
                for st in &mut states {
                    if !arena.is_globstar(*st) {
                        let Some(next) = arena.push_seg(*st, 2, 1) else {
                            *total_work = max_work;
                            return None;
                        };
                        *st = next;
                    } else if !arena.step() {
                        *total_work = max_work;
                        return None;
                    }
                }
                states.sort_unstable();
                states.dedup();
            }
            ".." => {
                let mut run = 1usize;
                while let Some(&next_seg) = iter.peek() {
                    match next_seg {
                        "" | "." => {
                            iter.next();
                            if !arena.step() {
                                *total_work = max_work;
                                return None;
                            }
                        }
                        ".." => {
                            iter.next();
                            run += 1;
                        }
                        _ => break,
                    }
                }
                let Some(next_states) = apply_dotdot_run(&states, abs, run, cap, &mut arena) else {
                    *total_work = total_work.saturating_add(arena.work);
                    return None;
                };
                states = next_states;
            }
            _ => {
                let seg_id = arena.intern_seg(c);
                for st in &mut states {
                    let Some(next) = arena.push_seg(*st, seg_id, 1) else {
                        *total_work = max_work;
                        return None;
                    };
                    *st = next;
                }
            }
        }
    }
    *total_work = total_work.saturating_add(arena.work);
    let mut out = Vec::with_capacity(states.len());
    let mut seen = std::collections::HashSet::with_capacity(states.len());
    let mut segs: Vec<&str> = vec![];
    let mut total_bytes = 0usize;
    for mut id in states {
        segs.clear();
        let mut seg_bytes = if abs { 1 } else { 0 };
        while id != 0 {
            let node = &arena.nodes[id];
            let seg = arena.segs[node.seg_id as usize];
            let add_bytes = node.count.saturating_mul(seg.len() + 1);
            if total_bytes.saturating_add(seg_bytes + add_bytes) > MAX_NORM_OUTPUT_BYTES {
                *total_work = max_work;
                return None;
            }
            seg_bytes += add_bytes;
            for _ in 0..node.count {
                segs.push(seg);
            }
            id = node.parent;
        }
        segs.reverse();
        let s = if abs {
            format!("/{}", segs.join("/"))
        } else if segs.is_empty() {
            ".".to_string()
        } else {
            segs.join("/")
        };
        total_bytes += s.len();
        if seen.insert(s.clone()) {
            out.push(s);
        }
    }
    Some(out)
}

#[cfg(test)]
mod glob_norm_tests {
    use super::*;

    #[test]
    fn k10_normalizer_work_is_linear_and_bounded() {
        let star_dd = |n: usize| format!("{}{}", "*/".repeat(n), "../".repeat(n));

        for (n, expected_variants, max_work) in [
            (100usize, 101usize, 350usize),
            (200, 201, 650),
            (255, 256, 800),
        ] {
            let mut work = 0usize;
            let norms =
                normalize_glob_variants(&star_dd(n), MAX_GLOB_VARIANTS, &mut work, MAX_GLOB_WORK)
                    .expect("within MAX_GLOB_VARIANTS");
            assert_eq!(norms.len(), expected_variants, "n={n}");
            assert!(
                work <= max_work,
                "n={n}: expected linear work <= {max_work}, got {work}"
            );
        }

        // Doubling n from 100 to 200 no more than doubles normalizer work.
        let mut w100 = 0usize;
        let mut w200 = 0usize;
        let _ = normalize_glob_variants(&star_dd(100), MAX_GLOB_VARIANTS, &mut w100, MAX_GLOB_WORK);
        let _ = normalize_glob_variants(&star_dd(200), MAX_GLOB_VARIANTS, &mut w200, MAX_GLOB_WORK);
        assert!(
            w200 <= w100 * 2,
            "doubling 100->200 should at most double work: w100={w100}, w200={w200}"
        );

        // 400..20000 */ + .. fail closed at cap=256 in linear O(n) steps.
        for n in [400usize, 2000, 20_000] {
            let mut work = 0usize;
            let norms =
                normalize_glob_variants(&star_dd(n), MAX_GLOB_VARIANTS, &mut work, MAX_GLOB_WORK);
            assert!(norms.is_none(), "n={n} should trip MAX_GLOB_VARIANTS");
            assert!(
                work <= n + MAX_GLOB_VARIANTS + 16,
                "n={n}: expected bounded work <= {}, got {work}",
                n + MAX_GLOB_VARIANTS + 16
            );
        }

        // 500 **/ + 500 .. fails closed in bounded steps.
        let mut ss_work = 0usize;
        let ss_glob = format!("{}{}", "**/".repeat(500), "../".repeat(500));
        assert!(
            normalize_glob_variants(&ss_glob, MAX_GLOB_VARIANTS, &mut ss_work, MAX_GLOB_WORK)
                .is_none()
        );
        assert!(ss_work <= 2_000, "500 **/ + 500 .. work={ss_work}");

        // `search_variants_from_cands` filters out useless `..` variants for ~/proj.
        let cands =
            expand_glob_candidates(&star_dd(200), Path::new("/h/u/proj"), Path::new("/h/u"));
        let (variants, unbounded) = search_variants_from_cands(
            &[PathBuf::from("/h/u/proj")],
            Path::new("/h/u/proj"),
            Path::new("/h/u"),
            &cands,
        );
        assert!(!unbounded);
        assert!(
            variants.len() <= 4,
            "expected filtered variants <= 4, got {}: {variants:?}",
            variants.len()
        );
    }
}

/// Widens `?` and any character class `[...]` that can match `.` to `.`.
fn widen_pattern_dots(s: &str) -> String {
    let bytes = s.as_bytes();
    let last_rb = s.rfind(']');
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        let ch = s[i..].chars().next().unwrap();
        let len = ch.len_utf8();
        if ch == '?' {
            out.push('.');
            i += len;
        } else if ch == '[' && last_rb.is_some_and(|end| i < end) {
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
    let last_rb = s.rfind(']');
    let mut out = String::with_capacity(s.len());
    let mut changed = false;
    let mut i = 0;
    while i < bytes.len() {
        let ch = s[i..].chars().next().unwrap();
        let len = ch.len_utf8();
        if ch == '[' && last_rb.is_some_and(|end| i < end) {
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
    if glob.len() > MAX_BYTES || glob.contains('\0') {
        res.unknown = true;
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
    let home_lower = lower(home);
    let home_has_dotdot = home_lower.split('/').any(|s| s == "..");
    let root_lowers: Vec<String> = roots.iter().map(|r| lower(r)).collect();
    let mut tree_may_hit = false;
    let mut max_tree_slashes = 0usize;
    for t in ASK_TREES {
        let t_abs = format!("{home_lower}/{t}");
        for rl in &root_lowers {
            if under(rl, &t_abs).is_some() {
                continue;
            }
            if let Some(tree_rel) = under(&t_abs, rl) {
                tree_may_hit = true;
                max_tree_slashes = max_tree_slashes
                    .max(tree_rel.bytes().filter(|&b| b == b'/').count())
                    .max(t_abs.bytes().filter(|&b| b == b'/').count());
            }
        }
    }
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut unjoined_norm = 0usize;
    let mut joined_norm = 0usize;
    let mut work = 0usize;
    let mut push_norms = |pat: &str, total: &mut usize| -> bool {
        let remaining = MAX_GLOB_VARIANTS.saturating_sub(*total);
        if remaining == 0 {
            return false;
        }
        let Some(norms) = normalize_glob_variants(pat, remaining, &mut work, MAX_GLOB_WORK) else {
            return false;
        };
        *total += norms.len();
        if *total > MAX_GLOB_VARIANTS {
            return false;
        }
        for n in norms {
            let norm = fold(&n);
            if norm.is_empty() {
                continue;
            }
            // Variants containing a `..` segment cannot match probe paths
            // (and have valid glob syntax when free of `[`/`{`/`\`), so skip
            // them unless `ask_search_glob_hit`'s `tree_targets` prefix check
            // could inspect the segment prefix before `..`.
            if !home_has_dotdot
                && !norm.contains(['[', '{', '\\'])
                && norm.split('/').any(|s| s == "..")
            {
                if !tree_may_hit {
                    continue;
                }
                let mut prefix_segs = 0usize;
                for seg in norm.split('/') {
                    if seg == ".." {
                        break;
                    }
                    if seg != "**" {
                        prefix_segs += 1;
                    }
                }
                if prefix_segs == 0 || prefix_segs > max_tree_slashes + 2 {
                    continue;
                }
            }
            if seen.insert(norm.clone()) {
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
    let mut work = 0usize;
    for gv in &cands.variants {
        check_escape_variant(gv, roots, cwd, home, &mut total_norm, &mut work, &mut out);
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
    work: &mut usize,
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
        let Some(norms) = normalize_glob_variants(&j, remaining, work, MAX_GLOB_WORK) else {
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
