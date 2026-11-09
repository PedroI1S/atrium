# Handoff: M1-10-host

Model: Gemini 4 Argon · Status: done

## Done

- Implemented `crates/atrium-host/src/supervise.rs` per contract §10:
  - `spawn_group`: spawns `command` as the leader of a new process group (`process_group(0)`) with `kill_on_drop(true)` and records `ProcessIdentity` (`pid`, `pgid`, `start_time`, `executable`), handling ultra-fast-exiting children that become zombies before their executable vnode can be read.
  - `terminate`: waits up to `grace` for the leader using non-reaping `waitid(..., WEXITED | WNOHANG | WNOWAIT)` so `tokio::process::Child` remains sole reaper, sends `SIGKILL` to `-pgid` **before** reaping the leader (pinning `pgid` against reuse, and killing surviving descendants even when the leader exited early), reaps the leader, and waits for the group to drain (`ESRCH`). Handles macOS XNU `killpg1` returning `EPERM` when a group contains only unreaped zombies.
  - `identify`: reads `proc_pidinfo(PROC_PIDTBSDINFO)` + `proc_pidpath` on macOS (returning `None` on `ESRCH` or `SZOMB`), and `/proc/<pid>/stat` (field 3 state, field 5 `pgrp`, field 22 `starttime`) + `/proc/<pid>/exe` on Linux (returning `None` on `NotFound`/`ESRCH` or dead `'Z' | 'X' | 'x'` states).
  - `kill_recorded`: verifies `live == *recorded` before signalling `-pgid` with `SIGKILL`; when the leader is gone (`identify` returns `None`), returns `Cleanup::Verified` only if `kill(-pgid, 0)` fails with `ESRCH`, and `Cleanup::Unverified` without killing anything if any process still occupies `pgid` or if `pid` was reused by another executable or start time.
- Implemented `crates/atrium-host/src/peer.rs` per contract §4 and D-045:
  - `verify_app_peer`: checks `ucred.uid() == geteuid()` first (`PeerError::OtherUser`), then delegates to platform peer verification with zero partial-success fallbacks.
  - macOS: extracts kernel `audit_token_t` via `getsockopt(SOL_LOCAL, LOCAL_PEERTOKEN)`, verifies `audit_token_to_euid`, creates the live kernel code object via `SecCodeCreateWithAuditToken`, validates designated requirements (`MacSigned`) via `SecCodeCheckValidity`, and for `MacUnsigned` validates structural signature validity and compares the code object's `kSecCodeInfoUnique` cdhash + `kSecCodeInfoMainExecutable` (never hashing the file on disk).
  - Linux: reads `SO_PEERCRED` PID and pins the peer via `getsockopt(SOL_SOCKET, SO_PEERPIDFD)` (refusing `App` authority if `SO_PEERPIDFD` is unavailable — no `pidfd_open` fallback), inspects `/proc/<pid>/exe` path + `(dev, ino)` (`dev_ino: Some` pins device/inode; `dev_ino: None` verifies the binary and all ancestors on both nominal and canonical paths are UID 0 and non-writable by group/others), then calls `syscall(SYS_pidfd_send_signal, pidfd, 0, NULL, 0)` to prove the inspected `/proc/<pid>` is still the exact process pinned by `SO_PEERPIDFD`.
  - `current_app_identity`: returns `AppIdentity::MacUnsigned` (via `SecCodeCopySelf` + `kSecCodeInfoUnique`) on macOS and `AppIdentity::Linux { executable, dev_ino: Some(...) }` (from `/proc/self/exe`) on Linux.
- Implemented `crates/atrium-host/src/lifecycle.rs` per contract §10 and PLAN §3.1:
  - `default_paths`: populates all `Paths` fields (`ATRIUM_HOME` honoured only under `#[cfg(debug_assertions)]`; `~/Library/Application Support/Atrium` on macOS; `$XDG_RUNTIME_DIR/atrium` or validated `/run/user/<uid>/atrium` fallback + `$XDG_DATA_HOME/atrium` on Linux).
  - `secure_dir`: creates missing directories mode `0700` (enforcing `0700` via `O_DIRECTORY | O_NOFOLLOW` + `fchmod`), and refuses symlinks, non-directories, other UIDs, or modes other than `0700`.
  - `InstanceLock::acquire`: validates `runtime_dir` via `secure_dir`, opens `paths.lock` mode `0600` with `O_NOFOLLOW`, acquires non-blocking exclusive `flock` (`File::try_lock`), reports `AlreadyRunning(holder_pid)` without touching `paths.socket` if contended, writes the daemon's `ProcessIdentity` JSON, and removes any stale `paths.socket` strictly **after** acquiring the lock (`Drop` unlinks `socket` while `_file` still holds the `flock`).
  - `spawn_detached`: opens/creates the log file with `O_NOFOLLOW` and mode `0600` (refusing symlinks, other owners, or looser modes on existing files), calls `setsid()` in `pre_exec`, redirects stdin to `/dev/null` and stdout/stderr to the private log, and reaps in a background thread.
  - `install_keep_running` / `remove_keep_running`: atomically writes/removes `~/Library/LaunchAgents/dev.atrium.daemon.plist` on macOS and `$XDG_CONFIG_HOME/systemd/user/atriumd.service` (+ `default.target.wants/atriumd.service` symlink) on Linux without touching any sibling launchd or systemd files.
