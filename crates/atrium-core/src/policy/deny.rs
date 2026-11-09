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
    /// Text the raw-string checks run on.
    pub raw: Vec<String>,
    /// The starting directory of a command.
    pub start: PathBuf,
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
        raw: vec![],
        start: input.cwd.to_path_buf(),
    };
    let lex_cmd = |a: &mut Analysis, line: &str, start: &Path| {
        let l = shell::lex(line);
        a.units = l.units;
        a.reasons = l.reasons;
        let walk: Walk = words::walk(&a.units, start, home, resolve);
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
        ToolSubject::Paths { paths, .. } => {
            let role = match check.kind {
                ToolKind::Read | ToolKind::Search => Role::Read,
                _ => Role::Write,
            };
            for p in paths {
                pseudo(&mut a, p, role);
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
            for x in s.iter().filter(|x| path_like(x)) {
                pseudo(&mut a, x, Role::Read);
            }
        }
        ToolSubject::None => {}
        ToolSubject::Unknown => {
            let mut s = vec![];
            strings_of(&check.input, &mut s);
            let mut units = vec![];
            for x in &s {
                units.extend(shell::lex(x).units);
                a.raw.push(x.clone());
            }
            a.units = units;
            let walk = words::walk(&a.units, input.cwd, home, resolve);
            a.unit_cwd = walk.unit_cwd.clone();
            a.cwd_unknown = walk.unknown;
            a.uses = walk.uses;
            a.reasons.push("unknown tool input".into());
        }
    }
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
    let s = p.to_string_lossy().to_lowercase();
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
        if role == Role::Write {
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
            if !(public || (readable_config && role == Role::Read)) {
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
const PRIV: &[&str] = &["sudo", "su", "doas", "pkexec", "run0"];

fn is_interpreter(cmd: &str) -> bool {
    let b = base(cmd);
    is_shell(b)
        || b.starts_with("python")
        || matches!(
            b,
            "perl" | "ruby" | "node" | "deno" | "bun" | "php" | "lua" | "osascript" | "pwsh"
        )
}

fn reads_stdin_as_code(eff: &[String]) -> bool {
    let Some(first) = eff.first() else {
        return false;
    };
    let b = base(first);
    let args = &eff[1..];
    if matches!(b, "source" | ".") {
        return args
            .first()
            .is_some_and(|a| matches!(a.as_str(), "/dev/stdin" | "/dev/fd/0" | "-"));
    }
    if b == "eval" || !is_interpreter(b) {
        return false;
    }
    if args.iter().any(|a| a == "-" || a == "-s") {
        return true;
    }
    let code_flag = args.iter().any(|a| {
        a.starts_with('-')
            && !a.starts_with("--")
            && a.len() > 1
            && a[1..].chars().any(|c| matches!(c, 'c' | 'e' | 'E' | 'm'))
            || matches!(a.as_str(), "--eval" | "--command" | "--print")
    });
    if code_flag {
        return false;
    }
    !args.iter().any(|a| !a.starts_with('-'))
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
            match name {
                "--force" | "--force-with-lease" | "--force-if-includes" | "--mirror" => {
                    force = true
                }
                "--all" | "--tags" | "--branches" | "--prune" => implicit = true,
                "--push-option" | "--receive-pack" | "--exec" if !w.contains('=') => j += 1,
                _ => {}
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
        } else if w == "--recursive" || (!w.starts_with("--") && w[1..].contains(['r', 'R'])) {
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
    let mut roots = vec![];
    let mut j = 1;
    while j < eff.len() {
        let w = eff[j].as_str();
        if w.starts_with('-') && w != "-" || w == "!" || w == "(" {
            break;
        }
        roots.push(eff[j].clone());
        j += 1;
    }
    if roots.is_empty() {
        roots.push(".".into());
    }
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
                .unwrap_or(CwdState::Known(self.a.start.clone()));
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
        if a.aligned.is_none() {
            for p in self.input.resolved_paths {
                if let Some(h) = self.s.hit(p, Role::Read) {
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
            && *recursive
        {
            hits.extend(self.search_glob_hit(paths, g));
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
                    (w.starts_with("find-") && w.ends_with("-password")) || w == "dump-keychain"
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
                let downstream = a.units.iter().enumerate().any(|(j, v)| {
                    j > i && v.pipe == u.pipe && v.seg.piped && reads_stdin_as_code(&v.eff)
                });
                if downstream {
                    hits.push("pipe-to-shell");
                }
            }
            if is_executor(eff) && u.subs.iter().any(|s| contains_downloader(s)) {
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

    fn search_glob_hit(&self, roots: &[String], glob: &str) -> Option<&'static str> {
        let g = GlobBuilder::new(glob)
            .literal_separator(false)
            .build()
            .ok()?
            .compile_matcher();
        let root_paths: Vec<PathBuf> = if roots.is_empty() {
            vec![self.input.cwd.to_path_buf()]
        } else {
            roots
                .iter()
                .map(|r| lexical_resolve(r, self.input.cwd, self.home()))
                .collect()
        };
        for probe in self.s.probes() {
            let pp = Path::new(&probe);
            let name = pp.file_name()?.to_string_lossy().to_string();
            if (g.is_match(&name) || g.is_match(&probe))
                && self.s.hit(pp, Role::Read).is_some()
                && root_paths
                    .iter()
                    .any(|r| under(&probe, &lower(r)).is_some())
            {
                return self.s.hit(pp, Role::Read);
            }
        }
        None
    }
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
                let w: Vec<&str> = after.split_whitespace().collect();
                let eff: Vec<String> =
                    shell::unwrap(&w.iter().map(|s| s.to_string()).collect::<Vec<_>>());
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
