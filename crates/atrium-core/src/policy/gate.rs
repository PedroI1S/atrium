//! `SessionGate`: one session's `ToolGate`. Resolves paths (its only
//! filesystem access), asks the engine, opens and redeems ledger entries.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use atrium_agents::{ApprovalTicket, BoxFuture, GateDecision, ToolCheck, ToolGate};
use atrium_protocol::agent::{Provider, ToolKind};
use atrium_protocol::policy::{
    HARD_DENY_DEFAULTS, PolicyContext, Preset, Rule, RuleMatcher, RuleOrigin, ToolSubject,
};
use atrium_protocol::thread::ThreadId;

use super::deny;
use super::engine;
use super::shell;
use super::words::{self, expand_home, normalize};
use super::{SessionGate, input_sha256};
use crate::{ApprovalLedger, Binding, PolicyEngine, PolicyInput, PolicyVerdict, RequestKey, Store};

/// Where a gate reads the current rules from: `(revision, rules)`. `None`
/// when they can't be read; the gate then denies.
pub trait RulesSource: Send + Sync {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>>;
}

/// [`RulesSource`] over the store.
pub struct StoreRules(pub Arc<dyn Store>);

impl RulesSource for StoreRules {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>> {
        Box::pin(async move { self.0.rules().await.ok() })
    }
}

/// What a gate is for: one thread, one incarnation.
#[derive(Debug, Clone)]
pub struct SessionGateConfig {
    pub thread: ThreadId,
    pub incarnation: u32,
    /// Absolute, symlinks resolved.
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub atrium_data_paths: Vec<PathBuf>,
    pub atrium_protected_paths: Vec<PathBuf>,
    pub preset: Preset,
    pub provider: Provider,
}

const MAX_HOPS: u8 = 40;

/// Absolute path with the deepest existing ancestor canonicalized and the
/// rest appended (dangling symlinks are followed by hand).
pub fn fs_resolve(word: &str, cwd: &Path, home: &Path) -> PathBuf {
    let w = expand_home(word, home, cwd);
    let p = Path::new(&w);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    resolve_fs(&abs, 0)
}

fn resolve_fs(p: &Path, hops: u8) -> PathBuf {
    let mut head = p.to_path_buf();
    let mut tail: Vec<OsString> = vec![];
    let base = loop {
        match std::fs::canonicalize(&head) {
            Ok(c) => break c,
            Err(_) => {
                if hops < MAX_HOPS
                    && let Ok(md) = std::fs::symlink_metadata(&head)
                    && md.is_symlink()
                    && let Ok(t) = std::fs::read_link(&head)
                {
                    let target = if t.is_absolute() {
                        t
                    } else {
                        head.parent().unwrap_or(Path::new("/")).join(t)
                    };
                    break resolve_fs(&normalize(&target), hops + 1);
                }
                match head.file_name() {
                    Some(n) => {
                        tail.push(n.to_owned());
                        head.pop();
                    }
                    None => {
                        let n = normalize(&head);
                        if n == head || hops >= MAX_HOPS {
                            break head;
                        }
                        // `missing/..`: start over on the cleaned-up path
                        let mut rebuilt = n;
                        for t in tail.into_iter().rev() {
                            rebuilt.push(t);
                        }
                        return resolve_fs(&rebuilt, hops + 1);
                    }
                }
            }
        }
    };
    let mut out = base;
    for t in tail.into_iter().rev() {
        out.push(t);
    }
    out
}

/// Escapes glob metacharacters so the pattern matches only `p`.
fn escape_glob(p: &Path) -> String {
    let mut s = String::new();
    for c in p.to_string_lossy().chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

fn deny_reason(id: &str) -> String {
    let d = HARD_DENY_DEFAULTS.iter().find(|d| d.id == id);
    format!("Always blocked ({id}): {}", d.map_or("", |d| d.description))
}

#[cfg(unix)]
fn identity(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().map(|m| (m.dev(), m.ino()))
}
#[cfg(not(unix))]
fn identity(_: &Path) -> Option<(u64, u64)> {
    None
}

/// A regular file with more than one link (a hard link, or the original of
/// one). Its other names can't be found cheaply, so it is never auto-allowed.
#[cfg(unix)]
fn multiply_linked_file(p: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_file() && m.nlink() > 1)
}
#[cfg(not(unix))]
fn multiply_linked_file(_: &Path) -> bool {
    false
}

const FIRMLINK: &str = "/System/Volumes/Data";

/// Aliases of sensitive paths (`/.vol/<dev>/<ino>`, the Data firmlink, hard
/// links, case-folded spellings) compare by identity: a path with a prefix
/// that is the same file or folder as a sensitive root is rewritten onto that
/// root, so the pure engine sees the canonical spelling.
fn alias_fix(p: &Path, roots: &[((u64, u64), PathBuf)]) -> Result<PathBuf, String> {
    let s = p.to_string_lossy();
    if s == "/.vol" || s.starts_with("/.vol/") {
        return Err("paths under /.vol can't be checked".into());
    }
    let p = match p.strip_prefix(FIRMLINK) {
        Ok(rest) => Path::new("/").join(rest),
        Err(_) => p.to_path_buf(),
    };
    let mut prefix = PathBuf::new();
    let comps: Vec<_> = p.components().collect();
    for (i, c) in comps.iter().enumerate() {
        prefix.push(c.as_os_str());
        if i == 0 {
            continue;
        }
        if let Some(id) = identity(&prefix)
            && let Some((_, root)) = roots.iter().find(|(rid, _)| *rid == id)
        {
            let mut out = root.clone();
            for rest in &comps[i + 1..] {
                out.push(rest.as_os_str());
            }
            return Ok(normalize(&out));
        }
    }
    Ok(p)
}

/// The members of `~/.ssh` (a small folder): a hard link to a key is the
/// same file. The big trees (browser profiles) aren't walked; a path inside
/// one is caught through its ancestors.
///
/// Every member is identified through links (another name for a key is the
/// key: review M1-13 finding 1). A folder linked into `~/.ssh` is walked, but
/// only the files in it are members, not the folder: listing it or `cd`-ing
/// into it stays ordinary (M1-03 finding 5), reading a file in it doesn't.
/// The real paths of those linked folders are returned too: every file below
/// one, at any depth, is a key, and a recursive search that reaches one reads
/// keys (re-review blocker 1).
struct SshTree {
    members: Vec<PathBuf>,
    linked_dirs: Vec<PathBuf>,
}

