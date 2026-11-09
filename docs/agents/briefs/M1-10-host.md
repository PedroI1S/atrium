# M1-10-host: process supervision, peer verification, daemon lifecycle

Model: Gemini 4 Argon · Reviewer: Sonnet 5.5, then Opus 5.5 for every `unsafe` block · Depends on: contract (M1 rev 2)
Branch: `agent/argon/M1-10-host` · Worktree: `../atrium-wt/M1-10-host`

## Goal
Implement `crates/atrium-host`: the platform code everything else relies on to kill what it started, to know who is on the other end of the daemon socket, and to run exactly one daemon in a private directory. This is security-sensitive (D-045).

## Context
- `docs/contracts/m1.md` §4 (authority, peer verification, in full) and §10 (lifecycle, supervision).
- `crates/atrium-host/src/{supervise,peer,lifecycle}.rs`: the signatures you fill in.
- Prior art: `spikes/daemon-socket/` (M0-05: private dir, owner/mode checks, lock before cleanup, PID/start/executable identity) and `spikes/browser-screencast/src-tauri/src/profile.rs`.
- `docs/agents/reviews/M0-gate.md` §2 ("socket permissions protect against other users, not host agents", "reaping is improved, but termination guarantees remain incomplete").
- Decision D-045.

## Owned paths
- `crates/atrium-host/src/**`, `crates/atrium-host/tests/**`
- `crates/atrium-host/Cargo.toml`: dependency lines only, each with `# M1-10` (e.g. `libc`, `nix`, `security-framework`)
- `docs/agents/handoffs/M1-10-host.md`

## Contract to implement
- `supervise`: exactly as documented. `spawn_group` (new process group, `kill_on_drop`, identity recorded); `terminate` (grace, then `SIGKILL` to the group **before** reaping the leader, so the group id can't be reused; also kills leftover descendants when the leader exited); `identify`; `kill_recorded` (a gone leader is `Verified` only if its group is empty; remaining processes that can't be shown to descend from it → `Unverified`, nothing killed).
- `peer`: exactly as documented on `AppIdentity` and `verify_app_peer`. macOS: audit token, `SecCodeCheckValidity` against the requirement compiled into release daemons; unsigned dev builds compare the code object's cdhash (`kSecCodeInfoUnique`), never a hash of the file. Linux: uid, pidfd, `stat` of `/proc/<pid>/exe`, then `pidfd_send_signal(pidfd, 0)`; release builds check the packaged app path isn't writable by the user. `current_app_identity` for dev builds.
- `lifecycle`: `default_paths` (every field of `Paths`, `ATRIUM_HOME` honoured only in debug builds), `secure_dir`, `InstanceLock::acquire` (lock taken before a stale socket is removed; the lock file holds the daemon's `ProcessIdentity` as JSON), `spawn_detached` (log `O_NOFOLLOW`, mode 0600, refused if it exists with another owner or looser mode), `install_keep_running`/`remove_keep_running` (launchd LaunchAgent / systemd user unit; never touch anything else in the user's launchd or systemd config). Signal handling in `atriumd` is M1-05's.
- Every `unsafe` block has a `// SAFETY:` comment; keep them few and small.

## Definition of Done
- [ ] Tests: a child that spawns grandchildren is fully gone after `terminate` (check by pid); a leader that exits early still has its descendants killed; `kill_recorded` returns `Unverified` for a reused pid (simulate with a different executable) and never kills it; a gone leader with a live descendant in its group is not `Verified`.
- [ ] Peer tests: the test binary verifying itself with its own identity succeeds; a different executable (`/bin/cat` connected through a socketpair helper) fails with `NotApp`.
- [ ] `secure_dir` refuses a symlink, a 0755 directory, and a directory owned by someone else (where testable without root; say which weren't).
- [ ] Install/remove of the keep-running service tested against a temporary `HOME`/`XDG_CONFIG_HOME`, never the real one.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-host`
- [ ] Handoff with outputs, a list of every `unsafe` block with its justification, and what is macOS-only or Linux-only and untested on this machine (Linux runs on rodrigo's shift).

## Out of scope
The daemon's socket server and handshake (M1-05 calls `verify_app_peer`), signing the app (packaging, later), Windows.

## Escalate if
Peer verification can't be done reliably on a platform without root or entitlements: stop and report what's possible (this is a D-045 decision, so Pedro decides). Two fix rounds without passing.
