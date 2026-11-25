# M1-38-rev4-p2-paths: review round 10

Reviewer: Claude Sonnet 5.5, read-only (D-162). Reviewed: rounds 11 and 12 (`0721aa88`, `bf62832c`, `b550fdfe`, `1022a652`).

Orchestrator's verification (macOS, round 12 merged into dev): fmt and clippy clean; protocol up to date; workspace 988/0/7; typecheck clean; 377 vitest. Linux not run.

## Verdict
**Request changes.** One Low finding (D-155 count: 14, 9, 8, 7, 6, 5, 4, 3, 2, **1**).

Review 9 is closed:
- **V1:** builds, and both platforms charge the same per entry through `push_unix_dir_entry`.
- **Finding 1:** `canonicalize` charges are the same on both platforms. Only the last ~2 KiB of allocation budget differ (Exhausted either way, never Clear).
- **The `ancestor_stats` cache:** safe.

Round 12's fixes are correct:
- Skipping the alias scan after a HardDenyHit drops only records that can't change a HardDeny verdict (§1 step 2).
- The `lnk/../.ssh/sub` expectation matches `dev`'s `Analysis::all_paths` (`deny.rs:362`) and `Sensitive::hit` (`deny.rs:523-532`) under D-147.

The frozen interface is unchanged, and P2 is not wired into `SessionGate`.

## Findings
1. **Low: the non-Linux Unix `read_dir` makes an uncharged `openat` and an uncharged heap allocation (§8).**
   - **Where:** `paths/resolver.rs`, the `cfg(all(unix, not(target_os = "linux")))` block, ~464-466: `rustix::fs::Dir::read_from(&dir_fd)`.
   - **What it does:** in rustix 1.1.5, `read_from` runs `fcntl_getfl`, a second `openat(fd, ".")` and `fdopendir`, which mallocs the `DIR` and its buffer.
   - **What is charged:** only `charge_dir_open` and the path charges. Linux's `RawDir` uses a stack buffer and the fd it already holds, so on Linux there is nothing extra to charge.
   - **Failing case:** on macOS, up to the `DirOpens` cap of listings each make an extra open and allocation that the budget never sees. A small allocation budget then gives different verdicts on the two platforms.
   - **Fix:**
     - Build the stream from a duplicate of the fd you already hold, with no re-open: `Dir::new(rustix::io::dup(&dir_fd)?)` or `fcntl_dupfd_cloexec`. Keep `dir_fd` for `statat`.
     - Before it, call `charge_alloc(DIR_STREAM_ALLOC)` with a fixed upper bound for the libc stream.
     - Optionally, charge the same constant on Linux too, for parity.

## Test gaps
- **For the finding:** on non-Linux Unix, `read_dir` on a tiny directory with allocation budgets of `[entries' bytes, + DIR_STREAM_ALLOC - 1, + DIR_STREAM_ALLOC]` gives Exhausted, Exhausted, success. Also test with `DirOpens` = 1. Add a parity check that both platforms charge the same total.
- **Round 12, fix 1:**
  - a HardDenyHit coming only from the alias spelling (a bind-mounted `~/.ssh` written outside home) with an incomplete table gives no other Unread;
  - the same case through `check_recursive_root`'s followed-link walk.
- **The `ancestor_stats` cache:** test the 1,024-entry bound and hits against misses. For example, 1,100 operands in different directories give the same verdict as a fresh `RootAliases` per call.
- **Linux, by reading only:** `round3_r2f5` and the 1,100-operand real-fs test.