fn ssh_members(home: &Path) -> SshTree {
    const MAX_ENTRIES: usize = 1024;
    const MAX_DEPTH: u8 = 8;
    let mut out = SshTree {
        members: vec![],
        linked_dirs: vec![],
    };
    let mut found = 0usize;
    // (folder, depth, reached through a link)
    let mut stack = vec![(home.join(".ssh"), 0u8, false)];
    while let Some((dir, depth, linked)) = stack.pop() {
        if depth >= MAX_DEPTH || found >= MAX_ENTRIES {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten().take(MAX_ENTRIES) {
            found += 1;
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                match std::fs::metadata(&p) {
                    Ok(m) if m.is_dir() => {
                        if let Ok(real) = std::fs::canonicalize(&p)
                            && !out.linked_dirs.contains(&real)
                        {
                            out.linked_dirs.push(real);
                        }
                        stack.push((p, depth + 1, true));
                    }
                    Ok(_) => out.members.push(p),
                    Err(_) => {}
                }
                continue;
            }
            if ft.is_dir() {
                stack.push((p.clone(), depth + 1, linked));
                if linked {
                    continue;
                }
            }
            out.members.push(p);
        }
    }
    out
}

fn evaluate_sync(
    c: &SessionGateConfig,
    policy: &dyn PolicyEngine,
    check: &ToolCheck,
    rules: &[Rule],
    preset: Preset,
) -> Result<Evaluated, String> {
    let mut input = PolicyInput {
        check,
        thread: &c.thread,
        provider: c.provider,
        cwd: &c.cwd,
        home: &c.home,
        atrium_data_paths: &c.atrium_data_paths,
        atrium_protected_paths: &c.atrium_protected_paths,
        resolved_paths: &[],
        preset,
    };
    let analysis = deny::analyze_with(&input, &fs_resolve);
    let ssh = ssh_members(&c.home);
    let mut resolved = vec![];
    let mut ask_ids: Vec<(u64, u64)> = vec![];
    if !analysis.overflow {
        let mut roots: Vec<((u64, u64), PathBuf)> =
            deny::sensitive_roots(&c.home, &c.atrium_data_paths, &c.atrium_protected_paths)
                .into_iter()
                .filter_map(|r| identity(&r).map(|id| (id, r)))
                .collect();
        roots.extend(
            ssh.members
                .iter()
                .filter_map(|r| identity(r).map(|id| (id, r.clone()))),
        );
        // the credential stores that ask (CR-12): the same identity and
        // hard-link handling
        for r in deny::ask_roots(&c.home) {
            if let Some(id) = identity(&r) {
                ask_ids.push(id);
                roots.push((id, r));
            }
        }
        for u in &analysis.uses {
            for p in &u.paths {
                resolved.push(alias_fix(p, &roots)?);
            }
        }
    }
    input.resolved_paths = &resolved;
    let mut verdict = policy.evaluate(&input, rules);
    // A file below a folder linked into `~/.ssh`, at any depth, or a
    // recursive search or glob that reaches such a folder, reads keys.
    // Maybe (options or a glob it can't read): never auto-allow, ask.
    if !matches!(verdict, PolicyVerdict::HardDeny { .. }) && !analysis.overflow {
        match reaches_linked_ssh_dir(check, &analysis, &resolved, &ssh.linked_dirs, &c.cwd) {
            Reach::Yes => {
                verdict = PolicyVerdict::HardDeny {
                    id: "credential-read",
                };
            }
            Reach::Maybe if matches!(verdict, PolicyVerdict::Allow { .. }) => {
                verdict = PolicyVerdict::Ask;
            }
            _ => {}
        }
    }
    // A link from elsewhere into a credential tree (`proj/keys -> ~/.ssh`)
    // or Atrium's own paths, reached through a glob or a search or walker
    // that follows links (round 8 blocker 2, M1-18 K2).
    if !matches!(verdict, PolicyVerdict::HardDeny { .. }) && !analysis.overflow {
        let mut trees = trees_of(
            &deny::sensitive_roots(&c.home, &[], &[]),
            "credential-read",
            true,
        );
        trees.extend(trees_of(&ssh.linked_dirs, "credential-read", true));
        trees.extend(trees_of(&c.atrium_data_paths, "atrium-self", true));
        trees.extend(trees_of(&c.atrium_protected_paths, "atrium-self", false));
        match reaches_tree_through_links(check, &analysis, &resolved, &trees) {
            (Reach::Yes, t) => {
                verdict = PolicyVerdict::HardDeny {
                    id: t.map_or("credential-read", |t| t.id),
                };
            }
            (Reach::Maybe, _) if matches!(verdict, PolicyVerdict::Allow { .. }) => {
                verdict = PolicyVerdict::Ask;
            }
            _ => {}
        }
    }
    // A credential store that asks (CR-12), reached through a link by a
    // glob or a following search: Home never auto-allows it. Rules (a
    // remembered approval) and FullAccess keep their Allow.
    if matches!(verdict, PolicyVerdict::Allow { .. })
        && !analysis.overflow
        && matches!(preset, Preset::Home | Preset::Normal | Preset::Trusted)
    {
        let asks = trees_of(&deny::ask_roots(&c.home), "credential-read", true);
        if reaches_tree_through_links(check, &analysis, &resolved, &asks).0 != Reach::No {
            let careful = PolicyInput {
                preset: Preset::Careful,
                ..input
            };
            if !matches!(
                policy.evaluate(&careful, rules),
                PolicyVerdict::Allow { .. }
            ) {
                verdict = PolicyVerdict::Ask;
            }
        }
    }
    // A file with several links may be another name for a credential in a
    // tree the identity map doesn't walk: never auto-allow it, ask instead,
    // except when an explicit rule authorizes a positively identified ask-store alias.
    if matches!(verdict, PolicyVerdict::Allow { .. }) && !analysis.overflow {
        let rule_allowed = matches!(
            policy.evaluate(
                &PolicyInput {
                    preset: Preset::Careful,
                    ..input
                },
                rules,
            ),
            PolicyVerdict::Allow { .. }
        );
        let known_ask_ids: &[(u64, u64)] = if rule_allowed { &ask_ids } else { &[] };
        if reaches_multiply_linked(check, &analysis, &resolved, known_ask_ids) {
            verdict = PolicyVerdict::Ask;
        }
    }
    Ok(Evaluated { verdict, resolved })
}

/// Commands that read every file below a folder they are given. `grep`'s
/// spellings are read by the same parser Home's read-only list uses, so the
/// two can't drift; a `grep` line it can't parse counts as recursive.
fn recursive_reader(eff: &[String]) -> bool {
    let Some(first) = eff.first() else {
        return false;
    };
    match shell::base(first) {
        "rg" | "ag" | "ack" | "rgrep" | "find" => true,
        "grep" | "egrep" | "fgrep" | "zgrep" | "ggrep" => {
            engine::search_roots(false, &eff[1..]).is_none_or(|s| s.recursive)
        }
        _ => false,
    }
}

/// The positional words of an option-taking command, read with an option
/// table, for each way it can be read: a short option whose value is
/// optional (`ack -C [N]`) followed by a separate word gives two readings.
/// `None`: an option the table doesn't know.
struct ReaderOpts {
    flags: &'static str,
    vals: &'static str,
    opt_vals: &'static str,
    long_vals: &'static [&'static str],
    /// Long options whose value is optional (`ag --context [N]`, `ack
    /// --context[=N]`): `--name=v` takes it, `--name v` is read both ways.
    long_opt_vals: &'static [&'static str],
    long_flags: &'static [&'static str],
    /// Short options whose value is the pattern (`ag -g PAT`, `ack -g RE`)
    /// or that take no pattern (`ack -f`): the positional words are paths.
    no_pattern: &'static str,
    /// The same for long options (`ag --filename-pattern PAT`, `ack --match
    /// RE`): round 8, review major 6.
    long_no_pattern: &'static [&'static str],
}

