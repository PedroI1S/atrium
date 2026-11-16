# M1-21-demo: the M1 gate demo, scripted end to end

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-104) · Reviewer: Sonnet 5.5 (read-only, cross-provider) · Depends on: nothing open (it runs on `dev`)
Go: D-104 (Pedro, 2026-11-16) · Branch: `agent/hydrogen/M1-21-demo` · Worktree: `../atrium-wt/M1-21-demo`

## Goal
The gate needs "the milestone demo runs end to end" (PLAN §8). Give the drivers two things:
- a demo they can run with one command, with the synthetic (fake) agent, that walks through what M1 promises and prints pass/fail per step;
- a short written script for the live demo.

No real provider is ever run. Reuse the existing harness (`app/scripts/live_check.py`, `chat_check.py`, and the app's `ATRIUM_BENCH` modes in `app/src/`). Don't build a second one.

## Context
- `PLAN.md` "M1: Daemon + Home + attention" (around line 727); §8, the gate.
- Contract `docs/contracts/m1.md`: §6.2 for the fake agent's approval and question (`approval_at_ms`, `question_at_ms`, rev 3.1, CR-13), §8 for states and notifications, §13 for history and export.
- The existing live checks:
  - `app/scripts/live_check.py` (windows, drafts, daemon kill and reconnect, app crash);
  - `app/scripts/chat_check.py` (Home chat streaming, pop-out, daemon kill mid-stream, Stop);
  - their app sides, `app/src/roundtrip.ts` and `app/src/chat-live.ts`;
  - `app/src/harness.ts`, `app/src-tauri/src/lib.rs` (the `ATRIUM_BENCH` switch).
- Daemon tests already covering pieces you can point to: `crates/atrium-daemon/tests/{requests_fake,requests_notifications,recovery,sessions}.rs`.

## Owned paths
- New `app/src/m1-demo.ts` (an `ATRIUM_BENCH=m1-demo` mode), and its registration in the app's bench switch (`app/src/index.tsx` or `harness.ts`, and `app/src-tauri/src/lib.rs` only if the switch lives there).
- New `app/scripts/m1_demo.py`.
- New `docs/demos/M1-demo.md`.
- `docs/agents/handoffs/M1-21-demo.md`.
- Not yours: daemon, protocol or policy code. If the demo finds a bug, report it. Don't fix it.

## What to do
1. **`app/scripts/m1_demo.py`**, in the style of `chat_check.py` (throwaway `ATRIUM_HOME` under `/tmp`, builds debug binaries, prints each check, exit 0 only if all pass). It drives an `ATRIUM_BENCH=m1-demo` run that covers, in order:
   1. A Home chat with the fake agent streams.
   2. The fake approval (`approval_at_ms`) appears in the approval queue and the thread shows `NeedsApproval`.
      - Allow it: the call succeeds.
      - Next turn, Deny it: the call is denied.
      - "Always": a rule is created, and the next turn doesn't ask.
   3. The fake question (`question_at_ms`) shows as a question form (`NeedsInput`).
      - Answer it; on the next turn, decline it. Streaming goes on both times.
   4. The inbox lists the threads with their states, and marking a thread read works.
   5. `⌘J`, if the app implements it: say what it opens and check it. If it isn't implemented, report that as a finding, not a failure of the demo.
   6. Notifications are posted for approval and question, and a kind turned off in Settings is not. If notifications can't be observed from the script, use the daemon's recording notifier the daemon tests use, or report how you checked.
   7. Stop a turn; the thread ends. Resume it: a new incarnation streams (N1 is fixed, so a resume right after `Ended` must work).
   8. Kill the daemon mid-turn: the app reconnects and the thread shows `Interrupted`.
   9. Export the thread to Markdown and JSONL: both files exist and contain the transcript.
   10. Archive, if M1 has it.

   Reuse `live_check.py`'s `App`, `build`, `daemon_pid` and the like. Steps already proven by `live_check.py`/`chat_check.py` can call those scripts instead of repeating them.
2. **`docs/demos/M1-demo.md`:** a one-page script for a human demo with real Claude Code and Codex:
   - each step, what to click, and what to expect;
   - the command for the automated run.
3. **Run it.**
   - On this Mac, run `m1_demo.py` 3 times, under `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock` so it doesn't overlap a benchmark.
   - It opens windows on the desktop: that is expected.
   - Report each run's results.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` (if Rust changed).
- [ ] In `app/`: `pnpm typecheck`, `pnpm test`, `pnpm build`.
- [ ] `python3 app/scripts/m1_demo.py` passes 3 runs in a row, or each failing step is reported with the app's log and marked "bug" or "not in M1".
- [ ] Handoff:
  - which steps passed;
  - any product bugs found, with how to reproduce each;
  - what the demo can't check automatically, and how a human checks it.

## Rules
- Work only in your worktree, and commit on your branch only. Never push, merge or rebase.
- Never run a real provider.
- Stop every app and daemon the script starts, even on failure. Leave no `/tmp/atrium-*` home behind unless `--keep` is given.

## Escalate if
- a step needs a daemon or protocol change;
- the app has no way to observe a step (notifications, `⌘J`) without one.

Write down what you found and go on with the other steps.
