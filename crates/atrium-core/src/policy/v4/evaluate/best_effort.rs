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
    PipelineId, ProtectedSet, UnreadReason,
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
#[derive(Default)]
pub(super) struct BestEffortOptions<'a> {
    /// Exact modeled `(unit_index_in_top_level, argv_index)` pairs that are
    /// known to have NO unquoted glob characters in exact model provenance.
    pub(super) quoted_literal_words: Option<&'a HashSet<Vec<u8>>>,
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

    // 2. Top-level unit metadata (O(N) scan of raw_texts[0]) so `cd DIR && ...`
    //    tracks the `&&`-chain pool, subshells, compounds, loops, functions,
    //    `&` backgrounding, and here-strings (matching `dev` `words.rs`).
    let top_meta = if record.raw_texts.len() == 1 {
        scan_top_level_meta(&record.raw_texts[0], budget)?
    } else {
        Vec::new()
    };

    // 3. Per-unit checks with budgeted lexical `cd` tracking.
    let mut cwd_tracker = BeCwdTracker::new(start_cwd, &protected.home, budget)?;
    let mut top_unit_ordinal = 0usize;
    let mut last_find_exec_context: Option<(u8, Vec<String>)> = None;
    let mut env_child_cands: Option<(Vec<Vec<u8>>, bool)> = None;
    let mut pending_derived_meta: Vec<BeUnitMeta> = Vec::new();
    let mut derived_unit_ordinal = 0usize;

    budget.charge(record.units.len().max(1) as u64)?;
    for (i, u) in record.units.iter().enumerate() {
        budget.charge(1)?;
        let part = PartId::BestEffortUnit(u.index);

        // Determine join/compound metadata for this unit.
        let default_seq = BeUnitMeta::seq();
        let default_and = BeUnitMeta::and();
        let unit_meta: &BeUnitMeta = if !u.derived && u.depth == 0 {
            let m = top_meta.get(top_unit_ordinal).unwrap_or(&default_seq);
            top_unit_ordinal += 1;
            m
        } else {
            let m = pending_derived_meta
                .get(derived_unit_ordinal)
                .unwrap_or(&default_and);
            derived_unit_ordinal += 1;
            m
        };

        // Update depth-scoped & compound-scoped cwd tracker before inspecting this unit.
        cwd_tracker.enter_unit(u, unit_meta, &mut env_child_cands, budget)?;
        let base_cands = cwd_tracker.current_cands(budget)?;
        let base_cwd_unknown = cwd_tracker.is_unknown();

        // Check if `u.raw_argv` starts with `env` (or `$(which env)`) using
        // `-C DIR` / `--chdir` / `--ch*` and compute shifted per-unit cwds.
        let env_shift = extract_env_chdir_cands(
            &u.raw_argv,
            &base_cands,
            base_cwd_unknown,
            &cwd_tracker,
            protected.folds,
            budget,
        )?;
        let (unit_cands, unit_cwd_unknown) = match env_shift {
            Some((shifted, shifted_unknown)) => {
                // If this `env -C` wraps a nested shell (`sh -c ...`) or `-S`,
                // propagate its shifted directory into the derived child frame.
                env_child_cands = Some((clone_bytes_vec(&shifted, budget)?, shifted_unknown));
                (shifted, shifted_unknown)
            }
            None => (base_cands, base_cwd_unknown),
        };

        // Track `find` roots for immediately-following derived `-exec` units.
        let mut next_find_exec_context: Option<(u8, Vec<String>)> = None;
        let active_exec_roots: Option<&[String]> = match &last_find_exec_context {
            Some((fdepth, roots)) if u.derived && u.depth > *fdepth => Some(roots.as_slice()),
            _ => None,
        };

        // Build extra cluster-aware / nested-substitution readings from `raw_argv`
        // (catches `exec -ca foo rm -rf ~`, `flock /tmp/l -s rm -rf ~`,
        // `$(which env) $(which sudo) id`, `script -qc 'rm -rf ~' /dev/null`, etc.).
        let (extra_readings, extra_payloads) =
            extra_readings_from_raw(&u.raw_argv, &u.effective_argv, protected.folds, budget)?;

        // Check every reading (`effective_argv` + `alt_argv` + cluster-unwrapped readings).
        let readings_count = 1 + u.alt_argv.len() + extra_readings.len();
        budget.charge(readings_count as u64)?;
        let mut unit_is_same_shell_parent = false;
        let mut same_shell_eval_payload: Option<String> = None;
        let mut new_derived_meta: Option<Vec<BeUnitMeta>> = None;
        for reading in std::iter::once(&u.effective_argv)
            .chain(u.alt_argv.iter())
            .chain(extra_readings.iter())
        {
            if reading.is_empty() {
                continue;
            }
            let folded_cmd = fold_cmd_cr19(&reading[0], protected.folds, budget)?;
            if matches!(folded_cmd.as_str(), "eval" | "source" | ".") {
                unit_is_same_shell_parent = true;
                if same_shell_eval_payload.is_none() && folded_cmd == "eval" {
                    let args = match reading.get(1).map(String::as_str) {
                        Some("--") => &reading[2..],
                        _ => &reading[1..],
                    };
                    if !args.is_empty() {
                        let total: usize = args.iter().map(|s| s.len() + 1).sum();
                        budget.charge(total as u64)?;
                        budget.charge_alloc(total)?;
                        same_shell_eval_payload = Some(args.join(" "));
                    }
                }
            }
            if !u.derived
                && new_derived_meta.is_none()
                && is_shell_folded(&folded_cmd, budget)?
                && let Some(code) = extract_shell_c_code(&reading[1..], budget)?
            {
                new_derived_meta = Some(scan_top_level_meta(code, budget)?);
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
        // Check any inline code payloads extracted by our cluster-aware unwrapper
        // (e.g. `script -qc 'rm -rf ~' /dev/null`, `flock /tmp/l --comm 'rm -rf ~'`).
        for payload in &extra_payloads {
            check_inline_payload_be(
                payload,
                &part,
                protected,
                &shared,
                &unit_cands,
                unit_cwd_unknown,
                opts,
                budget,
                out,
            )?;
        }
        // If our alt-wrapper unwrapper exposed a shell / `source` / `.` reading
        // stdin (piped `echo`, heredoc, or `<(echo ...)`), check its payload too.
        if !extra_readings.is_empty() {
            let mut exposes_stdin_shell = false;
            for r in &extra_readings {
                if let Some(first) = r.first() {
                    let rf = fold_cmd_cr19(first, protected.folds, budget)?;
                    if matches!(rf.as_str(), "source" | ".")
                        || reads_stdin_as_script_budgeted(&rf, r, budget)?
                        || is_shell_folded(&rf, budget)?
                    {
                        exposes_stdin_shell = true;
                        break;
                    }
                }
            }
            if exposes_stdin_shell {
                if u.piped
                    && let Some(pid) = u.pipeline_id
                    && let Some(piped_body) = extract_piped_echo_for_source(
                        &record.units[..i],
                        pid,
                        protected.folds,
                        budget,
                    )?
                {
                    check_inline_payload_be(
                        &piped_body,
                        &part,
                        protected,
                        &shared,
                        &unit_cands,
                        unit_cwd_unknown,
                        opts,
                        budget,
                        out,
                    )?;
                }
                if let Some((hs, he)) = unit_meta.heredoc_body
                    && let Some(raw_line) = record.raw_texts.first()
                    && he <= raw_line.len()
                {
                    check_inline_payload_be(
                        &raw_line[hs..he],
                        &part,
                        protected,
                        &shared,
                        &unit_cands,
                        unit_cwd_unknown,
                        opts,
                        budget,
                        out,
                    )?;
                }
                for psub_payload in extract_proc_sub_echo_payloads(u, protected.folds, budget)? {
                    check_inline_payload_be(
                        &psub_payload,
                        &part,
                        protected,
                        &shared,
                        &unit_cands,
                        unit_cwd_unknown,
                        opts,
                        budget,
                        out,
                    )?;
                }
            }
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

        // If this unit is `eval`/`source`/`.` at depth 0, apply its same-shell
        // `cd` effects immediately (before subsequent depth-0 siblings run,
        // because P1's tolerant lexer emits deferred depth-1 units in BFS order).
        let same_shell_payload = if unit_is_same_shell_parent && !u.derived && u.depth == 0 {
            if let Some(p) = same_shell_eval_payload {
                Some(p)
            } else if let Some((hs_start, hs_end)) = unit_meta.herestring
                && let Some(raw_line) = record.raw_texts.first()
                && hs_end <= raw_line.len()
            {
                let unq = unquote_herestring_be(&raw_line[hs_start..hs_end], budget)?;
                Some(unq)
            } else if u.piped
                && let Some(pid) = u.pipeline_id
            {
                extract_piped_echo_for_source(&record.units[..i], pid, protected.folds, budget)?
            } else {
                None
            }
        } else {
            None
        };

        // Check whether the immediately following sibling is a plain `exit`
        // (not `! exit`, not `exit &`, not in a pipeline/group).
        let next_meta = if !u.derived && u.depth == 0 {
            top_meta.get(top_unit_ordinal)
        } else {
            None
        };
        let next_is_exit = record.units.get(i + 1).is_some_and(|n| {
            n.depth == u.depth
                && n.pipeline_id.is_none()
                && !n.piped
                && n.raw_argv.first().is_some_and(|f| f == "exit")
                && n.effective_argv.first().is_some_and(|f| f == "exit")
                && next_meta
                    .is_none_or(|m| m.join != BeJoin::Bg && m.compound == unit_meta.compound)
        });
        cwd_tracker.after_unit(
            u,
            primary_cmd.as_deref(),
            unit_meta,
            next_is_exit,
            unit_is_same_shell_parent,
            same_shell_payload.as_deref(),
            protected.folds,
            budget,
        )?;
        if let Some(dm) = new_derived_meta {
            pending_derived_meta = dm;
            derived_unit_ordinal = 0;
        }
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
    for dl in ["curl", "wget", "fetch"] {
        budget.charge(lower.len().max(1) as u64)?;
        let mut rest = lower.as_str();
        while let Some(i) = rest.find(dl) {
            rest = &rest[i + dl.len()..];
            let seg = rest.split([';', '\n', '&']).next().unwrap_or("");
            let mut pipe_stages = seg.split('|');
            let _ = pipe_stages.next();
            for after in pipe_stages {
                let words = raw_words_budgeted(after, budget)?;
                if reads_stdin_as_code_budgeted(&words, folds, 0, budget)? {
                    return Ok(true);
                }
                let eff0 = unwrap_simple_be(&words, folds, budget)?;
                if eff0 != words && reads_stdin_as_code_budgeted(&eff0, folds, 0, budget)? {
                    return Ok(true);
                }
                let eff1 = unwrap_simple_be_mode(&words, folds, true, budget)?;
                if eff1 != words
                    && eff1 != eff0
                    && reads_stdin_as_code_budgeted(&eff1, folds, 0, budget)?
                {
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
                    prev = c;
                    continue;
                }
            }
            '\'' | '"' if depth == 0 && !tick => {
                quote = Some(c);
                prev = c;
                continue;
            }
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

/// Checks if a short-option cluster (e.g. `-it`, `-fs`, `-sw`, `-iu`, `-ca`,
/// `-tc`, `-tI`) consumes the next argv word as its value (i.e. contains a
/// value-taking letter from `val_chars` at the end of the cluster; if chars
/// follow that letter, the value is attached to the same token).
fn short_cluster_takes_next(w: &str, val_chars: &str) -> bool {
    if !w.starts_with('-') || w.starts_with("--") || w.len() < 2 {
        return false;
    }
    let body = &w[1..];
    for (off, ch) in body.char_indices() {
        if val_chars.contains(ch) {
            return off + ch.len_utf8() == body.len();
        }
    }
    false
}

/// Checks whether `w` matches `--long` or an unambiguous GNU prefix of `--long`
/// (with at least `min_len` chars including `--`, e.g. `--chd` for `--chdir` or
/// `--comm` for `--command`), returning `Some(attached_value)` where `None`
/// means the value is in the next argv word.
fn match_gnu_long_opt<'a>(w: &'a str, full: &str, min_len: usize) -> Option<Option<&'a str>> {
    if !w.starts_with("--") {
        return None;
    }
    let (name, attached) = match w.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (w, None),
    };
    (name.len() >= min_len && full.starts_with(name)).then_some(attached)
}

fn unwrap_simple_be(
    words: &[String],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    unwrap_simple_be_mode(words, folds, false, budget)
}

fn unwrap_simple_be_mode(
    words: &[String],
    folds: FoldRules,
    opt_takes_val: bool,
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let mut cur: Vec<String> = {
        let bytes: usize = words
            .iter()
            .map(|s| s.len() + std::mem::size_of::<String>())
            .sum();
        budget.charge(bytes.max(1) as u64)?;
        budget.charge_alloc(bytes)?;
        words.to_vec()
    };
    let mut idx = 0usize;
    for _ in 0..16 {
        let Some(first) = cur.get(idx) else { break };
        // Resolve nested command-word substitutions (`$(which env) $(which sudo) id`).
        let base_raw = first.rsplit('/').next().unwrap_or(first.as_str());
        if (base_raw.contains("$(") || base_raw.contains('`'))
            && let Some(sub_name) = substituted_program_simple(base_raw, folds, budget)?
        {
            cur[idx] = sub_name;
        }
        let Some(first) = cur.get(idx) else { break };
        let f = fold_cmd_cr19(first, folds, budget)?;
        match f.as_str() {
            "builtin" | "busybox" | "toybox" => {
                idx += 1;
            }
            "nohup" => {
                idx += 1;
                while let Some(w) = cur.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if w.starts_with('-') && w.len() > 1 {
                        if opt_takes_val && !w.contains('=') {
                            idx += 2;
                        } else {
                            idx += 1;
                        }
                    } else {
                        break;
                    }
                }
            }
            "command" => {
                idx += 1;
                while let Some(w) = cur.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if w.starts_with('-') && w.len() > 1 {
                        let known_bool = !w.starts_with("--")
                            && w[1..].bytes().all(|b| matches!(b, b'p' | b'v' | b'V'));
                        if opt_takes_val && !known_bool && !w.contains('=') {
                            idx += 2;
                        } else {
                            idx += 1;
                        }
                    } else {
                        break;
                    }
                }
            }
            "exec" => {
                idx += 1;
                while let Some(w) = cur.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if w.starts_with('-') && w.len() > 1 {
                        idx += if short_cluster_takes_next(w, "a") {
                            2
                        } else {
                            1
                        };
                    } else {
                        break;
                    }
                }
            }
            "time" => {
                idx += 1;
                while let Some(w) = cur.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if w.starts_with('-') && w.len() > 1 {
                        let long_takes = match_gnu_long_opt(w, "--format", 5) == Some(None)
                            || match_gnu_long_opt(w, "--output", 5) == Some(None);
                        let known_bool = matches!(
                            w.as_str(),
                            "--portability" | "--verbose" | "--quiet" | "--append"
                        ) || (!w.starts_with("--")
                            && w[1..]
                                .bytes()
                                .all(|b| matches!(b, b'a' | b'p' | b'q' | b'v')));
                        if long_takes
                            || short_cluster_takes_next(w, "fo")
                            || (opt_takes_val && !known_bool && !w.contains('='))
                        {
                            idx += 2;
                        } else {
                            idx += 1;
                        }
                    } else {
                        break;
                    }
                }
            }
            "nice" => {
                idx += 1;
                while let Some(w) = cur.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    let long_takes = match_gnu_long_opt(w, "--adjustment", 5) == Some(None);
                    let is_num = w.starts_with('-')
                        && w.len() > 1
                        && w[1..].bytes().all(|b| b.is_ascii_digit());
                    if long_takes
                        || short_cluster_takes_next(w, "n")
                        || (opt_takes_val
                            && w.starts_with('-')
                            && w.len() > 1
                            && !is_num
                            && !w.contains('=')
                            && !w.starts_with("-n"))
                    {
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
                while let Some(w) = cur.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    let long_takes = match_gnu_long_opt(w, "--signal", 4) == Some(None)
                        || match_gnu_long_opt(w, "--kill-after", 6) == Some(None);
                    let known_bool = matches!(
                        w.as_str(),
                        "--foreground" | "--preserve-status" | "--verbose"
                    ) || (!w.starts_with("--")
                        && w[1..].bytes().all(|b| matches!(b, b'f' | b'v')));
                    if long_takes
                        || short_cluster_takes_next(w, "sk")
                        || (opt_takes_val
                            && w.starts_with('-')
                            && w.len() > 1
                            && !known_bool
                            && !w.contains('='))
                    {
                        idx += 2;
                    } else if w.starts_with('-') && w.len() > 1 {
                        idx += 1;
                    } else {
                        break;
                    }
                }
                idx = (idx + 1).min(cur.len());
            }
            "env" => {
                idx += 1;
                while let Some(w) = cur.get(idx) {
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
                            "--unset" | "--chdir" | "--split-string" | "--argv0"
                        ) || match_gnu_long_opt(w, "--chdir", 4) == Some(None)
                            || match_gnu_long_opt(w, "--split-string", 5) == Some(None)
                            || match_gnu_long_opt(w, "--unset", 4) == Some(None)
                            || match_gnu_long_opt(w, "--argv0", 5) == Some(None)
                            || w == "--arg0"
                            || short_cluster_takes_next(w, "uCPaS")
                            || (!w.contains('=')
                                && !is_known_env_noarg_opt(w)
                                && (opt_takes_val
                                    || cur.get(idx + 2).is_some_and(|n2| {
                                        (n2.starts_with('-') && n2.len() > 1) || n2.contains('=')
                                    })))
                        {
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
            "caffeinate" | "ionice" | "stdbuf" | "xargs" | "setsid" | "unbuffer"
            | "sandbox-exec" | "chrt" | "taskset" | "arch" => {
                let short_vals = match f.as_str() {
                    "caffeinate" => "tw",
                    "ionice" => "cnpPu",
                    "stdbuf" => "ioe",
                    "xargs" => "nPILsdEJRSa",
                    "sandbox-exec" => "fnpD",
                    "chrt" => "TPD",
                    "taskset" => "c",
                    "arch" => "ed",
                    _ => "",
                };
                let opt_short_vals = match f.as_str() {
                    "xargs" => "iel",
                    _ => "",
                };
                let known_short_bools = match f.as_str() {
                    "caffeinate" => "dimsu",
                    "ionice" => "t",
                    "xargs" => "0oprtx",
                    "setsid" => "cfw",
                    "chrt" => "bfiorpm",
                    "taskset" => "ap",
                    "unbuffer" => "p",
                    _ => "",
                };
                let mut taskset_has_cpu_list = false;
                idx += 1;
                while let Some(w) = cur.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    if w.starts_with('-') && w.len() > 1 {
                        if f == "taskset"
                            && (match_gnu_long_opt(w, "--cpu-list", 5).is_some()
                                || (!w.starts_with("--") && w[1..].contains('c')))
                        {
                            taskset_has_cpu_list = true;
                        }
                        let long_takes = match f.as_str() {
                            "ionice" => {
                                match_gnu_long_opt(w, "--class", 5) == Some(None)
                                    || match_gnu_long_opt(w, "--classdata", 8) == Some(None)
                                    || matches!(w.as_str(), "--pid" | "--pgid" | "--uid")
                            }
                            "stdbuf" => {
                                match_gnu_long_opt(w, "--input", 4) == Some(None)
                                    || match_gnu_long_opt(w, "--output", 5) == Some(None)
                                    || match_gnu_long_opt(w, "--error", 4) == Some(None)
                            }
                            "xargs" => {
                                match_gnu_long_opt(w, "--max-args", 6) == Some(None)
                                    || match_gnu_long_opt(w, "--max-procs", 6) == Some(None)
                                    || match_gnu_long_opt(w, "--max-chars", 6) == Some(None)
                                    || match_gnu_long_opt(w, "--delimiter", 5) == Some(None)
                                    || match_gnu_long_opt(w, "--arg-file", 5) == Some(None)
                                    || (opt_takes_val
                                        && (match_gnu_long_opt(w, "--replace", 5) == Some(None)
                                            || match_gnu_long_opt(w, "--eof", 4) == Some(None)
                                            || match_gnu_long_opt(w, "--max-lines", 6)
                                                == Some(None)))
                            }
                            "chrt" => {
                                match_gnu_long_opt(w, "--sched-runtime", 9) == Some(None)
                                    || match_gnu_long_opt(w, "--sched-period", 9) == Some(None)
                                    || match_gnu_long_opt(w, "--sched-deadline", 9) == Some(None)
                            }
                            "taskset" => match_gnu_long_opt(w, "--cpu-list", 5) == Some(None),
                            "arch" => w == "-arch",
                            _ => false,
                        };
                        let is_known_bool = (!w.starts_with("--")
                            && w[1..].chars().all(|c| known_short_bools.contains(c)))
                            || matches!(
                                w.as_str(),
                                "--all-tasks"
                                    | "--pid"
                                    | "--null"
                                    | "--fork"
                                    | "--wait"
                                    | "--ctty"
                                    | "--ignore"
                                    | "--rr"
                                    | "--fifo"
                                    | "--batch"
                                    | "--idle"
                                    | "--other"
                            );
                        let is_short_attached_val = !w.starts_with("--")
                            && w[1..].chars().any(|c| short_vals.contains(c))
                            && !short_cluster_takes_next(w, short_vals);
                        if long_takes
                            || short_cluster_takes_next(w, short_vals)
                            || (opt_takes_val && short_cluster_takes_next(w, opt_short_vals))
                            || (opt_takes_val
                                && !is_known_bool
                                && !is_short_attached_val
                                && !w.contains('='))
                        {
                            idx += 2;
                        } else {
                            idx += 1;
                        }
                    } else {
                        break;
                    }
                }
                if f == "chrt" || (f == "taskset" && !taskset_has_cpu_list) {
                    idx = (idx + 1).min(cur.len());
                }
            }
            "chroot" => {
                let saved = idx;
                idx += 1;
                while let Some(w) = cur.get(idx) {
                    budget.charge(1)?;
                    if w == "--" {
                        idx += 1;
                        break;
                    }
                    let long_takes = match_gnu_long_opt(w, "--userspec", 6) == Some(None)
                        || match_gnu_long_opt(w, "--groups", 5) == Some(None);
                    if long_takes
                        || short_cluster_takes_next(w, "Ggu")
                        || (opt_takes_val
                            && w.starts_with('-')
                            && w.len() > 1
                            && !w.contains('=')
                            && w != "--skip-chdir")
                    {
                        idx += 2;
                    } else if w.starts_with('-') && w.len() > 1 {
                        idx += 1;
                    } else {
                        break;
                    }
                }
                if idx + 1 < cur.len() {
                    idx += 1;
                } else {
                    // `chroot NEWROOT` with no command starts a shell on stdin: keep it intact.
                    idx = saved;
                    break;
                }
            }
            "flock" | "script" => {
                let saved = idx;
                let short_vals = if f == "flock" { "wE" } else { "TIOBmo" };
                let known_bools = if f == "flock" { "sexnuoF" } else { "aefkqr" };
                let mut j = idx + 1;
                let mut file_seen = false;
                let mut has_c = false;
                while let Some(w) = cur.get(j) {
                    budget.charge(1)?;
                    if w == "--" {
                        j += 1;
                        if !file_seen && j < cur.len() {
                            j += 1;
                        }
                        break;
                    }
                    if w.starts_with('-') && w.len() > 1 {
                        if match_gnu_long_opt(w, "--command", 3).is_some()
                            || (!w.starts_with("--") && w[1..].contains('c'))
                        {
                            has_c = true;
                            break;
                        }
                        let long_takes = if f == "flock" {
                            match_gnu_long_opt(w, "--timeout", 6) == Some(None)
                                || match_gnu_long_opt(w, "--wait", 5) == Some(None)
                                || match_gnu_long_opt(w, "--conflict-exit-code", 6) == Some(None)
                        } else {
                            match_gnu_long_opt(w, "--log-io", 6) == Some(None)
                                || match_gnu_long_opt(w, "--log-in", 6) == Some(None)
                                || match_gnu_long_opt(w, "--log-out", 7) == Some(None)
                                || match_gnu_long_opt(w, "--output-limit", 5) == Some(None)
                                || (opt_takes_val
                                    && match_gnu_long_opt(w, "--timing", 5) == Some(None))
                        };
                        let script_opt_t = f == "script" && short_cluster_takes_next(w, "t");
                        let is_known_bool = (!w.starts_with("--")
                            && w[1..].chars().all(|c| known_bools.contains(c)))
                            || matches!(
                                w.as_str(),
                                "--quiet"
                                    | "--append"
                                    | "--flush"
                                    | "--return"
                                    | "--shared"
                                    | "--exclusive"
                                    | "--nonblock"
                                    | "--unlock"
                                    | "--no-fork"
                                    | "--verbose"
                            );
                        let is_attached_val = !w.starts_with("--")
                            && w[1..]
                                .chars()
                                .any(|c| short_vals.contains(c) || (f == "script" && c == 't'))
                            && !short_cluster_takes_next(w, short_vals)
                            && !script_opt_t;
                        if long_takes
                            || short_cluster_takes_next(w, short_vals)
                            || (opt_takes_val && script_opt_t)
                            || (opt_takes_val
                                && !is_known_bool
                                && !is_attached_val
                                && !w.contains('='))
                        {
                            j += 2;
                        } else {
                            j += 1;
                        }
                    } else if !file_seen {
                        file_seen = true;
                        j += 1;
                    } else {
                        break;
                    }
                }
                if has_c || j >= cur.len() {
                    // Keep `script`/`flock -c ...` or bare `script FILE` intact for code/stdin checks.
                    idx = saved;
                    break;
                }
                idx = j;
            }
            _ => break,
        }
    }
    let slice = &cur[idx.min(cur.len())..];
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
    Bg,
    Pipe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BeCompound {
    Brace,
    Paren,
    Cond,
    Loop,
    Func,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BeUnitMeta {
    join: BeJoin,
    /// Join written immediately after a `)` group (possibly after its redirects)
    /// that closes this unit's subshell (`( … ) && x`, `( … ) | x`, `( … ) || x`).
    close_join: Option<BeJoin>,
    /// Open compound commands surrounding this unit, outermost first, each with
    /// a scan-unique id (matching `dev` `shell.rs` + `words.rs`).
    compound: Vec<(BeCompound, u32)>,
    /// Byte slice of a `<<<WORD` here-string attached to this unit (if any).
    herestring: Option<(usize, usize)>,
    /// Byte slice of a `<<EOF` here-doc body attached to this unit (if any).
    heredoc_body: Option<(usize, usize)>,
}

impl BeUnitMeta {
    const fn seq() -> Self {
        Self {
            join: BeJoin::Seq,
            close_join: None,
            compound: Vec::new(),
            herestring: None,
            heredoc_body: None,
        }
    }

    const fn and() -> Self {
        Self {
            join: BeJoin::And,
            close_join: None,
            compound: Vec::new(),
            herestring: None,
            heredoc_body: None,
        }
    }
}

fn is_posix_name_be(s: &str) -> bool {
    let mut bytes = s.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Whether the argv words read so far leave the next unquoted word in command
/// position where a reserved word opens or closes a compound command
/// (matching `dev` `shell.rs::opens_body`).
fn opens_body_be(words: &[&str]) -> bool {
    let mut k = 0;
    while k < words.len() {
        match words[k] {
            "then" | "do" | "else" | "elif" | "!" | "{" | "if" | "while" | "until" => k += 1,
            "time" => {
                k += 1;
                if words.get(k).is_some_and(|w| *w == "-p") {
                    k += 1;
                }
            }
            "function" => {
                if k + 1 >= words.len() {
                    return false;
                }
                k += 2;
            }
            "coproc" => {
                k += 1;
                if k < words.len()
                    && is_posix_name_be(words[k])
                    && words.get(k + 1).is_none_or(|w| *w == "{")
                {
                    k += 1;
                }
            }
            _ => return false,
        }
    }
    true
}

fn is_known_env_noarg_opt(w: &str) -> bool {
    matches!(
        w,
        "--ignore-environment"
            | "--null"
            | "--debug"
            | "--help"
            | "--version"
            | "--ign"
            | "--ignore"
    ) || (w.starts_with('-')
        && !w.starts_with("--")
        && w.len() > 1
        && w[1..].bytes().all(|b| matches!(b, b'i' | b'0' | b'v')))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanPending {
    Redirect,
    Heredoc(bool),
    HereString,
}

/// Linear scan of a command line matching P1 `lex_one_text`'s unit boundaries
/// 1-to-1 and recording each unit's trailing separator, surrounding compound
/// stack (`Brace`/`Paren`/`Cond`/`Loop`/`Func`), `close_join` after `)` +
/// redirects, and attached `<<<` here-string range.
fn scan_top_level_meta(
    text: &str,
    budget: &mut ComputeBudget,
) -> Result<Vec<BeUnitMeta>, Exhausted> {
    let bytes = text.as_bytes();
    budget.charge(bytes.len().max(1) as u64)?;
    let mut out: Vec<BeUnitMeta> = Vec::new();
    let mut i = 0usize;
    let mut has_content = false;
    let mut has_assign = false;
    let mut cur_words: Vec<&str> = Vec::new();
    let mut groups: Vec<(BeCompound, u32)> = Vec::new();
    let mut next_group_id: u32 = 1;
    let mut fn_def = false;
    let mut after_paren = false;
    let mut pending: Option<ScanPending> = None;
    let mut heredocs: Vec<(String, bool)> = Vec::new();
    let mut cur_herestring: Option<(usize, usize)> = None;

    let flush_meta = |out: &mut Vec<BeUnitMeta>,
                      has_content: &mut bool,
                      has_assign: &mut bool,
                      cur_words: &mut Vec<&str>,
                      cur_herestring: &mut Option<(usize, usize)>,
                      groups: &[(BeCompound, u32)],
                      join: BeJoin,
                      budget: &mut ComputeBudget|
     -> Result<(), Exhausted> {
        if *has_content {
            budget
                .charge_alloc(std::mem::size_of::<BeUnitMeta>() + std::mem::size_of_val(groups))?;
            out.push(BeUnitMeta {
                join,
                close_join: None,
                compound: groups.to_vec(),
                herestring: cur_herestring.take(),
                heredoc_body: None,
            });
            *has_content = false;
            *has_assign = false;
            cur_words.clear();
        } else {
            *cur_herestring = None;
        }
        Ok(())
    };

    let group_tail =
        |has_content: bool, has_assign: bool, cur_words: &[&str], after_paren: bool| {
            !has_content || (after_paren && !has_assign && cur_words.is_empty())
        };

    let scan_redirect_op = |bytes: &[u8], i: usize, pending: &mut Option<ScanPending>| -> usize {
        let b = bytes[i];
        let b1 = bytes.get(i + 1).copied();
        let b2 = bytes.get(i + 2).copied();
        if b == b'<' && b1 == Some(b'<') && b2 == Some(b'<') {
            *pending = Some(ScanPending::HereString);
            i + 3
        } else if b == b'<' && b1 == Some(b'<') {
            if b2 == Some(b'-') {
                *pending = Some(ScanPending::Heredoc(true));
                i + 3
            } else {
                *pending = Some(ScanPending::Heredoc(false));
                i + 2
            }
        } else if matches!(
            (b, b1),
            (b'>', Some(b'>' | b'&' | b'|')) | (b'<', Some(b'&' | b'>'))
        ) {
            *pending = Some(ScanPending::Redirect);
            i + 2
        } else {
            *pending = Some(ScanPending::Redirect);
            i + 1
        }
    };

    while i < bytes.len() {
        budget.charge(1)?;
        match bytes[i] {
            b' ' | b'\t' => {
                i += 1;
            }
            b'\\' if bytes.get(i + 1) == Some(&b'\n') => {
                i += 2;
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'\n' => {
                i += 1;
                flush_meta(
                    &mut out,
                    &mut has_content,
                    &mut has_assign,
                    &mut cur_words,
                    &mut cur_herestring,
                    &groups,
                    BeJoin::Seq,
                    budget,
                )?;
                after_paren = false;
                pending = None;
                // Skip heredoc bodies matching P1 `lex_one_text`, recording the
                // body byte range on the owning unit.
                let owner_idx = out.len().checked_sub(1);
                for (delim, strip_tabs) in std::mem::take(&mut heredocs) {
                    let body_start = i;
                    let mut body_end = i;
                    while i < bytes.len() {
                        let line_start = i;
                        while i < bytes.len() && bytes[i] != b'\n' {
                            i += 1;
                        }
                        let line = &text[line_start..i];
                        if i < bytes.len() && bytes[i] == b'\n' {
                            i += 1;
                        }
                        let trimmed = if strip_tabs {
                            line.trim_start_matches('\t')
                        } else {
                            line
                        };
                        if trimmed == delim {
                            break;
                        }
                        body_end = i;
                    }
                    if body_end > body_start
                        && let Some(oi) = owner_idx
                        && let Some(m) = out.get_mut(oi)
                        && m.heredoc_body.is_none()
                    {
                        m.heredoc_body = Some((body_start, body_end));
                    }
                }
            }
            b';' => {
                i += if bytes.get(i + 1) == Some(&b';') {
                    2
                } else {
                    1
                };
                flush_meta(
                    &mut out,
                    &mut has_content,
                    &mut has_assign,
                    &mut cur_words,
                    &mut cur_herestring,
                    &groups,
                    BeJoin::Seq,
                    budget,
                )?;
                after_paren = false;
                pending = None;
            }
            b'&' if bytes.get(i + 1) == Some(&b'&') => {
                i += 2;
                if group_tail(has_content, has_assign, &cur_words, after_paren)
                    && let Some(last) = out.last_mut()
                {
                    last.close_join = Some(BeJoin::And);
                }
                flush_meta(
                    &mut out,
                    &mut has_content,
                    &mut has_assign,
                    &mut cur_words,
                    &mut cur_herestring,
                    &groups,
                    BeJoin::And,
                    budget,
                )?;
                after_paren = false;
                pending = None;
            }
            b'&' if bytes.get(i + 1) == Some(&b'>') => {
                i += if bytes.get(i + 2) == Some(&b'>') {
                    3
                } else {
                    2
                };
                pending = Some(ScanPending::Redirect);
            }
            b'&' => {
                i += 1;
                flush_meta(
                    &mut out,
                    &mut has_content,
                    &mut has_assign,
                    &mut cur_words,
                    &mut cur_herestring,
                    &groups,
                    BeJoin::Bg,
                    budget,
                )?;
                after_paren = false;
                pending = None;
            }
            b'|' => {
                let is_pipe = bytes.get(i + 1) != Some(&b'|');
                i += if is_pipe && bytes.get(i + 1) == Some(&b'&') {
                    2
                } else if is_pipe {
                    1
                } else {
                    2
                };
                if group_tail(has_content, has_assign, &cur_words, after_paren)
                    && let Some(last) = out.last_mut()
                {
                    if is_pipe && after_paren {
                        last.close_join = Some(BeJoin::And);
                    } else if !is_pipe {
                        last.close_join = Some(BeJoin::Or);
                    }
                }
                flush_meta(
                    &mut out,
                    &mut has_content,
                    &mut has_assign,
                    &mut cur_words,
                    &mut cur_herestring,
                    &groups,
                    if is_pipe { BeJoin::Pipe } else { BeJoin::Or },
                    budget,
                )?;
                after_paren = false;
                pending = None;
            }
            b'(' => {
                let mut k = i + 1;
                while matches!(bytes.get(k), Some(b' ' | b'\t')) {
                    k += 1;
                }
                if bytes.get(k) == Some(&b')') && !cur_words.is_empty() {
                    // `f()` function definition: matches P1 `lex_one_text` and
                    // `dev` `shell.rs` (`join = And` so it keeps an outer `&&` chain).
                    flush_meta(
                        &mut out,
                        &mut has_content,
                        &mut has_assign,
                        &mut cur_words,
                        &mut cur_herestring,
                        &groups,
                        BeJoin::And,
                        budget,
                    )?;
                    fn_def = true;
                    after_paren = false;
                    pending = None;
                    i = k + 1;
                } else {
                    flush_meta(
                        &mut out,
                        &mut has_content,
                        &mut has_assign,
                        &mut cur_words,
                        &mut cur_herestring,
                        &groups,
                        BeJoin::Seq,
                        budget,
                    )?;
                    let kind = if std::mem::take(&mut fn_def) {
                        BeCompound::Func
                    } else {
                        BeCompound::Paren
                    };
                    let id = next_group_id;
                    next_group_id = next_group_id.wrapping_add(1).max(1);
                    groups.push((kind, id));
                    after_paren = false;
                    pending = None;
                    i += 1;
                }
            }
            b')' => {
                flush_meta(
                    &mut out,
                    &mut has_content,
                    &mut has_assign,
                    &mut cur_words,
                    &mut cur_herestring,
                    &groups,
                    BeJoin::Seq,
                    budget,
                )?;
                if groups
                    .last()
                    .is_some_and(|g| matches!(g.0, BeCompound::Paren | BeCompound::Func))
                {
                    groups.pop();
                    after_paren = true;
                } else {
                    // `case x in a)`: pattern terminator, not a subshell group.
                    after_paren = false;
                }
                pending = None;
                i += 1;
            }
            b'<' | b'>' if bytes.get(i + 1) != Some(&b'(') => {
                i = scan_redirect_op(bytes, i, &mut pending);
            }
            _ => {
                // Scan one shell word linearly (matching P1 `lex_one_text`).
                let word_start = i;
                let mut quoted_prefix_empty = true;
                let mut all_unquoted_digits = true;
                let mut seen_eq_pos: Option<usize> = None;
                while i < bytes.len() {
                    let c = bytes[i];
                    match c {
                        b' ' | b'\t' | b'\n' | b';' | b'&' | b'|' | b'(' | b')' => break,
                        b'<' | b'>' if bytes.get(i + 1) != Some(&b'(') => break,
                        b'\\' => {
                            quoted_prefix_empty = false;
                            all_unquoted_digits = false;
                            i += if i + 1 < bytes.len() { 2 } else { 1 };
                        }
                        b'\'' => {
                            quoted_prefix_empty = false;
                            all_unquoted_digits = false;
                            i += 1;
                            while i < bytes.len() {
                                if bytes[i] == b'\'' {
                                    i += 1;
                                    break;
                                }
                                i += 1;
                            }
                        }
                        b'"' => {
                            quoted_prefix_empty = false;
                            all_unquoted_digits = false;
                            i += 1;
                            let mut sub_depth = 0usize;
                            while i < bytes.len() {
                                match bytes[i] {
                                    b'\\' if i + 1 < bytes.len() => i += 2,
                                    b'"' if sub_depth == 0 => {
                                        i += 1;
                                        break;
                                    }
                                    b'$' if bytes.get(i + 1) == Some(&b'(') => {
                                        sub_depth += 1;
                                        i += 2;
                                    }
                                    b'(' if sub_depth > 0 => {
                                        sub_depth += 1;
                                        i += 1;
                                    }
                                    b')' if sub_depth > 0 => {
                                        sub_depth -= 1;
                                        i += 1;
                                    }
                                    _ => i += 1,
                                }
                            }
                        }
                        b'$' if bytes.get(i + 1) == Some(&b'\'') => {
                            quoted_prefix_empty = false;
                            all_unquoted_digits = false;
                            i += 2;
                            while i < bytes.len() {
                                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                                    i += 2;
                                } else if bytes[i] == b'\'' {
                                    i += 1;
                                    break;
                                } else {
                                    i += 1;
                                }
                            }
                        }
                        b'$' if bytes.get(i + 1) == Some(&b'(') => {
                            all_unquoted_digits = false;
                            i += 2;
                            i = skip_balanced_paren_be(bytes, i);
                        }
                        b'<' | b'>' if bytes.get(i + 1) == Some(&b'(') => {
                            all_unquoted_digits = false;
                            i += 2;
                            i = skip_balanced_paren_be(bytes, i);
                        }
                        b'$' if bytes.get(i + 1) == Some(&b'{') => {
                            all_unquoted_digits = false;
                            i += 2;
                            i = skip_balanced_brace_be(bytes, i);
                        }
                        b'`' => {
                            all_unquoted_digits = false;
                            i += 1;
                            while i < bytes.len() {
                                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                                    i += 2;
                                } else if bytes[i] == b'`' {
                                    i += 1;
                                    break;
                                } else {
                                    i += 1;
                                }
                            }
                        }
                        b'=' => {
                            if seen_eq_pos.is_none() && quoted_prefix_empty {
                                seen_eq_pos = Some(i - word_start);
                            }
                            all_unquoted_digits = false;
                            i += 1;
                        }
                        _ => {
                            if !c.is_ascii_digit() {
                                all_unquoted_digits = false;
                            }
                            i += 1;
                        }
                    }
                }
                if i == word_start {
                    i += 1;
                    continue;
                }
                // Unquoted digits directly before `<` or `>` are the redirect fd.
                if pending.is_none()
                    && quoted_prefix_empty
                    && all_unquoted_digits
                    && matches!(bytes.get(i), Some(b'<' | b'>'))
                    && bytes.get(i + 1) != Some(&b'(')
                {
                    i = scan_redirect_op(bytes, i, &mut pending);
                    continue;
                }
                let raw_word = &text[word_start..i];
                match pending.take() {
                    Some(ScanPending::Redirect) => {
                        has_content = true;
                        // Keep `after_paren` intact so `( … ) > /dev/null && x`
                        // still attributes `&&` to the group's `close_join`.
                    }
                    Some(ScanPending::HereString) => {
                        cur_herestring = Some((word_start, i));
                    }
                    Some(ScanPending::Heredoc(strip_tabs)) => {
                        let delim = unquote_heredoc_delim_be(raw_word, budget)?;
                        budget.charge_alloc(std::mem::size_of::<(String, bool)>())?;
                        heredocs.push((delim, strip_tabs));
                    }
                    None => {
                        let is_assign = cur_words.is_empty()
                            && seen_eq_pos.is_some_and(|eq| {
                                let name = raw_word[..eq].trim_end_matches('+');
                                is_posix_name_be(name)
                            });
                        if is_assign {
                            has_assign = true;
                            has_content = true;
                            after_paren = false;
                        } else {
                            let command_position = !has_assign && opens_body_be(&cur_words);
                            if command_position && quoted_prefix_empty {
                                let open_kind = match raw_word {
                                    "{" if std::mem::take(&mut fn_def)
                                        || cur_words.contains(&"function") =>
                                    {
                                        Some(BeCompound::Func)
                                    }
                                    "{" if cur_words.contains(&"coproc") => Some(BeCompound::Cond),
                                    "{" => Some(BeCompound::Brace),
                                    "if" | "case" => Some(BeCompound::Cond),
                                    "while" | "until" | "for" | "select" => Some(BeCompound::Loop),
                                    _ => None,
                                };
                                if let Some(kind) = open_kind {
                                    let kind = if std::mem::take(&mut fn_def) {
                                        BeCompound::Func
                                    } else {
                                        kind
                                    };
                                    let id = next_group_id;
                                    next_group_id = next_group_id.wrapping_add(1).max(1);
                                    groups.push((kind, id));
                                } else if matches!(raw_word, "}" | "fi" | "done" | "esac") {
                                    if groups.last().is_some_and(|g| g.0 != BeCompound::Paren) {
                                        groups.pop();
                                    }
                                } else {
                                    fn_def = false;
                                }
                            }
                            after_paren = false;
                            has_content = true;
                            cur_words.push(raw_word);
                        }
                    }
                }
            }
        }
    }
    flush_meta(
        &mut out,
        &mut has_content,
        &mut has_assign,
        &mut cur_words,
        &mut cur_herestring,
        &groups,
        BeJoin::Seq,
        budget,
    )?;
    Ok(out)
}

fn skip_balanced_paren_be(bytes: &[u8], mut i: usize) -> usize {
    let mut depth = 1usize;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

fn skip_balanced_brace_be(bytes: &[u8], mut i: usize) -> usize {
    let mut depth = 1usize;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'\'' => {
                i += 1;
                while i < bytes.len() && bytes[i] != b'\'' {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
            }
            b'"' => {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && i + 1 < bytes.len() {
                        i += 2;
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            }
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    i
}

fn unquote_heredoc_delim_be(raw: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    budget.charge(raw.len().max(1) as u64)?;
    budget.charge_alloc(raw.len())?;
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            '\'' | '"' => {}
            _ => out.push(c),
        }
    }
    Ok(out)
}

/// One open compound command inside a `CwdFrame` (matching `dev` `words.rs::Frame`).
struct BeCompoundFrame {
    id: u32,
    kind: BeCompound,
    entry: Vec<Vec<u8>>,
    saved_pool: Vec<Vec<u8>>,
    saved_last_join: BeJoin,
}

struct CwdFrame {
    depth: u8,
    cands: Vec<Vec<u8>>,
    pool: Vec<Vec<u8>>,
    pending: bool,
    last_join: BeJoin,
    last_close: Option<BeJoin>,
    unknown: bool,
    same_shell: bool,
    union: bool,
    frames: Vec<BeCompoundFrame>,
}

struct BeCwdTracker {
    home: Vec<u8>,
    start: Vec<u8>,
    seen: Vec<Vec<u8>>,
    stack: Vec<CwdFrame>,
    next_child_same_shell: bool,
    next_child_union: bool,
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
                pool: Vec::new(),
                pending: false,
                last_join: BeJoin::Seq,
                last_close: None,
                unknown: false,
                same_shell: false,
                union: false,
                frames: Vec::new(),
            }],
            next_child_same_shell: false,
            next_child_union: false,
        })
    }

    fn close_compound_frame(
        lv: &mut CwdFrame,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        let Some(f) = lv.frames.pop() else {
            return Ok(());
        };
        let inner = std::mem::take(&mut lv.pool);
        lv.pool = f.saved_pool;
        merge_cands_into(&mut lv.pool, inner, budget)?;
        if f.kind != BeCompound::Brace && !lv.unknown {
            merge_cands_into(&mut lv.cands, f.entry, budget)?;
        }
        lv.last_join = f.saved_last_join;
        lv.pending = false;
        Ok(())
    }

    fn flush_top_pool(&mut self, budget: &mut ComputeBudget) -> Result<(), Exhausted> {
        let Some(frame) = self.stack.last_mut() else {
            return Ok(());
        };
        if !frame.pending {
            return Ok(());
        }
        frame.pending = false;
        if frame.pool.is_empty() {
            return Ok(());
        }
        let pooled = std::mem::take(&mut frame.pool);
        if frame.unknown {
            let _ = frame;
            self.push_seen(&pooled, budget)?;
            let lost_list = self.lost_cands(budget)?;
            if let Some(frame) = self.stack.last_mut() {
                frame.cands = lost_list;
            }
        } else {
            for p in pooled {
                if frame.cands.len() >= MAX_CWD_CANDS {
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
                if !frame.cands.iter().any(|c| c == &p) {
                    frame.cands.push(p);
                }
            }
        }
        Ok(())
    }

    fn enter_unit(
        &mut self,
        u: &BestEffortUnit,
        meta: &BeUnitMeta,
        child_override: &mut Option<(Vec<Vec<u8>>, bool)>,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        while self.stack.len() > 1 && self.stack.last().is_some_and(|f| f.depth > u.depth) {
            let Some(mut done) = self.stack.pop() else {
                break;
            };
            while !done.frames.is_empty() {
                Self::close_compound_frame(&mut done, budget)?;
            }
            if done.same_shell && !self.stack.is_empty() {
                if done.unknown {
                    let lost_list = self.lost_cands(budget)?;
                    if let Some(parent) = self.stack.last_mut() {
                        parent.unknown = true;
                        parent.pool.clear();
                        parent.cands = lost_list;
                    }
                } else if let Some(parent) = self.stack.last_mut() {
                    if done.union {
                        merge_cands_into(&mut parent.cands, done.cands, budget)?;
                    } else {
                        parent.cands = done.cands;
                    }
                    if !done.pool.is_empty() && !parent.unknown {
                        merge_cands_into(&mut parent.pool, done.pool, budget)?;
                        parent.pending |= !matches!(parent.last_join, BeJoin::And | BeJoin::Pipe);
                    }
                }
            }
        }
        let top_depth = self.stack.last().map_or(0, |f| f.depth);
        if u.depth > top_depth {
            let (cands, unknown) = match child_override.take() {
                Some(ov) => ov,
                None => match self.stack.last() {
                    Some(top) => (clone_bytes_vec(&top.cands, budget)?, top.unknown),
                    None => (vec![clone_bytes(&self.start, budget)?], false),
                },
            };
            budget.charge_alloc(std::mem::size_of::<CwdFrame>())?;
            self.stack.push(CwdFrame {
                depth: u.depth,
                cands,
                pool: Vec::new(),
                pending: false,
                last_join: BeJoin::Seq,
                last_close: None,
                unknown,
                same_shell: self.next_child_same_shell,
                union: self.next_child_union,
                frames: Vec::new(),
            });
            self.next_child_same_shell = false;
            self.next_child_union = false;
        }

        // Transition compound frames (matching `dev` `words.rs:884-929`).
        let common = self.stack.last().map_or(0, |lv| {
            lv.frames
                .iter()
                .zip(&meta.compound)
                .take_while(|(f, c)| f.id == c.1)
                .count()
        });
        let need_close = self.stack.last().is_some_and(|lv| lv.frames.len() > common);
        if need_close && let Some(lv) = self.stack.last_mut() {
            while lv.frames.len() > common {
                Self::close_compound_frame(lv, budget)?;
            }
            let closer = u
                .raw_argv
                .first()
                .is_some_and(|f| matches!(f.as_str(), "}" | "fi" | "done" | "esac"));
            if !closer {
                let j = lv.last_close.unwrap_or(BeJoin::Seq);
                if j == BeJoin::And && !lv.unknown {
                    let cur = clone_bytes_vec(&lv.cands, budget)?;
                    merge_cands_into(&mut lv.pool, cur, budget)?;
                }
                lv.last_join = j;
                lv.pending = j != BeJoin::And && !lv.pool.is_empty();
            }
        }
        for &(raw_kind, id) in &meta.compound[common..] {
            if self.stack.last().is_some_and(|lv| lv.pending) {
                self.flush_top_pool(budget)?;
            }
            if let Some(lv) = self.stack.last_mut() {
                let kind = if raw_kind == BeCompound::Brace && lv.last_join == BeJoin::Or {
                    BeCompound::Cond
                } else {
                    raw_kind
                };
                let entry = clone_bytes_vec(&lv.cands, budget)?;
                let saved_pool = std::mem::take(&mut lv.pool);
                let saved_last_join = lv.last_join;
                budget.charge_alloc(std::mem::size_of::<BeCompoundFrame>())?;
                lv.frames.push(BeCompoundFrame {
                    id,
                    kind,
                    entry,
                    saved_pool,
                    saved_last_join,
                });
                lv.last_join = BeJoin::Seq;
            }
        }
        if self.stack.last().is_some_and(|lv| lv.pending) {
            self.flush_top_pool(budget)?;
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
        meta: &BeUnitMeta,
        next_is_exit: bool,
        same_shell_parent: bool,
        same_shell_payload: Option<&str>,
        folds: FoldRules,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        let in_pipe = u.pipeline_id.is_some() || u.piped;
        let last_join = self.stack.last().map_or(BeJoin::Seq, |f| f.last_join);
        let skipped_from = last_join == BeJoin::Or;
        let repeats = self.stack.last().is_some_and(|f| {
            f.frames
                .iter()
                .any(|cf| matches!(cf.kind, BeCompound::Loop | BeCompound::Func))
        });
        let in_cond_or_sub = self
            .stack
            .last()
            .is_some_and(|f| f.frames.iter().any(|cf| cf.kind != BeCompound::Brace));
        let same_shell_union = in_pipe || skipped_from || in_cond_or_sub || meta.join == BeJoin::Bg;
        self.next_child_same_shell = same_shell_parent && same_shell_payload.is_none();
        self.next_child_union = same_shell_union;

        let join_eff = if meta.join == BeJoin::Pipe {
            BeJoin::And
        } else {
            meta.join
        };

        // Apply inline `eval` / `source <<<` / piped `source` same-shell `cd`
        // effects right away so following depth-0 siblings see the updated cwd.
        if let Some(payload) = same_shell_payload {
            self.apply_same_shell_payload(payload, join_eff, same_shell_union, folds, budget)?;
            if let Some(frame) = self.stack.last_mut() {
                frame.last_join = join_eff;
                frame.last_close = meta.close_join;
                if join_eff != BeJoin::And && !frame.pool.is_empty() {
                    frame.pending = true;
                }
            }
            return Ok(());
        }

        let is_cd = primary_cmd.is_some_and(|c| matches!(c, "cd" | "pushd" | "popd" | "chdir"));
        if !is_cd {
            if let Some(frame) = self.stack.last_mut() {
                if join_eff == BeJoin::And && !frame.unknown {
                    let cur = clone_bytes_vec(&frame.cands, budget)?;
                    merge_cands_into(&mut frame.pool, cur, budget)?;
                }
                frame.last_join = join_eff;
                frame.last_close = meta.close_join;
                if join_eff != BeJoin::And && !frame.pool.is_empty() {
                    frame.pending = true;
                }
            }
            return Ok(());
        }

        let cmd = primary_cmd.unwrap();
        let eff = &u.effective_argv;
        let mut args: Vec<&str> = Vec::new();
        let mut readable =
            cmd != "pushd" && cmd != "popd" && !in_pipe && !repeats && meta.join != BeJoin::Bg;
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
            let old_pool = std::mem::take(&mut frame.pool);
            frame.unknown = true;
            frame.last_join = join_eff;
            frame.last_close = meta.close_join;
            frame.pending = false;
            let _ = frame;
            self.push_seen(&old, budget)?;
            self.push_seen(&old_pool, budget)?;
            let lost_list = self.lost_cands(budget)?;
            if let Some(frame) = self.stack.last_mut() {
                frame.cands = lost_list;
            }
            return Ok(());
        }

        if frame.unknown {
            frame.last_join = join_eff;
            frame.last_close = meta.close_join;
            let lost_list = self.lost_cands(budget)?;
            if let Some(frame) = self.stack.last_mut() {
                frame.cands = lost_list;
            }
            return Ok(());
        }

        if join_eff == BeJoin::And {
            let old = std::mem::replace(&mut frame.cands, targets);
            if skipped_from {
                merge_cands_into(&mut frame.cands, clone_bytes_vec(&old, budget)?, budget)?;
            }
            merge_cands_into(&mut frame.pool, old, budget)?;
        } else if join_eff == BeJoin::Or && next_is_exit {
            let old = std::mem::replace(&mut frame.cands, targets);
            if skipped_from {
                merge_cands_into(&mut frame.cands, old, budget)?;
            }
        } else {
            // `;`, `\n`, or unguarded `||`: keep both targets and old cwds.
            let old = std::mem::replace(&mut frame.cands, targets);
            merge_cands_into(&mut frame.cands, old, budget)?;
        }
        frame.last_join = join_eff;
        frame.last_close = meta.close_join;
        if join_eff != BeJoin::And && !frame.pool.is_empty() {
            frame.pending = true;
        }
        Ok(())
    }

    fn apply_same_shell_payload(
        &mut self,
        payload: &str,
        outer_join: BeJoin,
        union_with_parent: bool,
        folds: FoldRules,
        budget: &mut ComputeBudget,
    ) -> Result<(), Exhausted> {
        let metas = scan_top_level_meta(payload, budget)?;
        let segs = split_payload_segments(payload, budget)?;
        let entry_cands = self.current_cands(budget)?;
        let entry_unknown = self.is_unknown();
        let mut inner = BeCwdTracker::new(&self.start, &self.home, budget)?;
        inner.seen = clone_bytes_vec(&self.seen, budget)?;
        if let Some(f) = inner.stack.last_mut() {
            f.cands = clone_bytes_vec(&entry_cands, budget)?;
            f.unknown = entry_unknown;
        }
        let default_seq = BeUnitMeta::seq();
        let mut no_override = None;
        for (idx, seg) in segs.iter().enumerate() {
            let words = raw_words_budgeted(seg, budget)?;
            let eff = unwrap_simple_be(&words, folds, budget)?;
            let meta = metas.get(idx).unwrap_or(&default_seq);
            let dummy = BestEffortUnit {
                index: idx,
                raw_argv: words,
                effective_argv: eff,
                alt_argv: Vec::new(),
                assignments: Vec::new(),
                redirects: Vec::new(),
                pipeline_id: None,
                piped: false,
                depth: 0,
                derived: true,
            };
            inner.enter_unit(&dummy, meta, &mut no_override, budget)?;
            let primary = match dummy.effective_argv.first() {
                Some(f) => Some(fold_cmd_cr19(f, folds, budget)?),
                None => None,
            };
            let next_is_exit = segs
                .get(idx + 1)
                .is_some_and(|ns| ns.trim() == "exit" || ns.trim().starts_with("exit "));
            inner.after_unit(
                &dummy,
                primary.as_deref(),
                meta,
                next_is_exit,
                false,
                None,
                folds,
                budget,
            )?;
        }
        self.push_seen(&inner.seen, budget)?;
        let Some(mut done) = inner.stack.pop() else {
            return Ok(());
        };
        while !done.frames.is_empty() {
            Self::close_compound_frame(&mut done, budget)?;
        }
        let Some(parent) = self.stack.last_mut() else {
            return Ok(());
        };
        if done.unknown {
            parent.unknown = true;
            parent.pool.clear();
            let _ = parent;
            let lost_list = self.lost_cands(budget)?;
            if let Some(parent) = self.stack.last_mut() {
                parent.cands = lost_list;
            }
        } else {
            if union_with_parent {
                merge_cands_into(&mut parent.cands, done.cands, budget)?;
            } else {
                parent.cands = done.cands;
            }
            if !done.pool.is_empty() && !parent.unknown {
                merge_cands_into(&mut parent.pool, done.pool, budget)?;
                parent.pending |= outer_join != BeJoin::And;
            }
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

fn merge_cands_into(
    dst: &mut Vec<Vec<u8>>,
    src: Vec<Vec<u8>>,
    budget: &mut ComputeBudget,
) -> Result<(), Exhausted> {
    for p in src {
        if dst.len() >= MAX_CWD_CANDS {
            break;
        }
        budget.charge(p.len().max(1) as u64)?;
        if !dst.iter().any(|e| e == &p) {
            dst.push(p);
        }
    }
    Ok(())
}

fn split_payload_segments(
    text: &str,
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let bytes = text.as_bytes();
    budget.charge(bytes.len().max(1) as u64)?;
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut in_sq = false;
    let mut in_dq = false;
    let mut paren = 0usize;
    let mut tick = false;
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
        if tick {
            if b == b'`' {
                tick = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'\'' => {
                in_sq = true;
                i += 1;
            }
            b'"' => {
                in_dq = true;
                i += 1;
            }
            b'`' => {
                tick = true;
                i += 1;
            }
            b'$' | b'<' | b'>' if bytes.get(i + 1) == Some(&b'(') => {
                paren += 1;
                i += 2;
            }
            b'(' if paren > 0 => {
                paren += 1;
                i += 1;
            }
            b')' if paren > 0 => {
                paren -= 1;
                i += 1;
            }
            b';' | b'\n' | b'|' | b'&' if paren == 0 => {
                let seg = text[start..i].trim();
                if !seg.is_empty() {
                    budget.charge_alloc(std::mem::size_of::<String>())?;
                    out.push(clone_str_budgeted(seg, budget)?);
                }
                i += if matches!(
                    (b, bytes.get(i + 1)),
                    (b'&', Some(b'&')) | (b'|', Some(b'|')) | (b';', Some(b';'))
                ) {
                    2
                } else {
                    1
                };
                start = i;
            }
            _ => i += 1,
        }
    }
    let tail = text[start..].trim();
    if !tail.is_empty() {
        budget.charge_alloc(std::mem::size_of::<String>())?;
        out.push(clone_str_budgeted(tail, budget)?);
    }
    Ok(out)
}

fn unquote_herestring_be(raw: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    budget.charge(raw.len().max(1) as u64)?;
    budget.charge_alloc(raw.len())?;
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    let mut q: Option<char> = None;
    while let Some(c) = chars.next() {
        match q {
            Some(qc) => {
                if qc == '"' && c == '\\' {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                } else if c == qc {
                    q = None;
                } else {
                    out.push(c);
                }
            }
            None => match c {
                '\'' | '"' => q = Some(c),
                '\\' => {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                }
                _ => out.push(c),
            },
        }
    }
    Ok(out)
}

fn extract_piped_echo_for_source(
    earlier: &[BestEffortUnit],
    pid: PipelineId,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<String>, Exhausted> {
    for prev in earlier.iter().rev() {
        if prev.pipeline_id != Some(pid) {
            break;
        }
        let Some(first) = prev.effective_argv.first() else {
            continue;
        };
        let f = fold_cmd_cr19(first, folds, budget)?;
        if f == "echo" {
            let mut j = 1usize;
            while j < prev.effective_argv.len() {
                let w = prev.effective_argv[j].as_str();
                if w.starts_with('-')
                    && w.len() > 1
                    && w[1..].bytes().all(|b| matches!(b, b'n' | b'e' | b'E'))
                {
                    j += 1;
                } else {
                    break;
                }
            }
            if j < prev.effective_argv.len() {
                let len: usize = prev.effective_argv[j..].iter().map(|s| s.len() + 1).sum();
                budget.charge(len as u64)?;
                budget.charge_alloc(len)?;
                return Ok(Some(prev.effective_argv[j..].join(" ")));
            }
        } else if f == "printf" {
            let j = if prev.effective_argv.get(1).map(String::as_str) == Some("--") {
                2
            } else {
                1
            };
            if let Some(fmt) = prev.effective_argv.get(j) {
                return Ok(Some(clone_str_budgeted(fmt, budget)?));
            }
        }
    }
    Ok(None)
}

/// Candidate working directories, and whether the real one may be unknown.
type CwdCands = (Vec<Vec<u8>>, bool);

/// Extracts shifted per-unit cwd candidates when `raw_argv` starts with `env`
/// (or `$(which env)`) and carries `-C DIR` / `-CDIR` / `--chdir` / `--ch*`.
fn extract_env_chdir_cands(
    raw_argv: &[String],
    base_cands: &[Vec<u8>],
    base_unknown: bool,
    tracker: &BeCwdTracker,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<CwdCands>, Exhausted> {
    if raw_argv.is_empty() {
        return Ok(None);
    }
    // Scan `raw_argv` and any inner `env` reached past outer wrappers
    // (e.g. `$(which timeout) 5 $(which env) -C ..`, `env -i $(which env) -C ..`,
    // `command -Z X $(which env) -C ..`).
    let mut dirs: Vec<&str> = Vec::new();
    for start in 0..raw_argv.len() {
        let w = raw_argv[start].as_str();
        let mut wf = fold_cmd_cr19(w, folds, budget)?;
        if wf != "env" {
            let base_raw = w.rsplit('/').next().unwrap_or(w);
            if (base_raw.contains("$(") || base_raw.contains('`'))
                && let Some(sub) = substituted_program_simple(base_raw, folds, budget)?
            {
                wf = fold_cmd_cr19(&sub, folds, budget)?;
            }
        }
        if wf != "env" {
            continue;
        }
        // Only treat `raw_argv[start]` as an `env` wrapper when it is at index 0
        // or preceded only by wrapper words/flags/duration/assignments (never
        // after an inner command like `cat`).
        if start > 0 {
            let head_unwrapped0 = unwrap_simple_be_mode(&raw_argv[..=start], folds, false, budget)?;
            let head_unwrapped1 = unwrap_simple_be_mode(&raw_argv[..=start], folds, true, budget)?;
            let reaches_env = head_unwrapped0.is_empty()
                || head_unwrapped1.is_empty()
                || (head_unwrapped0.len() == 1 && head_unwrapped0[0] == raw_argv[start])
                || (head_unwrapped1.len() == 1 && head_unwrapped1[0] == raw_argv[start]);
            if !reaches_env {
                break;
            }
        }
        let mut j = start + 1;
        while j < raw_argv.len() {
            let w = raw_argv[j].as_str();
            budget.charge(w.len().max(1) as u64)?;
            if w == "--" {
                break;
            }
            if w == "-" {
                j += 1;
                continue;
            }
            if let Some(attached) = match_gnu_long_opt(w, "--chdir", 4) {
                if let Some(val) = attached {
                    dirs.push(val);
                    j += 1;
                } else if let Some(next) = raw_argv.get(j + 1) {
                    dirs.push(next.as_str());
                    j += 2;
                } else {
                    j += 1;
                }
                continue;
            }
            if w.starts_with('-') && !w.starts_with("--") && w.len() > 1 {
                if let Some(pos) = w[1..].find('C') {
                    let after = &w[1 + pos + 1..];
                    if !after.is_empty() {
                        dirs.push(after);
                        j += 1;
                    } else if let Some(next) = raw_argv.get(j + 1) {
                        dirs.push(next.as_str());
                        j += 2;
                    } else {
                        j += 1;
                    }
                    continue;
                }
                if short_cluster_takes_next(w, "uPaS")
                    || (!is_known_env_noarg_opt(w)
                        && raw_argv.get(j + 2).is_some_and(|n2| {
                            (n2.starts_with('-') && n2.len() > 1) || n2.contains('=')
                        }))
                {
                    j += 2;
                } else {
                    j += 1;
                }
                continue;
            }
            if w.starts_with("--") {
                if matches!(w, "--unset" | "--split-string" | "--argv0")
                    || match_gnu_long_opt(w, "--split-string", 5) == Some(None)
                    || match_gnu_long_opt(w, "--unset", 4) == Some(None)
                    || (!w.contains('=')
                        && !is_known_env_noarg_opt(w)
                        && raw_argv.get(j + 2).is_some_and(|n2| {
                            (n2.starts_with('-') && n2.len() > 1) || n2.contains('=')
                        }))
                {
                    j += 2;
                } else {
                    j += 1;
                }
                continue;
            }
            if w.contains('=') {
                j += 1;
                continue;
            }
            break;
        }
    }
    if dirs.is_empty() {
        return Ok(None);
    }
    let mut cur = clone_bytes_vec(base_cands, budget)?;
    let mut unk = base_unknown;
    for d in dirs {
        match tracker.resolve_cd_arg(d, &cur, folds, budget)? {
            Some(next) => merge_cands_into(&mut cur, next, budget)?,
            None => {
                unk = true;
                cur = tracker.lost_cands(budget)?;
                break;
            }
        }
    }
    Ok(Some((cur, unk)))
}

fn extract_proc_sub_echo_payloads(
    u: &BestEffortUnit,
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    let mut out = Vec::new();
    for w in u.raw_argv.iter().chain(u.redirects.iter().map(|(_, t)| t)) {
        budget.charge(w.len().max(1) as u64)?;
        if let Some(inner) = w.strip_prefix("<(").and_then(|s| s.strip_suffix(')')) {
            let words = raw_words_budgeted(inner, budget)?;
            if let Some(first) = words.first() {
                let f = fold_cmd_cr19(first, folds, budget)?;
                if f == "echo" && words.len() > 1 {
                    let mut k = 1;
                    while k < words.len()
                        && words[k].starts_with('-')
                        && words[k].len() > 1
                        && words[k][1..]
                            .bytes()
                            .all(|b| matches!(b, b'n' | b'e' | b'E'))
                    {
                        k += 1;
                    }
                    if k < words.len() {
                        let s = words[k..].join(" ");
                        budget.charge_alloc(s.len() + std::mem::size_of::<String>())?;
                        out.push(s);
                    }
                } else if f == "printf" && words.len() > 1 {
                    let k = if words.get(1).map(String::as_str) == Some("--") {
                        2
                    } else {
                        1
                    };
                    if let Some(s) = words.get(k) {
                        budget.charge_alloc(std::mem::size_of::<String>())?;
                        out.push(clone_str_budgeted(s, budget)?);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Derives cluster-aware wrapper readings and `script`/`flock -c` inline
/// payloads directly from `raw_argv` (complementing P1's `effective_argv`).
fn extra_readings_from_raw(
    raw_argv: &[String],
    effective_argv: &[String],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<(Vec<Vec<String>>, Vec<String>), Exhausted> {
    let mut readings = Vec::new();
    let mut payloads = Vec::new();
    if raw_argv.is_empty() {
        return Ok((readings, payloads));
    }
    let unwrapped0 = unwrap_simple_be_mode(raw_argv, folds, false, budget)?;
    let unwrapped1 = unwrap_simple_be_mode(raw_argv, folds, true, budget)?;
    for unwrapped in [&unwrapped0, &unwrapped1] {
        let is_new = !unwrapped.is_empty()
            && unwrapped.as_slice() != raw_argv
            && unwrapped.as_slice() != effective_argv
            && !readings.iter().any(|r: &Vec<String>| r == unwrapped);
        if is_new {
            readings.push(unwrapped.clone());
            // If our cluster-aware unwrapper exposed a nested shell (`sh -c ...`)
            // that P1's lexer couldn't unwrap (e.g. `exec -ca foo sh -c 'rm -rf ~'`,
            // `flock /tmp/l -s sh -c 'rm -rf ~'`), extract its `-c` payload too.
            if let Some(first) = unwrapped.first() {
                let f = fold_cmd_cr19(first, folds, budget)?;
                if is_shell_folded(&f, budget)?
                    && let Some(code) = extract_shell_c_code(&unwrapped[1..], budget)?
                    && !payloads.iter().any(|p| p == code)
                {
                    budget.charge_alloc(std::mem::size_of::<String>())?;
                    payloads.push(clone_str_budgeted(code, budget)?);
                }
            }
        }
    }
    // Check raw and unwrapped heads for `script`/`flock` `-c`/`--comm` and `env` `-S`/`--spl*`.
    for cand in [raw_argv, unwrapped0.as_slice(), unwrapped1.as_slice()] {
        if let Some(code) = extract_script_flock_c_code(cand, folds, budget)?
            && !payloads.iter().any(|p| p == &code)
        {
            budget.charge_alloc(std::mem::size_of::<String>())?;
            payloads.push(code);
        }
        if let Some(code) = extract_env_split_string_be(cand, folds, budget)?
            && !payloads.iter().any(|p| p == &code)
        {
            budget.charge_alloc(std::mem::size_of::<String>())?;
            payloads.push(code);
        }
    }
    Ok((readings, payloads))
}

fn extract_env_split_string_be(
    argv: &[String],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<String>, Exhausted> {
    let Some(first) = argv.first() else {
        return Ok(None);
    };
    let mut f = fold_cmd_cr19(first, folds, budget)?;
    if f != "env" {
        let base_raw = first.rsplit('/').next().unwrap_or(first.as_str());
        if (base_raw.contains("$(") || base_raw.contains('`'))
            && let Some(sub) = substituted_program_simple(base_raw, folds, budget)?
        {
            f = fold_cmd_cr19(&sub, folds, budget)?;
        }
    }
    if f != "env" {
        return Ok(None);
    }
    let mut j = 1usize;
    let mut saw_unknown = false;
    let join_with_tail = |val: &str,
                          tail: &[String],
                          saw_unknown: bool,
                          budget: &mut ComputeBudget|
     -> Result<String, Exhausted> {
        if saw_unknown || tail.is_empty() {
            return clone_str_budgeted(val, budget);
        }
        let len: usize = val.len() + tail.iter().map(|s| 1 + s.len()).sum::<usize>();
        budget.charge(len.max(1) as u64)?;
        budget.charge_alloc(len)?;
        let mut s = String::with_capacity(len);
        s.push_str(val);
        for t in tail {
            s.push(' ');
            s.push_str(t);
        }
        Ok(s)
    };
    while let Some(w) = argv.get(j) {
        budget.charge(w.len().max(1) as u64)?;
        if w == "--" {
            break;
        }
        if let Some(attached) = match_gnu_long_opt(w, "--split-string", 5) {
            let (val, tail_start) = match attached {
                Some(v) => (Some(v), j + 1),
                None => (argv.get(j + 1).map(String::as_str), j + 2),
            };
            if let Some(s) = val {
                let tail = argv.get(tail_start..).unwrap_or(&[]);
                return Ok(Some(join_with_tail(s, tail, saw_unknown, budget)?));
            }
            return Ok(None);
        }
        if w.starts_with('-') && !w.starts_with("--") && w.len() > 1 {
            if let Some(pos) = w[1..].find('S') {
                let after = &w[1 + pos + 1..];
                if !after.is_empty() {
                    let tail = argv.get(j + 1..).unwrap_or(&[]);
                    return Ok(Some(join_with_tail(after, tail, saw_unknown, budget)?));
                }
                if let Some(next) = argv.get(j + 1) {
                    let tail = argv.get(j + 2..).unwrap_or(&[]);
                    return Ok(Some(join_with_tail(next, tail, saw_unknown, budget)?));
                }
                return Ok(None);
            }
            if short_cluster_takes_next(w, "uCPa") {
                j += 2;
            } else if !is_known_env_noarg_opt(w)
                && argv
                    .get(j + 2)
                    .is_some_and(|n2| (n2.starts_with('-') && n2.len() > 1) || n2.contains('='))
            {
                saw_unknown = true;
                j += 2;
            } else {
                if !is_known_env_noarg_opt(w) {
                    saw_unknown = true;
                }
                j += 1;
            }
        } else if w.starts_with("--") {
            if match_gnu_long_opt(w, "--chdir", 4) == Some(None)
                || match_gnu_long_opt(w, "--unset", 4) == Some(None)
                || match_gnu_long_opt(w, "--argv0", 5) == Some(None)
            {
                j += 2;
            } else if !w.contains('=')
                && !is_known_env_noarg_opt(w)
                && argv
                    .get(j + 2)
                    .is_some_and(|n2| (n2.starts_with('-') && n2.len() > 1) || n2.contains('='))
            {
                saw_unknown = true;
                j += 2;
            } else {
                if !is_known_env_noarg_opt(w)
                    && match_gnu_long_opt(w, "--chdir", 4).is_none()
                    && match_gnu_long_opt(w, "--unset", 4).is_none()
                    && match_gnu_long_opt(w, "--argv0", 5).is_none()
                {
                    saw_unknown = true;
                }
                j += 1;
            }
        } else if w.contains('=') {
            j += 1;
        } else {
            break;
        }
    }
    Ok(None)
}

/// Checks an inline payload extracted from `script -qc` / `flock --comm` on a
/// single unit (split into simple segments and run through `check_unit_reading`).
#[allow(clippy::too_many_arguments)]
fn check_inline_payload_be(
    payload: &str,
    part: &PartId,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    unit_cands: &[Vec<u8>],
    unit_cwd_unknown: bool,
    opts: &BestEffortOptions<'_>,
    budget: &mut ComputeBudget,
    out: &mut Vec<PartResult>,
) -> Result<(), Exhausted> {
    for seg in split_payload_segments(payload, budget)? {
        let words = raw_words_budgeted(&seg, budget)?;
        let eff = unwrap_simple_be(&words, protected.folds, budget)?;
        let Some(first) = eff.first() else {
            continue;
        };
        let folded = fold_cmd_cr19(first, protected.folds, budget)?;
        let dummy = BestEffortUnit {
            index: 0,
            raw_argv: words,
            effective_argv: eff.clone(),
            alt_argv: Vec::new(),
            assignments: Vec::new(),
            redirects: Vec::new(),
            pipeline_id: None,
            piped: false,
            depth: 1,
            derived: true,
        };
        check_unit_reading(
            0,
            &dummy,
            &eff,
            &folded,
            part,
            &[],
            protected,
            shared,
            unit_cands,
            unit_cwd_unknown,
            None,
            opts,
            budget,
            out,
        )?;
    }
    Ok(())
}

fn clone_bytes(b: &[u8], budget: &mut ComputeBudget) -> Result<Vec<u8>, Exhausted> {
    budget.charge(b.len().max(1) as u64)?;
    budget.charge_alloc(b.len())?;
    Ok(b.to_vec())
}

fn clone_bytes_vec(v: &[Vec<u8>], budget: &mut ComputeBudget) -> Result<Vec<Vec<u8>>, Exhausted> {
    budget.charge(v.len().max(1) as u64)?;
    budget.charge_alloc(std::mem::size_of_val(v))?;
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
        && unit_idx < all_units.len()
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
                if !hits_stdin && !downstream.raw_argv.is_empty() {
                    let raw0 = unwrap_simple_be(&downstream.raw_argv, folds, budget)?;
                    if reads_stdin_as_code_budgeted(&raw0, folds, 0, budget)? {
                        hits_stdin = true;
                    } else {
                        let raw1 =
                            unwrap_simple_be_mode(&downstream.raw_argv, folds, true, budget)?;
                        if raw1 != raw0 && reads_stdin_as_code_budgeted(&raw1, folds, 0, budget)? {
                            hits_stdin = true;
                        }
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
    let is_recursive_search = matches!(folded, "rg" | "rgrep")
        || (matches!(folded, "grep" | "egrep" | "fgrep" | "zgrep" | "ggrep")
            && eff[1..].iter().any(|a| {
                a == "--recursive"
                    || a == "--dereference-recursive"
                    || (a.starts_with('-')
                        && !a.starts_with("--")
                        && a[1..].bytes().any(|b| matches!(b, b'r' | b'R')))
            }));
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
        let is_rg = folded == "rg";
        let has_g_glob = matches!(folded, "rg" | "ag" | "ack");
        while j < eff.len() {
            let w = eff[j].as_str();
            budget.charge(w.len().max(1) as u64)?;
            if !dd && w == "--" {
                dd = true;
                j += 1;
                continue;
            }
            if !dd && w.starts_with('-') && w != "-" {
                if matches!(w, "-e" | "--regexp")
                    || (!w.starts_with("--") && short_cluster_takes_next(w, "e"))
                {
                    has_pat_flag = true;
                    if j + 1 < eff.len() {
                        skip_flag_val_indices.push(j + 1);
                    }
                    j += 2;
                    continue;
                }
                if w.starts_with("--regexp=") || (!w.starts_with("--") && w[1..].contains('e')) {
                    has_pat_flag = true;
                    j += 1;
                    continue;
                }
                if matches!(w, "-f" | "--file")
                    || (!w.starts_with("--") && short_cluster_takes_next(w, "f"))
                {
                    // `-f FILE`: file operand IS a candidate path (never skip it).
                    has_pat_flag = true;
                    j += 2;
                    continue;
                }
                if w.starts_with("--file=") || (!w.starts_with("--") && w[1..].contains('f')) {
                    has_pat_flag = true;
                    j += 1;
                    continue;
                }
                let skips_next = matches!(
                    w,
                    "--glob"
                        | "--iglob"
                        | "--include"
                        | "--exclude"
                        | "--exclude-dir"
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
                ) || (has_g_glob && w == "-g")
                    || (is_rg && w == "-r");
                if skips_next {
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
            // Skip `-ePAT` (all search tools), `-gGLOB` (`rg`/`ag`/`ack`), and
            // `-rREPL` (`rg` only; on `grep`, `-r` is boolean `--recursive` so
            // `-rf.npmrc` is `-r -f .npmrc` and MUST still be checked!).
            let skip_attached = is_search_tool
                && (w.starts_with("-e")
                    || (matches!(folded, "rg" | "ag" | "ack") && w.starts_with("-g"))
                    || (folded == "rg" && w.starts_with("-r")));
            if skip_attached {
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
            check_candidate_word_ex(
                w,
                access,
                Some(folded),
                is_recursive_search,
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
    check_candidate_word_ex(
        word, access, folded_cmd, false, part, protected, shared, unit_cands, opts, budget, out,
    )
}

#[allow(clippy::too_many_arguments)]
fn check_candidate_word_ex(
    word: &str,
    access: AccessMode,
    folded_cmd: Option<&str>,
    is_recursive_search: bool,
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
            if is_recursive_search && !is_s8_exempt_be(&folded, shared.home_folded()) {
                for d in shared.ssh_linked_folded() {
                    if under_bytes(d, &folded).is_some() {
                        push_be_hit(
                            out,
                            part.clone(),
                            "credential-read",
                            HitMatch::ProtectedPath { path: abs },
                            budget,
                        )?;
                        return Ok(());
                    }
                }
            }
            if globby
                && let Some(id) = glob_hit_budgeted(
                    &folded,
                    access,
                    is_recursive_search,
                    protected,
                    shared,
                    protected.folds,
                    budget,
                )?
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

fn is_s8_exempt_be(folded_root: &[u8], home_folded: &[u8]) -> bool {
    if under_bytes(home_folded, folded_root).is_some() {
        return true;
    }
    let dot_config = join_folded(home_folded, b".config");
    folded_root == dot_config.as_slice()
}

fn is_ssh_dir_folded(folded: &[u8], home_folded: &[u8]) -> bool {
    under_bytes(folded, home_folded) == Some(b".ssh")
}

/// Budgeted port of `dev` `Sensitive::glob_hit` (`deny.rs:594-619`), extended
/// with protected-directory prefix matching for `CRED_TREES`, Atrium data paths,
/// and write-protected Atrium config paths (M1-13 finding 11).
fn glob_hit_budgeted(
    folded_pattern: &[u8],
    access: AccessMode,
    is_recursive_search: bool,
    protected: &ProtectedSet,
    shared: &SharedSensitive,
    folds: FoldRules,
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

    // 3. Protected-directory prefix matching: any pattern whose leading
    //    components match a protected tree (`CRED_TREES`, Atrium data dir,
    //    or write-protected Atrium config dir when `access != Read`) targets
    //    inside that tree even when the leaf filename isn't `"x"`.
    for d in shared.data_folded() {
        if let Ok(ds) = std::str::from_utf8(d)
            && matcher.matches_dir_prefix(ds, budget)?
        {
            return Ok(Some("atrium-self"));
        }
    }
    if access != AccessMode::Read {
        for raw_p in &protected.atrium_protected_paths {
            let pf = fold_path_listed(raw_p, folds, budget)?;
            if let Ok(ps) = std::str::from_utf8(&pf)
                && (matcher.is_match(ps, budget)? || matcher.matches_dir_prefix(ps, budget)?)
            {
                return Ok(Some("atrium-self"));
            }
        }
    }
    for &t in CRED_TREES {
        let tree = join_folded(shared.home_folded(), t.as_bytes());
        if let Ok(ts) = std::str::from_utf8(&tree)
            && matcher.matches_dir_prefix(ts, budget)?
        {
            return Ok(Some("credential-read"));
        }
    }
    for d in shared.ssh_linked_folded() {
        if let Ok(ds) = std::str::from_utf8(d) {
            if matcher.matches_strict_descendant(ds, budget)?
                || (access != AccessMode::Read && matcher.matches_dir_prefix(ds, budget)?)
            {
                return Ok(Some("credential-read"));
            }
            if is_recursive_search {
                // Recursive search (`rg`, `grep -r`) matching an ancestor of an
                // ssh-linked directory (outside S8-exempt roots like home itself).
                let mut cur = ds;
                while !cur.is_empty() && !is_s8_exempt_be(cur.as_bytes(), shared.home_folded()) {
                    if matcher.is_match(cur, budget)? {
                        return Ok(Some("credential-read"));
                    }
                    match cur.rsplit_once('/') {
                        Some((parent, _)) if !parent.is_empty() => cur = parent,
                        _ => break,
                    }
                }
            }
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

    /// Returns `true` when `self` matches `dir` itself or any path below `dir`
    /// (its leading `k` components match `dir`'s `k` components, respecting
    /// shell dot-matching rules).
    fn matches_dir_prefix(&self, dir: &str, budget: &mut ComputeBudget) -> Result<bool, Exhausted> {
        budget.charge(dir.len().max(1) as u64)?;
        if let Some(w) = &self.whole {
            if w.is_match(dir) {
                return Ok(true);
            }
            let sample = join_folded(dir.as_bytes(), b"x");
            return Ok(std::str::from_utf8(&sample).is_ok_and(|s| w.is_match(s)));
        }
        let mut idx = 0usize;
        for seg in dir.split('/') {
            let Some((dot, matcher)) = self.parts.get(idx) else {
                return Ok(false);
            };
            if (!*dot && seg.starts_with('.')) || !matcher.as_ref().is_some_and(|m| m.is_match(seg))
            {
                return Ok(false);
            }
            idx += 1;
        }
        Ok(idx > 0 && idx <= self.parts.len())
    }

    fn matches_strict_descendant(
        &self,
        dir: &str,
        budget: &mut ComputeBudget,
    ) -> Result<bool, Exhausted> {
        budget.charge(dir.len().max(1) as u64)?;
        if let Some(w) = &self.whole {
            let sample = join_folded(dir.as_bytes(), b"x");
            return Ok(std::str::from_utf8(&sample).is_ok_and(|s| w.is_match(s)));
        }
        let mut idx = 0usize;
        for seg in dir.split('/') {
            let Some((dot, matcher)) = self.parts.get(idx) else {
                return Ok(false);
            };
            if (!*dot && seg.starts_with('.')) || !matcher.as_ref().is_some_and(|m| m.is_match(seg))
            {
                return Ok(false);
            }
            idx += 1;
        }
        Ok(idx > 0 && idx < self.parts.len())
    }
}

// ---------------------------------------------------------------------------
// Best-effort search-glob check for `ToolSubject::Paths { glob: Some(g) }`
// ---------------------------------------------------------------------------

/// Handles brace-expanded, `$HOME`/`$VAR`, backslash-escaped, wildcard-`..`,
/// and dot/slash-class (`[.][.][/]`) search globs on `ToolSubject::Paths`
/// (budgeted port of `dev` `deny.rs::search_glob_hit` + `escape_from_cands`,
/// complementing P2's `resolve_subject_paths`, §2, §5, D-100).
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
                outcome: PartOutcome::Exhausted(ExhaustReason::ComputeBudget),
            },
            budget,
        )?;
        return Ok(());
    }

    // Fast path: plain relative filter with no braces, backslashes, `$`, `[`,
    // `~`/`/` anchor, `//`, `.`/`..` segment, or `?` dot-component.
    let has_special = glob.contains(['{', '}', '\\', '$', '['])
        || glob.contains("//")
        || glob.starts_with(['~', '/'])
        || glob
            .split('/')
            .any(|c| matches!(c, "." | "..") || may_be_dot_component(c));
    if !has_special {
        return Ok(());
    }

    let cands = expand_glob_candidates_budgeted(glob, &protected.cwd, &protected.home, budget)?;
    if cands.unbounded {
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
    let mut root_strs: Vec<String> = Vec::with_capacity(root_paths.len());
    let mut root_folded: Vec<Vec<u8>> = Vec::with_capacity(root_paths.len());
    for rp in &root_paths {
        root_folded.push(fold_path_listed(rp, protected.folds, budget)?);
        let s = String::from_utf8_lossy(rp).into_owned();
        budget.charge_alloc(s.len())?;
        root_strs.push(s);
    }

    // Escape check (D-100): determines whether any normalized variant leaves
    // every search root or overflows `MAX_SEARCH_GLOB_VARIANTS`.
    let (escaped, esc_unbounded) = escape_from_cands_budgeted(
        &root_strs,
        &root_folded,
        &protected.cwd,
        &protected.home,
        protected.folds,
        &cands,
        budget,
    )?;
    if esc_unbounded {
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

    let shared = SharedSensitive::for_protected_budgeted(protected, budget)?;
    let all_probes = shared.hard_deny_probes(budget)?;
    let all_store_probes = shared.store_probes(budget)?;

    // Filter probes to only those reachable: when no variant escapes or is
    // anchored, only probes under `root_folded` can possibly match. This avoids
    // compiling 256 matchers for clean project searches like `{1..64}{a,b,c,d}/**/*.rs`.
    let any_anchored = cands
        .variants
        .iter()
        .any(|v| v.starts_with('/') || v.starts_with('~'));
    let active_probes: Vec<(Vec<u8>, &'static str)> = if escaped || any_anchored {
        all_probes
    } else {
        all_probes
            .into_iter()
            .filter(|(p, _)| root_folded.iter().any(|rf| under_bytes(p, rf).is_some()))
            .collect()
    };
    let active_store_probes: Vec<(Vec<u8>, &'static str)> = if escaped || any_anchored {
        all_store_probes
    } else {
        all_store_probes
            .into_iter()
            .filter(|(p, _)| root_folded.iter().any(|rf| under_bytes(p, rf).is_some()))
            .collect()
    };

    if !active_probes.is_empty() || !active_store_probes.is_empty() {
        let (variants, search_unbounded) = search_variants_from_cands_budgeted(
            &root_strs,
            &protected.cwd,
            &protected.home,
            protected.folds,
            &cands,
            budget,
        )?;
        if search_unbounded {
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

        let mut matchers: Vec<GlobMatcher> = Vec::with_capacity(variants.len());
        for norm in &variants {
            budget.charge((norm.len() as u64).saturating_mul(4).max(4))?;
            budget.charge_alloc(norm.len().saturating_mul(8).max(32))?;
            if let Ok(g) = GlobBuilder::new(norm).literal_separator(false).build() {
                matchers.push(g.compile_matcher());
            }
        }

        if !matchers.is_empty() {
            let match_probe = |probe_bytes: &[u8],
                               probe_s: &str,
                               budget: &mut ComputeBudget|
             -> Result<bool, Exhausted> {
                let name = probe_s.rsplit('/').next().unwrap_or(probe_s);
                for m in &matchers {
                    budget.charge(probe_s.len().max(1) as u64)?;
                    if m.is_match(probe_s) {
                        return Ok(true);
                    }
                }
                for rf in &root_folded {
                    let Some(rel_b) = under_bytes(probe_bytes, rf) else {
                        continue;
                    };
                    let Ok(rel) = std::str::from_utf8(rel_b) else {
                        continue;
                    };
                    let name_allowed = recursive || !rel.contains('/');
                    for m in &matchers {
                        budget.charge(rel.len().max(1) as u64)?;
                        if (name_allowed && m.is_match(name))
                            || (!rel.is_empty() && m.is_match(rel))
                        {
                            return Ok(true);
                        }
                    }
                }
                Ok(false)
            };

            for (probe_bytes, hard_id) in &active_probes {
                let Ok(probe_s) = std::str::from_utf8(probe_bytes) else {
                    continue;
                };
                if match_probe(probe_bytes, probe_s, budget)? {
                    push_be_hit(
                        out,
                        part.clone(),
                        hard_id,
                        HitMatch::ProbeName {
                            probe: clone_str_budgeted(probe_s, budget)?,
                        },
                        budget,
                    )?;
                    return Ok(());
                }
            }

            for (probe_bytes, family) in &active_store_probes {
                let Ok(probe_s) = std::str::from_utf8(probe_bytes) else {
                    continue;
                };
                if match_probe(probe_bytes, probe_s, budget)? {
                    push_charged(
                        out,
                        PartResult {
                            part: part.clone(),
                            outcome: PartOutcome::StoreHit(Hit {
                                hard_deny_id: None,
                                source: HitSource::BestEffort,
                                matched: HitMatch::StoreFamily { family },
                                part: part.clone(),
                            }),
                        },
                        budget,
                    )?;
                    break;
                }
            }
        }
    }

    if cands.unknown || escaped {
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

#[derive(Default)]
struct GlobCandidatesBe {
    variants: Vec<String>,
    unknown: bool,
    unbounded: bool,
}

fn class_may_match_dot(class: &str) -> bool {
    if class.len() < 2 {
        return false;
    }
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

fn widen_pattern_dots_budgeted(s: &str, budget: &mut ComputeBudget) -> Result<String, Exhausted> {
    let bytes = s.as_bytes();
    budget.charge(bytes.len().max(1) as u64)?;
    budget.charge_alloc(bytes.len())?;
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0usize;
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
    Ok(out)
}

fn widen_pattern_slashes_budgeted(
    s: &str,
    budget: &mut ComputeBudget,
) -> Result<Option<String>, Exhausted> {
    let bytes = s.as_bytes();
    budget.charge(bytes.len().max(1) as u64)?;
    let mut out = String::new();
    let mut changed = false;
    let mut i = 0usize;
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
                    if !changed {
                        budget.charge_alloc(bytes.len())?;
                        out.reserve(bytes.len());
                        out.push_str(&s[..i]);
                        changed = true;
                    }
                    out.push('/');
                } else if changed {
                    out.push_str(class);
                }
                i = j + 1;
                continue;
            }
        }
        if changed {
            out.push(ch);
        }
        i += len;
    }
    Ok(changed.then_some(out))
}

fn expand_glob_vars_budgeted(
    s: &str,
    budget: &mut ComputeBudget,
) -> Result<Vec<String>, Exhausted> {
    if !s.contains('$') {
        return Ok(Vec::new());
    }
    let bytes = s.as_bytes();
    budget.charge(bytes.len().max(1) as u64)?;
    budget.charge_alloc(bytes.len().saturating_mul(3).saturating_add(32))?;
    let mut as_globstar = String::with_capacity(bytes.len() + 8);
    let mut as_empty = String::with_capacity(bytes.len());
    let mut first_abs: Option<String> = None;
    let mut i = 0usize;
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
    Ok(out)
}

fn expand_glob_candidates_budgeted(
    glob: &str,
    cwd: &[u8],
    home: &[u8],
    budget: &mut ComputeBudget,
) -> Result<GlobCandidatesBe, Exhausted> {
    let mut res = GlobCandidatesBe::default();
    let mut raw = vec![clone_str_budgeted(glob, budget)?];
    if glob.contains('\\') {
        res.unknown = true;
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
        raw.push(unesc);
    }
    let mut expansions = Vec::new();
    for r in &raw {
        match brace_expand_budgeted(r, budget)? {
            Some(exps) => {
                if expansions.len() + exps.len() > MAX_SEARCH_GLOB_VARIANTS {
                    res.unknown = true;
                    res.unbounded = true;
                    return Ok(res);
                }
                expansions.extend(exps);
            }
            None => {
                res.unknown = true;
                res.unbounded = true;
                return Ok(res);
            }
        }
    }
    let mut seen: HashSet<String> = HashSet::new();
    for e in expansions {
        if e.is_empty() {
            continue;
        }
        let h = expand_home_be(&e, home, cwd, budget)?;
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
            bases.extend(expand_glob_vars_budgeted(&h, budget)?);
        }
        let mut cands: Vec<String> = Vec::new();
        for b in bases {
            if !cands.contains(&b) {
                cands.push(b.clone());
            }
            if b.contains(['?', '[']) {
                let dots = widen_pattern_dots_budgeted(&b, budget)?;
                if dots != b && !cands.contains(&dots) {
                    cands.push(dots.clone());
                }
                if let Some(sl) = widen_pattern_slashes_budgeted(&b, budget)? {
                    res.unknown = true;
                    if !cands.contains(&sl) {
                        cands.push(sl.clone());
                    }
                    let sl_dots = widen_pattern_dots_budgeted(&sl, budget)?;
                    if sl_dots != sl && !cands.contains(&sl_dots) {
                        cands.push(sl_dots);
                    }
                }
                if dots != b
                    && let Some(dots_sl) = widen_pattern_slashes_budgeted(&dots, budget)?
                {
                    res.unknown = true;
                    if !cands.contains(&dots_sl) {
                        cands.push(dots_sl);
                    }
                }
            }
        }
        for cand in cands {
            if cand.is_empty() {
                continue;
            }
            if !seen.contains(&cand) {
                if res.variants.len() >= MAX_SEARCH_GLOB_VARIANTS {
                    res.unknown = true;
                    res.unbounded = true;
                    res.variants.clear();
                    return Ok(res);
                }
                budget.charge_alloc(cand.len() + 32)?;
                seen.insert(cand.clone());
                res.variants.push(cand);
            }
        }
    }
    Ok(res)
}

fn apply_dotdot_budgeted<'a>(
    mut st: Vec<&'a str>,
    abs: bool,
    out: &mut Vec<Vec<&'a str>>,
    cap: usize,
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    budget.charge(st.len().max(1) as u64)?;
    if out.len() > cap.saturating_mul(4).max(16) {
        out.sort_unstable();
        out.dedup();
        if out.len() > cap {
            return Ok(false);
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
            budget.charge_alloc(st.len() * 2 * std::mem::size_of::<&str>())?;
            out.push(st.clone());
            st.pop();
            out.push(st.clone());
            if !apply_dotdot_budgeted(st, abs, out, cap, budget)? {
                return Ok(false);
            }
        }
        Some(last) if !last.contains(['*', '?', '[']) => {
            st.pop();
            out.push(st);
        }
        Some(_) => {
            budget.charge_alloc(st.len() * std::mem::size_of::<&str>())?;
            let mut popped = st.clone();
            popped.pop();
            out.push(popped);
            st.push("..");
            out.push(st);
        }
    }
    Ok(true)
}

fn normalize_glob_variants_budgeted(
    g: &str,
    cap: usize,
    budget: &mut ComputeBudget,
) -> Result<Option<Vec<String>>, Exhausted> {
    budget.charge(g.len().max(1) as u64)?;
    let abs = g.starts_with('/');
    let mut states: Vec<Vec<&str>> = vec![Vec::new()];
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
                    if !apply_dotdot_budgeted(st, abs, &mut next, cap, budget)? {
                        return Ok(None);
                    }
                }
                next.sort_unstable();
                next.dedup();
                if next.len() > cap {
                    return Ok(None);
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
        let len: usize = st.iter().map(|s| s.len() + 1).sum::<usize>().max(1);
        budget.charge(len as u64)?;
        budget.charge_alloc(len)?;
        let s = if abs {
            format!("/{}", st.join("/"))
        } else if st.is_empty() {
            ".".to_owned()
        } else {
            st.join("/")
        };
        if !out.contains(&s) {
            out.push(s);
        }
    }
    Ok(Some(out))
}

fn escape_from_cands_budgeted(
    root_strs: &[String],
    root_folded: &[Vec<u8>],
    cwd: &[u8],
    home: &[u8],
    folds: FoldRules,
    cands: &GlobCandidatesBe,
    budget: &mut ComputeBudget,
) -> Result<(bool, bool), Exhausted> {
    let mut total_norm = 0usize;
    let mut escaped = false;
    for gv in &cands.variants {
        if gv.is_empty() {
            continue;
        }
        let anchored = gv.starts_with('/') || gv.starts_with('~');
        let raw_joined: Vec<String> = if anchored {
            vec![expand_home_be(gv, home, cwd, budget)?]
        } else {
            let mut v = Vec::with_capacity(root_strs.len());
            for r in root_strs {
                let base = r.trim_end_matches('/');
                let len = base.len() + 1 + gv.len();
                budget.charge(len as u64)?;
                budget.charge_alloc(len)?;
                v.push(format!("{base}/{gv}"));
            }
            v
        };
        for j in raw_joined {
            let remaining = MAX_SEARCH_GLOB_VARIANTS.saturating_sub(total_norm);
            if remaining == 0 {
                return Ok((true, true));
            }
            let Some(norms) = normalize_glob_variants_budgeted(&j, remaining, budget)? else {
                return Ok((true, true));
            };
            total_norm += norms.len();
            if total_norm > MAX_SEARCH_GLOB_VARIANTS {
                return Ok((true, true));
            }
            for n in norms {
                let nf = fold_path_listed(n.as_bytes(), folds, budget)?;
                let Ok(ns) = std::str::from_utf8(&nf) else {
                    escaped = true;
                    continue;
                };
                let mut lit_prefix = Vec::new();
                for seg in ns.split('/').filter(|s| !s.is_empty()) {
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
                    escaped = true;
                }
            }
        }
    }
    Ok((escaped, false))
}

fn search_variants_from_cands_budgeted(
    root_strs: &[String],
    cwd: &[u8],
    home: &[u8],
    folds: FoldRules,
    cands: &GlobCandidatesBe,
    budget: &mut ComputeBudget,
) -> Result<(Vec<String>, bool), Exhausted> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut unjoined_norm = 0usize;
    let mut joined_norm = 0usize;

    let push_norms = |pat: &str,
                      total: &mut usize,
                      out: &mut Vec<String>,
                      seen: &mut HashSet<String>,
                      budget: &mut ComputeBudget|
     -> Result<bool, Exhausted> {
        let remaining = MAX_SEARCH_GLOB_VARIANTS.saturating_sub(*total);
        if remaining == 0 {
            return Ok(false);
        }
        let Some(norms) = normalize_glob_variants_budgeted(pat, remaining, budget)? else {
            return Ok(false);
        };
        *total += norms.len();
        if *total > MAX_SEARCH_GLOB_VARIANTS {
            return Ok(false);
        }
        for n in norms {
            let folded = fold_str_listed(&n, folds, budget)?;
            if !folded.is_empty() && !seen.contains(&folded) {
                budget.charge_alloc(folded.len() + 32)?;
                seen.insert(folded.clone());
                out.push(folded);
            }
        }
        Ok(true)
    };

    for gv in &cands.variants {
        if gv.is_empty() {
            continue;
        }
        if !push_norms(gv, &mut unjoined_norm, &mut out, &mut seen, budget)? {
            return Ok((Vec::new(), true));
        }
        let anchored = gv.starts_with('/') || gv.starts_with('~');
        if !anchored {
            for r in root_strs {
                let base = r.trim_end_matches('/');
                let len = base.len() + 1 + gv.len();
                budget.charge(len as u64)?;
                budget.charge_alloc(len)?;
                let joined = format!("{base}/{gv}");
                if !push_norms(&joined, &mut joined_norm, &mut out, &mut seen, budget)? {
                    return Ok((Vec::new(), true));
                }
            }
        } else {
            let expanded = expand_home_be(gv, home, cwd, budget)?;
            if expanded != *gv
                && !push_norms(&expanded, &mut joined_norm, &mut out, &mut seen, budget)?
            {
                return Ok((Vec::new(), true));
            }
        }
    }
    Ok((out, false))
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

fn extract_script_flock_c_code(
    argv: &[String],
    folds: FoldRules,
    budget: &mut ComputeBudget,
) -> Result<Option<String>, Exhausted> {
    let Some(first) = argv.first() else {
        return Ok(None);
    };
    let folded = fold_cmd_cr19(first, folds, budget)?;
    if !matches!(folded.as_str(), "script" | "flock") {
        return Ok(None);
    }
    let short_vals = if folded == "flock" { "wE" } else { "tTIOBmo" };
    let mut j = 1usize;
    let mut file_seen = false;
    while let Some(w) = argv.get(j) {
        budget.charge(w.len().max(1) as u64)?;
        if w == "--" {
            break;
        }
        if let Some(attached) = match_gnu_long_opt(w, "--command", 3) {
            if let Some(val) = attached {
                return Ok(Some(clone_str_budgeted(val, budget)?));
            }
            if let Some(next) = argv.get(j + 1) {
                return Ok(Some(clone_str_budgeted(next, budget)?));
            }
            return Ok(None);
        }
        if w.starts_with('-') && !w.starts_with("--") && w.len() > 1 {
            if let Some(pos) = w[1..].find('c') {
                let after = &w[1 + pos + 1..];
                if !after.is_empty() {
                    return Ok(Some(clone_str_budgeted(after, budget)?));
                }
                if let Some(next) = argv.get(j + 1) {
                    return Ok(Some(clone_str_budgeted(next, budget)?));
                }
                return Ok(None);
            }
            if short_cluster_takes_next(w, short_vals) {
                j += 2;
            } else {
                j += 1;
            }
            continue;
        }
        if w.starts_with("--") {
            let long_takes = if folded == "flock" {
                matches!(w.as_str(), "--timeout" | "--wait" | "--conflict-exit-code")
            } else {
                matches!(
                    w.as_str(),
                    "--log-io" | "--log-in" | "--log-out" | "--output-limit" | "--timing"
                )
            };
            j += if long_takes { 2 } else { 1 };
            continue;
        }
        if !file_seen {
            file_seen = true;
            j += 1;
        } else {
            break;
        }
    }
    Ok(None)
}

/// Budgeted port of `dev` `shell::reads_stdin_as_script` (`at`, `batch`,
/// `script` without command, `chroot` without command, `crontab`, `make -f -`).
fn reads_stdin_as_script_budgeted(
    folded: &str,
    args: &[String],
    budget: &mut ComputeBudget,
) -> Result<bool, Exhausted> {
    let stdin = |w: &str| matches!(w, "-" | "/dev/stdin" | "/dev/fd/0");
    budget.charge(args.len().max(1) as u64)?;
    match folded {
        "at" | "batch" | "script" => Ok(true),
        "chroot" => {
            let mut operands = 0usize;
            let mut k = 0usize;
            while k < args.len() {
                let a = args[k].as_str();
                if matches!(a, "--userspec" | "--groups") || short_cluster_takes_next(a, "Ggu") {
                    k += 2;
                    continue;
                }
                if !a.starts_with('-') {
                    operands += 1;
                }
                k += 1;
            }
            Ok(operands <= 1)
        }
        "crontab" => {
            let mut k = 0usize;
            let mut file = false;
            while k < args.len() {
                let a = args[k].as_str();
                budget.charge(a.len().max(1) as u64)?;
                if stdin(a) {
                    return Ok(true);
                }
                if let Some(flags) = a.strip_prefix('-') {
                    let (before, user_next) = match flags.find('u') {
                        Some(p) => (&flags[..p], p + 1 == flags.len()),
                        None => (flags, false),
                    };
                    if before.contains(['l', 'r', 'e']) {
                        return Ok(false);
                    }
                    k += if user_next { 2 } else { 1 };
                    continue;
                }
                file = true;
                k += 1;
            }
            Ok(!file)
        }
        "make" | "gmake" => {
            for (i, a) in args.iter().enumerate() {
                budget.charge(a.len().max(1) as u64)?;
                let next_is_stdin = args.get(i + 1).is_some_and(|n| stdin(n));
                if let Some(v) = a
                    .strip_prefix("--file=")
                    .or_else(|| a.strip_prefix("--makefile="))
                {
                    if stdin(v) {
                        return Ok(true);
                    }
                    continue;
                }
                if matches!(a.as_str(), "--file" | "--makefile") {
                    if next_is_stdin {
                        return Ok(true);
                    }
                    continue;
                }
                if a.starts_with('-')
                    && !a.starts_with("--")
                    && let Some(k) = a.find('f')
                {
                    let rest = &a[k + 1..];
                    let hit = if rest.is_empty() {
                        next_is_stdin
                    } else {
                        stdin(rest)
                    };
                    if hit {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
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

    // `script -c CODE` / `flock -c CODE` / `--comm CODE`: check if CODE reads stdin as code.
    if matches!(folded.as_str(), "script" | "flock")
        && let Some(code) = extract_script_flock_c_code(eff, folds, budget)?
    {
        for seg in split_payload_segments(&code, budget)? {
            let words = raw_words_budgeted(&seg, budget)?;
            let inner_eff = unwrap_simple_be(&words, folds, budget)?;
            if reads_stdin_as_code_budgeted(&inner_eff, folds, depth + 1, budget)? {
                return Ok(true);
            }
        }
        return Ok(false);
    }

    if reads_stdin_as_script_budgeted(&folded, args, budget)? {
        return Ok(true);
    }

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
                budget.charge_alloc(std::mem::size_of_val(eff))?;
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
            || (is_sh && (a == "--command" || a.starts_with("--command=")))
    });
    if code_flag {
        if is_sh {
            // `sh -c bash`: check if the `-c` code itself reads stdin as code.
            if let Some(code) = extract_shell_c_code(args, budget)? {
                for seg in split_payload_segments(code, budget)? {
                    let words = raw_words_budgeted(&seg, budget)?;
                    let inner_eff = unwrap_simple_be(&words, folds, budget)?;
                    if reads_stdin_as_code_budgeted(&inner_eff, folds, depth + 1, budget)? {
                        return Ok(true);
                    }
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
        if matches!(w, "--rcfile" | "--init-file") {
            j += 2;
            continue;
        }
        if w.starts_with("--") {
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