fn readings(args: &[String], o: &ReaderOpts, prefixes: bool) -> Option<Vec<(Vec<String>, bool)>> {
    // (positional words, the pattern came from an option, the next word is
    // an optional value this reading takes)
    let mut out: Vec<(Vec<String>, bool, bool)> = vec![(vec![], false, false)];
    let mut j = 0;
    let mut dd = false;
    while j < args.len() {
        let w = args[j].as_str();
        j += 1;
        if dd || !w.starts_with('-') || w == "-" {
            for r in &mut out {
                if r.2 {
                    r.2 = false;
                } else {
                    r.0.push(w.to_string());
                }
            }
            continue;
        }
        for r in &mut out {
            r.2 = false;
        }
        if w == "--" {
            dd = true;
            continue;
        }
        if let Some(long) = w.strip_prefix("--") {
            let (name, has_val) = match long.split_once('=') {
                Some((n, _)) => (n, true),
                None => (long, false),
            };
            // `--fol` is `--follow` when no other name starts so (GNU
            // `getopt_long`, Perl `Getopt::Long`: M1-18, K2)
            let all: Vec<&str> = o
                .long_vals
                .iter()
                .chain(o.long_opt_vals)
                .chain(o.long_flags)
                .copied()
                .collect();
            let full = if prefixes && !name.is_empty() && !all.contains(&name) {
                let mut hits = all.iter().filter(|n| n.starts_with(name));
                match (hits.next(), hits.next()) {
                    (Some(f), None) => *f,
                    _ => name,
                }
            } else {
                name
            };
            let name = full;
            if o.long_no_pattern.contains(&name) {
                for r in &mut out {
                    r.1 = true;
                }
            }
            if o.long_vals.contains(&name) {
                if !has_val {
                    j += 1;
                }
            } else if o.long_opt_vals.contains(&name) {
                if !has_val {
                    if out.len() >= 16 {
                        return None;
                    }
                    let with: Vec<(Vec<String>, bool, bool)> =
                        out.iter().map(|r| (r.0.clone(), r.1, true)).collect();
                    out.extend(with);
                }
            } else if !o.long_flags.contains(&name) {
                return None;
            }
            continue;
        }
        let cs: Vec<char> = w[1..].chars().collect();
        for (k, c) in cs.iter().enumerate() {
            let attached = k + 1 < cs.len();
            if o.no_pattern.contains(*c) {
                for r in &mut out {
                    r.1 = true;
                }
            }
            if o.vals.contains(*c) {
                if !attached {
                    j += 1;
                }
                break;
            }
            if o.opt_vals.contains(*c) {
                if !attached {
                    if out.len() >= 16 {
                        return None;
                    }
                    // with the value (it takes the next word), and without
                    let with: Vec<(Vec<String>, bool, bool)> =
                        out.iter().map(|r| (r.0.clone(), r.1, true)).collect();
                    out.extend(with);
                }
                break;
            }
            if !o.flags.contains(*c) {
                return None;
            }
        }
    }
    Some(out.into_iter().map(|r| (r.0, r.1)).collect())
}

