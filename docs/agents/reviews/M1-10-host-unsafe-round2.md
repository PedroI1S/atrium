# Unsafe re-audit: M1-10-host (round 2)
Reviewer: Opus 5.5 (read-only) · 2026-11-09 · diff `40437e3..2e8d270` in `/Users/pedro/atrium-wt/M1-10-host`

## Inventory

The `unsafe` sites in the diff match the worker's list, with no others.

- Changed or new:
  - `supervise.rs`: :213, :235, :311.
  - `peer.rs`: extern blocks :155, :164, :206; and :247, :260, :289, :312, :327, :335, :359, :392, :426, :444, :582, :629, :661.
  - `tests/lifecycle.rs`: :262.
- Removed:
  - the old `tests/supervise.rs:262` raw `kill`;
  - the old `tests/lifecycle.rs` unguarded `kill(pid)` calls;
  - the `SecCodeCreateWithAuditToken` and `SecCodeCopyStaticCode` declarations.
- Unchanged: `lib.rs:17`, `supervise.rs:408` and `:440`, `lifecycle.rs:262`, and the other test probes.

There is still no `unsafe impl`, `static mut` or `set_var`, and no new `pre_exec`.

Checks run: `cargo clippy --workspace --all-targets -- -D warnings` is clean, and `cargo test -p atrium-host` passes all 18 tests on macOS arm64. After the run there were no leftover `sleep` or helper processes, and `git status` was clean.

## Verdict
**sound**: every changed or new block is sound. The FFI signatures, constants and Create/Get ownership are all correct, and all 7 earlier findings are fixed. The two new findings are comment-accuracy nits.

New FFI checked against the Apple headers:
- `CFDataCreate(CFAllocatorRef, const UInt8 *, CFIndex)`: Create rule, copies the bytes.
- `CFDictionaryCreate(alloc, const void **keys, const void **values, CFIndex, const CFDictionaryKeyCallBacks *, const CFDictionaryValueCallBacks *)`: Create rule. The callback structs have the right shape (version plus 5 function pointers for keys, plus 4 for values), and they are only passed by address, never read.
- With `kCFType…CallBacks` the dictionary retains its key and value. `audit_data` (+1) and `attrs` (+1) are each released once by `CfRef`, in either order. The `keys` and `values` arrays outlive the call. The borrowed constant `kSecGuestAttributeAudit` is never released.
- `SecCodeCopyGuestWithAttributes(SecCodeRef _Nullable host, CFDictionaryRef, SecCSFlags, SecCodeRef *)`: Copy rule. A null host means the kernel. The `kSecGuestAttributeAudit` value is a `CFData` holding the full 32-byte `audit_token_t`, which is correct.
- `kSecCSDynamicInformation = 1 << 3` is correct (CSCommon.h). Passing a `SecCodeRef` where `SecCodeCopySigningInformation` takes a `SecStaticCodeRef` is documented as allowed.
- `getpgrp` has no preconditions, and the own-group guard compares like types (`pid_t`).

## Earlier findings

| # | Fixed? | Note |
|---|---|---|
| 1 `CfRef::new` safe constructor | yes | Now `unsafe fn from_create_rule` with a `# Safety` doc. All 7 call sites pass a fresh Create/Copy result, and `Drop`'s SAFETY comment now points at the constructor's invariant. |
| 2 own process group signalled | yes | `spawn_group` rejects `identity.pgid != pid` (it kills the child and returns an error). `signal_group` refuses `raw_pgid == getpgrp()`. The new test `kill_recorded_never_signals_the_callers_own_process_group` covers it. |
| 3 pidfd leak on bad `len` | yes | The fd is wrapped in `OwnedFd` before the `len` check. See new nit N2 for the comment. |
| 4 hard-coded `SOL_LOCAL`/`LOCAL_PEERTOKEN` | yes | Now `libc::SOL_LOCAL` and `libc::LOCAL_PEERTOKEN`. |
| 5 call order and SAFETY completeness | yes | `len` is checked before `audit_token_to_*`. `peek_exited` reads `si_pid` only when `rc == 0`, and its comment explains the Linux union read. |
| 6 test `kill` hygiene | yes | The descendant is killed through the guarded `KillPidGuard` drop. The `spawn_detached` test kills `-pid` (guarded `> 1`). No orphaned `sleep 30` after the run. |
| 7 macOS 11-only symbol | yes (moot) | `SecCodeCreateWithAuditToken` is gone. `SecCodeCopyGuestWithAttributes` and `kSecGuestAttributeAudit` are older (pre-11) APIs. |

## New findings

1. [nit] **The `extract_dynamic_signing_details` comments overstate where the cdhash comes from**
   - Where: `crates/atrium-host/src/peer.rs:435-443`.
   - Problem: the doc says it reads signing info "without converting to a disk-backed static code object". Inside Security.framework, `SecCodeCopySigningInformation` on a `SecCodeRef` still resolves the code to its static code, and `kSecCSDynamicInformation` only adds `kSecCodeInfoStatus`. So `kSecCodeInfoUnique` and `kSecCodeInfoMainExecutable` are the static (on-disk) view. This is memory-safe. The real binding to the running process is the earlier `SecCodeCheckValidity` on the dynamic code. That call rejects a kernel/disk cdhash mismatch with `errSecCSStaticCodeChanged` (-67034) and enforces `cdhash H"<expected>"`. `current_identity` makes the same call first with no requirement.
   - Fix: reword the comment to say the values come from the code's static view and are trusted only because `check_code_validity` ran first on the same `SecCodeRef`. Mark that call order as load-bearing so nobody reorders it.

2. [nit] **The `peer_pidfd` SAFETY comment leaves out that the kernel always writes a full `int`**
   - Where: `crates/atrium-host/src/peer.rs:623-647`.
   - Problem: the fd is now wrapped before the `len` check. That is correct only because `sock_getsockopt` clamps `len = min(optlen, sizeof(int))`, so with `optlen == 4` it always writes all 4 bytes. If a kernel ever did a short write on a big-endian target, the partly overwritten `-1` could form a positive number that isn't our fd, and `OwnedFd` would then close someone else's descriptor. This can't happen with real kernels.
   - Fix: add one sentence to the comment citing the clamp. Optionally assert `len == expected_len` before `from_raw_fd`, accepting a theoretical leak instead of a theoretical foreign `close`.

## CR-05 (outside unsafe, brief)

Yes, in effect. The `MacUnsigned` path:
1. builds the code object from the audit token through `SecCodeCopyGuestWithAttributes`, which pins the exact process instance, so pid reuse is caught;
2. runs `SecCodeCheckValidity` on that dynamic `SecCodeRef` with the requirement `cdhash H"<expected>"`. That fails unless the kernel's cdhash equals the on-disk cdhash, which equals the expected value. A binary replaced on disk after launch gives -67034, a different binary gives a requirement failure, and both fail closed;
3. only then reads `kSecCodeInfoUnique` and compares it again.

`SecCodeCopyStaticCode` is no longer called. The dictionary read on its own is still the static view (N1). The test's step 4b accepts either `Ok` or -67034, which matches this behaviour, and step 4a (on-disk replacement must never pass) holds.
