# Unsafe audit: M1-10-host
Reviewer: Opus 5.5 (read-only) · 2026-11-09

Scope: memory safety and FFI soundness of every `unsafe` block, `unsafe extern` block and `unsafe impl` in `crates/atrium-host` (library and tests) at commit `40437e3`. Behaviour and contract compliance are covered by the separate Sonnet review.

Inventory: 18 sites in `src/` (1 in `lib.rs`, 4 in `supervise.rs`, 3 `unsafe extern` blocks + 9 blocks in `peer.rs`, 1 in `lifecycle.rs`) and 11 in `tests/`. This matches the handoff's list exactly. There is no `unsafe impl`, `static mut`, `transmute`, `env::set_var`/`remove_var`, or other `pre_exec` anywhere in the crate.

Checks run: `cargo clippy --workspace --all-targets -- -D warnings` (clean), `cargo test -p atrium-host` (15/15 pass on macOS arm64). The Linux paths were checked by reading only: the libc 0.2.191 sources confirm `libc::SO_PEERPIDFD` is exported on Linux (77, or 0x56 on sparc) and `SYS_pidfd_send_signal` is 424 (+4000/5000 on MIPS).

## Verdict
**sound with changes**: no undefined behaviour, wrong FFI signature, wrong constant or ownership error found. Two minor hardening changes and five nits are listed below.

## Blocks