// The Silver Searcher's `getopt_long` table (`src/options.c`): `-A`, `-B`,
// `-C` and their long names take an optional number (read both ways).
const AG_OPTS: ReaderOpts = ReaderOpts {
    flags: "aDcFfHhiLlnoQRrSsvVtuUwz0",
    vals: "GgmpW",
    opt_vals: "ABC",
    long_vals: &[
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
    long_opt_vals: &["after", "before", "context"],
    long_flags: &[
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
        "files-without-matches",
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
    no_pattern: "g",
    long_no_pattern: &["filename-pattern"],
};

// ack 3's `Getopt::Long` table: `-C`/`--context` take an optional number
// (read both ways); `-A`/`-B` and their long names need one.
const ACK_OPTS: ReaderOpts = ReaderOpts {
    flags: "iISvwQlLchHo1nrRsxfk",
    vals: "ABmgt",
    opt_vals: "C",
    long_vals: &[
        "after-context",
        "before-context",
        "type",
        "ignore-dir",
        "noignore-dir",
        "match",
        "output",
        "max-count",
    ],
    long_opt_vals: &["context"],
    long_flags: &[
        "ignore-case",
        "smart-case",
        "invert-match",
        "word-regexp",
        "literal",
        "files-with-matches",
        "files-without-matches",
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
    no_pattern: "gf",
    long_no_pattern: &["match"],
};

/// Whether a recursive reader searches the directory it runs in, because it
/// was given no folder (round 6 item 2): `Some(true)` under some reading of
/// its options, `None` when its options can't be read (the caller asks).
/// `find` reads its roots with `shell::find_roots_explicit`, `rg` and
/// `grep` with the parser Home's read-only list uses (`-R` read as `-r`),
/// `ag` and `ack` with their option tables.
fn searches_cwd(eff: &[String]) -> Option<bool> {
    searches_cwd_with(eff, false)
}

/// [`searches_cwd`], with abbreviated long options read by unique prefix
/// (`grep --deref`, `ag --fol`) when `prefixes`: for the inbound-link walk,
/// which `dev` didn't run for those spellings at all (M1-18, K2).
fn searches_cwd_with(eff: &[String], prefixes: bool) -> Option<bool> {
    let first = eff.first()?;
    let args = &eff[1..];
    // `-f FILE` / `--file FILE` give the patterns like `-e`: every
    // positional word is then a root (round 8, beside major 6)
    let mut dd = false;
    let args: Vec<String> = args
        .iter()
        .map(|a| {
            if dd || !a.starts_with('-') {
                return a.clone();
            }
            if a == "--" {
                dd = true;
                return a.clone();
            }
            if a == "--file" {
                "--regexp".into()
            } else if let Some(v) = a.strip_prefix("--file=") {
                format!("--regexp={v}")
            } else if !a.starts_with("--")
                && matches!(
                    shell::base(first),
                    "rg" | "grep" | "egrep" | "fgrep" | "zgrep" | "ggrep" | "rgrep"
                )
            {
                a.replacen('f', "e", 1)
            } else {
                a.clone()
            }
        })
        .collect();
    let args = args.as_slice();
    match shell::base(first) {
        // `-L` / `--follow` change which files, not where it searches
        // (links below the roots are `follows_links`'s business); the
        // parser pairs flags with values, so `-e -L` keeps `-L` a value
        "rg" => engine::rg_roots_following(args).map(|s| s.roots.is_empty()),
        b @ ("grep" | "egrep" | "fgrep" | "zgrep" | "ggrep" | "rgrep") => {
            // `-R` follows every link below the roots: the same roots
            let args: Vec<String> = args
                .iter()
                .map(|a| {
                    if a.starts_with('-') && !a.starts_with("--") {
                        a.replace('R', "r")
                    } else if a == "--dereference-recursive"
                        || (prefixes
                            && long_is(
                                a,
                                "dereference-recursive",
                                &engine::grep_long_names().collect::<Vec<_>>(),
                            ))
                    {
                        "--recursive".into()
                    } else {
                        a.clone()
                    }
                })
                .collect();
            engine::search_roots(false, &args)
                .map(|s| (s.recursive || b == "rgrep") && s.roots.is_empty())
        }
        b @ ("ag" | "ack") => {
            let o = if b == "ag" { &AG_OPTS } else { &ACK_OPTS };
            let cwd = |rs: Vec<(Vec<String>, bool)>| {
                rs.iter()
                    .any(|(ws, no_pattern)| ws.len() <= usize::from(!*no_pattern))
            };
            match readings(args, o, prefixes) {
                Some(rs) => Some(cwd(rs)),
                // abbreviated long options (`--fol`) can only make it surer
                // that it searches here (M1-18, K2); otherwise it asks
                None => readings(args, o, true)
                    .filter(|rs| cwd(rs.clone()))
                    .map(|_| true),
            }
        }
        "find" => Some(!shell::find_roots_explicit(eff).2),
        _ => Some(false),
    }
}

/// Commands that walk every file below a folder they are given and copy,
/// pack, move or delete it (round 5 blocker 1): `cp -r`, `scp -r`, `rsync`,
/// `tar`, `zip -r`, `rm -r`, `mv`, `ditto`, `pax`, `cpio`, `7z`.
fn tree_walker(eff: &[String]) -> bool {
    let Some(first) = eff.first() else {
        return false;
    };
    let args = &eff[1..];
    let short = |letters: &[char]| {
        args.iter().any(|a| {
            a.starts_with('-')
                && !a.starts_with("--")
                && a[1..].chars().any(|c| letters.contains(&c))
        })
    };
    let long = |names: &[&str]| {
        args.iter().any(|a| {
            names
                .iter()
                .any(|n| a == n || a.starts_with(&format!("{n}=")))
        })
    };
    match shell::base(first) {
        "rsync" | "tar" | "bsdtar" | "gtar" | "mv" | "ditto" | "pax" | "cpio" | "7z" | "7za"
        | "rclone" => true,
        "cp" | "gcp" | "scp" => short(&['r', 'R', 'a']) || long(&["--recursive", "--archive"]),
        "rm" | "grm" => short(&['r', 'R']) || long(&["--recursive"]),
        "zip" => short(&['r', 'R']) || long(&["--recurse-paths"]),
        _ => false,
    }
}

/// Whether a call reaches a folder linked into `~/.ssh`: surely, maybe (it
/// asks), or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    No,
    Maybe,
    Yes,
}

/// What a glob expands to on the filesystem, as the shell would expand it
/// (round 6 item 1): component by component from its literal prefix, `**`
/// as zero or more folders, `{a,b}` as alternation. Bounded: `None` when the
/// walk is too big or a component can't be read as a pattern (the caller
/// asks).
fn expand_glob(p: &Path) -> Option<Vec<PathBuf>> {
    const MAX_ENTRIES: usize = 4096;
    const MAX_DEPTH: usize = 12;
    let comps: Vec<String> = p
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let is_pat = |c: &str| c.contains(['*', '?', '[', '{']);
    // A `$`, a backtick or a brace left in any component (round 7 item 1):
    // the shell expands it to something the pattern doesn't show
    // (`~/${DIR}/work/*`, `~/$(echo v*)/w`); comma lists and ranges were
    // expanded before (`words::brace_expand`), so a brace still here is one
    // this walk can't read.
    if comps.iter().any(|c| c.contains(['$', '`', '{', '}'])) {
        return None;
    }
    let first = comps.iter().position(|c| is_pat(c))?;
    let prefix: PathBuf = comps[..first].iter().collect();
    let mut seen = 0usize;
    // (folder, index of the next component, folders `**` went down)
    let mut stack: Vec<(PathBuf, usize, usize)> = vec![(prefix, first, 0)];
    let mut out = vec![];
    while let Some((dir, k, deep)) = stack.pop() {
        if k == comps.len() {
            out.push(dir);
            continue;
        }
        let c = comps[k].as_str();
        if !is_pat(c) {
            let next = dir.join(c);
            // a glob only expands to names that exist
            if std::fs::symlink_metadata(&next).is_ok() {
                stack.push((next, k + 1, deep));
            }
            continue;
        }
        if c == "**" {
            // zero folders here, or one more and `**` again
            stack.push((dir.clone(), k + 1, deep));
            if deep > MAX_DEPTH {
                return None;
            }
        }
        let m = globset::GlobBuilder::new(c)
            .literal_separator(true)
            .build()
            .ok()?
            .compile_matcher();
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return None;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && !c.starts_with('.') {
                continue;
            }
            let path = dir.join(&name);
            if c == "**" {
                if stack.len() > MAX_ENTRIES {
                    return None;
                }
                // `**` ends at this entry, or goes on below it
                stack.push((path.clone(), k + 1, deep));
                if path.is_dir() {
                    stack.push((path, k, deep + 1));
                }
            } else if m.is_match(&name) {
                stack.push((path, k + 1, deep));
            }
        }
    }
    Some(out)
}

/// Whether a glob reaches a file inside the linked folder `d`, or with
/// `names_dir` (a recursive reader or walker) names `d` or a folder holding
/// it. `ls vault/*` only names the folder: it stays ordinary.
fn glob_reaches(p: &Path, d: &Path, names_dir: bool) -> Reach {
    let Some(found) = expand_glob(p) else {
        return Reach::Maybe;
    };
    for f in found {
        let real = real_path(&f);
        if real.starts_with(d) && real != *d {
            return Reach::Yes;
        }
        if names_dir && d.starts_with(&real) {
            return Reach::Yes;
        }
    }
    Reach::No
}

/// The real path of `p`, or of its folder when `p` itself doesn't resolve
/// (a glob, a file not made yet).
fn real_path(p: &Path) -> PathBuf {
    if let Ok(r) = std::fs::canonicalize(p) {
        return r;
    }
    if let (Some(dir), Some(name)) = (p.parent(), p.file_name())
        && let Ok(r) = std::fs::canonicalize(dir)
    {
        return r.join(name);
    }
    p.to_path_buf()
}

/// Whether the call reaches a folder linked into `~/.ssh`: a file below one
/// (any depth), a glob in one, or a recursive search of one or of a folder
/// holding one (re-review blocker 1). Listing or entering the folder stays
/// ordinary.
fn reaches_linked_ssh_dir(
    check: &ToolCheck,
    a: &deny::Analysis,
    resolved: &[PathBuf],
    linked: &[PathBuf],
    cwd: &Path,
) -> Reach {
    if linked.is_empty() {
        return Reach::No;
    }
    let recursive_paths = matches!(
        &check.subject,
        ToolSubject::Paths {
            recursive: true,
            ..
        }
    );
    let reaches_root = |root: &Path| {
        let real = real_path(root);
        linked
            .iter()
            .any(|d| real.starts_with(d) || d.starts_with(&real))
    };
    // A search tool given no paths searches the session's folder (round 5
    // blocker 1).
    if let ToolSubject::Paths {
        paths,
        recursive: true,
        ..
    } = &check.subject
        && paths.is_empty()
        && reaches_root(cwd)
    {
        return Reach::Yes;
    }
    // A recursive reader with no folder operand searches where it runs: each
    // directory the walk is sure of (an uncertain one keeps D-078's verdict).
    // Options it can't read: it may, so it asks (round 6 item 2).
    let mut maybe = false;
    for (i, u) in a.units.iter().enumerate() {
        if !recursive_reader(&u.eff) {
            continue;
        }
        let Some(words::CwdState::Known(cands)) = a.unit_cwd.get(i) else {
            continue;
        };
        if !cands.iter().any(|c| reaches_root(c)) {
            continue;
        }
        match searches_cwd(&u.eff) {
            Some(true) => return Reach::Yes,
            Some(false) => {}
            None => maybe = true,
        }
    }
    let mut k = 0usize;
    for u in &a.uses {
        let unit = a.units.get(u.unit);
        let command_word = unit.is_some_and(|x| x.seg.argv.first() == Some(&u.word));
        let recursive = recursive_paths
            || unit.is_some_and(|x| recursive_reader(&x.eff) || tree_walker(&x.eff));
        let globby = u.word.contains(['*', '?', '[', '{']);
        for _ in &u.paths {
            let Some(p) = resolved.get(k) else {
                return Reach::Maybe;
            };
            k += 1;
            if command_word {
                continue;
            }
            // a word the shell expands at run time (`$X`, a backtick) may
            // name anything (round 7 item 1)
            if p.to_string_lossy().contains(['$', '`']) {
                maybe = true;
                continue;
            }
            if globby {
                for d in linked {
                    match glob_reaches(p, d, recursive) {
                        Reach::Yes => return Reach::Yes,
                        Reach::Maybe => maybe = true,
                        Reach::No => {}
                    }
                }
            }
            let real = real_path(p);
            let is_dir = real.is_dir();
            for d in linked {
                let inside = real.starts_with(d) && real != *d;
                if inside && !is_dir {
                    return Reach::Yes;
                }
                if globby && real.parent().is_some_and(|parent| parent.starts_with(d)) {
                    return Reach::Yes;
                }
                if recursive && (real.starts_with(d) || d.starts_with(&real)) {
                    return Reach::Yes;
                }
            }
        }
    }
    if maybe { Reach::Maybe } else { Reach::No }
}

/// Whether a command follows the links it meets below the folders it is
/// given: surely, maybe (it asks), or not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Follow {
    No,
    Maybe,
    Yes,
}

