//! Budgeted §2 best-effort pass (owned by P5; contract §2, §8, §11).
//!
//! Covers:
//! - raw-text credential / Atrium-data / pipe-to-shell matching (`dev` `deny.rs`);
//! - per-unit hard-deny rules on tolerant-lexer units (including derived `-c`,
//!   `eval`, `env -S`, `script -c`, `flock -c`, `find -exec`, substitutions,
//!   here-docs, and piped `echo`/`printf` units): privilege escalation,
//!   disk wipe, keychain/secret-tool commands, force-push to protected branches,
//!   recursive delete outside cwd, pipe-to-shell, path & glob-probe matching
//!   with CR-19 folding and N11 bounded brace expansion;
//! - best-effort search-glob probe / brace-escape checks for `ToolSubject::Paths`.
//!
//! Every scan, copy, comparison, and heap allocation charges the shared
//! [`ComputeBudget`] before running (§8). Hits found before exhaustion are
//! preserved via [`push_charged`], and exhaustion never creates a hit.

use std::collections::HashSet;

use globset::{GlobBuilder, GlobMatcher};

use crate::policy::model::{
    AccessMode, BestEffortRecord, BestEffortRedirectKind, BestEffortUnit, ComputeBudget,
    ExhaustReason, Exhausted, FoldRules, Hit, HitMatch, HitSource, PartId, PartOutcome, PartResult,
    ProtectedSet, UnreadReason,
};
use crate::policy::tools::{self, ToolRowKind};
use crate::policy::v4::lex::brace_expand_budgeted;
use crate::policy::v4::paths::sensitive::{
    CRED_FILES, CRED_TREES, SharedSensitive, fold_path_listed, join_folded, push_charged,
    under_bytes,
};

const DOWNLOADERS: &[&str] = &["curl", "wget", "fetch"];
const PRIV: &[&str] = &["sudo", "sudoedit", "su", "doas", "pkexec", "run0"];

const WRITERS: &[&str] = &[
    "rm",
    "mv",
    "cp",
    "tee",
    "touch",
    "chmod",
    "chown",
    "chgrp",
    "ln",
    "truncate",
    "install",
    "mkdir",
    "rmdir",
    "rsync",
    "unlink",
    "shred",
    "patch",
    "ed",
    "ex",
    "wipefs",
    "blkdiscard",
];

const READERS: &[&str] = &[
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "wc",
    "ls",
    "stat",
    "file",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "ack",
    "diff",
    "cmp",
    "md5",
    "md5sum",
    "shasum",
    "sha1sum",
    "sha256sum",
    "du",
    "df",
    "tree",
    "echo",
    "printf",
    "which",
    "whereis",
    "type",
    "readlink",
    "realpath",
    "basename",
    "dirname",
    "test",
    "[",
    "pwd",
    "sort",
    "uniq",
    "cut",
    "tr",
    "nl",
    "od",
    "xxd",
    "hexdump",
    "strings",
    "bat",
    "jq",
    "column",
    "fold",
    "expand",
    "true",
    "false",
    "id",
    "whoami",
    "uname",
    "date",
    "cd",
    "pushd",
    "popd",
    "find",
    "open",
    "spctl",
    "mdls",
    "mdfind",
    "codesign",
    "awk",
    "gawk",
    "mawk",
    "sed",
    "perl",
];

/// Commands for which naming `~/.ssh` itself is a listing/navigation operation
/// rather than reading/copying/searching its private keys (`dev` `deny.rs`).
const SSH_DIR_LISTERS: &[&str] = &[
    "ls", "stat", "cd", "pushd", "popd", "pwd", "test", "[", "du", "dir", "readlink", "realpath",
    "basename", "dirname", "echo", "printf", "chmod", "chown", "chgrp", "mkdir", "rmdir",
];

const MAX_CWD_CANDS: usize = 32;
const MAX_CWD_SEEN: usize = 64;
const MAX_SEARCH_GLOB_VARIANTS: usize = 256;

/// Optional per-call context passed by `evaluate_call` so modeled quoted
/// literals (whose provenance has zero unquoted glob characters, §3 / §10 row 9)
/// are not spuriously glob-matched as patterns by the best-effort pass.
pub(super) struct BestEffortOptions<'a> {
    /// Exact modeled `(unit_index_in_top_level, argv_index)` pairs that are
    /// known to have NO unquoted glob characters in exact model provenance.
    pub(super) quoted_literal_words: Option<&'a HashSet<Vec<u8>>>,
}

impl Default for BestEffortOptions<'_> {
    fn default() -> Self {
        Self {
            quoted_literal_words: None,
        }
    }
}

/// Runs the §2 best-effort pass over `record`, appending every hit found into
/// `out` as soon as it is discovered (so exhaustion never loses earlier hits).
pub(super) fn run_best_effort_into(
    record: &BestEffortRecord,
    protected: &ProtectedSet,
    start_cwd: &[u8],
    opts: &BestEffortOptions<'_>,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    if budget.is_exhausted() {
        return Err(Exhausted {
            reason: budget
                .exhausted_reason()
                .unwrap_or(ExhaustReason::ComputeBudget),
        });
    }
    let shared = SharedSensitive::for_protected_budgeted(protected, budget)?;

    // 1. Raw-text checks (home-anchored credentials, Atrium data, raw pipe-to-shell).
    budget.charge(record.raw_texts.len().max(1) as u64)?;
    for raw in &record.raw_texts {
        raw_text_checks(raw, &shared, protected.folds, budget, out)?;
    }

    // 2. Top-level separator map (O(N) scan of raw_texts[0]) so `cd DIR && ...`
    //    only keeps `DIR`, while `cd DIR; ...` / `cd DIR\n...` keeps both `DIR`
    //    and the previous directory (matching `dev` and `adversarial_ordinary.rs`).
    let top_joins = if record.raw_texts.len() == 1 {
        scan_top_level_joins(&record.raw_texts[0], budget)?
    } else {
        Vec::new()
    };

    // 3. Per-unit checks with budgeted lexical `cd` tracking.
    let mut cwd_tracker = BeCwdTracker::new(start_cwd, &protected.home, budget)?;
    let mut top_unit_ordinal = 0usize;
    let mut last_find_exec_context: Option<(u8, Vec<String>)> = None;

    budget.charge(record.units.len().max(1) as u64)?;
    for (i, u) in record.units.iter().enumerate() {
        budget.charge(1)?;
        let part = PartId::BestEffortUnit(u.index);

        // Update depth-scoped cwd tracker before inspecting this unit.
        cwd_tracker.enter_unit(u, budget)?;
        let unit_cands = cwd_tracker.current_cands(budget)?;
        let unit_cwd_unknown = cwd_tracker.is_unknown();

        // Track `find` roots for immediately-following derived `-exec` units.
        let mut next_find_exec_context: Option<(u8, Vec<String>)> = None;
        let active_exec_roots: Option<&[String]> = match &last_find_exec_context {
            Some((fdepth, roots)) if u.derived && u.depth > *fdepth => Some(roots.as_slice()),
            _ => None,
        };

        // Check every reading (`effective_argv` + `alt_argv`) of this unit.
        let readings_count = 1 + u.alt_argv.len();
        budget.charge(readings_count as u64)?;
        let mut unit_is_same_shell_parent = false;
        for reading in std::iter::once(&u.effective_argv).chain(u.alt_argv.iter()) {
            if reading.is_empty() {
                continue;
            }
            let folded_cmd = fold_cmd_cr19(&reading[0], protected.folds, budget)?;
            if matches!(folded_cmd.as_str(), "eval" | "source" | ".") {
                unit_is_same_shell_parent = true;
            }
            if folded_cmd == "find" && has_find_exec_flag(reading, budget)? {
                let (roots, _) = find_roots_budgeted(reading, budget)?;
                next_find_exec_context = Some((u.depth, roots));
            }
            check_unit_reading(
                i,
                u,
                reading,
                &folded_cmd,
                &part,
                &record.units,
                protected,
                &shared,
                &unit_cands,
                unit_cwd_unknown,
                active_exec_roots,
                opts,
                budget,
                out,
            )?;
        }
        if let Some(ctx) = next_find_exec_context {
            last_find_exec_context = Some(ctx);
        } else if !u.derived {
            last_find_exec_context = None;
        }

        // Check unit assignments and redirects as paths / devices.
        let primary_cmd = match u.effective_argv.first() {
            Some(f) => Some(fold_cmd_cr19(f, protected.folds, budget)?),
            None => None,
        };
        check_unit_assignments_and_redirects(
            u,
            primary_cmd.as_deref(),
            &part,
            protected,
            &shared,
            &unit_cands,
            opts,
            budget,
            out,
        )?;

        // Advance cwd tracker if this unit is `cd`/`pushd`/`popd`/`chdir`.
        let join_after = if !u.derived && u.depth == 0 {
            let j = top_joins
                .get(top_unit_ordinal)
                .copied()
                .unwrap_or(BeJoin::Seq);
            top_unit_ordinal += 1;
            j
        } else {
            // Inside derived units (`sh -c 'cd proj && cat .npmrc'`, `eval 'cd proj && ...'`),
            // check if a subsequent sibling exists at the same depth with a `cd ... &&` hint.
            BeJoin::And
        };
        let next_is_exit = record.units.get(i + 1).is_some_and(|n| {
            n.depth == u.depth
                && n.pipeline_id.is_none()
                && !n.piped
                && n.effective_argv.first().is_some_and(|f| f == "exit")
        });
        cwd_tracker.after_unit(
            u,
            primary_cmd.as_deref(),
            join_after,
            next_is_exit,
            unit_is_same_shell_parent,
            protected.folds,
            budget,
        )?;
    }

    Ok(())
}

/// Runs raw-text hard-deny checks (home-anchored credentials, Atrium data,
/// raw pipe-to-shell) on a single string (used by `run_best_effort_into` and
/// `ToolSubject::Mcp`).
pub(super) fn raw_text_checks(
    text: &str,
    shared: &SharedSensitive,
    folds: FoldRules,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    if text.is_empty() {
        return Ok(());
    }
    if let Some(id) = raw_hit_budgeted(text, shared, folds, budget)? {
        push_be_hit(
            out,
            PartId::WholeCall,
            id,
            HitMatch::CommandWord {
                word: id.to_owned(),
            },
            budget,
        )?;
    }
    if raw_pipe_to_shell_budgeted(text, folds, budget)? {
        push_be_hit(
            out,
            PartId::WholeCall,
            "pipe-to-shell",
            HitMatch::CommandWord {
                word: "pipe-to-shell".to_owned(),
            },
            budget,
        )?;
    }
    Ok(())
}