| file:line | what it does | verdict | note |
|---|---|---|---|
| src/lib.rs:17 | `geteuid()` | ok | No preconditions. std has no safe stable wrapper. |
| src/supervise.rs:193 | `kill(-raw_pgid, sig)` | ok | `u32 → i32` with `try_from`, `raw_pgid >= 2`, so the target is never 0, -1 or `i32::MIN`. Own-group hardening: finding 2. |
| src/supervise.rs:263 | `waitid(P_PID, pid, &info, WEXITED\|WNOHANG\|WNOWAIT)` | ok | `siginfo_t` is zeroed, so the `WNOHANG` "nothing yet" case reads `si_pid == 0` on both OSes. Linux `si_pid()` reads the union, which is valid because the struct is zeroed and has the SIGCHLD layout. Comment gap: finding 5. `pid > 0` is checked, and `as id_t` is lossless. |
| src/supervise.rs:352 | `proc_pidinfo(PROC_PIDTBSDINFO)` into a zeroed `proc_bsdinfo` | ok | Size passed equals the buffer size. `ret <= 0` and `ret < size` are both checked before `info` is used. `SZOMB = 5` matches XNU `proc_internal.h`. |
| src/supervise.rs:384 | `proc_pidpath` into a 4096-byte stack buffer | ok | `PROC_PIDPATHINFO_MAXSIZE` is the documented capacity. The returned length is clamped and NUL-searched. |
| src/peer.rs:129 | `unsafe extern` libbsm: `audit_token_to_euid/pid` | ok | Matches `<bsm/libbsm.h>`: by-value `audit_token_t` (`unsigned int val[8]`), returns `uid_t`/`pid_t`. The 32-byte struct by value lowers to the same indirect passing on arm64 and x86_64 as C. |
| src/peer.rs:138 | `unsafe extern` CoreFoundation (9 fns) | ok | LP64 types are right: `CFTypeID` = `unsigned long`, `CFIndex` = `long`, `Boolean` = `UInt8`, `CFStringEncoding` = `UInt32`, `kCFStringEncodingUTF8 = 0x08000100`. Every parameter order matches the headers. |
| src/peer.rs:168 | `unsafe extern` Security.framework (2 statics + 6 fns) | ok | `SecCSFlags` = `uint32_t`, `OSStatus` = `SInt32`. `SecCodeCreateWithAuditToken` takes `const audit_token_t *` (correct, as the handoff notes). `kSecCodeInfo*` are `extern const CFStringRef`, so declaring them as `static …: CFStringRef` is right. Symbol needs macOS 11+: finding 7. |
| src/peer.rs:217 | `CfRef::drop` → `CFRelease` | ok | Holds today because every `CfRef::new` call site wraps a Create/Copy (+1) result. The invariant is not enforced by the type: finding 1. `CfRef` is neither `Clone` nor `Send`/`Sync`. |
| src/peer.rs:242 | `getsockopt(SOL_LOCAL, LOCAL_PEERTOKEN)` + `audit_token_to_*` | ok | `len` is in/out, bounded by `sizeof(AuditToken)`, and `rc` is checked. The constants 0 and 6 are correct, but libc already exports them: finding 4. `audit_token_to_*` runs before the `len` check (harmless, token zeroed): finding 5. |
| src/peer.rs:278 | `SecCodeCreateWithAuditToken` | ok | The token pointer is valid for the call. The `+1` result is wrapped in `CfRef` only on success, with null rejected. |
| src/peer.rs:307 | `CFStringCreateWithBytes` + `SecRequirementCreateWithString` | ok | `&str` is valid UTF-8, the length is converted with `try_from`, both results are owned (`CfRef`), and `cf_str` drops on every early return. |
| src/peer.rs:343 | `SecCodeCheckValidity(code, 0, req or NULL)` | ok | The requirement is nullable per header. `req_ref` keeps it alive across the call. |
| src/peer.rs:358 | `SecCodeCopyStaticCode` + `SecCodeCopySigningInformation` + dictionary reads | ok | Both Copy results go into `CfRef`. Dictionary values are Get-rule borrows: they are never released and are used only while `dict` lives. `CFGetTypeID` is checked before `CFData`/`CFURL` access. `from_raw_parts` runs only on `len > 0 && !ptr.is_null()`, and the bytes are copied out by `hex_lower` before `dict` drops. The `CFURLGetFileSystemRepresentation` buffer is `PATH_MAX` with the matching length, the call returns false if the path doesn't fit, and the NUL is searched. |
| src/peer.rs:500 | `SecCodeCopySelf` | ok | The `+1` result is wrapped in `CfRef` on success. |
| src/peer.rs:543 | `getsockopt(SOL_SOCKET, SO_PEERPIDFD)` → `OwnedFd::from_raw_fd` | ok (nit) | The kernel installs a fresh `O_CLOEXEC` pidfd, so wrapping it is correct single ownership. It leaks if `len` mismatches: finding 3. |
| src/peer.rs:569 | `syscall(SYS_pidfd_send_signal, pidfd, 0, NULL, 0)` | ok | Argument order matches `pidfd_send_signal(int, int, siginfo_t *, unsigned int)`. The `int` variadics are fine: the kernel's `SYSCALL_DEFINE4` truncates from `long`. The fd is borrowed and outlives the call. |
| src/lifecycle.rs:262 | `pre_exec(\|\| setsid())` | ok | `setsid` and the errno read (`io::Error::last_os_error` → `Repr::Os`, no allocation) are async-signal-safe, with no locks. Unsafe is needed because `CommandExt::setsid` is still unstable. |
| tests/supervise.rs:20 | `kill(pid, 0)` probe | ok | Guarded `raw > 0`. |
| tests/supervise.rs:36 | `kill(-raw, SIGKILL)` guard | ok | Guarded `raw > 1`. |
| tests/supervise.rs:51 | `kill(raw, SIGKILL)` guard | ok | Guarded `raw > 1`. |
| tests/supervise.rs:262 | `kill(descendant_pid as i32, SIGKILL)` | fix (nit) | No guard on the cast: finding 6. |
| tests/peer.rs:119 | `geteuid()` | ok | |
| tests/lifecycle.rs:111 | `geteuid()` | ok | |
| tests/lifecycle.rs:261 | `kill(raw, SIGKILL)` in `DetachKill` | ok (nit) | Guarded `raw > 1`. It kills only the session leader: finding 6. |
| tests/lifecycle.rs:271 | `getsid(pid as i32)` | ok | The pid comes from `Child::id()`, so it is positive. |
| tests/lifecycle.rs:290 | `kill(pid as i32, SIGKILL)` | ok (nit) | Same as 261: finding 6. |
| tests/lifecycle.rs:295 | `kill(pid as i32, 0)` probe | ok | |
| tests/lifecycle.rs:316 | `geteuid()` | ok | |

## Findings

1. [minor] **`CfRef::new` is a safe function, but `Drop`'s `CFRelease` depends on an ownership invariant it doesn't check**
   - Where: `src/peer.rs:200-219`.
   - Problem: `CfRef::new(ptr)` only checks for null. Safe code in the module could wrap a Get-rule pointer, such as a `CFDictionaryGetValue` result or `kSecCodeInfoUnique`, and `Drop` would then over-release it: a use-after-free or a crash. Every current call site is correct, but the soundness argument sits in the `Drop` SAFETY comment instead of at the point where the invariant is established.
   - Fix: make it `unsafe fn from_create_rule(ptr) -> Option<Self>` with a `# Safety` doc ("`ptr` is null or a +1 reference the caller transfers"). All six call sites are already inside `unsafe` blocks, so callers need no other change.