/// A long option as written (`--fol`, `--deref=x`) is `full`: its name, or
/// a prefix of it no other name in `names` shares (GNU `getopt_long`, Perl
/// `Getopt::Long`: M1-18, K2).
fn long_is(arg: &str, full: &str, names: &[&str]) -> bool {
    let Some(name) = arg.strip_prefix("--") else {
        return false;
    };
    let name = name.split_once('=').map_or(name, |(n, _)| n);
    if name.is_empty() {
        return false;
    }
    name == full
        || (full.starts_with(name) && !names.iter().any(|n| *n != full && n.starts_with(name)))
}

/// Whether a search or a walker follows links below its roots (`rg -L`,
/// `grep -R`, `find -L`, `find … -follow`, `ag -f`, `ack --follow`, `cp -L`,
/// `tar -h`, `rsync -L`, `zip -r`, `scp -r`), with long options read by
/// unique prefix (`grep --deref`, `ag --fol`: M1-18, K2).
fn reader_flag_tokens<'a>(
    args: &'a [String],
    bool_s: &str,
    val_s: &str,
    bool_l: &[&str],
    val_l: &[&str],
    allow_long_prefix: bool,
) -> Option<(Vec<char>, Vec<&'a str>)> {
    let mut shorts = vec![];
    let mut longs = vec![];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a == "--" {
            break;
        }
        i += 1;
        if !a.starts_with('-') || a == "-" {
            continue;
        }
        if let Some(rest) = a.strip_prefix("--") {
            let (name, has_val) = rest
                .split_once('=')
                .map_or((rest, false), |(n, _)| (n, true));
            let all: Vec<&str> = bool_l.iter().chain(val_l.iter()).copied().collect();
            let is_val = val_l.contains(&name)
                || (allow_long_prefix && val_l.iter().any(|f| long_is(a, f, &all)));
            let is_bool = bool_l.contains(&name)
                || (allow_long_prefix && bool_l.iter().any(|f| long_is(a, f, &all)));
            if is_val {
                longs.push(a);
                if !has_val {
                    i += 1;
                }
            } else if is_bool {
                longs.push(a);
            } else {
                return None;
            }
        } else {
            let cs: Vec<char> = a[1..].chars().collect();
            for (k, c) in cs.iter().enumerate() {
                if val_s.contains(*c) {
                    shorts.push(*c);
                    if k + 1 == cs.len() {
                        i += 1;
                    }
                    break;
                }
                if !bool_s.contains(*c) {
                    return None;
                }
                shorts.push(*c);
            }
        }
    }
    Some((shorts, longs))
}

