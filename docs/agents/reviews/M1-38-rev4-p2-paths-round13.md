# M1-38-rev4-p2-paths review round 13 — Claude Sonnet 5.5 (Claude Code subagent on the driver's plan, read-only; D-170 fallback)

Branch at `2c304075` (rounds 15 and 15b). Orchestrator's verification (macOS, round 15b merged into local `dev` with M1-43 and M1-44): fmt/clippy clean, protocol up to date, workspace 1049/0/7, typecheck clean, 373 vitest. Round 15's first verification failed `round15_read_dir_large_blksize_100_dir_walk_and_call_level_worst_case` (the call-level worst case ended `Exhausted(AllocationBudget)`, on Linux too); round 15b fixed it in the product. D-155 count: 14, 9, 8, 7, 6, 5, 4, 3, 2, 1, 1, 2, **0**.

## Verdict

**Approve (no findings).** Checked P2's own changes (`git diff 6f2d5c57...HEAD`; rounds 15 and 15b touch only `resolver.rs`, `p2_tests.rs` and the handoff). Linux was checked by reading only. The one remaining assumption is Darwin's `fdopendir` allocation, which couldn't be verified locally; the design bounds it with a 64 KiB floor and a 1 MiB `st_blksize` cap.

- **Finding 1, the stream charge.**
  - Linux: `RawDir` reads into a 4,096-byte stack array and makes no heap allocation, so deducting `DIR_STREAM_ALLOC` (256) is a pure over-charge: stricter, and still charge-before-allocation.
  - Non-Linux: `charge_dir_stream` runs before `Dir::new`. A negative, unconvertible or > 1 MiB blksize returns `Io(Other)` without touching the budget. Otherwise it checks `remaining >= max(blksize, 64 KiB) + 256`, deducts 256, and returns `peak - 256` as `stream_extra`, held as headroom in every later allocation while the `Dir` is open (the per-iteration check, the name `Vec`, `push_entry_charged`, the `stat_dir_child_by_path` heap branch). The only other allocations are `fdopendir`'s (covered by `charge_dir_stream`) and the per-entry `CString` (covered by the per-iteration check).
  - Non-unix: the same shape with 4,096 + 256.
  - The call-level worst case (4,096 operands + 1,024 `DirOpens`) leaves about 3.3 MiB, more than 3× the largest valid peak.
- **Finding 2, the name copy.** `NAME_COMPUTE_PEAK` is checked before every `dir.next()`; after it, `.` and `..` are skipped and `len - 1` is charged before the copy, with `name_compute_precharged = true` so there's no double charge. Totals equal Linux's, plus 1 compute per non-Linux listing for the `fstat`.
- **`DT_UNKNOWN`.** `ENOENT` now gives `Io(Other)` (Unread in `glob.rs`, `recursive.rs`, `sensitive.rs`); before, `NotFound` was treated as an absent directory, which gave Clear. So a listing can only move from Clear to Unread, never toward a hit or a hard deny. The macOS stat-by-path differs from Linux's fd-relative stat only under a concurrent rename (excluded by §2 "Time") or `ENAMETOOLONG`, both Unread.
- **Other checks.** The frozen P0 and P3 files are untouched; P2 still isn't wired into `SessionGate`; `attach_applicable_gap` is unchanged since review 12. Every divergence from Linux can only turn Clear or Unread into Unread or Exhausted near budget exhaustion, never a hit into Clear.

## Findings

None.

## Test gaps (optional hardening, not defects)

1. Headroom on retained pushes isn't pinned directly: a regression passing `alloc_headroom = 0` in `push_unix_dir_entry` or `push_entry_charged` would still end `Exhausted`, one step later. Only the `DT_UNKNOWN` heap branch pins it.
2. `DT_UNKNOWN` wiring through `read_dir_unix_impl` is untested (APFS never returns it; needs an injectable file-type hook).
3. On Linux the blksize wrappers ignore `blksize`, so the 100-dir walk and worst case reduce to the plain 256-byte case. Linux runs weren't done.
4. Missing blksize cases through the real `read_dir`: `0` (should floor to 64 KiB) and `1_048_577`.
5. Missing: a `DirOpens`-capped listing at the cap boundary with the budget already exhausted, pinning that no fd is opened.