- Added `libc = "0.2" # M1-10` in `crates/atrium-host/Cargo.toml` (already present in `Cargo.lock` at `0.2.191`; zero new crates downloaded).
- Added integration and unit tests in `crates/atrium-host/tests/{supervise,peer,lifecycle}.rs` and `src/{supervise,lifecycle}.rs` covering every Definition of Done item.

## Not done

- Live Linux kernel execution of `#[cfg(target_os = "linux")]` runtime paths (runs on rodrigo's shift; see platform breakdown below).
- Creating a brand-new `0700` directory owned by a non-root other UID in `secure_dir` tests (requires `CAP_CHOWN` / root; see notes below).

## Definition of Done results

```text
$ cargo fmt --all --check
(exit 0, no output)

$ cargo clippy --workspace --all-targets -- -D warnings
    Checking atrium-host v0.0.0 (/Users/pedro/atrium-wt/M1-10-host/crates/atrium-host)
    Checking atrium-agents v0.0.0 (/Users/pedro/atrium-wt/M1-10-host/crates/atrium-agents)
    Checking atrium-core v0.0.0 (/Users/pedro/atrium-wt/M1-10-host/crates/atrium-core)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.08s

$ cargo test -p atrium-host
     Running unittests src/lib.rs
running 2 tests
test supervise::unit_tests::linux_stat_parser_handles_spaces_and_parens_in_comm ... ok
test lifecycle::unit_tests::launchd_plist_and_systemd_unit_preserve_unrelated_files ... ok
test result: ok. 2 passed; 0 failed

     Running tests/lifecycle.rs
running 4 tests
test secure_dir_creates_0700_and_refuses_symlink_0755_and_other_owner ... ok
test instance_lock_records_identity_removes_only_stale_socket_and_recovers_after_crash ... ok
test spawn_detached_creates_new_session_and_enforces_private_log_0600_nofollow ... ok
test default_paths_and_keep_running_service_use_temporary_home_and_xdg_only ... ok
test result: ok. 4 passed; 0 failed

     Running tests/peer.rs
running 3 tests
test test_binary_verifying_itself_succeeds ... ok
test different_executable_connecting_after_exec_fails_with_not_app ... ok
test macos_requirement_check_and_in_kernel_code_object_cdhash ... ok
test result: ok. 3 passed; 0 failed

     Running tests/supervise.rs
running 6 tests
test kill_recorded_returns_unverified_for_reused_pid_and_never_kills_it ... ok
test fast_exiting_process_records_identity_and_terminates_cleanly ... ok
test gone_leader_with_live_descendant_in_group_is_not_verified ... ok
test leader_exiting_early_still_has_descendants_killed ... ok
test child_spawning_grandchildren_is_fully_gone_after_terminate ... ok
test kill_recorded_kills_matching_live_orphan_group ... ok
test result: ok. 6 passed; 0 failed

$ cargo test -p atrium-host --release
test result: ok. 2 unit + 4 lifecycle + 3 peer + 6 supervise passed; 0 failed
(also verified release builds ignore ATRIUM_HOME in default_paths)
```

### Audit of every `unsafe` block in `crates/atrium-host`

#### Library (`crates/atrium-host/src/`)

1. `src/lib.rs:17` (`current_euid`): calls `libc::geteuid()`. Takes no arguments, has no memory-safety preconditions, thread-safe on all POSIX targets.
2. `src/supervise.rs:193` (`signal_group`): calls `libc::kill(-raw_pgid, sig)`. Guarded by `i32::try_from(pgid)` and `raw_pgid > 1`, so `-raw_pgid <= -2` cannot overflow `i32` and never targets `0` or `-1`.
3. `src/supervise.rs:263-276` (`peek_exited`): zero-initializes POD `libc::siginfo_t`, calls `libc::waitid(P_PID, raw_pid as id_t, &mut info, WEXITED | WNOHANG | WNOWAIT)` with validated `raw_pid > 0`, and reads `info.si_pid` / `info.si_pid()`. `WNOWAIT` leaves the zombie unreaped for `tokio::process::Child`.
4. `src/supervise.rs:352-362` (`proc_bsd_info`, macOS): zero-initializes POD `libc::proc_bsdinfo` and passes `(&raw mut info).cast()` with exact `size_of::<proc_bsdinfo>()` to `libc::proc_pidinfo(raw_pid, PROC_PIDTBSDINFO, 0, ...)`, verifying `ret >= size` before reading.
5. `src/supervise.rs:384-390` (`proc_exe_path`, macOS): passes a `PROC_PIDPATHINFO_MAXSIZE` (4096-byte) mutable stack buffer and its exact length to `libc::proc_pidpath`, clamping `ret` to `buf.len()`.
6. `src/peer.rs:129-132` (`unsafe extern "C"`, macOS `libbsm`): FFI declarations for `audit_token_to_euid` and `audit_token_to_pid` matching `<bsm/libbsm.h>` (`audit_token_t` is `unsigned int val[8]` passed by value).
7. `src/peer.rs:138-162` (`unsafe extern "C"`, macOS `CoreFoundation.framework`): FFI declarations for `CFRelease`, `CFGetTypeID`, `CFDataGetTypeID`, `CFURLGetTypeID`, `CFDataGetBytePtr`, `CFDataGetLength`, `CFDictionaryGetValue`, `CFStringCreateWithBytes`, and `CFURLGetFileSystemRepresentation` matching LP64 macOS headers.
8. `src/peer.rs:168-198` (`unsafe extern "C"`, macOS `Security.framework`): FFI declarations for `kSecCodeInfoUnique`, `kSecCodeInfoMainExecutable`, `SecCodeCopySelf`, `SecCodeCreateWithAuditToken`, `SecCodeCopyStaticCode`, `SecCodeCheckValidity`, `SecCodeCopySigningInformation`, and `SecRequirementCreateWithString`.
9. `src/peer.rs:217` (`CfRef::drop`, macOS): calls `CFRelease(self.0)` once on drop; `CfRef::new` rejects null pointers and only wraps owned Create/Copy references.
10. `src/peer.rs:242-260` (`peer_audit_token`, macOS): calls `libc::getsockopt(fd, SOL_LOCAL, LOCAL_PEERTOKEN, ...)` into a stack `AuditToken` bounded by `size_of::<AuditToken>()` and extracts `euid`/`pid` via `audit_token_to_euid`/`audit_token_to_pid` only when `rc == 0` and `len == expected_len`.
11. `src/peer.rs:278-296` (`sec_code_from_token`, macOS): passes a valid `*const AuditToken` to `SecCodeCreateWithAuditToken` and wraps `code_raw` in `CfRef` on `ERR_SEC_SUCCESS`.
12. `src/peer.rs:307-333` (`check_code_validity`, macOS): creates an owned `CFStringRef` from UTF-8 bytes via `CFStringCreateWithBytes` and compiles an owned `SecRequirementRef` via `SecRequirementCreateWithString`, both RAII-managed by `CfRef`.
13. `src/peer.rs:343` (`check_code_validity`, macOS): calls `SecCodeCheckValidity` with live `code.as_ptr()` and optional `raw_req` kept alive across the call by `req_ref`.
14. `src/peer.rs:358-423` (`extract_signing_details`, macOS): copies static code and signing dictionary (`CfRef`-owned), checks `CFGetTypeID` before reading `CFData` (`kSecCodeInfoUnique` -> slice bounded by `CFDataGetLength`) or `CFURL` (`kSecCodeInfoMainExecutable` -> `PATH_MAX` stack buffer via `CFURLGetFileSystemRepresentation`).
15. `src/peer.rs:500-509` (`current_identity`, macOS): calls `SecCodeCopySelf` and wraps the owned `SecCodeRef` in `CfRef`.
16. `src/peer.rs:543-561` (`peer_pidfd`, Linux): calls `libc::getsockopt(fd, SOL_SOCKET, SO_PEERPIDFD, ...)` on a live `BorrowedFd`; when `rc == 0`, `len == size_of::<c_int>()`, and `raw_pidfd >= 0`, immediately transfers ownership of the new kernel pidfd to `OwnedFd::from_raw_fd`.
17. `src/peer.rs:569-576` (`verify_pidfd_alive`, Linux): calls `libc::syscall(SYS_pidfd_send_signal, pidfd, 0, NULL, 0)` on a live `BorrowedFd` with typed `c_int`/`*const c_void`/`c_uint` variadic arguments.
18. `src/lifecycle.rs:262-269` (`spawn_detached`): registers `cmd.pre_exec` closure calling only async-signal-safe `libc::setsid()`, with no allocations or locks between `fork` and `exec`.

#### Integration tests (`crates/atrium-host/tests/`)

19. `tests/supervise.rs:20`, `36-38`, `51-53`, `262-264`: `libc::kill` for `ESRCH` existence probing (`sig = 0`) and test child/group `SIGKILL` cleanup guards (`raw > 1`).
20. `tests/peer.rs:119`: `libc::geteuid()` to assert `PeerInfo.uid`.
21. `tests/lifecycle.rs:111`, `261-263`, `271`, `290-292`, `295`, `316`: `libc::geteuid()` for non-root test gating, `libc::getsid(pid)` to assert new session leadership after `spawn_detached`, and `libc::kill` for detached child cleanup/existence check.

### Platform test coverage (macOS vs Linux)

- **Tested on this machine (macOS)**:
  - All `supervise` paths on macOS (`proc_pidinfo`, `proc_pidpath`, `waitid(WNOWAIT)`, XNU zombie-group `EPERM` handling, `terminate`, `identify`, `kill_recorded`), plus the pure Linux `/proc/<pid>/stat` parser unit test (`linux_stat_parser_handles_spaces_and_parens_in_comm`).
  - All macOS `peer` paths (`LOCAL_PEERTOKEN`, `SecCodeCreateWithAuditToken`, `SecCodeCheckValidity` with matching and non-matching requirements, `kSecCodeInfoUnique` in-kernel cdhash verification even after replacing the binary on disk, post-`exec` helper rejection, cross-OS identity rejection).
  - All cross-platform `lifecycle` paths (`secure_dir`, `InstanceLock::acquire` + crash recovery, `spawn_detached` + `setsid` + `0600`/`O_NOFOLLOW` log checks, debug vs release `ATRIUM_HOME`), macOS `default_paths` and `LaunchAgent` plist install/remove in a temporary `HOME`, plus the cross-platform `install_systemd_unit` / `remove_systemd_unit` unit test.
  - `secure_dir` other-owner check: tested without root against existing UID-0 directories (`/usr` and `/private/var/root`); `secure_dir` checks `meta.uid() != my_uid` before checking permissions. Creating a fresh `0700` directory owned by an arbitrary non-root UID cannot be tested without `CAP_CHOWN` / root.
- **Linux-only and untested at runtime on this machine (for rodrigo's shift)**:
  - `supervise`: live `/proc/<pid>/stat` and `/proc/<pid>/exe` reads in `platform_identify` and `platform_zombie_timings`, and Linux `siginfo_t::si_pid()` in `peek_exited`.
  - `peer`: live `SO_PEERCRED` + `SO_PEERPIDFD` + `/proc/<pid>/exe` + `check_root_owned_tree` (`dev_ino: None`) + `SYS_pidfd_send_signal(0)` (requires Linux kernel 6.5+; `linux_release_identity_refuses_user_writable_path_and_wrong_os_identity` in `tests/peer.rs` is wired and ready to run on Linux).
  - `lifecycle`: live `default_platform_paths` (`XDG_RUNTIME_DIR` / `/run/user/<uid>` / `XDG_DATA_HOME`) and `install_keep_running` / `remove_keep_running` dispatch on Linux (`tests/lifecycle.rs` already covers the Linux branch under `#[cfg(target_os = "linux")]`).

## Gotchas

- **XNU `killpg1` zombie-group `EPERM`**: On macOS (`bsd/kern/kern_sig.c`), when `pgfind(pgid)` finds a process group whose only remaining members are unreaped `SZOMB` processes (`nfound == 0`), `kill(-pgid, SIGKILL)` returns `EPERM` instead of `0` or `ESRCH`. `supervise::terminate` checks `peek_exited(pid)` on macOS `EPERM`, reaps the zombie leader, and then verifies `group_is_empty(pgid)` (`ESRCH`), surfacing `EPERM` if any non-zombie process actually remained.
- **`SecCodeCreateWithAuditToken` parameter ABI**: Despite some third-party wrappers describing the first parameter as a `CFDataRef`, `<Security/SecCode.h>` takes `const audit_token_t *` directly; passing a `CFDataRef` fails with `OSStatus 100003` (`ESRCH`).

## Follow-ups

- Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test -p atrium-host` on a Linux 6.5+ host during rodrigo's shift to exercise `SO_PEERPIDFD`, `SYS_pidfd_send_signal`, `/proc/<pid>` inspection, and XDG path/systemd unit integration tests.