fn push_be_hit(
    out: &mut Vec<PartResult>,
    part: PartId,
    id: &'static str,
    matched: HitMatch,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    push_charged(
        out,
        PartResult {
            part: part.clone(),
            outcome: PartOutcome::HardDenyHit(Hit {
                hard_deny_id: Some(id),
                source: HitSource::BestEffort,
                matched,
                part,
            }),
        },
        budget,
    )
}

// ---------------------------------------------------------------------------
// CR-19 folding & string helpers (all budgeted)
// ---------------------------------------------------------------------------

/// Basename + CR-19 command-name folding (`lowercase` + `ſ`->`s` + `K`->`k`).
fn fold_cmd_cr19(
    word: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    let base = word.rsplit('/').next().unwrap_or(word);
    let folded_bytes = fold_path_listed(base.as_bytes(), folds, budget)?;
    budget.charge(folded_bytes.len().max(1) as u64)?;
    budget.charge_alloc(folded_bytes.len())?;
    Ok(String::from_utf8_lossy(&folded_bytes).into_owned())
}

fn fold_str_listed(
    s: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    let folded_bytes = fold_path_listed(s.as_bytes(), folds, budget)?;
    budget.charge(folded_bytes.len().max(1) as u64)?;
    budget.charge_alloc(folded_bytes.len())?;
    Ok(String::from_utf8_lossy(&folded_bytes).into_owned())
}

fn clone_str_budgeted(s: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    budget.charge(s.len().max(1) as u64)?;
    budget.charge_alloc(s.len())?;
    Ok(s.to_owned())
}

// ---------------------------------------------------------------------------
// Raw-text hard-deny matching (`deny.rs::raw_hit` + `raw_pipe_to_shell`)
// ---------------------------------------------------------------------------

fn raw_hit_budgeted(
    text: &str,
    shared: &SharedSensitive,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static str>, Exhausted> {
    // Strip quotes & backslashes and apply listed folds (lowercase, ſ->s, K->k).
    let stripped_cap = text.len();
    budget.charge(stripped_cap.max(1) as u64)?;
    budget.charge_alloc(stripped_cap)?;
    let mut stripped = Vec::with_capacity(stripped_cap);
    for &b in text.as_bytes() {
        if !matches!(b, b'"' | b'\'' | b'\\') {
            stripped.push(b);
        }
    }
    let folded = fold_path_listed(&stripped, folds, budget)?;
    let t = match std::str::from_utf8(&folded) {
        Ok(s) => s,
        Err(_) => return Ok(None),
    };

    // Atrium data paths (highest specificity in `raw_forms`).
    for d in shared.data_folded() {
        if d.is_empty() {
            continue;
        }
        if let Ok(ds) = std::str::from_utf8(d) {
            budget.charge((t.len() + ds.len()).max(1) as u64)?;
            if t.contains(ds) {
                return Ok(Some("atrium-self"));
            }
        }
    }

    let home_str = std::str::from_utf8(shared.home_folded()).unwrap_or("");
    let anchors: [&str; 4] = ["~/", "$home/", "${home}/", home_str];

    // Check anchored credential files and trees without quadratic string rebuilds:
    // find each anchor occurrence in `t` once, and match the suffix right after it.
    for (idx, anchor) in anchors.iter().enumerate() {
        if anchor.is_empty() {
            continue;
        }
        let need_slash = idx == 3;
        budget.charge(t.len().max(1) as u64)?;
        let mut rest = t;
        while let Some(pos) = rest.find(anchor) {
            let after_anchor = &rest[pos + anchor.len()..];
            rest = after_anchor;
            let after = if need_slash {
                match after_anchor.strip_prefix('/') {
                    Some(s) => s,
                    None => continue,
                }
            } else {
                after_anchor
            };
            // Charge constant-bounded comparison against CRED_FILES, CRED_TREES, `.ssh/`.
            budget.charge((CRED_FILES.len() + CRED_TREES.len() + 2) as u64 * 8)?;
            for &rel in CRED_FILES.iter().chain(CRED_TREES.iter()) {
                if after.starts_with(rel) {
                    return Ok(Some("credential-read"));
                }
            }
            if let Some(ssh_rest) = after.strip_prefix(".ssh/") {
                let end = ssh_rest
                    .find(|c: char| c.is_whitespace() || matches!(c, ';' | '|' | '&' | ')' | '`'))
                    .unwrap_or(ssh_rest.len());
                budget.charge(end.max(1) as u64)?;
                let name = &ssh_rest[..end];
                if !name.is_empty()
                    && !(name.ends_with(".pub")
                        || name.starts_with("known_hosts")
                        || name == "config"
                        || name == "authorized_keys")
                {
                    return Ok(Some("credential-read"));
                }
            }
        }
    }
    Ok(None)
}

fn raw_pipe_to_shell_budgeted(
    text: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let lower = fold_str_listed(text, folds, budget)?;
    for dl in ["curl", "wget"] {
        budget.charge(lower.len().max(1) as u64)?;
        let mut rest = lower.as_str();
        while let Some(i) = rest.find(dl) {
            rest = &rest[i + dl.len()..];
            let seg = rest.split([';', '\n', '&']).next().unwrap_or("");
            if let Some((_, after)) = seg.split_once('|') {
                let words = raw_words_budgeted(after, budget)?;
                let eff = unwrap_simple_be(&words, folds, budget)?;
                if reads_stdin_as_code_budgeted(&eff, folds, 0, budget)? {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn raw_words_budgeted(t: &str, budget: &mut ComputeBudget) -> Result<Vec<String>, Exhausted> {
    budget.charge(t.len().max(1) as u64)?;
    budget.charge_alloc(t.len())?;
    let mut out = Vec::new();
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
                    budget.charge_alloc(std::mem::size_of::<String>())?;
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
        budget.charge_alloc(std::mem::size_of::<String>())?;
        out.push(cur);
    }
    Ok(out)
}

fn unwrap_simple_be(
    words: &[String],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let mut idx = 0usize;
    for _ in 0..16 {
        let Some(first) = words.get(idx) else { break };
        let f = fold_cmd_cr19(first, folds, budget)?;
        match f.as_str() {
            "nohup" | "builtin" | "time" | "command" | "exec" | "busybox" | "toybox" => {
                idx += 1;
                while let Some(w) = words.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if w.starts_with('-') && w.len() > 1 {
                        idx += if w == "-a" || w == "-f" || w == "-o" {
                            2
                        } else {
                            1
                        };
                    } else {
                        break;
                    }
                }
            }
            "nice" => {
                idx += 1;
                while let Some(w) = words.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if w == "-n" || w == "--adjustment" {
                        idx += 2;
                    } else if w.starts_with('-') && w.len() > 1 {
                        idx += 1;
                    } else {
                        break;
                    }
                }
            }
            "timeout" => {
                idx += 1;
                while let Some(w) = words.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if matches!(w.as_str(), "-s" | "-k" | "--signal" | "--kill-after") {
                        idx += 2;
                    } else if w.starts_with('-') && w.len() > 1 {
                        idx += 1;
                    } else {
                        break;
                    }
                }
                idx = (idx + 1).min(words.len());
            }
            "env" => {
                idx += 1;
                while let Some(w) = words.get(idx) {
                    budget.charge(w.len().max(1) as u64)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if w == "-" {
                        idx += 1;
                        continue;
                    }
                    if w.starts_with('-') && w.len() > 1 {
                        if matches!(
                            w.as_str(),
                            "-u" | "-C"
                                | "-P"
                                | "-a"
                                | "-S"
                                | "--unset"
                                | "--chdir"
                                | "--split-string"
                                | "--argv0"
                        ) {
                            idx += 2;
                        } else {
                            idx += 1;
                        }
                    } else if w.contains('=') {
                        idx += 1;
                    } else {
                        break;
                    }
                }
            }
            _ => break,
        }
    }
    let slice = &words[idx.min(words.len())..];
    let bytes: usize = slice
        .iter()
        .map(|s| s.len() + std::mem::size_of::<String>())
        .sum();
    budget.charge(bytes.max(1) as u64)?;
    budget.charge_alloc(bytes)?;
    Ok(slice.to_vec())
}

// ---------------------------------------------------------------------------
// Top-level separator scan & budgeted lexical cwd tracker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeJoin {
    Seq,
    And,
    Or,
}

/// Linear scan of a command line recording the separator (`&&`, `||`, or
/// `;`/`\n`/`|`/`&`) after each top-level command unit.
fn scan_top_level_joins(text: &str, budget: &mut ComputeBudget) -> Result<Vec<BeJoin>, Exhausted> {
    let bytes = text.as_bytes();
    budget.charge(bytes.len().max(1) as u64)?;
    let mut joins = Vec::new();
    let mut i = 0usize;
    let mut has_token = false;
    let mut in_sq = false;
    let mut in_dq = false;
    let mut paren_depth = 0usize;
    let mut backtick = false;

    while i < bytes.len() {
        let b = bytes[i];
        if in_sq {
            if b == b'\'' {
                in_sq = false;
            }
            i += 1;
            continue;
        }
        if in_dq {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_dq = false;
            }
            i += 1;
            continue;
        }
        if backtick {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == b'`' {
                backtick = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\\' if i + 1 < bytes.len() => {
                has_token = true;
                i += 2;
            }
            b'\'' => {
                in_sq = true;
                has_token = true;
                i += 1;
            }
            b'"' => {
                in_dq = true;
                has_token = true;
                i += 1;
            }
            b'`' => {
                backtick = true;
                has_token = true;
                i += 1;
            }
            b'$' if bytes.get(i + 1) == Some(&b'(') => {
                paren_depth += 1;
                has_token = true;
                i += 2;
            }
            b'<' | b'>' if bytes.get(i + 1) == Some(&b'(') => {
                paren_depth += 1;
                has_token = true;
                i += 2;
            }
            b'(' if paren_depth > 0 => {
                paren_depth += 1;
                i += 1;
            }
            b')' if paren_depth > 0 => {
                paren_depth -= 1;
                i += 1;
            }
            b'#' if paren_depth == 0 && !has_token => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'&' if paren_depth == 0 && bytes.get(i + 1) == Some(&b'&') => {
                if has_token {
                    budget.charge_alloc(std::mem::size_of::<BeJoin>())?;
                    joins.push(BeJoin::And);
                    has_token = false;
                }
                i += 2;
            }
            b'|' if paren_depth == 0 && bytes.get(i + 1) == Some(&b'|') => {
                if has_token {
                    budget.charge_alloc(std::mem::size_of::<BeJoin>())?;
                    joins.push(BeJoin::Or);
                    has_token = false;
                }
                i += 2;
            }
            b';' | b'\n' | b'|' | b'&' | b'(' | b')' if paren_depth == 0 => {
                if has_token {
                    budget.charge_alloc(std::mem::size_of::<BeJoin>())?;
                    joins.push(BeJoin::Seq);
                    has_token = false;
                }
                i += 1;
            }
            b' ' | b'\t' => {
                i += 1;
            }
            _ => {
                has_token = true;
                i += 1;
            }
        }
    }
    if has_token {
        budget.charge_alloc(std::mem::size_of::<BeJoin>())?;
        joins.push(BeJoin::Seq);
    }
    Ok(joins)
}