fn follows_links(eff: &[String]) -> Follow {
    let Some(first) = eff.first() else {
        return Follow::No;
    };
    let args = &eff[1..];
    let opts = || {
        args.iter()
            .take_while(|a| a.as_str() != "--")
            .filter(|a| a.starts_with('-'))
    };
    let short = |c: char| opts().any(|a| !a.starts_with("--") && a[1..].contains(c));
    let long = |full: &str, names: &[&str]| opts().any(|a| long_is(a, full, names));
    let yes = |b: bool| if b { Follow::Yes } else { Follow::No };
    let grep_longs: Vec<&str> = ["recursive", "dereference-recursive"]
        .into_iter()
        .chain(engine::grep_long_names())
        .collect();
    match shell::base(first) {
        "rg" => {
            let bool_s = format!("{}L", engine::RG_BOOL_S);
            let val_s = format!("{}f", engine::RG_VAL_S);
            let bool_l: Vec<&str> = std::iter::once("follow")
                .chain(engine::RG_BOOL_L.iter().copied())
                .collect();
            let val_l: Vec<&str> = std::iter::once("file")
                .chain(engine::RG_VAL_L.iter().copied())
                .collect();
            if let Some((shorts, longs)) =
                reader_flag_tokens(args, &bool_s, &val_s, &bool_l, &val_l, false)
            {
                yes(shorts.contains(&'L') || longs.contains(&"--follow"))
            } else {
                yes(short('L') || args.iter().any(|a| a == "--follow"))
            }
        }
        "grep" | "egrep" | "fgrep" | "zgrep" | "ggrep" | "rgrep" => {
            let val_s = format!("{}f", engine::GREP_VAL_S);
            let val_l: Vec<&str> = std::iter::once("file")
                .chain(engine::GREP_VAL_L.iter().copied())
                .collect();
            if let Some((shorts, longs)) = reader_flag_tokens(
                args,
                engine::GREP_BOOL_S,
                &val_s,
                engine::GREP_BOOL_L,
                &val_l,
                true,
            ) {
                yes(shorts.contains(&'R')
                    || longs
                        .iter()
                        .any(|a| long_is(a, "dereference-recursive", &grep_longs)))
            } else {
                yes(short('R') || long("dereference-recursive", &grep_longs))
            }
        }
        "find" => yes(args.iter().any(|a| a == "-L" || a == "-follow")),
        "ag" => {
            let bool_s = format!("{}{}", AG_OPTS.flags, AG_OPTS.opt_vals);
            let bool_l: Vec<&str> = AG_OPTS.long_flags.to_vec();
            let val_l: Vec<&str> = AG_OPTS
                .long_vals
                .iter()
                .chain(AG_OPTS.long_opt_vals.iter())
                .copied()
                .collect();
            if let Some((shorts, longs)) =
                reader_flag_tokens(args, &bool_s, AG_OPTS.vals, &bool_l, &val_l, true)
            {
                yes(shorts.contains(&'f')
                    || longs.iter().any(|a| long_is(a, "follow", &ag_long_names())))
            } else {
                yes(short('f') || long("follow", &ag_long_names()))
            }
        }
        "ack" => {
            let bool_s = format!("{}{}", ACK_OPTS.flags, ACK_OPTS.opt_vals);
            let bool_l: Vec<&str> = ACK_OPTS.long_flags.to_vec();
            let val_l: Vec<&str> = ACK_OPTS
                .long_vals
                .iter()
                .chain(ACK_OPTS.long_opt_vals.iter())
                .copied()
                .collect();
            if let Some((_, longs)) =
                reader_flag_tokens(args, &bool_s, ACK_OPTS.vals, &bool_l, &val_l, true)
            {
                yes(longs
                    .iter()
                    .any(|a| long_is(a, "follow", &ack_long_names())))
            } else {
                yes(long("follow", &ack_long_names()))
            }
        }
        // GNU `cp -L/--dereference`, BSD `cp -L`; `cp -r` follows links on
        // some systems (BSD's legacy `-r`): maybe
        "cp" | "gcp" => {
            if short('L') || long("dereference", &["debug", "dereference"]) {
                Follow::Yes
            } else if short('r') && !short('P') && !short('d') && !short('a') {
                Follow::Maybe
            } else {
                Follow::No
            }
        }
        "tar" | "bsdtar" | "gtar" => {
            // `tar -chzf`, `tar chf` (old style), `--dereference`
            let old_style = args.first().is_some_and(|a| {
                !a.starts_with('-') && a.contains('h') && a.contains(['c', 'r', 'u'])
            });
            yes(short('h')
                || old_style
                || long("dereference", &["delete", "dereference", "dir-mode"]))
        }
        "rsync" => yes(short('L')
            || short('k')
            || opts().any(|a| {
                long_is(
                    a,
                    "copy-links",
                    &[
                        "copy-links",
                        "copy-dirlinks",
                        "copy-unsafe-links",
                        "copy-as",
                        "copy-dest",
                        "copy-devices",
                    ],
                ) || long_is(
                    a,
                    "copy-dirlinks",
                    &[
                        "copy-links",
                        "copy-dirlinks",
                        "copy-unsafe-links",
                        "copy-as",
                        "copy-dest",
                        "copy-devices",
                    ],
                ) || long_is(
                    a,
                    "copy-unsafe-links",
                    &[
                        "copy-links",
                        "copy-dirlinks",
                        "copy-unsafe-links",
                        "copy-as",
                        "copy-dest",
                        "copy-devices",
                    ],
                )
            })),
        // `zip -r` stores what links point at unless `-y`/`--symlinks`
        "zip" => yes((short('r')
            || short('R')
            || long("recurse-paths", &["recurse-paths", "recurse-patterns"]))
            && !short('y')
            && !long("symlinks", &["symlinks"])),
        // OpenSSH `scp -r` follows the links it meets
        "scp" => yes(short('r')),
        "pax" | "cpio" => yes(short('L') || long("dereference", &["dereference"])),
        "rclone" => yes(short('L') || long("copy-links", &["copy-links"])),
        "7z" | "7za" => Follow::Maybe,
        _ => Follow::No,
    }
}

/// The long option names of `ag` and `ack`, for [`long_is`].
fn ag_long_names() -> Vec<&'static str> {
    AG_OPTS
        .long_vals
        .iter()
        .chain(AG_OPTS.long_opt_vals)
        .chain(AG_OPTS.long_flags)
        .copied()
        .collect()
}

fn ack_long_names() -> Vec<&'static str> {
    ACK_OPTS
        .long_vals
        .iter()
        .chain(ACK_OPTS.long_opt_vals)
        .chain(ACK_OPTS.long_flags)
        .copied()
        .collect()
}

/// A place a link may lead into: a credential tree or file (reading or
/// writing it is a hard deny), Atrium's data (the same) or Atrium's
/// protected paths (only writing), or a credential store that asks (CR-12).
struct Tree {
    path: PathBuf,
    id: &'static str,
    /// Reading counts, not only writing.
    read: bool,
    /// A recursive reader given a folder that holds it reaches it (the
    /// credential trees, as since round 8); for Atrium's paths only the
    /// folder itself counts.
    held: bool,
}

fn trees_of(paths: &[PathBuf], id: &'static str, read: bool) -> Vec<Tree> {
    paths
        .iter()
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .map(|path| Tree {
            path,
            id,
            read,
            held: id == "credential-read",
        })
        .collect()
}

/// The tree a real path is in (or, with `names_dir`, names or holds), for a
/// use of this role.
fn in_trees<'a>(real: &Path, trees: &'a [Tree], names_dir: bool, write: bool) -> Option<&'a Tree> {
    trees.iter().find(|t| {
        (t.read || write)
            // a credential file itself (`~/.aws/credentials`) counts; a
            // tree's own folder only for a reader that walks into it
            && ((real.starts_with(&t.path) && (real != t.path || !real.is_dir()))
                || (names_dir && (t.path == real || (t.held && t.path.starts_with(real)))))
    })
}