2. [minor] **`signal_group` can SIGKILL the daemon's own process group**
   - Where: `src/supervise.rs:181-199`, used by `terminate` (pgid from `spawn_group`) and `kill_recorded`. `src/supervise.rs:65-71` records `pgid` from the live process table.
   - Problem: the guard excludes only pgid 0 and 1. `spawn_group` records whatever pgid the kernel reports at `identify` time, after `exec`. A child that immediately calls `setpgid(0, getpgid(getppid()))` is allowed to, because it is in the daemon's session, and `ProcessIdentity.pgid` then becomes the daemon's group. `terminate` would then `kill(-daemon_pgid, SIGKILL)`, killing the daemon and anything sharing its group. This is not memory-unsafe, and it needs a hostile same-UID child, but it is exactly the "never signal the wrong group" property that §10 relies on.
   - Fix: in `spawn_group`, fail (and kill the child) if `identity.pgid != identity.pid`, since the contract says they're equal for processes started here. In `signal_group`, also refuse `raw_pgid == getpgrp()`, with one more documented `unsafe { libc::getpgrp() }`.

3. [nit] **`peer_pidfd` leaks the pidfd when `len` comes back wrong, and the SAFETY comment says it never does**
   - Where: `src/peer.rs:557-560`.
   - Problem: if `rc == 0 && raw_pidfd >= 0 && len != expected_len`, the fd the kernel installed is never closed. The kernel always returns `sizeof(int)`, so this can't happen in practice, but the comment says ownership is transferred "immediately".
   - Fix: wrap first: `if raw_pidfd >= 0 { let fd = OwnedFd::from_raw_fd(raw_pidfd); if len != expected_len { return Err(..) } return Ok(fd) }`.

4. [nit] **`SOL_LOCAL` and `LOCAL_PEERTOKEN` are hard-coded though libc exports them**
   - Where: `src/peer.rs:96-97`.
   - Problem: the values (0, 0x006) are correct, but libc 0.2.191 already exports `libc::SOL_LOCAL` and `libc::LOCAL_PEERTOKEN` for Apple targets. Hand copies drift.
   - Fix: use the libc constants. Keep the local `AuditToken`, because libc has no `audit_token_t`.

5. [nit] **Two SAFETY comments are incomplete, and one call is out of order**
   - Where: `src/peer.rs:238-266`, `src/supervise.rs:260-276`.
   - Problem:
     - `audit_token_to_euid/pid` are called before `len` is validated. This is harmless because the token was zeroed, but it reads a possibly short token.
     - The `peek_exited` comment doesn't mention that on Linux `si_pid()` reads a union field, and that this is only meaningful because the struct is zeroed and `waitid` fills the SIGCHLD variant.
   - Fix: check `len` before the `audit_token_to_*` calls. Add one sentence to the `peek_exited` comment.

6. [nit] **Test `kill` calls: one unguarded cast, and one helper process left behind**
   - Where: `tests/supervise.rs:262`, `tests/lifecycle.rs:255-292`.
   - Problem:
     - `libc::kill(descendant_pid as i32, SIGKILL)` skips the `> 1` guard that `KillPidGuard` uses. A parse of `0` would SIGKILL the test runner's own process group, and a value above `i32::MAX` would wrap negative.
     - Separately, the `spawn_detached` test kills only the `sh` session leader, so `sleep 30` survives orphaned in the new session. After `cargo test -p atrium-host` I found two `sleep 30` orphans (ppid 1, pgid = the dead `sh`); they expired after 30 s.
   - Fix: kill through the guarded helper. The detached child leads its own session and process group, so in `DetachKill` and the explicit kill, use `kill(-pid, SIGKILL)` (with `pid > 1`).

7. [nit] **`SecCodeCreateWithAuditToken` needs macOS 11 and is linked unconditionally**
   - Where: `src/peer.rs:173`.
   - Problem: on `x86_64-apple-darwin` the default deployment target is below 11, so a binary built that way fails to launch on 10.15 with a dyld missing-symbol error. On `aarch64` the minimum is already 11.
   - Fix: pin `MACOSX_DEPLOYMENT_TARGET=11.0` (or document macOS 11 as the minimum), if Intel/10.15 is ever a target.

Outside this audit's scope, for the behaviour reviewer: `MacUnsigned` reads `kSecCodeInfoUnique` from `SecCodeCopyStaticCode(code)`, which is the static (on-disk) code object. Whether that equals the kernel's cdhash, or Security.framework rejects a mismatch with `errSecCSStaticCodeChanged`, is what the "replace the binary on disk" test is meant to show. Either way the check fails closed, and there is no memory-safety consequence.