struct CwdFrame {
    depth: u8,
    cands: Vec<Vec<u8>>,
    unknown: bool,
    same_shell: bool,
}

struct BeCwdTracker {
    home: Vec<u8>,
    start: Vec<u8>,
    seen: Vec<Vec<u8>>,
    stack: Vec<CwdFrame>,
    next_child_same_shell: bool,
}

impl BeCwdTracker {
    fn new(start: &[u8], home: &[u8], budget: &mut ComputeBudget) -> Result<Self, Exhausted> {
        budget.charge((start.len() + home.len()).max(1) as u64)?;
        budget.charge_alloc(start.len() * 2 + home.len() + std::mem::size_of::<CwdFrame>())?;
        Ok(Self {
            home: home.to_vec(),
            start: start.to_vec(),
            seen: Vec::new(),
            stack: vec![CwdFrame {
                depth: 0,
                cands: vec![start.to_vec()],
                unknown: false,
                same_shell: false,
            }],
            next_child_same_shell: false,
        })
    }

    fn enter_unit(
        &mut self,
        u: &BestEffortUnit,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        while self.stack.len() > 1 && self.stack.last().is_some_and(|f| f.depth > u.depth) {
            let Some(done) = self.stack.pop() else { break };
            if done.same_shell && !self.stack.is_empty() {
                if done.unknown {
                    let lost_list = self.lost_cands(budget)?;
                    if let Some(parent) = self.stack.last_mut() {
                        parent.unknown = true;
                        parent.cands = lost_list;
                    }
                } else if let Some(parent) = self.stack.last_mut() {
                    parent.cands = done.cands;
                }
            }
        }
        let top_depth = self.stack.last().map_or(0, |f| f.depth);
        if u.depth > top_depth {
            let (cands, unknown) = match self.stack.last() {
                Some(top) => (clone_bytes_vec(&top.cands, budget)?, top.unknown),
                None => (vec![clone_bytes(&self.start, budget)?], false),
            };
            budget.charge_alloc(std::mem::size_of::<CwdFrame>())?;
            self.stack.push(CwdFrame {
                depth: u.depth,
                cands,
                unknown,
                same_shell: self.next_child_same_shell,
            });
            self.next_child_same_shell = false;
        }
        Ok(())
    }

    fn current_cands(&self, budget: &mut ComputeBudget) -> Result<Vec<Vec<u8>>, Exhausted> {
        match self.stack.last() {
            Some(f) => clone_bytes_vec(&f.cands, budget),
            None => Ok(vec![clone_bytes(&self.start, budget)?]),
        }
    }

    fn is_unknown(&self) -> bool {
        self.stack.last().is_some_and(|f| f.unknown)
    }

    fn push_seen(
        &mut self,
        paths: &[Vec<u8>],
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        for p in paths {
            if self.seen.len() >= MAX_CWD_SEEN {
                return Ok(());
            }
            budget.charge(p.len().max(1) as u64)?;
            if !self.seen.iter().any(|s| s == p) {
                budget.charge_alloc(p.len() + std::mem::size_of::<Vec<u8>>())?;
                self.seen.push(p.clone());
            }
        }
        Ok(())
    }

    fn lost_cands(&self, budget: &mut ComputeBudget) -> Result<Vec<Vec<u8>>, Exhausted> {
        let mut v = Vec::with_capacity(2 + self.seen.len());
        budget.charge_alloc(
            (2 + self.seen.len()) * std::mem::size_of::<Vec<u8>>()
                + self.home.len()
                + self.start.len(),
        )?;
        v.push(self.home.clone());
        if self.start != self.home {
            v.push(self.start.clone());
        }
        for s in &self.seen {
            if v.len() >= MAX_CWD_CANDS {
                break;
            }
            if !v.iter().any(|e| e == s) {
                v.push(clone_bytes(s, budget)?);
            }
        }
        Ok(v)
    }

    #[allow(clippy::too_many_arguments)]
    fn after_unit(
        &mut self,
        u: &BestEffortUnit,
        primary_cmd: Option<&str>,
        join_after: BeJoin,
        next_is_exit: bool,
        same_shell_parent: bool,
        folds: FoldRules,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        self.next_child_same_shell = same_shell_parent;
        let Some(cmd) = primary_cmd else {
            return Ok(());
        };
        if !matches!(cmd, "cd" | "pushd" | "popd" | "chdir") {
            return Ok(());
        }
        let eff = &u.effective_argv;
        let in_pipe = u.pipeline_id.is_some() || u.piped;
        let mut args: Vec<&str> = Vec::new();
        let mut readable = cmd != "pushd" && cmd != "popd" && !in_pipe;
        let mut j = 1usize;
        while j < eff.len() {
            let a = eff[j].as_str();
            budget.charge(a.len().max(1) as u64)?;
            if a == "--" {
                j += 1;
                break;
            }
            if a.len() > 1
                && a.starts_with('-')
                && !a[1..].starts_with(|c: char| c.is_ascii_digit())
            {
                if !a[1..].chars().all(|c| matches!(c, 'L' | 'P' | 'e' | '@')) {
                    readable = false;
                }
                j += 1;
            } else {
                break;
            }
        }
        for a in &eff[j..] {
            budget.charge_alloc(std::mem::size_of::<&str>())?;
            args.push(a.as_str());
        }

        let cur_cands = self.current_cands(budget)?;
        let mut targets: Vec<Vec<u8>> = Vec::new();
        let mut lost = !readable || cmd == "popd";

        if cmd != "popd" {
            match args.as_slice() {
                [] if cmd == "pushd" => {
                    lost = true;
                }
                [] => {
                    targets.push(clone_bytes(&self.home, budget)?);
                }
                ["-"] => {
                    lost = true;
                }
                [a] => {
                    if let Some(ts) = self.resolve_cd_arg(a, &cur_cands, folds, budget)? {
                        targets = ts;
                    } else {
                        lost = true;
                    }
                }
                more => {
                    lost = true;
                    for a in more {
                        if *a != "-"
                            && let Some(ts) = self.resolve_cd_arg(a, &cur_cands, folds, budget)?
                        {
                            for t in ts {
                                if targets.len() < MAX_CWD_CANDS {
                                    targets.push(t);
                                }
                            }
                        }
                    }
                }
            }
        }

        self.push_seen(&targets, budget)?;
        let Some(frame) = self.stack.last_mut() else {
            return Ok(());
        };
        if lost {
            let old = std::mem::take(&mut frame.cands);
            frame.unknown = true;
            let _ = frame;
            self.push_seen(&old, budget)?;
            let lost_list = self.lost_cands(budget)?;
            if let Some(frame) = self.stack.last_mut() {
                frame.cands = lost_list;
            }
            return Ok(());
        }

        if frame.unknown {
            let lost_list = self.lost_cands(budget)?;
            if let Some(frame) = self.stack.last_mut() {
                frame.cands = lost_list;
            }
        } else if join_after == BeJoin::And || (join_after == BeJoin::Or && next_is_exit) {
            frame.cands = targets;
        } else {
            // `;`, `\n`, or unguarded `||`: `cd` may fail, so keep both targets and old cwds.
            let mut merged = targets;
            for old in &frame.cands {
                if merged.len() >= MAX_CWD_CANDS {
                    break;
                }
                if !merged.iter().any(|m| m == old) {
                    merged.push(clone_bytes(old, budget)?);
                }
            }
            frame.cands = merged;
        }
        Ok(())
    }

    fn resolve_cd_arg(
        &self,
        a: &str,
        cur_cands: &[Vec<u8>],
        _folds: FoldRules,
        budget: &mut ComputeBudget,
    ) -> Result<Option<Vec<Vec<u8>>>, Exhausted> {
        budget.charge(a.len().max(1) as u64)?;
        let is_stack = a
            .strip_prefix(['-', '+'])
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if a.starts_with("~+") || a.starts_with("~-") || is_stack {
            return Ok(None);
        }
        let expanded = expand_home_be(a, &self.home, &self.start, budget)?;
        if expanded.contains('$')
            || expanded.contains('`')
            || expanded.contains(['*', '?', '['])
            || expanded.starts_with('~')
        {
            return Ok(None);
        }
        if expanded.starts_with('/') {
            let norm = lexical_normalize_bytes(expanded.as_bytes(), budget)?;
            return Ok(Some(vec![norm]));
        }
        let mut out = Vec::new();
        for c in cur_cands {
            if out.len() >= MAX_CWD_CANDS {
                break;
            }
            let joined = join_lexical_bytes(c, expanded.as_bytes(), budget)?;
            if !out.iter().any(|x| x == &joined) {
                out.push(joined);
            }
        }
        Ok(Some(out))
    }
}

fn clone_bytes(b: &[u8], budget: &mut ComputeBudget) -> Result<Vec<u8>, Exhausted> {
    budget.charge(b.len().max(1) as u64)?;
    budget.charge_alloc(b.len())?;
    Ok(b.to_vec())
}