/// A bounded walk below `root`, following the links it meets the way a
/// following search does (a link to a folder is walked too, at any number
/// of hops: M1-18, K2), for a link into a tree: Yes (with the tree), No, or
/// Maybe when the walk is too big.
fn links_below<'a>(root: &Path, trees: &'a [Tree]) -> (Reach, Option<&'a Tree>) {
    const BUDGET: usize = 4096;
    const MAX_DEPTH: u8 = 16;
    let mut seen = 0usize;
    let mut visited: Vec<PathBuf> = vec![];
    if let Ok(r) = std::fs::canonicalize(root) {
        visited.push(r);
    }
    let mut stack = vec![(root.to_path_buf(), 0u8)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir()
                && matches!(
                    e.file_name().to_str(),
                    Some(".git" | "node_modules" | "target")
                )
            {
                continue;
            }
            seen += 1;
            if seen > BUDGET || depth > MAX_DEPTH {
                return (Reach::Maybe, None);
            }
            let p = e.path();
            if ft.is_symlink() {
                let Ok(real) = std::fs::canonicalize(&p) else {
                    continue;
                };
                if let Some(t) = in_trees(&real, trees, true, false) {
                    return (Reach::Yes, Some(t));
                }
                // a link to a folder elsewhere: walk it too, once
                if real.is_dir() && !visited.contains(&real) {
                    visited.push(real.clone());
                    stack.push((real, depth + 1));
                }
            } else if ft.is_dir() {
                stack.push((p, depth + 1));
            }
        }
    }
    (Reach::No, None)
}

/// Whether the call reaches a tree through a link from elsewhere (round 8
/// blocker 2, M1-18 K2): a glob whose matches resolve into one, or a search
/// or walker that follows links below its roots (or where it runs) meeting
/// a link into one. A glob that can't be expanded may reach one (Maybe).
fn reaches_tree_through_links<'a>(
    check: &ToolCheck,
    a: &deny::Analysis,
    resolved: &[PathBuf],
    trees: &'a [Tree],
) -> (Reach, Option<&'a Tree>) {
    if trees.is_empty() {
        return (Reach::No, None);
    }
    let mut maybe = false;
    let recursive_paths = matches!(
        &check.subject,
        ToolSubject::Paths {
            recursive: true,
            ..
        }
    );
    for (i, u) in a.units.iter().enumerate() {
        let follow = follows_links(&u.eff);
        if !recursive_reader(&u.eff) || follow == Follow::No {
            continue;
        }
        // options it can't read: it may search there (it asks)
        let sure = match searches_cwd_with(&u.eff, true) {
            Some(false) => continue,
            Some(true) => follow == Follow::Yes,
            None => false,
        };
        if let Some(words::CwdState::Known(cands)) = a.unit_cwd.get(i) {
            for c in cands {
                match links_below(c, trees) {
                    (Reach::Yes, t) if sure => return (Reach::Yes, t),
                    (Reach::No, _) => {}
                    _ => maybe = true,
                }
            }
        }
    }
    let mut k = 0usize;
    for u in &a.uses {
        let unit = a.units.get(u.unit);
        let command_word = unit.is_some_and(|x| x.seg.argv.first() == Some(&u.word));
        let recursive = recursive_paths
            || unit.is_some_and(|x| recursive_reader(&x.eff) || tree_walker(&x.eff));
        let follow = unit.map_or(Follow::No, |x| follows_links(&x.eff));
        let write = u.role != words::Role::Read;
        // `ls proj/*` only names what it lists
        let lister = !recursive
            && unit.is_some_and(|x| x.eff.first().is_some_and(|f| shell::base(f) == "ls"));
        let globby = u.word.contains(['*', '?', '[']);
        for _ in &u.paths {
            let Some(p) = resolved.get(k) else {
                return (Reach::Maybe, None);
            };
            k += 1;
            if command_word {
                continue;
            }
            let mut targets = vec![p.clone()];
            if globby {
                match expand_glob(p) {
                    Some(found) => targets = found,
                    // a glob this walk can't expand may match anything
                    // (M1-18, K2)
                    None => maybe = true,
                }
            }
            for f in &targets {
                if globby && !lister {
                    let real = real_path(f);
                    if let Some(t) = in_trees(&real, trees, recursive, write) {
                        return (Reach::Yes, Some(t));
                    }
                }
                if recursive && follow != Follow::No && f.is_dir() {
                    match links_below(f, trees) {
                        (Reach::Yes, t) if follow == Follow::Yes => return (Reach::Yes, t),
                        (Reach::No, _) => {}
                        _ => maybe = true,
                    }
                }
            }
        }
    }
    if maybe {
        (Reach::Maybe, None)
    } else {
        (Reach::No, None)
    }
}

/// Whether the call reads a regular file with more than one link: a named
/// file (anywhere, but not a command being run), a file a glob's folder holds,
/// or a file below a folder a recursive search is given (review M1-13
/// finding 11). The search walk is bounded and skips `node_modules` and
/// `.git` (package managers and local clones hard-link their files there).
fn is_known_ask_file(p: &Path, known_ask_ids: &[(u64, u64)]) -> bool {
    !known_ask_ids.is_empty() && identity(p).is_some_and(|id| known_ask_ids.contains(&id))
}

fn reaches_multiply_linked(
    check: &ToolCheck,
    a: &deny::Analysis,
    resolved: &[PathBuf],
    known_ask_ids: &[(u64, u64)],
) -> bool {
    let recursive_paths = matches!(
        &check.subject,
        ToolSubject::Paths {
            recursive: true,
            ..
        }
    );
    let mut k = 0usize;
    for u in &a.uses {
        let unit = a.units.get(u.unit);
        let command_word = unit.is_some_and(|x| x.seg.argv.first() == Some(&u.word));
        let recursive = recursive_paths || unit.is_some_and(|x| recursive_reader(&x.eff));
        let globby = u.word.contains(['*', '?', '[']);
        for _ in &u.paths {
            let Some(p) = resolved.get(k) else {
                return false;
            };
            k += 1;
            if command_word {
                continue;
            }
            if multiply_linked_file(p) && !is_known_ask_file(p, known_ask_ids) {
                return true;
            }
            if globby
                && let Some(dir) = p.parent()
                && tree_has_multiply_linked(dir, 1, known_ask_ids)
            {
                return true;
            }
            if recursive && p.is_dir() && tree_has_multiply_linked(p, 8, known_ask_ids) {
                return true;
            }
        }
    }
    false
}

fn tree_has_multiply_linked(root: &Path, max_depth: u8, known_ask_ids: &[(u64, u64)]) -> bool {
    const BUDGET: usize = 4096;
    let mut seen = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0u8)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > BUDGET {
                return false;
            }
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                let skip = e.file_name() == "node_modules" || e.file_name() == ".git";
                if !skip && depth + 1 < max_depth {
                    stack.push((p, depth + 1));
                }
            } else if ft.is_file()
                && multiply_linked_file(&p)
                && !is_known_ask_file(&p, known_ask_ids)
            {
                return true;
            }
        }
    }
    false
}

pub struct Evaluated {
    pub verdict: PolicyVerdict,
    pub resolved: Vec<PathBuf>,
}

impl SessionGate {
    pub fn new(
        config: SessionGateConfig,
        policy: Arc<dyn PolicyEngine>,
        ledger: Arc<dyn ApprovalLedger>,
        rules: Arc<dyn RulesSource>,
    ) -> Self {
        let preset = Mutex::new(config.preset);
        Self {
            config,
            preset,
            policy,
            ledger,
            rules,
            opened: Mutex::new(HashMap::new()),
        }
    }

