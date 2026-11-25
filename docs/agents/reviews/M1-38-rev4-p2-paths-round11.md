# M1-38-rev4-p2-paths review round 11 — Claude Sonnet 5.5 (Claude Code subagent on the driver's plan, read-only; D-170 fallback, agy's Sonnet quota ran out)

Branch at `f7f906d5` (round 13). Orchestrator's verification: fmt/clippy clean, protocol up to date, workspace 1037/0/7, typecheck clean, 373 vitest.

## Verdict

**Request changes.** One Low (D-155 count: 14, 9, 8, 7, 6, 5, 4, 3, 2, 1, **1**), the same class as review 10's: macOS `read_dir` still makes a heap allocation that isn't charged first (§8). Review 10's finding is closed: `DIR_STREAM_ALLOC` is charged after the one `open` and before the stream is built (`resolver.rs:458-463`), with exactly one `openat` per listing, macOS using `fcntl_dupfd_cloexec` + `Dir::new`. The round 13 gap-attachment change (`attach_applicable_gap`) drops no Unread that §1, §2 or D-154 needs; standalone `check_target_impl` keeps attaching gaps. The `ancestor_stats` cache is safe: only `Ok` stats of proper ancestors are cached, a hit returns the same identity a fresh stat would, and past 1,024 entries lookups fall back to a fresh stat. P0 signatures unchanged, P2 not wired into `SessionGate`, no conflict with P3.

## Findings

1. **Low: non-Linux Unix `read_dir` allocates uncharged, per entry (§8).**
   - **Where:** `crates/atrium-core/src/policy/v4/paths/resolver.rs`, the `cfg(all(unix, not(target_os = "linux")))` loop, ~496-519; `dir.next()` at ~506.
   - **What happens:** in rustix 1.1.5, `Dir::read` builds each `DirEntry` with `CStr::from_ptr(dirent.d_name).to_owned()`, a heap `CString` made inside `next()`. The loop charges `entries_read` and 1 compute unit before `next()`, but no allocation until `push_unix_dir_entry` (line 320), after the entry exists. Linux's `RawDir` makes no such allocation, and the non-unix branch already charges `PATH_MAX` before advancing (~530). So the `DIR_STREAM_ALLOC` doc's "same budget on both platforms" claim is untrue for entries.
   - **Failing case (macOS):** a directory with a 255-byte name, `remaining_alloc_bytes` just under `DIR_STREAM_ALLOC` plus the earlier entries. Expected: Exhausted(AllocationBudget) before the `CString` is made, as on Linux. Actual: the `CString` is allocated first, and Exhausted comes only at the next charge.
   - **Possible second part (the reviewer checked it from memory only):** `DIR_STREAM_ALLOC` assumes a 4,096-byte `dd_buf`. Darwin and FreeBSD `fdopendir` may size it from the directory's `st_blksize`, so a network or FUSE filesystem with a large `st_blksize` would break the fixed bound.
   - **Fix:** before each `dir.next()` on non-Linux Unix, check `remaining_alloc_bytes() >= NAME_ALLOC_PEAK` (the peak-check-without-deduction pattern already used for `FALLBACK_CANON_PEAK`), with `NAME_ALLOC_PEAK` covering `d_name` plus `CString` overhead. Or read with `libc::readdir` on the existing stream and copy the name with no `CString`. For `st_blksize`: `fstat` the open `dir_fd` and charge `max(DIR_STREAM_ALLOC, st_blksize + overhead)` before `Dir::new`, or treat a directory above the constant as Unread. Update the `DIR_STREAM_ALLOC` doc.

## Test gaps

- Finding 1: a non-Linux Unix test of `read_dir` on a directory with one 255-byte name, at budgets `DIR_STREAM_ALLOC`, `DIR_STREAM_ALLOC + NAME_ALLOC_PEAK - 1` and enough: Exhausted, Exhausted, success.
- No test counts opens: `round13_read_dir_charges_dir_stream_alloc_and_single_dir_open` checks only the `DirOpens` cap. Count open fds before, during and after a listing.
- A directory removed between `open` and `readdir` on macOS: the old `read_from` gave an empty listing; the dup path may give NotFound. Pin that the verdict is unchanged, above all for recursive roots.
- A target with both an applicable `AnyTarget` gap and an incomplete alias table still gets Unread; its reason now comes from the alias record only. Assert what D-144 needs.
