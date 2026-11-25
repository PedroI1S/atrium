# M1-38-rev4-p2-paths review round 12 — Claude Sonnet 5.5 (agy, read-only, D-169)

Branch at `2ed5cbde` (round 14). Orchestrator's verification: fmt/clippy clean, protocol up to date, workspace 1041/0/7, typecheck clean, 373 vitest. **D-155: the count stalled at review 11 (1, 1) and rose at review 12 (2), so P2 goes back to the driver.**

## Verdict

**Request changes.** Review 11's finding is closed in substance. On non-Linux Unix, `rustix::Dir::next` still builds a `CString` per entry. `NAME_ALLOC_PEAK` (1,088 bytes) is checked before every `dir.next()` and covers it, since Darwin's `d_name` is at most 1,024 bytes. `Dir::new(dir_fd)` owns the only fd, so nothing calls `statat` on an fd the stream owns. `DT_UNKNOWN` now stats by path, and the only differences from an fd-relative `statat` need a concurrent rename or removal, which §2 "Time" excludes. The one deterministic difference is that a path longer than the platform's `PATH_MAX` (1,024 on macOS) fails with `ENAMETOOLONG`. That becomes `Io(Other)`, which is Unread, so it is stricter, not looser. On a local 4 KiB volume the allocation total matches Linux. Compute differs by 1 unit per listing, and the peak headroom is extra. Both are stricter. Round 14's `attach_applicable_gap` is sound: it keeps both distinct reasons, dedupes equal ones, and attaches nothing to a hard-deny hit, a non-`Resolved` target or a variant-only target. It adds an Unread only where D-144 already put a gap Unread on a Clear or Store target, or to a target that is already Unread. The P0 signatures are unchanged, P2 is not wired into `SessionGate` (`evaluate.rs` only imports the `PathResolver` trait), and P2 and P3 don't conflict. Two Low findings remain, so the count goes 1, 1, **2**. Under D-155 that is a rise, so the task goes back to the driver. I couldn't verify Darwin's `fdopendir` allocation: Apple's source isn't local and the reads and probes I tried were blocked by the pre-tool hook. Linux code was checked by reading only.

## Findings