    /// Changes the preset for later checks. Approvals opened under another
    /// preset are re-evaluated when redeemed.
    pub fn set_preset(&self, preset: Preset) {
        if let Ok(mut p) = self.preset.lock() {
            *p = preset;
        }
    }

    fn preset(&self) -> Option<Preset> {
        self.preset.lock().ok().map(|p| *p)
    }

    /// Path resolution touches the filesystem, so it runs on the blocking
    /// pool; a panic anywhere in it becomes an error (the caller denies).
    async fn evaluate(
        &self,
        check: &ToolCheck,
        rules: &[Rule],
        preset: Preset,
    ) -> Result<Evaluated, String> {
        let cfg = self.config.clone();
        let policy = self.policy.clone();
        let check = check.clone();
        let rules = rules.to_vec();
        tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                evaluate_sync(&cfg, policy.as_ref(), &check, &rules, preset)
            }))
            .map_err(|_| "internal error while checking".to_string())?
        })
        .await
        .map_err(|e| format!("internal error while checking: {e}"))?
    }

    /// The rule "always…" would create (contract §5, `SessionGate` docs).
    fn remember_matcher(&self, check: &ToolCheck, e: &Evaluated) -> Option<RuleMatcher> {
        match &check.subject {
            ToolSubject::Command { line, .. } => {
                let l = shell::lex(line);
                if !l.reasons.is_empty() || l.units.len() != 1 {
                    return None;
                }
                let u = &l.units[0];
                if !u.seg.assignments.is_empty()
                    || u.seg.argv.is_empty()
                    || !u.seg.redirects.iter().all(super::Redirect::harmless)
                {
                    return None;
                }
                Some(RuleMatcher::CommandExact {
                    argv: u.seg.argv.clone(),
                })
            }
            ToolSubject::Paths { paths, .. } => {
                let file_tool = matches!(
                    check.kind,
                    ToolKind::Read
                        | ToolKind::Edit
                        | ToolKind::Delete
                        | ToolKind::Move
                        | ToolKind::Search
                );
                if !file_tool || paths.len() != 1 || e.resolved.len() != 1 {
                    return None;
                }
                Some(RuleMatcher::PathGlob {
                    glob: escape_glob(&e.resolved[0]),
                })
            }
            _ => None,
        }
    }

    async fn do_check(&self, check: &ToolCheck) -> Result<GateDecision, String> {
        let (rev, rules) = self.rules.snapshot().await.ok_or("rules unavailable")?;
        let preset = self.preset().ok_or("preset unavailable")?;
        let e = self.evaluate(check, &rules, preset).await?;
        match &e.verdict {
            PolicyVerdict::Allow { .. } => Ok(GateDecision::Allow),
            PolicyVerdict::Deny { reason, .. } => Ok(GateDecision::Deny {
                reason: reason.clone(),
            }),
            PolicyVerdict::HardDeny { id } => Ok(GateDecision::Deny {
                reason: deny_reason(id),
            }),
            PolicyVerdict::Ask => {
                let context = PolicyContext {
                    rules_revision: rev,
                    preset,
                };
                let binding = Binding {
                    turn: check.turn,
                    tool_name: check.name.clone(),
                    tool_kind: check.kind,
                    input_sha256: input_sha256(check),
                    context,
                    title: check.title.clone(),
                    remember_matcher: self.remember_matcher(check, &e),
                };
                let ticket = self
                    .ledger
                    .open_approval(&self.config.thread, self.config.incarnation, binding)
                    .await
                    .map_err(|e| e.to_string())?;
                self.opened
                    .lock()
                    .map_err(|_| "gate state poisoned")?
                    .insert(ticket.request_id.clone(), context);
                Ok(GateDecision::Ask { ticket })
            }
        }
    }

    async fn do_redeem(
        &self,
        ticket: &ApprovalTicket,
        check: &ToolCheck,
    ) -> Result<GateDecision, String> {
        let key = RequestKey {
            thread: self.config.thread.clone(),
            incarnation: self.config.incarnation,
            request_id: ticket.request_id.clone(),
        };
        let (_rev, rules) = self.rules.snapshot().await.ok_or("rules unavailable")?;
        let preset = self.preset().ok_or("preset unavailable")?;
        let known = self
            .opened
            .lock()
            .map_err(|_| "gate state poisoned")?
            .get(&ticket.request_id)
            .copied();
        if known.is_none() {
            return Err("unknown approval".into());
        }
        // Without the rule this approval's own "always…" created.
        let others: Vec<Rule> = rules
            .into_iter()
            .filter(|r| !matches!(&r.origin, RuleOrigin::Approval { request_id } if *request_id == ticket.request_id))
            .collect();
        // Always evaluated, so a symlink swapped since the approval, a deny
        // rule added meanwhile or a changed preset can't reach a blocked
        // place. `Ask` is fine: the user already approved this call.
        let e = self.evaluate(check, &others, preset).await?;
        match e.verdict {
            PolicyVerdict::HardDeny { id } => {
                let _ = self.ledger.withdraw(&key).await;
                return Ok(GateDecision::Deny {
                    reason: deny_reason(id),
                });
            }
            PolicyVerdict::Deny { reason, .. } => {
                let _ = self.ledger.withdraw(&key).await;
                return Ok(GateDecision::Deny { reason });
            }
            PolicyVerdict::Allow { .. } | PolicyVerdict::Ask => {}
        }
        self.ledger
            .redeem(&key, check)
            .await
            .map_err(|e| e.to_string())?;
        if let Ok(mut m) = self.opened.lock() {
            m.remove(&ticket.request_id);
        }
        Ok(GateDecision::Allow)
    }

    /// Withdraws an open ticket as cancelled, freeing its ledger slot and
    /// preventing future redemption. Idempotent: resolved, redeemed, or
    /// unknown tickets are a no-op and never fail the turn.
    pub async fn withdraw(&self, ticket: &ApprovalTicket) {
        let key = RequestKey {
            thread: self.config.thread.clone(),
            incarnation: self.config.incarnation,
            request_id: ticket.request_id.clone(),
        };
        if let Ok(mut m) = self.opened.lock() {
            m.remove(&ticket.request_id);
        }
        let _ = self.ledger.withdraw(&key).await;
    }
}

impl ToolGate for SessionGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            self.do_check(check)
                .await
                .unwrap_or_else(|reason| GateDecision::Deny { reason })
        })
    }

    fn redeem<'a>(
        &'a self,
        ticket: &'a ApprovalTicket,
        check: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            self.do_redeem(ticket, check)
                .await
                .unwrap_or_else(|reason| GateDecision::Deny { reason })
        })
    }

    fn question_ticket(&self, turn: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        Box::pin(async move {
            self.ledger
                .open_question(&self.config.thread, self.config.incarnation, turn)
                .await
                .ok()
        })
    }

    fn withdraw<'a>(&'a self, ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
        Box::pin(self.withdraw(ticket))
    }
}
