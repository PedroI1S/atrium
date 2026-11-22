# M1-38-rev4-p2-paths: rev 4's paths, globs and links (P2)

Model: Gemini 4 Neon (`gemini-4-neon-high`, `scripts/agent-run agy`, D-134) · Reviewer: Codex GPT-6.1 Sol `high`, read-only (D-124) · Depends on: P0 (`M1-36-rev4-p0-interface`, branched from its tip `1a09705`)
Go: D-138, D-140 · Branch: `agent/neon/M1-38-rev4-p2-paths` · Worktree: `../atrium-wt/M1-38-rev4-p2-paths`
Contract: `docs/contracts/m1-policy-rev4.md`. Read all of it. P2 implements §5, with §2's filesystem caps and walk-cap rule, and §8's filesystem budgets.

## Goal
Implement the stubs P0 left in `crates/atrium-core/src/policy/v4/paths.rs`:
- `resolve_literal`;
- `expand_shell_glob`;
- `check_target`;
- `check_recursive_root`;
- `resolve_command_targets`;
- `resolve_subject_paths`.

They must match the signatures and the `model.rs` types exactly. Also implement P0's `PathResolver` trait twice:
- **For the real filesystem** (`FsResolver`). It charges P0's filesystem caps and the compute and allocation budget before each operation, and keeps absence apart from I/O failure.
- **An in-memory fake** for tests, which can plant symlinks, dangling links, hard links, unreadable folders and case-variant names.

**Nothing is wired into `SessionGate`** (that's P5), so no behaviour changes.

## What it must do (contract §5)
- **Literal paths:**
  - resolve as on `dev`: deepest existing ancestor canonicalized, symlinks followed up to `MAX_HOPS`, a dangling entry counts under its own name;
  - compare the (device, inode) identities of the path **and of its existing ancestors** with the protected set;
  - a path that can't be resolved is Unread;
  - a process-dependent path (`/proc/self`, `/proc/<pid>`, `/dev/fd/`, `/dev/std*`, directly or through a link) is Unread.
- **Names:**
  - the listed folds (CR-19's and `deny.rs`'s path folds: Unicode lowercase, `ſ`→`s`, K→k) define the protected spellings, **in addition to** identity;
  - a name that matches only under another equivalence (NFC/NFD, an unlisted case fold) is Unread, and never downgrades a hit (§11 item 13).
- **Shell globs:**
  - expand against the filesystem at the unit's cwd, within budget and caps;
  - each match is a literal path;
  - no match: check the literal word;
  - matches under all three dialects can be hits;
  - variant-only matches (dot-matching, recursive `**`) are Unread, never hits;
  - no pattern-versus-pattern reasoning.
- **Search selectors and filters:**
  - a selector (`..` or absolute) expands like a shell glob, and D-100's verdicts stay;
  - a filter, in the dialects P0's table names, takes two steps:
    1. enumerate the selected entries within the walk cap and resolve each one;
    2. match the filter against `dev`'s probe names (a Store hit, or HardDeny for the hard-deny probes, as on `dev`).
  - Negated filters, several filters and other dialects are Unread.
- **Recursive reads:**
  - **S8:** a root that contains an existing hard-deny path is HardDeny, whatever its filters. The exceptions are home, ancestors of home and `~/.config` (D-133), and the rule extends to ancestors of Atrium data paths outside home;
  - **followed links:** walk them within budget. A protected link target is a hit without a filter and Unread with one, and reaching the cap is Unread;
  - **hard links:** `dev`'s bounded walk exactly (4,096 entries, depth 8, skipping `node_modules` and `.git`, clear when it runs out); the result carries the multiply-linked flag;
  - **item 15 (D-139):** a followed search with no protected link within budget is Clear.
- **`cd` and writers:**
  - `cd` to a literal path moves resolution; any other `cd` makes the cwd unknown, so later units are Unread;
  - **effects within one call:** after a possible writer, the targets of every later unit, and of every stage of a pipeline that contains one, are Unread, and their hits still stand;
  - **derived destinations** make the writer Unread, and a destination root inside or equal to a protected path is still a hit;
  - use `ToolEffects` as P0 defined it, conditional derived destinations included (P3 fills it later; take it as input).
- **The shared `Sensitive`:** the protected set and probes come in as parameters, as P0's signatures say. Port M1-31's shared `Sensitive` from `engine.rs` if P0's signatures need it.

## Work carried from the M1-27 batch (contract §12)
For each item, read the source branch read-only (`git diff dev...<branch>`). Then keep, rewrite or drop each hunk and test, and record the disposition list in the handoff:
- **`agent/hydrogen/M1-29-cr17-stores`:** CR-17's four stores.
- **`agent/hydrogen/M1-30-tree-wildcards`:** its test cases, migrated one by one, absent targets especially; drop its matcher.
- **`agent/hydrogen/M1-31-credential-folder-search`:** S8's verdicts and the shared `Sensitive`; drop its root parser and glob normalizer.
- **`agent/hydrogen/M1-32-link-globs`:** as obligations:
  - ordered resolution;
  - dangling targets;
  - I/O uncertainty;
  - identity;
  - the `rg -L` verdict.

  Drop its link-glob expansion.

A kept test gets this contract's verdict, citing §10 or §11, never the old one.

## Tests
- **The §10 rows P2 decides:** 2, 8, 9, 11, 12, 15, 16, 17 (its target part), 18, 22, 23 and 24, asserting P2's outcomes with the fake resolver.
- **The boundaries:**
  - absent and dangling targets;
  - loops and `MAX_HOPS`;
  - unreadable folders (Unread);
  - case and fold variants on case-sensitive and case-insensitive fakes;
  - NFC/NFD;
  - variant-only glob matches;
  - walk caps (Unread, never a hit);
  - hard links (the multiply-linked flag);
  - writer effects across `;`, `&&` and pipelines.
- **The real-filesystem resolver:** tests on a temp folder with a fake `HOME`, never the real home. Use symlinks, a dangling link and an unreadable folder.
- **Budgets:** each filesystem operation charged before it runs, and caps reaching Unread while the compute budget gives Exhausted.

## Owned paths
- `crates/atrium-core/src/policy/v4/paths.rs`, and new files under `v4/` for P2 only (for example `v4/paths/`).
- The handoff `docs/agents/handoffs/M1-38-rev4-p2-paths.md`.
- **The interface is frozen.** If you need a change in `model.rs`, `limits.rs`, `tools.rs` or another task's stub, don't make it. Write it in the handoff as a change request, with the reason, and work around it.

## Rules
- Follow the worker rules in `docs/agents/README.md`. Under D-123, run only `cargo fmt --all`, `cargo check -p atrium-core --tests` and `cargo clippy -p atrium-core --tests -- -D warnings`. No `cargo test` and no `pnpm`.
- Never put test data in shell heredocs. Tests that need a home folder use a temp folder and a fake `HOME`. Never read real credential paths. Commit in small steps. Never push, merge or switch branches.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy and `cargo test --workspace --no-fail-fast` clean; no existing test changed.
- [ ] Codex review: approve with no findings (D-120), against contract §2, §5 and §8.
- [ ] If P0 changes after `1a09705`, the branch is rebased onto P0's final tip, and verification and review run again.
