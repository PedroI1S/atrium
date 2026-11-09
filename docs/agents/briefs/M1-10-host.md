# M1-10-host: process supervision, peer verification, daemon lifecycle

Model: Gemini 4 Argon · Reviewer: Sonnet 5.5, then Opus 5.5 for every `unsafe` block · Depends on: contract (M1 rev 1)
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
- `supervise`: `spawn_group` (new process group, `kill_on_drop`, identity recorded), `terminate` (grace, then `SIGKILL` to the group, reap; also kills leftover descendants when the leader exited), `identify`, `kill_recorded` (verify pid + start time + executable before killing; `Unverified` otherwise).
- `peer`: `verify_app_peer` for macOS (audit token via `LOCAL_PEERTOKEN`, `SecCodeCreateWithAuditToken`, `SecCodeCheckValidity` against the requirement; unsigned builds: executable path + cdhash) and Linux (`SO_PEERCRED`, a pidfd via `SO_PEERPIDFD` when the kernel has it, `/proc/<pid>/exe` device + inode). Always check the peer's uid first. `current_app_identity`.
- `lifecycle`: `default_paths`, `secure_dir`, `InstanceLock::acquire` (lock taken before a stale socket is removed), `spawn_detached`, `install_keep_running`/`remove_keep_running` (launchd LaunchAgent / systemd user unit; never touch anything else in the user's launchd or systemd config).
- Every `unsafe` block has a `// SAFETY:` comment; keep them few and small.

## Definition of Done
- [ ] Tests: a child that spawns grandchildren is fully gone after `terminate` (check by pid); a leader that exits early still has its descendants killed; `kill_recorded` returns `Unverified` for a reused pid (simulate with a different executable) and never kills it.
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
