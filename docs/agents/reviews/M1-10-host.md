# Review: M1-10-host
Reviewer: Sonnet 5.5 (read-only) · 2026-11-09

## Verdict
**approve with changes**: the peer, supervision and lifecycle rules are implemented as the contract says and the tests exercise real processes and sockets, but the macOS peer check rests on a Security symbol that is not in the public SDK headers.

Ran on macOS: `cargo test -p atrium-host` (15 tests pass) and `cargo clippy --workspace --all-targets -- -D warnings` (clean). No stray processes left. Linux paths were read only (no Linux target installed).

## Findings

1. [major] **`SecCodeCreateWithAuditToken` is not a documented Security API**
   Where: crates/atrium-host/src/peer.rs:168-198 (declaration), :274-297 (use); handoff "Gotchas".
   Problem: the symbol is exported by Security.framework (it is in the SDK `.tbd`) but is absent from the public `SecCode.h`; the handoff's claim that the header declares `const audit_token_t *` is wrong. The documented way to get a code object from an audit token is `SecCodeCopyGuestWithAttributes(NULL, {kSecGuestAttributeAudit: CFData(token)}, flags, &code)` (`kSecGuestAttributeAudit` is in the header). Linking an undocumented symbol directly means an older or newer macOS lacking it fails at dyld load (the whole daemon, not just the check), and the ABI can change silently. It works on this machine (macOS 27.0.1) only.
   Fix: use `SecCodeCopyGuestWithAttributes` with the audit attribute (the tests already cover the behaviour), or at minimum resolve the symbol with `dlsym` and refuse App authority when it is missing. Record in the handoff/contract that §4 names the symbol informally. If Pedro prefers to keep the contract wording, this is his call (D-045).

2. [minor] **`kill_recorded` reports `Verified` without confirming the group drained**
   Where: crates/atrium-host/src/supervise.rs:175-176.
   Problem: after `SIGKILL` it waits up to 500 ms and returns `Verified` regardless of the outcome. A group member that is still alive (uninterruptible state, or a zombie no init reaps, e.g. a container without a reaping PID 1) is then shown as cleaned up, which §10 forbids ("never shown as stopped").
   Fix: after the wait, return `Verified` only if `group_is_empty(pgid)`; otherwise `Unverified`. Same for `terminate` (supervise.rs:117-121), which ignores a group that is still non-empty on Linux (only the macOS zombie `EPERM` case checks).

3. [minor] **`terminate` skips the group kill when the leader was already reaped by the caller**
   Where: crates/atrium-host/src/supervise.rs:86-92.
   Problem: if the adapter already awaited `child.wait()` (tokio then reports `id() == None`), `terminate` returns without touching the group, so descendants of an early-exited leader survive. Killing is correctly avoided (the pgid may be reused), but this contradicts "also kills leftover descendants when the leader exited" for that caller pattern, and nothing documents it.
   Fix: document on `terminate` that the caller must not reap the leader first (M1-05/adapters call `terminate` instead of `wait`), or have `SupervisedChild` own the only wait path.

4. [minor] **`kill_recorded` blocks the thread for up to 500 ms**
   Where: crates/atrium-host/src/supervise.rs:175, :219-228.
   Problem: it is a sync fn with `thread::sleep` polling; called from the daemon's async recovery it would stall a tokio worker (once per recorded thread).
   Fix: document "call from `spawn_blocking`", or shorten and make it async.

5. [minor] **Keep-running install/remove only edit files**
   Where: crates/atrium-host/src/lifecycle.rs:650-687.
   Problem: the launchd plist is written but never bootstrapped, and removal never boots it out, so turning the setting off leaves the job loaded (and `KeepAlive` restarting the daemon on failure) until logout. systemd gets a hand-made `default.target.wants` symlink but no `daemon-reload`/`enable --now`. Scope is correct (only Atrium's own file names are touched), but the toggle's behaviour is "next login".
   Fix: either run `launchctl bootout gui/<uid>/dev.atrium.daemon` / `systemctl --user disable --now atriumd` on remove (and the matching load on install), or state in the handoff and Settings text that it takes effect at next login.

6. [nit] **Log directory is not checked with `secure_dir`**
   Where: crates/atrium-host/src/lifecycle.rs:391-399.
   Problem: only a missing parent is created 0700; an existing, looser logs dir is accepted. The per-file owner/mode/`O_NOFOLLOW` checks still hold, so impact is small.
   Fix: call `secure_dir(parent)` when the log lives under `Paths::logs_dir`.

## To check live on Linux
- Kernel 6.5+: `SO_PEERPIDFD` returns a pidfd; on an older kernel `verify_app_peer` returns `Inspect` (App refused), never a fallback.
- `libc::SO_PEERPIDFD` and `SYS_pidfd_send_signal` compile on glibc and musl targets (the whole `cfg(linux)` side has not been compiled here, including `siginfo_t::si_pid()`).
- `tests/peer.rs::linux_release_identity_refuses_user_writable_path_and_wrong_os_identity`, plus a positive release case with a root-owned copy of a test binary (e.g. under `/usr/local/lib`, run via sudo setup) to prove `check_root_owned_tree` accepts a correct install and rejects a user-writable parent.
- A peer that has exited but is not yet reaped (zombie): `/proc/<pid>/exe` read fails, so the check refuses; a reaped one fails `pidfd_send_signal`.
- `identify` on Linux: `/proc/<pid>/stat` state `Z`/`X` returns `None`; start time (field 22) stable; `/proc/<pid>/exe` of a deleted binary ends in " (deleted)" and must still compare equal across calls.
- `waitid(WNOWAIT)` in `terminate` and the zombie-leader/orphan-descendant tests (reparenting to a subreaper or PID 1; in a container without a reaping init, zombie descendants keep the group non-empty, see finding 2).
- `default_paths` with and without `XDG_RUNTIME_DIR`; systemd user unit install under a temporary `XDG_CONFIG_HOME` is loadable (`systemd-analyze --user verify`).