fn clone_bytes_vec(v: &[Vec<u8>], budget: &mut ComputeBudget) -> Result<Vec<Vec<u8>>, Exhausted> {
    budget.charge(v.len().max(1) as u64)?;
    budget.charge_alloc(v.len() * std::mem::size_of::<Vec<u8>>())?;
    let mut out = Vec::with_capacity(v.len());
    for item in v {
        out.push(clone_bytes(item, budget)?);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Per-unit hard-deny rules
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn check_unit_reading(
    unit_idx: usize,
    u: &BestEffortUnit,
    eff: &[String],
    folded: &str,
    part: &PartId,
    all_units: &[BestEffortUnit],
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    unit_cands: &[Vec<u8>],
    unit_cwd_unknown: bool,
    active_exec_roots: Option<&[String]>,
    opts: &BestEffortOptions<'_>,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    let folds = protected.folds;

    // 1. Privilege escalation.
    if PRIV.contains(&folded) {
        push_be_hit(
            out,
            part.clone(),
            "privilege-escalation",
            HitMatch::CommandWord {
                word: clone_str_budgeted(folded, budget)?,
            },
            budget,
        )?;
    } else if folded == "osascript" {
        for w in &eff[1..] {
            let wl = fold_str_listed(w, folds, budget)?;
            budget.charge(wl.len().max(1) as u64)?;
            if wl.contains("administrator privileges") {
                push_be_hit(
                    out,
                    part.clone(),
                    "privilege-escalation",
                    HitMatch::CommandWord {
                        word: clone_str_budgeted("osascript", budget)?,
                    },
                    budget,
                )?;
                break;
            }
        }
    }

    // 2. Disk wipe.
    if folded.starts_with("mkfs") || folded == "mke2fs" || folded.starts_with("newfs") {
        push_be_hit(
            out,
            part.clone(),
            "disk-wipe",
            HitMatch::CommandWord {
                word: clone_str_budgeted(folded, budget)?,
            },
            budget,
        )?;
    }
    if folded == "dd" {
        for w in &eff[1..] {
            budget.charge(w.len().max(1) as u64)?;
            if let Some(v) = w.strip_prefix("of=")
                && is_device_word(v, unit_cands, &protected.home, folds, budget)?
            {
                push_be_hit(
                    out,
                    part.clone(),
                    "disk-wipe",
                    HitMatch::CommandWord {
                        word: clone_str_budgeted("dd", budget)?,
                    },
                    budget,
                )?;
                break;
            }
        }
    }
    if matches!(folded, "shred" | "wipefs" | "blkdiscard") {
        for w in &eff[1..] {
            if is_device_word(w, unit_cands, &protected.home, folds, budget)? {
                push_be_hit(
                    out,
                    part.clone(),
                    "disk-wipe",
                    HitMatch::CommandWord {
                        word: clone_str_budgeted(folded, budget)?,
                    },
                    budget,
                )?;
                break;
            }
        }
    }
    if folded == "diskutil" && diskutil_wipes(eff, folds, budget)? {
        push_be_hit(
            out,
            part.clone(),
            "disk-wipe",
            HitMatch::CommandWord {
                word: clone_str_budgeted("diskutil", budget)?,
            },
            budget,
        )?;
    }
    if is_known_writer_be(folded, eff, budget)? && !matches!(folded, "chmod" | "chown" | "chgrp") {
        for w in &eff[1..] {
            if is_device_word(w, unit_cands, &protected.home, folds, budget)? {
                push_be_hit(
                    out,
                    part.clone(),
                    "disk-wipe",
                    HitMatch::CommandWord {
                        word: clone_str_budgeted(folded, budget)?,
                    },
                    budget,
                )?;
                break;
            }
        }
    }

    // 3. Keychain / secret-tool credential commands.
    if folded == "security" {
        for w in &eff[1..] {
            budget.charge(w.len().max(1) as u64)?;
            if w.len() >= 6
                && [
                    "find-generic-password",
                    "find-internet-password",
                    "dump-keychain",
                ]
                .iter()
                .any(|f| f.starts_with(w.as_str()))
            {
                push_be_hit(
                    out,
                    part.clone(),
                    "credential-read",
                    HitMatch::CommandWord {
                        word: clone_str_budgeted("security", budget)?,
                    },
                    budget,
                )?;
                break;
            }
        }
    }
    if folded == "secret-tool" && eff.get(1).is_some_and(|s| s == "lookup") {
        push_be_hit(
            out,
            part.clone(),
            "credential-read",
            HitMatch::CommandWord {
                word: clone_str_budgeted("secret-tool", budget)?,
            },
            budget,
        )?;
    }

    // 4. Force push to protected branches.
    if folded == "git" && force_push_budgeted(eff, budget)? {
        push_be_hit(
            out,
            part.clone(),
            "force-push-protected",
            HitMatch::CommandWord {
                word: clone_str_budgeted("git push", budget)?,
            },
            budget,
        )?;
    }

    // 5. Recursive delete outside cwd (`rm -r`, `find -exec rm -rf {}`, `find -delete`).
    if folded == "rm"
        && let Some(mut targets) = rm_recursive_budgeted(eff, budget)?
    {
        if let Some(roots) = active_exec_roots {
            let mut expanded = Vec::new();
            for t in targets {
                if t == "{}" {
                    for r in roots {
                        budget.charge_alloc(std::mem::size_of::<String>())?;
                        expanded.push(clone_str_budgeted(r, budget)?);
                    }
                } else {
                    budget.charge_alloc(std::mem::size_of::<String>())?;
                    expanded.push(t);
                }
            }
            targets = expanded;
        }
        if delete_blocked_budgeted(
            &targets,
            None,
            unit_cands,
            unit_cwd_unknown,
            protected,
            budget,
        )? {
            push_be_hit(
                out,
                part.clone(),
                "recursive-delete-outside-cwd",
                HitMatch::CommandWord {
                    word: clone_str_budgeted("rm", budget)?,
                },
                budget,
            )?;
        }
    }
    if folded == "find"
        && let Some((roots, filtered)) = find_delete_budgeted(eff, budget)?
        && delete_blocked_budgeted(
            &roots,
            Some(filtered),
            unit_cands,
            unit_cwd_unknown,
            protected,
            budget,
        )?
    {
        push_be_hit(
            out,
            part.clone(),
            "recursive-delete-outside-cwd",
            HitMatch::CommandWord {
                word: clone_str_budgeted("find", budget)?,
            },
            budget,
        )?;
    }

    // 6. Pipe-to-shell (`curl ... | sh`, `bash -c "$(curl ...)"`, `source <(curl ...)`).
    if DOWNLOADERS.contains(&folded)
        && let Some(pid) = u.pipeline_id
    {
        for downstream in &all_units[unit_idx + 1..] {
            budget.charge(1)?;
            if downstream.pipeline_id == Some(pid) && downstream.piped {
                let mut hits_stdin = false;
                for dr in
                    std::iter::once(&downstream.effective_argv).chain(downstream.alt_argv.iter())
                {
                    if reads_stdin_as_code_budgeted(dr, folds, 0, budget)? {
                        hits_stdin = true;
                        break;
                    }
                }
                if hits_stdin {
                    push_be_hit(
                        out,
                        part.clone(),
                        "pipe-to-shell",
                        HitMatch::CommandWord {
                            word: clone_str_budgeted(folded, budget)?,
                        },
                        budget,
                    )?;
                    break;
                }
            }
        }
    }
    if is_executor_be(folded, budget)? && unit_subs_have_downloader(u, folds, budget)? {
        push_be_hit(
            out,
            part.clone(),
            "pipe-to-shell",
            HitMatch::CommandWord {
                word: clone_str_budgeted(folded, budget)?,
            },
            budget,
        )?;
    }

    // 7. Best-effort path, brace-expansion, glob-probe, and `.ssh`-dir checks on operands.
    check_reading_operands(
        folded, eff, part, protected, shared, unit_cands, opts, budget, out,
    )?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn check_unit_assignments_and_redirects(
    u: &BestEffortUnit,
    primary_cmd: Option<&str>,
    part: &PartId,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    unit_cands: &[Vec<u8>],
    opts: &BestEffortOptions<'_>,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    let folds = protected.folds;
    for (_, val) in &u.assignments {
        if val.is_empty() {
            continue;
        }
        check_candidate_word(
            val,
            AccessMode::Read,
            primary_cmd,
            part,
            protected,
            shared,
            unit_cands,
            opts,
            budget,
            out,
        )?;
    }
    for (kind, target) in &u.redirects {
        if *kind == BestEffortRedirectKind::Dup || target.is_empty() {
            continue;
        }
        let access = match kind {
            BestEffortRedirectKind::Read => AccessMode::Read,
            BestEffortRedirectKind::Write => AccessMode::Write,
            BestEffortRedirectKind::Dup => continue,
        };
        if access == AccessMode::Write
            && is_device_word(target, unit_cands, &protected.home, folds, budget)?
        {
            push_be_hit(
                out,
                part.clone(),
                "disk-wipe",
                HitMatch::ProtectedPath {
                    path: clone_bytes(target.as_bytes(), budget)?,
                },
                budget,
            )?;
        }
        check_candidate_word(
            target, access, None, part, protected, shared, unit_cands, opts, budget, out,
        )?;
    }
    Ok(())
}

/// Extracts candidate path words from a best-effort reading while respecting
/// §11 item 9 (skipping search-tool pattern operands and git format/grep values).
#[allow(clippy::too_many_arguments)]
fn check_reading_operands(
    folded: &str,
    eff: &[String],
    part: &PartId,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    unit_cands: &[Vec<u8>],
    opts: &BestEffortOptions<'_>,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    let access = if is_known_writer_be(folded, eff, budget)? {
        AccessMode::Write
    } else if is_writer_be(folded, eff, budget)? {
        AccessMode::ReadWrite
    } else {
        AccessMode::Read
    };

    let is_search_tool = matches!(
        folded,
        "grep" | "egrep" | "fgrep" | "zgrep" | "ggrep" | "rg" | "rgrep" | "ag" | "ack"
    );
    let is_git = folded == "git";
    let is_script_cmd = matches!(folded, "sed" | "awk" | "gawk" | "mawk");

    // For search tools, compute which positional index is the pattern operand
    // (§11 item 9: `grep .npmrc README.md` does NOT treat `.npmrc` as a path).
    let mut skip_pattern_idx: Option<usize> = None;
    let mut skip_flag_val_indices: Vec<usize> = Vec::new();
    if is_search_tool {
        let mut has_pat_flag = false;
        let mut dd = false;
        let mut j = 1usize;
        while j < eff.len() {
            let w = eff[j].as_str();
            budget.charge(w.len().max(1) as u64)?;
            if !dd && w == "--" {
                dd = true;
                j += 1;
                continue;
            }
            if !dd && w.starts_with('-') && w != "-" {
                if matches!(w, "-e" | "--regexp") {
                    has_pat_flag = true;
                    if j + 1 < eff.len() {
                        skip_flag_val_indices.push(j + 1);
                    }
                    j += 2;
                    continue;
                }
                if w.starts_with("--regexp=") || (w.starts_with("-e") && w.len() > 2) {
                    has_pat_flag = true;
                    j += 1;
                    continue;
                }
                if matches!(w, "-f" | "--file") {
                    has_pat_flag = true;
                    j += 2;
                    continue;
                }
                if w.starts_with("--file=") || (w.starts_with("-f") && w.len() > 2) {
                    has_pat_flag = true;
                    j += 1;
                    continue;
                }
                if matches!(
                    w,
                    "-g" | "--glob"
                        | "--iglob"
                        | "--include"
                        | "--exclude"
                        | "--exclude-dir"
                        | "-r"
                        | "--replace"
                        | "-t"
                        | "--type"
                        | "-T"
                        | "--type-not"
                        | "--type-add"
                        | "-m"
                        | "--max-count"
                        | "-A"
                        | "-B"
                        | "-C"
                ) {
                    if j + 1 < eff.len() {
                        skip_flag_val_indices.push(j + 1);
                    }
                    j += 2;
                    continue;
                }
                if w == "--files" || w == "--type-list" {
                    has_pat_flag = true;
                }
                j += 1;
                continue;
            }
            if !has_pat_flag && skip_pattern_idx.is_none() {
                skip_pattern_idx = Some(j);
            }
            j += 1;
        }
    }

    for (idx, w) in eff.iter().enumerate() {
        budget.charge(w.len().max(1) as u64)?;
        if idx == 0 {
            if w.contains('/') {
                check_candidate_word(
                    w,
                    AccessMode::Read,
                    Some(folded),
                    part,
                    protected,
                    shared,
                    unit_cands,
                    opts,
                    budget,
                    out,
                )?;
            }
            continue;
        }
        if skip_pattern_idx == Some(idx) || skip_flag_val_indices.contains(&idx) {
            continue;
        }
        if w.starts_with("--") {
            if let Some((flag, val)) = w.split_once('=') {
                if is_search_tool
                    && matches!(
                        flag,
                        "--regexp"
                            | "--glob"
                            | "--iglob"
                            | "--include"
                            | "--exclude"
                            | "--exclude-dir"
                            | "--replace"
                    )
                {
                    continue;
                }
                if is_git && matches!(flag, "--format" | "--pretty" | "--grep" | "--author") {
                    continue;
                }
                if !val.is_empty() {
                    check_candidate_word(
                        val,
                        access,
                        Some(folded),
                        part,
                        protected,
                        shared,
                        unit_cands,
                        opts,
                        budget,
                        out,
                    )?;
                    if is_script_cmd {
                        check_script_tokens(
                            val, access, folded, part, protected, shared, unit_cands, opts, budget,
                            out,
                        )?;
                    }
                }
            }
        } else if w.starts_with('-') && w.len() > 1 {
            // Skip `-ePAT` / `-gGLOB` / `-rREPL` on search tools (§11 item 9).
            if is_search_tool && (w.starts_with("-e") || w.starts_with("-g") || w.starts_with("-r"))
            {
                continue;
            }
            // Attached short-flag values (`-f.npmrc`, `-uf.npmrc`, `-o.npmrc`, `-éf.npmrc`).
            let mut char_offsets: Vec<usize> = Vec::new();
            for (off, _) in w.char_indices().skip(1).take(64) {
                char_offsets.push(off);
            }
            for &off in char_offsets.iter().skip(1) {
                let rest = w[off..].strip_prefix('=').unwrap_or(&w[off..]);
                if !rest.is_empty() {
                    check_candidate_word(
                        rest,
                        access,
                        Some(folded),
                        part,
                        protected,
                        shared,
                        unit_cands,
                        opts,
                        budget,
                        out,
                    )?;
                    if is_script_cmd {
                        check_script_tokens(
                            rest, access, folded, part, protected, shared, unit_cands, opts,
                            budget, out,
                        )?;
                    }
                }
            }
        } else {
            check_candidate_word(
                w,
                access,
                Some(folded),
                part,
                protected,
                shared,
                unit_cands,
                opts,
                budget,
                out,
            )?;
            if is_script_cmd {
                check_script_tokens(
                    w, access, folded, part, protected, shared, unit_cands, opts, budget, out,
                )?;
            }
            if let Some((k, v)) = w.split_once('=')
                && !k.is_empty()
                && !v.is_empty()
            {
                let kv_access = if k == "of" { AccessMode::Write } else { access };
                check_candidate_word(
                    v,
                    kv_access,
                    Some(folded),
                    part,
                    protected,
                    shared,
                    unit_cands,
                    opts,
                    budget,
                    out,
                )?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn check_script_tokens(
    s: &str,
    access: AccessMode,
    folded_cmd: &str,
    part: &PartId,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    unit_cands: &[Vec<u8>],
    opts: &BestEffortOptions<'_>,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    budget.charge(s.len().max(1) as u64)?;
    for tok in s.split(|c: char| {
        c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '(' | ')' | ',' | '{' | '}')
    }) {
        if tok.is_empty() || tok.len() == s.len() {
            continue;
        }
        if tok.starts_with(['.', '/', '~', '$']) || tok.contains('/') {
            check_candidate_word(
                tok,
                access,
                Some(folded_cmd),
                part,
                protected,
                shared,
                unit_cands,
                opts,
                budget,
                out,
            )?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn check_candidate_word(
    word: &str,
    access: AccessMode,
    folded_cmd: Option<&str>,
    part: &PartId,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    unit_cands: &[Vec<u8>],
    opts: &BestEffortOptions<'_>,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    let expanded_words: Vec<String> = if word.contains('{') {
        match brace_expand_budgeted(word, budget)? {
            Some(v) => v,
            None => vec![clone_str_budgeted(word, budget)?],
        }
    } else {
        vec![clone_str_budgeted(word, budget)?]
    };

    let skip_glob = opts
        .quoted_literal_words
        .is_some_and(|set| set.contains(word.as_bytes()));
    let ssh_dir_blocks = match folded_cmd {
        Some(c) => !SSH_DIR_LISTERS.contains(&c),
        None => access != AccessMode::Read,
    };

    for w in &expanded_words {
        budget.charge(w.len().max(1) as u64)?;
        let globby = !skip_glob && w.contains(['*', '?', '[']);
        for cand in unit_cands {
            let expanded = expand_home_be(w, &protected.home, cand, budget)?;
            let abs = if expanded.starts_with('/') {
                lexical_normalize_bytes(expanded.as_bytes(), budget)?
            } else {
                join_lexical_bytes(cand, expanded.as_bytes(), budget)?
            };
            let folded = fold_path_listed(&abs, protected.folds, budget)?;
            if let Some(id) = shared.hard_deny_hit_folded(&folded, access) {
                push_be_hit(
                    out,
                    part.clone(),
                    id,
                    HitMatch::ProtectedPath { path: abs },
                    budget,
                )?;
                return Ok(());
            }
            if ssh_dir_blocks && is_ssh_dir_folded(&folded, shared.home_folded()) {
                push_be_hit(
                    out,
                    part.clone(),
                    "credential-read",
                    HitMatch::ProtectedPath { path: abs },
                    budget,
                )?;
                return Ok(());
            }
            if globby
                && let Some(id) =
                    glob_hit_budgeted(&folded, access, shared, protected.folds, budget)?
            {
                push_be_hit(
                    out,
                    part.clone(),
                    id,
                    HitMatch::ProbeName {
                        probe: String::from_utf8_lossy(&folded).into_owned(),
                    },
                    budget,
                )?;
                return Ok(());
            }
        }
    }
    Ok(())
}

fn is_ssh_dir_folded(folded: &[u8], home_folded: &[u8]) -> bool {
    under_bytes(folded, home_folded) == Some(b".ssh")
}

/// Budgeted port of `dev` `Sensitive::glob_hit` (`deny.rs:594-619`).
fn glob_hit_budgeted(
    folded_pattern: &[u8],
    access: AccessMode,
    shared: &SharedSensitive,
    _folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<&'static str>, Exhausted> {
    let Ok(pat_str) = std::str::from_utf8(folded_pattern) else {
        return Ok(None);
    };
    budget.charge(pat_str.len().max(1) as u64)?;

    // 1. Literal directory prefix before the first glob component.
    let mut prefix = Vec::new();
    budget.charge_alloc(pat_str.len())?;
    for comp in pat_str.split('/').filter(|c| !c.is_empty()) {
        if comp.contains(['*', '?', '[']) {
            break;
        }
        prefix.push(b'/');
        prefix.extend_from_slice(comp.as_bytes());
    }
    if !prefix.is_empty()
        && prefix != b"/"
        && let Some(id) = shared.hard_deny_hit_folded(&prefix, access)
    {
        return Ok(Some(id));
    }

    // 2. Match against representative hard-deny probes using shell-glob semantics
    //    (leading dot must be matched explicitly unless `**`).
    let matcher = ShellGlobBe::compile(pat_str, budget)?;
    let probes = shared.hard_deny_probes(budget)?;
    for (probe, id) in probes {
        if let Ok(ps) = std::str::from_utf8(&probe)
            && matcher.is_match(ps, budget)?
        {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

struct ShellGlobBe {
    whole: Option<GlobMatcher>,
    parts: Vec<(bool, Option<GlobMatcher>)>,
}

impl ShellGlobBe {
    fn compile(pat: &str, budget: &mut ComputeBudget) -> Result<Self, Exhausted> {
        let build =
            |p: &str, budget: &mut ComputeBudget| -> Result<Option<GlobMatcher>, Exhausted> {
                budget.charge((p.len() as u64).saturating_mul(8).max(8))?;
                budget.charge_alloc(p.len().saturating_mul(16).max(64))?;
                Ok(GlobBuilder::new(p)
                    .literal_separator(true)
                    .build()
                    .ok()
                    .map(|g| g.compile_matcher()))
            };
        if pat.contains("**") {
            return Ok(Self {
                whole: build(pat, budget)?,
                parts: Vec::new(),
            });
        }
        let mut parts = Vec::new();
        for seg in pat.split('/') {
            budget.charge_alloc(std::mem::size_of::<(bool, Option<GlobMatcher>)>())?;
            parts.push((seg.starts_with('.'), build(seg, budget)?));
        }
        Ok(Self { whole: None, parts })
    }

    fn is_match(&self, probe: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
        budget.charge(probe.len().max(1) as u64)?;
        if let Some(w) = &self.whole {
            return Ok(w.is_match(probe));
        }
        if self.parts.is_empty() {
            return Ok(false);
        }
        let mut idx = 0usize;
        for seg in probe.split('/') {
            let Some((dot, matcher)) = self.parts.get(idx) else {
                return Ok(false);
            };
            if (!*dot && seg.starts_with('.')) || !matcher.as_ref().is_some_and(|m| m.is_match(seg))
            {
                return Ok(false);
            }
            idx += 1;
        }
        Ok(idx == self.parts.len())
    }
}

// ---------------------------------------------------------------------------
// Best-effort search-glob check for `ToolSubject::Paths { glob: Some(g) }`
// ---------------------------------------------------------------------------

/// Handles brace-expanded, `$HOME`-containing, backslash-escaped, and
/// dot-class (`[.][.]`) search globs on `ToolSubject::Paths` (complementing P2's
/// `resolve_subject_paths`, §2, §5, D-100).
pub(super) fn check_search_glob_best_effort(
    roots: &[String],
    glob: &str,
    recursive: bool,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    let part = PartId::Filter { unit: None };
    budget.charge(glob.len().max(1) as u64)?;
    if glob.contains('\0') {
        push_charged(
            out,
            PartResult {
                part,
                outcome: PartOutcome::Unread(UnreadReason::UnsupportedFilterForm),
            },
            budget,
        )?;
        return Ok(());
    }

    // Fast path: plain relative filter with no braces, backslashes, `$`, or dot-class.
    let has_special =
        glob.contains(['{', '}', '\\', '$']) || glob.split('/').any(may_be_dot_component);
    if !has_special {
        return Ok(());
    }

    let shared = SharedSensitive::for_protected_budgeted(protected, budget)?;

    // Unescape backslashes as an additional candidate, then brace-expand (N11).
    let mut raw_variants: Vec<String> = vec![clone_str_budgeted(glob, budget)?];
    let mut mark_unread = glob.contains(['\\', '$']);
    if glob.contains('\\') {
        budget.charge(glob.len() as u64)?;
        budget.charge_alloc(glob.len())?;
        let mut unesc = String::with_capacity(glob.len());
        let mut chars = glob.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(next) = chars.next() {
                    unesc.push(next);
                } else {
                    unesc.push('\\');
                }
            } else {
                unesc.push(c);
            }
        }
        raw_variants.push(unesc);
    }

    let mut expanded: Vec<String> = Vec::new();
    for r in &raw_variants {
        match brace_expand_budgeted(r, budget)? {
            Some(vs) => {
                if expanded.len() + vs.len() > MAX_SEARCH_GLOB_VARIANTS {
                    // Unbounded glob expansion -> Exhausted (Ask everywhere, §2).
                    push_charged(
                        out,
                        PartResult {
                            part,
                            outcome: PartOutcome::Exhausted(ExhaustReason::ComputeBudget),
                        },
                        budget,
                    )?;
                    return Ok(());
                }
                expanded.extend(vs);
            }
            None => {
                // Exceeded MAX_BRACE_RESULTS / MAX_SEQUENCE / MAX_BRACE_WORK -> Exhausted (§2).
                push_charged(
                    out,
                    PartResult {
                        part,
                        outcome: PartOutcome::Exhausted(ExhaustReason::ComputeBudget),
                    },
                    budget,
                )?;
                return Ok(());
            }
        }
    }

    // Resolve lexical root paths.
    let mut root_paths: Vec<Vec<u8>> = Vec::new();
    if roots.is_empty() {
        root_paths.push(clone_bytes(&protected.cwd, budget)?);
    } else {
        for r in roots {
            let exp = expand_home_be(r, &protected.home, &protected.cwd, budget)?;
            let abs = if exp.starts_with('/') {
                lexical_normalize_bytes(exp.as_bytes(), budget)?
            } else {
                join_lexical_bytes(&protected.cwd, exp.as_bytes(), budget)?
            };
            root_paths.push(abs);
        }
    }
    let mut root_folded: Vec<Vec<u8>> = Vec::with_capacity(root_paths.len());
    for rp in &root_paths {
        root_folded.push(fold_path_listed(rp, protected.folds, budget)?);
    }

    let probes = shared.hard_deny_probes(budget)?;

    for variant in &expanded {
        if variant.is_empty() {
            continue;
        }
        budget.charge(variant.len() as u64)?;
        if variant.split('/').any(may_be_dot_component) {
            mark_unread = true;
        }
        let h = expand_home_be(variant, &protected.home, &protected.cwd, budget)?;
        if let Some(rest) = h.strip_prefix('~')
            && !rest.is_empty()
            && !rest.starts_with('/')
        {
            mark_unread = true;
            continue;
        }
        let anchored = h.starts_with('/') || h.starts_with('~');
        if anchored {
            mark_unread = true;
        }
        let mut pat_abs_list: Vec<Vec<u8>> = Vec::new();
        if anchored {
            pat_abs_list.push(lexical_normalize_bytes(h.as_bytes(), budget)?);
        } else {
            for rp in &root_paths {
                pat_abs_list.push(join_lexical_bytes(rp, h.as_bytes(), budget)?);
            }
        }

        for pat_abs in &pat_abs_list {
            let pat_folded = fold_path_listed(pat_abs, protected.folds, budget)?;
            // Check whether the literal prefix before the first wildcard escapes all roots.
            if let Ok(ps) = std::str::from_utf8(&pat_folded) {
                let mut lit_prefix = Vec::new();
                for seg in ps.split('/').filter(|s| !s.is_empty()) {
                    if seg.contains(['*', '?', '[']) {
                        break;
                    }
                    lit_prefix.push(b'/');
                    lit_prefix.extend_from_slice(seg.as_bytes());
                }
                if lit_prefix.is_empty() {
                    lit_prefix.push(b'/');
                }
                if !root_folded
                    .iter()
                    .any(|rf| under_bytes(&lit_prefix, rf).is_some())
                {
                    mark_unread = true;
                }
                if let Some(id) = shared.hard_deny_hit_folded(&pat_folded, AccessMode::Read) {
                    push_be_hit(
                        out,
                        part.clone(),
                        id,
                        HitMatch::ProbeName {
                            probe: ps.to_owned(),
                        },
                        budget,
                    )?;
                    return Ok(());
                }
                if let Some(id) = glob_hit_budgeted(
                    &pat_folded,
                    AccessMode::Read,
                    &shared,
                    protected.folds,
                    budget,
                )? {
                    push_be_hit(
                        out,
                        part.clone(),
                        id,
                        HitMatch::ProbeName {
                            probe: ps.to_owned(),
                        },
                        budget,
                    )?;
                    return Ok(());
                }
                // Also test globset match (literal_separator=false) against probes inside roots
                // matching `dev` `search_glob_hit`.
                budget.charge((ps.len() as u64).saturating_mul(8).max(8))?;
                budget.charge_alloc(ps.len().saturating_mul(16).max(64))?;
                if let Ok(g) = GlobBuilder::new(ps).literal_separator(false).build() {
                    let m = g.compile_matcher();
                    for (probe, id) in &probes {
                        let Ok(probe_s) = std::str::from_utf8(probe) else {
                            continue;
                        };
                        budget.charge(probe_s.len().max(1) as u64)?;
                        if m.is_match(probe_s) {
                            push_be_hit(
                                out,
                                part.clone(),
                                id,
                                HitMatch::ProbeName {
                                    probe: probe_s.to_owned(),
                                },
                                budget,
                            )?;
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Non-anchored basename/relative probe match for recursive searches.
        if !anchored && let Ok(hf) = fold_str_listed(&h, protected.folds, budget) {
            budget.charge((hf.len() as u64).saturating_mul(8).max(8))?;
            budget.charge_alloc(hf.len().saturating_mul(16).max(64))?;
            if let Ok(g) = GlobBuilder::new(&hf).literal_separator(false).build() {
                let m = g.compile_matcher();
                for (probe, id) in &probes {
                    let Ok(probe_s) = std::str::from_utf8(probe) else {
                        continue;
                    };
                    let name = probe_s.rsplit('/').next().unwrap_or(probe_s);
                    for rf in &root_folded {
                        let Some(rel_b) = under_bytes(probe, rf) else {
                            continue;
                        };
                        let Ok(rel) = std::str::from_utf8(rel_b) else {
                            continue;
                        };
                        budget.charge(rel.len().max(1) as u64)?;
                        let name_ok = recursive || !rel.contains('/');
                        if (name_ok && m.is_match(name)) || (!rel.is_empty() && m.is_match(rel)) {
                            push_be_hit(
                                out,
                                part.clone(),
                                id,
                                HitMatch::ProbeName {
                                    probe: probe_s.to_owned(),
                                },
                                budget,
                            )?;
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    if mark_unread {
        push_charged(
            out,
            PartResult {
                part,
                outcome: PartOutcome::Unread(UnreadReason::UnsupportedFilterForm),
            },
            budget,
        )?;
    }

    Ok(())
}

fn may_be_dot_component(c: &str) -> bool {
    if c == "*" || c == "**" || !c.contains(['[', '?']) {
        return false;
    }
    let Ok(g) = GlobBuilder::new(c).literal_separator(true).build() else {
        return true;
    };
    let m = g.compile_matcher();
    m.is_match(".") || m.is_match("..")
}

// ---------------------------------------------------------------------------
// Command-specific helpers (ported from `deny.rs` / `words.rs`, all budgeted)
// ---------------------------------------------------------------------------

fn expand_home_be(
    word: &str,
    home: &[u8],
    cwd: &[u8],
    budget: &mut ComputeBudget,
) -> Result<String, Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    let h = String::from_utf8_lossy(home);
    let c = String::from_utf8_lossy(cwd);
    if let Some(after) = word.strip_prefix('~') {
        let (name, rest) = match after.find('/') {
            Some(i) => (&after[..i], &after[i..]),
            None => (after, ""),
        };
        let user = h.rsplit('/').next().filter(|s| !s.is_empty());
        if name.is_empty() || user == Some(name) {
            let len = h.len() + rest.len();
            budget.charge(len.max(1) as u64)?;
            budget.charge_alloc(len)?;
            let mut s = String::with_capacity(len);
            s.push_str(&h);
            s.push_str(rest);
            return Ok(s);
        }
        if name == "+" || name == "-" {
            let len = c.len() + rest.len();
            budget.charge(len.max(1) as u64)?;
            budget.charge_alloc(len)?;
            let mut s = String::with_capacity(len);
            s.push_str(&c);
            s.push_str(rest);
            return Ok(s);
        }
        return clone_str_budgeted(word, budget);
    }
    for (var, val) in [
        ("$HOME", h.as_ref()),
        ("${HOME}", h.as_ref()),
        ("$PWD", c.as_ref()),
        ("${PWD}", c.as_ref()),
    ] {
        if let Some(rest) = word.strip_prefix(var)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            let len = val.len() + rest.len();
            budget.charge(len.max(1) as u64)?;
            budget.charge_alloc(len)?;
            let mut s = String::with_capacity(len);
            s.push_str(val);
            s.push_str(rest);
            return Ok(s);
        }
    }
    clone_str_budgeted(word, budget)
}

fn join_lexical_bytes(
    base: &[u8],
    rel: &[u8],
    budget: &mut ComputeBudget,
) -> Result<Vec<u8>, Exhausted> {
    let total = base.len() + 1 + rel.len();
    budget.charge(total.max(1) as u64)?;
    budget.charge_alloc(total)?;
    let combined = join_folded(base, rel);
    lexical_normalize_bytes(&combined, budget)
}

fn lexical_normalize_bytes(abs: &[u8], budget: &mut ComputeBudget) -> Result<Vec<u8>, Exhausted> {
    budget.charge(abs.len().max(1) as u64)?;
    let mut segs: Vec<&[u8]> = Vec::new();
    for seg in abs.split(|&b| b == b'/').filter(|s| !s.is_empty()) {
        if seg == b"." {
            continue;
        }
        if seg == b".." {
            segs.pop();
        } else {
            budget.charge_alloc(std::mem::size_of::<&[u8]>())?;
            segs.push(seg);
        }
    }
    if segs.is_empty() {
        budget.charge_alloc(1)?;
        return Ok(b"/".to_vec());
    }
    let out_len: usize = segs.iter().map(|s| 1 + s.len()).sum();
    budget.charge(out_len as u64)?;
    budget.charge_alloc(out_len)?;
    let mut out = Vec::with_capacity(out_len);
    for seg in segs {
        out.push(b'/');
        out.extend_from_slice(seg);
    }
    Ok(out)
}

fn is_device_word(
    word: &str,
    unit_cands: &[Vec<u8>],
    home: &[u8],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    for cand in unit_cands {
        let exp = expand_home_be(word, home, cand, budget)?;
        let abs = if exp.starts_with('/') {
            lexical_normalize_bytes(exp.as_bytes(), budget)?
        } else {
            join_lexical_bytes(cand, exp.as_bytes(), budget)?
        };
        let folded = fold_path_listed(&abs, folds, budget)?;
        if let Ok(s) = std::str::from_utf8(&folded)
            && s.starts_with("/dev/")
            && !matches!(
                s,
                "/dev/null"
                    | "/dev/tty"
                    | "/dev/stdout"
                    | "/dev/stderr"
                    | "/dev/stdin"
                    | "/dev/zero"
            )
            && !s.starts_with("/dev/fd/")
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn diskutil_wipes(
    eff: &[String],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let Some(arg1) = eff.get(1) else {
        return Ok(false);
    };
    let s1 = fold_str_listed(arg1, folds, budget)?;
    if matches!(s1.as_str(), "apfs" | "cs" | "corestorage")
        && let Some(arg2) = eff.get(2)
    {
        let v = fold_str_listed(arg2, folds, budget)?;
        budget.charge(16)?;
        if [
            "delete", "erase", "reformat", "zero", "secure", "resize", "split", "merge",
        ]
        .iter()
        .any(|p| v.starts_with(p))
        {
            return Ok(true);
        }
    }
    budget.charge(16)?;
    Ok(s1.starts_with("erase")
        || s1.starts_with("reformat")
        || s1.starts_with("partitiondisk")
        || s1.starts_with("secureerase")
        || s1.starts_with("zerodisk")
        || s1.starts_with("randomdisk"))
}

fn is_known_writer_be(
    folded: &str,
    eff: &[String],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(1)?;
    if WRITERS.contains(&folded) {
        return Ok(true);
    }
    match folded {
        "sed" => {
            for a in &eff[1..] {
                budget.charge(a.len().max(1) as u64)?;
                if a == "-i"
                    || (a.starts_with("-i") && !a.starts_with("--"))
                    || a.starts_with("--in-place")
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        "perl" => {
            for a in &eff[1..] {
                budget.charge(a.len().max(1) as u64)?;
                if a.starts_with("-i")
                    || (a.starts_with('-') && !a.starts_with("--") && a.contains('i'))
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn is_writer_be(
    folded: &str,
    eff: &[String],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    if is_known_writer_be(folded, eff, budget)? {
        return Ok(true);
    }
    let args = &eff[1..];
    let flag = |a: &String, c: char| a.starts_with('-') && !a.starts_with("--") && a.contains(c);
    let long_is = |a: &String, full: &str| {
        let n = a.split('=').next().unwrap_or(a);
        n.starts_with("--") && n.len() >= 3 && full.starts_with(n)
    };
    budget.charge(args.len().max(1) as u64)?;
    let res = match folded {
        "sort" => args.iter().any(|a| flag(a, 'o') || long_is(a, "--output")),
        "uniq" | "xxd" => {
            args.iter()
                .filter(|a| !a.starts_with('-') || *a == "-")
                .count()
                >= 2
        }
        "find" => args.iter().any(|a| {
            a == "-delete"
                || a.starts_with("-exec")
                || a.starts_with("-ok")
                || a.starts_with("-fprint")
                || a == "-fls"
        }),
        "tree" => args.iter().any(|a| flag(a, 'o') || long_is(a, "--output")),
        "sed" => args.iter().any(|a| {
            let script = if let Some(r) = a.strip_prefix("--expression=") {
                Some(r)
            } else if a.starts_with("-e") && a.len() > 2 {
                Some(&a[2..])
            } else if !a.starts_with('-') && (a.contains(char::is_whitespace) || !a.contains('/')) {
                Some(a.as_str())
            } else {
                None
            };
            script.is_some_and(|s| s.contains(['w', 'W']))
        }),
        "awk" | "gawk" | "mawk" => args.iter().any(|a| {
            a.starts_with("-i")
                || a.starts_with("--include")
                || a.contains("inplace")
                || (!a.starts_with('-') && a.contains(['>', '|']))
                || a.contains("system(")
        }),
        "codesign" => {
            let signs = args.iter().any(|a| {
                matches!(a.as_str(), "--sign" | "--force" | "--remove-signature")
                    || flag(a, 's')
                    || flag(a, 'f')
            });
            signs
                || !args.iter().any(|a| {
                    a.starts_with("-d")
                        || a.starts_with("-v")
                        || a == "--display"
                        || a == "--verify"
                })
        }
        _ => !READERS.contains(&folded),
    };
    Ok(res)
}

fn force_push_budgeted(eff: &[String], budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    let mut i = 1usize;
    while i < eff.len() {
        let w = eff[i].as_str();
        budget.charge(w.len().max(1) as u64)?;
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
        return Ok(false);
    }
    let mut force = false;
    let mut positional: Vec<&str> = Vec::new();
    let mut implicit = false;
    let mut after_dd = false;
    let mut j = i + 1;
    while j < eff.len() {
        let w = eff[j].as_str();
        budget.charge(w.len().max(1) as u64)?;
        j += 1;
        if after_dd || !w.starts_with('-') || w == "-" {
            budget.charge_alloc(std::mem::size_of::<&str>())?;
            positional.push(w);
            continue;
        }
        if w == "--" {
            after_dd = true;
        } else if w.starts_with("--") {
            let name = w.split('=').next().unwrap_or(w);
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
        return Ok(implicit || refspecs.is_empty() || refspecs.iter().any(|s| check(s)));
    }
    Ok(refspecs.iter().any(|s| s.starts_with('+') && check(s)))
}

fn rm_recursive_budgeted(
    eff: &[String],
    budget: &mut ComputeBudget,
) -> Result<Option<Vec<String>>, Exhausted> {
    let mut rec = false;
    let mut targets = Vec::new();
    let mut dd = false;
    for w in &eff[1..] {
        budget.charge(w.len().max(1) as u64)?;
        if dd || !w.starts_with('-') || w == "-" {
            budget.charge_alloc(std::mem::size_of::<String>())?;
            targets.push(clone_str_budgeted(w, budget)?);
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
    Ok(rec.then_some(targets))
}

fn has_find_exec_flag(eff: &[String], budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    budget.charge(eff.len().max(1) as u64)?;
    Ok(eff
        .iter()
        .skip(1)
        .any(|a| matches!(a.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir")))
}

fn find_roots_budgeted(
    eff: &[String],
    budget: &mut ComputeBudget,
) -> Result<(Vec<String>, usize), Exhausted> {
    let mut roots = Vec::new();
    let mut j = 1usize;
    while j < eff.len() {
        let w = eff[j].as_str();
        budget.charge(w.len().max(1) as u64)?;
        if w == "--" {
            j += 1;
            break;
        }
        if w == "-D" {
            j += 2;
        } else if w == "-f" {
            if let Some(r) = eff.get(j + 1) {
                budget.charge_alloc(std::mem::size_of::<String>())?;
                roots.push(clone_str_budgeted(r, budget)?);
            }
            j += 2;
        } else if (w.starts_with("-O") && w.len() > 2 && w[2..].chars().all(|c| c.is_ascii_digit()))
            || (w.len() > 1
                && w.starts_with('-')
                && !w.starts_with("--")
                && w[1..].chars().all(|c| "HLPEXdsx".contains(c)))
        {
            j += 1;
        } else {
            break;
        }
    }
    while j < eff.len() {
        let w = eff[j].as_str();
        budget.charge(w.len().max(1) as u64)?;
        if (w.starts_with('-') && w != "-") || w == "!" || w == "(" {
            break;
        }
        budget.charge_alloc(std::mem::size_of::<String>())?;
        roots.push(clone_str_budgeted(w, budget)?);
        j += 1;
    }
    if roots.is_empty() {
        budget.charge_alloc(std::mem::size_of::<String>() + 1)?;
        roots.push(".".to_owned());
    }
    Ok((roots, j.min(eff.len())))
}

fn find_delete_budgeted(
    eff: &[String],
    budget: &mut ComputeBudget,
) -> Result<Option<(Vec<String>, bool)>, Exhausted> {
    budget.charge(eff.len().max(1) as u64)?;
    if !eff.iter().any(|a| a == "-delete") {
        return Ok(None);
    }
    let (roots, j) = find_roots_budgeted(eff, budget)?;
    let mut filtered = false;
    let mut k = j;
    while k < eff.len() {
        budget.charge(1)?;
        match eff[k].as_str() {
            "-delete" | "-depth" => {}
            "-maxdepth" | "-mindepth" => k += 1,
            _ => filtered = true,
        }
        k += 1;
    }
    Ok(Some((roots, filtered)))
}

fn delete_blocked_budgeted(
    targets: &[String],
    filtered_find: Option<bool>,
    unit_cands: &[Vec<u8>],
    unit_cwd_unknown: bool,
    protected: &ProtectedSet,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let home_folded = fold_path_listed(&protected.home, protected.folds, budget)?;
    let cwd_folded = fold_path_listed(&protected.cwd, protected.folds, budget)?;

    for t in targets {
        budget.charge(t.len().max(1) as u64)?;
        let exp_cwd = expand_home_be(t, &protected.home, &protected.cwd, budget)?;
        let relative = !exp_cwd.starts_with('/');
        if unit_cwd_unknown && relative {
            return Ok(true);
        }
        // `~other`: unknown home directory.
        if exp_cwd.starts_with('~') {
            return Ok(true);
        }
        let globby = t.contains(['*', '?', '[']);
        let expanded_words: Vec<String> = if t.contains('{') {
            match brace_expand_budgeted(t, budget)? {
                Some(v) => v,
                None => return Ok(true),
            }
        } else {
            vec![clone_str_budgeted(t, budget)?]
        };
        for w in &expanded_words {
            for cand in unit_cands {
                let exp = expand_home_be(w, &protected.home, cand, budget)?;
                let abs = if exp.starts_with('/') {
                    lexical_normalize_bytes(exp.as_bytes(), budget)?
                } else {
                    join_lexical_bytes(cand, exp.as_bytes(), budget)?
                };
                let pl = fold_path_listed(&abs, protected.folds, budget)?;
                if pl == b"/" || under_bytes(&home_folded, &pl).is_some() {
                    return Ok(true);
                }
                if globby {
                    let parent = parent_bytes(&pl);
                    if under_bytes(&home_folded, parent).is_some() {
                        return Ok(true);
                    }
                    if basename_bytes(&pl) == b".*" {
                        return Ok(true);
                    }
                }
                match under_bytes(&pl, &cwd_folded) {
                    None => return Ok(true),
                    Some(b"") => {
                        if filtered_find != Some(true) {
                            return Ok(true);
                        }
                    }
                    Some(_) => {}
                }
            }
        }
    }
    Ok(false)
}

fn parent_bytes(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&b| b == b'/') {
        Some(0) | None => b"/",
        Some(pos) => &p[..pos],
    }
}

fn basename_bytes(p: &[u8]) -> &[u8] {
    match p.iter().rposition(|&b| b == b'/') {
        Some(pos) => &p[pos + 1..],
        None => p,
    }
}

// ---------------------------------------------------------------------------
// Pipe-to-shell helpers (budgeted, CR-19 folded)
// ---------------------------------------------------------------------------

fn is_shell_folded(folded: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    budget.charge(folded.len().max(1) as u64)?;
    if let Some(spec) = tools::lookup(folded)
        && spec.is_nested_shell()
    {
        return Ok(true);
    }
    Ok(tools::lookup_outside_model(folded)
        .is_some_and(|e| e.contract_section == "§4" && !e.reason.starts_with("Wrapper")))
}

fn is_interpreter_folded(folded: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    if is_shell_folded(folded, budget)? {
        return Ok(true);
    }
    Ok(folded.starts_with("python")
        || matches!(
            folded,
            "perl" | "ruby" | "node" | "deno" | "bun" | "php" | "lua" | "osascript" | "pwsh"
        ))
}

fn is_executor_be(folded: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
    if is_interpreter_folded(folded, budget)? {
        return Ok(true);
    }
    Ok(matches!(folded, "eval" | "source" | "."))
}

fn reads_stdin_as_code_budgeted(
    eff: &[String],
    folds: FoldRules,
    depth: u8,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let Some(first) = eff.first() else {
        return Ok(false);
    };
    if depth > 4 {
        return Ok(true);
    }
    budget.charge(first.len().max(1) as u64)?;
    let folded = fold_cmd_cr19(first, folds, budget)?;
    let args = &eff[1..];

    if matches!(folded.as_str(), "source" | ".") {
        return Ok(args
            .first()
            .is_some_and(|a| matches!(a.as_str(), "/dev/stdin" | "/dev/fd/0" | "-")));
    }

    // `| s$(echo h)` vs `| $(brew --prefix)/bin/jq .`:
    // check command substitution only in the basename (`base_raw`), so
    // `$(brew --prefix)/bin/jq` has basename `"jq"` (not a shell!).
    let base_raw = first.rsplit('/').next().unwrap_or(first.as_str());
    let interp = is_interpreter_folded(&folded, budget)?;
    if !interp && (base_raw.contains("$(") || base_raw.contains('`')) {
        match substituted_program_simple(base_raw, folds, budget)? {
            None => return Ok(true),
            Some(name) => {
                let mut e = Vec::with_capacity(eff.len());
                budget.charge_alloc(eff.len() * std::mem::size_of::<String>())?;
                e.push(name);
                for a in args {
                    e.push(clone_str_budgeted(a, budget)?);
                }
                let u = unwrap_simple_be(&e, folds, budget)?;
                if !u.is_empty() && u != eff {
                    return reads_stdin_as_code_budgeted(&u, folds, depth + 1, budget);
                }
                return Ok(false);
            }
        }
    }
    // `$SHELL`, `$BASH`, `$0`, `$X -s`:
    if !interp
        && matches!(
            first.as_str(),
            "$SHELL" | "${SHELL}" | "$BASH" | "${BASH}" | "$0" | "${0}"
        )
    {
        return Ok(true);
    }
    if !interp && (base_raw.contains('$') || base_raw.contains(['*', '?', '['])) {
        return Ok(args.iter().any(|a| a == "-" || a == "-s"));
    }
    if folded == "eval" || !interp {
        return Ok(false);
    }
    if args.iter().any(|a| a == "-" || a == "-s") {
        return Ok(true);
    }

    let is_sh = is_shell_folded(&folded, budget)?;
    let code_flag = args.iter().any(|a| {
        (a.starts_with('-')
            && !a.starts_with("--")
            && a.len() > 1
            && a[1..]
                .chars()
                .any(|c| c == 'c' || (!is_sh && matches!(c, 'e' | 'E' | 'm'))))
            || (!is_sh && matches!(a.as_str(), "--eval" | "--command" | "--print"))
    });
    if code_flag {
        if is_sh {
            // `sh -c bash`: check if the `-c` code itself reads stdin as code.
            if let Some(code) = extract_shell_c_code(args, budget)? {
                let words = raw_words_budgeted(code, budget)?;
                let inner_eff = unwrap_simple_be(&words, folds, budget)?;
                if reads_stdin_as_code_budgeted(&inner_eff, folds, depth + 1, budget)? {
                    return Ok(true);
                }
            }
        }
        return Ok(false);
    }

    let mut positional = false;
    let mut k = 0usize;
    while k < args.len() {
        budget.charge(1)?;
        let a = args[k].as_str();
        if is_sh && matches!(a, "-o" | "+o" | "-O" | "+O") {
            k += 2;
            continue;
        }
        if !a.starts_with('-') && !a.starts_with('+') {
            positional = true;
        }
        k += 1;
    }
    Ok(!positional)
}

fn extract_shell_c_code<'a>(
    args: &'a [String],
    budget: &mut ComputeBudget,
) -> Result<Option<&'a str>, Exhausted> {
    let mut seen_c = false;
    let mut j = 0usize;
    while j < args.len() {
        let w = args[j].as_str();
        budget.charge(w.len().max(1) as u64)?;
        if w == "--" {
            j += 1;
            break;
        }
        if let Some(rest) = w.strip_prefix("--command=") {
            return Ok(Some(rest));
        }
        if w == "--command" {
            seen_c = true;
            j += 1;
            continue;
        }
        if (w.starts_with('-') || w.starts_with('+')) && w.len() > 1 {
            if w.starts_with('-') && w[1..].contains('c') {
                seen_c = true;
            }
            let extra = w[1..].bytes().filter(|&b| matches!(b, b'o' | b'O')).count();
            j += 1 + extra;
            continue;
        }
        break;
    }
    Ok(if seen_c {
        args.get(j).map(String::as_str)
    } else {
        None
    })
}

fn substituted_program_simple(
    word: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<String>, Exhausted> {
    budget.charge(word.len().max(1) as u64)?;
    let inner = word
        .strip_prefix("$(")
        .and_then(|s| s.strip_suffix(')'))
        .or_else(|| word.strip_prefix('`').and_then(|s| s.strip_suffix('`')));
    let Some(body) = inner else {
        return Ok(None);
    };
    let mut it = body.split_whitespace();
    let w0 = it.next();
    let w1 = it.next();
    let w2 = it.next();
    let w3 = it.next();
    let cand = match (w0, w1, w2, w3) {
        (Some(w0), Some(name), None, None) => {
            let f = fold_cmd_cr19(w0, folds, budget)?;
            matches!(f.as_str(), "which" | "whence" | "echo").then_some(name)
        }
        (Some(w0), Some("-v" | "-V"), Some(name), None) => {
            let f = fold_cmd_cr19(w0, folds, budget)?;
            (f == "command").then_some(name)
        }
        (Some(w0), Some("-p" | "-P"), Some(name), None) => {
            let f = fold_cmd_cr19(w0, folds, budget)?;
            (f == "type").then_some(name)
        }
        _ => None,
    };
    match cand {
        Some(c) if !c.is_empty() && !c.contains(['$', '`', '*', '?', '[']) => {
            Ok(Some(clone_str_budgeted(c, budget)?))
        }
        _ => Ok(None),
    }
}

/// Checks whether any substitution (`$(...)`, `` `...` ``, `<(...)`) inside
/// `u`'s raw argv or redirects invokes a downloader (`curl`, `wget`, `fetch`).
fn unit_subs_have_downloader(
    u: &BestEffortUnit,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let words = u.raw_argv.iter().chain(u.redirects.iter().map(|(_, t)| t));
    for w in words {
        budget.charge(w.len().max(1) as u64)?;
        if !(w.contains("$(") || w.contains('`') || w.contains("<(")) {
            continue;
        }
        if sub_string_contains_downloader(w, folds, budget)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn sub_string_contains_downloader(
    word: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let bytes = word.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        budget.charge(1)?;
        if (bytes[i] == b'$' || bytes[i] == b'<') && bytes.get(i + 1) == Some(&b'(') {
            let start = i + 2;
            let mut depth = 1usize;
            let mut j = start;
            while j < bytes.len() && depth > 0 {
                budget.charge(1)?;
                match bytes[j] {
                    b'(' => depth += 1,
                    b')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let end = if depth == 0 { j - 1 } else { bytes.len() };
            if body_has_downloader(&word[start..end], folds, budget)? {
                return Ok(true);
            }
            i = j;
        } else if bytes[i] == b'`' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'`' {
                budget.charge(1)?;
                j += 1;
            }
            if body_has_downloader(&word[start..j], folds, budget)? {
                return Ok(true);
            }
            i = (j + 1).min(bytes.len());
        } else {
            i += 1;
        }
    }
    Ok(false)
}

fn body_has_downloader(
    body: &str,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(body.len().max(1) as u64)?;
    for segment in body.split([';', '|', '&', '\n', '(', ')']) {
        let words = raw_words_budgeted(segment, budget)?;
        let eff = unwrap_simple_be(&words, folds, budget)?;
        if let Some(first) = eff.first() {
            let f = fold_cmd_cr19(first, folds, budget)?;
            if DOWNLOADERS.contains(&f.as_str()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Helper used by `evaluate.rs`: checks whether `tool_name` is a never-writing
/// `HomeReadOnly` tool in P0's normative table (so its operand access mode is
/// `Read` when no forbidden write flags are present).
pub(super) fn is_home_readonly_tool(tool_name: &str, subcommand: Option<&str>) -> bool {
    let Some(spec) = tools::lookup(tool_name) else {
        return false;
    };
    match spec.kind {
        ToolRowKind::HomeReadOnly(_) => true,
        ToolRowKind::ConditionalRunner(row) => subcommand.is_some_and(|sub| {
            row.subcommands
                .iter()
                .any(|s| s.name == sub && matches!(s.kind, tools::SubcommandKind::HomeReadOnly(_)))
        }),
        _ => false,
    }
}
