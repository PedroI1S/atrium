# M1-14a-linux-host: Linux checks for the host crate and the daemon

Model: Sonnet 5.5 · Reviewer: Gemini 4 Argon (read-only) · Depends on: M1-05, M1-10 (merged)
Go: D-059 (rodrigo), D-066 · Branch: `agent/sonnet/M1-14a-linux-host` · Worktree: `../atrium-wt/M1-14a-linux-host`

M1-14 (contract §15) is split in two so that this half doesn't wait for M1-09b: **M1-14a** (this brief) has no GUI and touches no app file; **M1-14b** covers the app (L1, L2, L3, M1-09a's scripts) and starts after M1-09b is merged.

## Goal
Close what M1 still owes on Linux below the app: the four findings of the review of M1-10's Linux fix, a live run of M1-10's Linux paths, and the L8 check of the daemon's idle memory with contract §14's statistics. This machine is Linux (kernel 7.3, CachyOS).

## Context
- `docs/agents/reviews/M1-10-host-linux-exit.md`: the four findings (three minor, one nit), each with its fix.
- `crates/atrium-host/src/supervise.rs` (`identify`, `identify_spawned`, `kill_recorded`, `parse_linux_proc_stat`), `crates/atrium-host/tests/supervise.rs`.
- `docs/contracts/m1.md` §4 (peer verification on Linux: `SO_PEERCRED`, `SO_PEERPIDFD`, `/proc/<pid>/exe`), §10 (lifecycle, cleanup `Verified`/`Unverified`), §14 (the row "Daemon idle RSS < 50 MB" and how it is measured).
- `docs/agents/m1-must-fix.md`, "From the M0 Linux checks": L8. `docs/spikes/M0-linux-checks.md` for the M0 numbers.
- `docs/agents/handoffs/M1-05-daemon.md` and `crates/atrium-daemon/examples/perf.rs`: how the daemon's §14 numbers were taken (one run, before the review fixes: idle RSS 19.4 MiB).
- `docs/agents/handoffs/M1-10-host.md`: which Linux paths were only read, never run.

## Owned paths
- `crates/atrium-host/src/supervise.rs` and `crates/atrium-host/tests/supervise.rs`: the four findings only.
- `crates/atrium-daemon/examples/perf.rs`: only what §14's statistics need (10 samples, 60 s or more after the last activity), if it doesn't do that yet.
- `docs/spikes/M1-linux-checks.md` (new): the report. Write the sections "Host" and "Daemon"; leave a heading "App (M1-14b)" empty.
- `docs/agents/handoffs/M1-14a-linux-host.md`
- Not yours: `crates/atrium-host/tests/adversarial_*.rs` and `crates/atrium-daemon/tests/adversarial_*.rs` (M1-13 is writing them now), `peer.rs`, `lifecycle.rs`, anything under `app/`, the contract. A bug outside your paths is a finding in the handoff with the failing output.

## What to do
1. **The four review findings**, as the review proposes:
   - 1: on `ENOENT` with a live state, return `None` only for an exiting task (`PF_EXITING`) and treat a kernel thread (`PF_KTHREAD`) as "not a process"; anything else is `Err`, as before the fix.
   - 2: after a successful `read_link`, and on the `ENOENT` path, re-read stat and require the same `start_time`; otherwise `Err` (or one retry).
   - 3: move the decision into a pure function that compiles on every platform and unit-test its table (the review lists the cases); run the existing fast-exit test in a loop inside the test.
   - 4: fix the two comments.
   No change may turn an `Err` (`Unverified`) into an identity or into `Verified`.
2. **Live run of the Linux paths.** Run `cargo test -p atrium-host` and `cargo test -p atrium-daemon` 30 times each in a row on this machine and report failures with their output (a flaky test is a finding, not something to retry away). List which Linux-only branches of `peer.rs`, `supervise.rs` and `lifecycle.rs` no test reaches (read the code and the tests; `cfg(target_os = "linux")` blocks), and say whether `SO_PEERPIDFD` is exercised.
3. **L8.** Run the daemon's perf example on Linux as §14 says: after a 3M-token session is closed, the median of 10 RSS samples taken 60 s or more after the last activity; also the peak right after load and after close, and the other numbers the example prints (reconnect with 1,000 events behind, 100k replay). Three full runs. Gate: median < 50 MB.

## Definition of Done
- [ ] The four findings fixed, with the unit-test table for finding 3.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-host -p atrium-daemon`, and the 30-run loops with their result.
- [ ] `docs/spikes/M1-linux-checks.md` with the Host and Daemon sections: commands, numbers, the unreached Linux branches.
- [ ] Handoff with the outputs.

## Rules
- Run every `cargo` command of this task on the host, not in the agent's container, so the numbers are the machine's: `distrobox-host-exec sh -c 'export PATH=$HOME/.cargo/bin:$PATH; cd <worktree> && cargo …'`. `git` and `python3` exist only on the host too.
- No test data in shell heredocs (worker rule 6). Loops go in a script file written with the file tools, or in a Rust test.
- Stop every daemon and process you start. Other workers are building on this machine, so take memory and timing numbers when your own builds are finished, and say in the report what else was running.

## Out of scope
The app, GUI runs, L1, L2, L3 (M1-14b). Codex's live checks X1–X3 (M1-08L). New adversarial tests (M1-13).

## Escalate if
A finding's fix needs a change outside `supervise.rs`; the daemon misses the 50 MB gate; a test fails on Linux for a reason that isn't the test's.