1. **Low: the non-Linux stream charge scales with `st_blksize` and is deducted permanently, so large-blksize volumes become Exhausted instead of Unread (§2, §8).**
   - **Where:** `resolver.rs:528-534` (`stream_alloc = dir_stream_alloc_for_blksize(..)` then `budget.charge_alloc(stream_alloc)`), and the doc at `:191-206`.
   - **What happens:** the `dd_buf` is freed when the listing ends, but the charge stays against the 8 MiB call-wide budget (`COMPUTE_BUDGET_ALLOC_BYTES`). Linux charges a flat 4,352 bytes.
   - **Failing case (macOS):** `rg -L pat proj` on a tree of about 70 directories, on a volume reporting `st_blksize` = 131,072 (exFAT, SMB or NFS are plausible, but I haven't checked what they report).
     - Expected: Clear, as on APFS, Linux and `dev`. §2 says a walk that hits a cap is Unread, "not Exhausted".
     - Actual: each listing deducts 131,328 bytes, so the 64th listing (a budget of 8,388,608 bytes holds 63) gives `Exhausted(AllocationBudget)`, which means Ask even under FullAccess. At 1 MiB it happens on the 8th listing.
   - **Related:** the 4,096 floor is still an assumption. The handoff concedes it can't be settled from files on this machine. If Darwin's buffer is larger than 4,096 on a 4 KiB volume (a page-size or `MAXBSIZE`-based floor), the charge is too small.
   - **Fix:** treat the stream like the other transient buffers (`FALLBACK_CANON_PEAK`, `NAME_ALLOC_PEAK`).
     - Check that `remaining_alloc_bytes() >= max(blksize, 16 KiB, size of one dirent) + DIR_STREAM_OVERHEAD` before `Dir::new`, without deducting it.
     - Deduct only `DIR_STREAM_ALLOC`, which keeps the totals equal to Linux's.
     - Update the doc.

2. **Low: non-Linux `read_dir` copies each name before charging its compute (§8 "charge before copy").**
   - **Where:** `resolver.rs:557-575`. `name_buf[..len].copy_from_slice(..)` at `:560` runs before `budget.charge(name_bytes.len() - 1)` at `:433` inside `push_unix_dir_entry`. Before `dir.next()` only `charge(1)` has been made, yet rustix's `CString` copy (up to 255 B) and this stack copy (up to 1,024 B) both happen.
   - **Linux:** it borrows `entry.file_name()` and charges before any copy.
   - **Failing case:** a compute budget with 1 unit left and a 255-byte name.
     - Expected: Exhausted(ComputeBudget) before any name byte is copied.
     - Actual: two ~255-byte copies, then Exhausted.
     - The verdict is the same, but the ordering contract is broken.
   - **Fix:** `remaining_units()` exists, so add a compute peak check, `remaining_units() >= MAX_D_NAME_BUF`, next to the `NAME_ALLOC_PEAK` check. Alternatively, charge `len - 1` from `entry.file_name().to_bytes().len()` before the copy, and have `push_unix_dir_entry` skip its own charge on that path.

## Test gaps

- **Finding 1:**
  - Inject `st_blksize`, for example by splitting out `charge_dir_stream(blksize, budget)` or giving the hooks a blksize override. Assert that a 128 KiB blksize deducts only `DIR_STREAM_ALLOC`, that a budget below the peak gives Exhausted before `Dir::new`, and that a negative blksize gives `Io(Other)`.
  - Add a real-FS or injected walk over about 100 directories with a large blksize that ends Clear or `Unread(DirOpens)`, never Exhausted.
  - Today `expected_dir_stream_alloc` reads the same blksize from the same directory, so a `read_dir` that ignored `st_blksize` would still pass.
  - No test pairs the per-listing charge with the call-level worst case: 1,024 `DirOpens` plus 4,096 operands under default limits must end at Unread(DirOpens), not Exhausted.
- **Peak check ordering and value (review 11):**
  - `round14_read_dir_255_byte_name_*` can't tell a peak check before `next()` from one after it.
  - `expected_dir_entry_peak_headroom()` returns `NAME_ALLOC_PEAK` itself, so changing the constant (even to 1) still passes.
  - Use `read_dir_unix_with_hooks`: assert `on_iter` runs 0 times when the allocation budget is below the peak.
  - Assert the literals (4,352 and 1,088) and `NAME_ALLOC_PEAK > MAX_D_NAME_BUF`.
- **`DT_UNKNOWN`:**
  - `stat_dir_child_by_path`, `DirStatBase::{Fd, Path}` and the unknown-type branch of `push_unix_dir_entry` have no test on either platform.
  - Feed `FileType::Unknown` straight into `push_unix_dir_entry` over a real tempdir. Cover file, dir, symlink and fifo, the heap branch where `dir + name >= PATH_MAX`, ENOENT, and the order (stat cap and compute before the syscall).
  - Also pin that an entry removed between listing and stat does not silently discard the whole listing as `NotFound`. That case is concurrency-only, so I haven't counted it as a finding.
- **fd leak on early failure:** `round14_read_dir_open_fd_count_*` checks 0 fds only after an `EntriesRead` failure. Add the same check after stream-charge Exhausted and after a negative blksize, both of which exit before `Dir::new`.
- **`attach_applicable_gap`:**
  - A variant-only glob match with both an `AnyTarget` gap and an incomplete alias table gets neither reason.
  - A target with an `UnlistedNameEquivalence` Unread plus a gap keeps both. This is the behaviour round 14 changed, and no test pins it.
  - A write-only gap on a Read target is not attached.
  - Exhaustion inside `push_alias_records` loses only the gap Unread, and the call stays Exhausted.

