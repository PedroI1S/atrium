# Review: M1-38-rev4-p2-paths, round 9 (round 10's code, `cae98db` plus the merge of `dev`, `635fe23b`)

Reviewer: Claude Sonnet 5.5, read-only (D-162; Codex has no usage on Pedro's account). The writer of round 10 was Opus 5.5, and round 11 goes to Neon `xhigh` (D-161). Saved by the orchestrator from the reviewer's report.

**Orchestrator's verification (macOS, D-123):**
- fmt is clean; the app typecheck and 377 vitest pass;
- **`atrium-core` does not compile on macOS:** `resolver.rs:369` imports `rustix::fs::RawDir`, which is Linux-only, so no Rust tests ran. This is item V1 below.

## Verdict

**Request changes.** Besides the known `RawDir` build failure on macOS, there is one defect: a platform divergence in `FsResolver::canonicalize`. Round 8's three findings and round 10's two verification fixes look closed:
- **R8 f1** (`push_ssh_members` matches `dev`'s `ssh_members`: bounds, order, linked folders);
- **R8 f2** (charges come before `self.incomplete.clone()` and the other reason clones);
- **R8 f3** (`lossy_lowercase_starts_with` agrees with `dev`'s expression, including U+0130 and U+212A);
- **R10 fix 1** (a written hard-deny hit ends the list only for a `Resolved` resolution);
- **R10 fix 2** (the `~/.ssh` walk names entries on the spelling checked).

The frozen P0 interface is unchanged, and P2 is not wired into `SessionGate`.

## Items for round 11

**V1 (verification): `read_dir` doesn't build on macOS.**
- **Where:** `resolver.rs:369`, `use rustix::fs::{…, RawDir, …}` inside `#[cfg(unix)]`. `rustix` provides `RawDir` only on Linux.
- **Fix:** give non-Linux Unix (macOS) its own implementation, for example `rustix::fs::Dir`, with the **same charges** as Linux, so the two platforms stop at the same point.
- **Note:** the only other branch, `#[cfg(not(unix))]`, charges `PATH_MAX` of allocation per entry, so copying it would diverge too.

**1. Medium: `FsResolver::canonicalize` stops a large operand list early on both platforms, for different reasons.**
- **Where:** `resolver.rs`, `canonicalize_via_proc` (`caps.charge_link_read()?`, about line 245) and the fallback in `canonicalize` (the per-component `charge_aux_stat` loop and `budget.charge_alloc(PATH_MAX * 2)`, about lines 340–350).
- **Linux:** each `/proc/self/fd` canonicalization charges a link read against `FS_MAX_READLINKS = 1,024`. That limit binds before the 4,096 canonicalization cap, which `limits.rs:96–98` sets high so "large argument lists do not trip prematurely".
- **Failing case on Linux:** `cat f1 … f1100` with 1,100 existing plain files, all under the cwd, and default `FsCaps`.
  - Expected: Clear, up to 4,096.
  - Actual: operands past about 1,024 are `Unread(CapReached(LinkReads))`, so the line asks. Symlinks among the operands push the cutoff lower still.
- **macOS:** the same call always takes the fallback.
  - It charges about 12 stats per operand at depth 6, so it reaches the 16,384 stat cap near 1,300 operands, as `Unread(Stats)`.
  - It charges 2 × `PATH_MAX` = 2,048 allocation bytes per canonicalization, so `Exhausted` comes at roughly 3,000 operands.
  - So the two platforms stop at different points, and neither reaches 4,096.
- **Fix:**
  - don't charge the link-read cap for the fixed `/proc/self/fd/N` read (the `Canonicalizations` cap and the compute charge already bound it);
  - on the fallback, charge the allocation at the real path length plus a bound, not 2 × `PATH_MAX`;
  - count the per-component stats once, or against `Canonicalizations` rather than `Stats`;
  - pick one set of charges, so that 4,096 single-component-deep operands get the same verdict on both platforms.

## Test gaps

- **For 1 and V1:** a real-`FsResolver` test (not `FakeFs`), on both Linux and macOS. It resolves 1,100 plain existing files through `resolve_command_targets` or `resolve_subject_paths` with default caps, and asserts that no target is Unread or Exhausted. The same check covers `read_dir` on a folder with more than 1,000 entries. Today the real-filesystem tests use only a few paths, and `round3_r2f4` and `r2f5` are `cfg(linux)`.
- **Charge ordering (rounds 8 and 10):** a library test can't count allocations. As a proxy, for each new charge site (the incomplete-table reason, the MSAL entry copy, the `~/.ssh` member spelling), assert the exact minimum allocation budget at which the call first succeeds, and that it is at least the size of the copy.
- **Round 10, finding 1:** `round10_f1` covers only a bound member folder. Add:
  - a member file reached through a bind or hard link;
  - a member folder reached through a linked `.ssh` (linked trees are not alias roots);
  - `/mnt/keys` when `~/.ssh` has more than 1,024 entries (a silent cutoff, so Clear, as on `dev`).
- **Round 10, fix 2:** `round8_f1` covers the folder root `lnk/../.ssh`. Add a root `~` or `~/proj` that reaches a `.ssh` below it holding a hard-deny entry, and the alias spelling of the same case.
- **macOS:** a `cfg(target_os = "macos")` test that canonicalizes `/System/Volumes/Data/<tmp>` and a mixed-case `~/.SSH` path on the real filesystem. Nothing exercises the firmlink strip or the case-insensitive canonical spelling against a real volume.

**Finding counts by review:** 14, 9, 8, 7, 6, 5, 4, 3, then **2** (V1 and finding 1). The count is still falling, so round 11 goes ahead under D-155 and D-161.
