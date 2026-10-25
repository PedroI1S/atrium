# Review: M0-05-daemon-socket

Author: GPT-6.1 Sol · Reviewer: Sonnet 5.5 (read-only, commit `9fa0f4f`) · Triage: orchestrator (Opus 5.5), 2026-10-25

## Verdict
**Approve with changes.** The core design is sound:
- Delivery is pull-based (subscriptions page `id > cursor` from SQLite; the watch channel only wakes them up), so the replay/live boundary has nothing to race. The brief's escalation condition (needing a global writer lock) doesn't apply.
- Socket security and lock-before-cleanup match `wire.rs`. Orphan identity checks fail closed.
- Fmt, clippy and the 11 tests pass. The reviewer ran the tests 7 times with no flakes.

Reproduced measurements:
- cursor 1,000 behind: 5.7 ms vs 8.7 ms reported;
- full replay: 340 ms;
- stored delivery: ~294k events/s.

## Findings and triage (all accepted → fix round 1)

| # | Sev. | Finding | Fix asked for |
|---|---|---|---|
| 1 | major | The orphan-reaping test can't fail: the fake agent dies by itself on EPIPE when the daemon is killed (5 ms tick). The report and handoff overstate it, and the identity assertion right after the crash races that self-exit. | Use an orphan that survives pipe loss (`/bin/sleep 60` with a real identity row, or a `Fake` that emits once and then idles) and assert the daemon kills it. Keep the mismatch case. Correct the report wording. |
| 2 | major | `Client::next()` claims cancel-safety but isn't during reconnect. Dropping it mid-resubscribe leaves a connected client with missing subscriptions, so events stop silently. M0-07's `select!` will hit this. | Only mark the client resubscribed after all subscribes succeed, and finish it at the start of `next()` (or reconnect in a spawned task). Add a test that drops `next()` at each await of the reconnect path. |
| 3 | major | `next()` retries every reconnect error forever (50 ms, no backoff), including `Incompatible`, bad handshake, `unknown thread`, `cursor past end`. | Retry only I/O errors with capped backoff. Return protocol/handshake/`CommandResult::Err` failures. Surface reconnects (`Reconnecting` / `Reconnected` or an accessor). Document that `ReplayDone` repeats after each reconnect. |
| 4 | minor | `command()` while disconnected returns `UnknownOutcome` though nothing was sent. | Reconnect before writing. Only failures after the write starts are `UnknownOutcome`. Extend the proxy test. |
| 5 | major (M0-07) | The single `&mut Client` API forces every consumer to build its own select loop, `queued` grows unbounded during commands, and webview-ack cursors can cause duplicate delivery to the UI after reconnect. | Ship a small `ClientHandle` actor (mpsc commands + an event stream owning the client), with bounded buffering. Document that the UI dedupes by `id <= last_ui_applied`. |
| 6 | major (report) | Idle RSS doesn't reproduce: 8.4 MiB at 60 s, but 69.8 MiB at 3 s. The 60 s figure depends on macOS page reclaim, and Linux may stay ~70 MiB (over budget). The "live throughput" number is bound by the generator. | Report RSS at 0/3/60 s over several runs, plus macOS `phys_footprint`. State that the result is OS-dependent. Investigate what holds the memory (per-connection SQLite connections, page caches, allocator). Relabel the live figure as "keeps up with 10k/s". |
| 7 | minor | The replay-boundary test doesn't prove appends landed during replay (current-thread runtime). Re-subscribe with in-flight events and the client's ≤-cursor drop are untested. | Make it deterministic (append after the Subscribe response and before the first page), assert that it happened, add a multi-thread variant, and cover the untested paths. |
| 8 | minor | Reconnect window (a) is tested as a daemon crash, not a client death. The automatic-reconnect test checks only `ReplayDone`, not continuity. | Add a new-`Client` + persisted-cursor variant and assert continuity. Fix the stale comment. |
| 9 | minor | ESRCH between the identity check and the kill aborts daemon startup. Only the leader pid is killed. | Treat ESRCH as success. Note `killpg` for M1. |
| 10 | minor (security) | The client doesn't verify the socket dir, so on a shared `/tmp` another user's socket could be used. The socket is created with the umask mode and then chmod'ed. The lock file mode isn't set. | Verify the dir (symlink, uid, 0700) and socket owner in `Client::connect`. Use a tight umask or bind-and-rename. Lock file 0600. Test that a live daemon survives a failed second start. |
| 11 | minor | The single-instance error is a Debug dump. | Print a clear message and exit 1. |
| 12 | minor | The `children` mutex is held across a stdin write, so a stuck child blocks everyone. `ListThreads` is O(all events). Every store job wakes all connections. | Per-child stdin writer task. `max(id)` subquery. Notify only on appends. |
| 13 | nit | Frame IO does ~3 syscalls per frame each way. Exact-`MAX_FRAME_LEN` and write-side oversize aren't tested. | One buffer per frame write, `BufReader` on reads, and the two tests. |
| 14 | nit | The handoff's "uncommitted" text is stale. `ctrl_c()` is re-created per loop iteration. Spawning the daemon lives only in the bin. Daemon stderr is lost. | Fix the text. Create `ctrl_c` once. `Client::connect_or_spawn` in the library. Log daemon stderr to a file in the dir. |

## Reviewer's full report
Kept in the orchestrator's raw archive (`.orchestration/archive/M0-05-daemon-socket/`, Pedro's Mac). The table above carries every finding.
