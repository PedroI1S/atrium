# Review: M1-21-demo (76a0943)

Reviewer: Claude Sonnet 5.5, read-only (cross-provider). Author: Gemini 4 Hydrogen `high`. Saved by the orchestrator from the reviewer's report.

## Verdict: **approve with changes**

The harness is real: steps 1-5, 8, 9 and 10 assert daemon and DOM state, and no step passes without the feature working. Changes needed: 7b must stop being a hard FAIL, 6b and 7c can pass vacuously, cleanup has two gaps, and the demo doc needs small fixes.

## Findings

### Major
1. **7b fails by design, so `m1_demo.py` can never exit 0.** `m1-demo.ts:405-423` shuts the fake thread down and resumes it; `commands.rs:465-470` rejects any `atrium-fake:` native id. Fix: assert the refusal (`code === "invalid"`, message "can't be resumed") and report it as a passing regression check on the guard, labelled a fake-agent limit.
2. **7c (and 6b) can pass with zero tests run.** `run_cargo_test` (`m1_demo.py:39-54`) accepts cargo's exit 0 with "0 passed". Fix: parse `N passed`, require N ≥ 1 (≥ 2 for 6b). 7c is a scripted-adapter daemon test, not an app step; don't read it as proof that Stop and Resume work in the app.

### Minor
3. **Cleanup gaps** (`m1_demo.py`): after SIGINT the daemon isn't waited on and has no SIGKILL fallback (fixed `sleep(0.5)`); `daemon_pid(home)` may return the stale pid after the step-8 kill, leaking the respawned daemon; no SIGTERM handler. Fix: poll ~3 s, then SIGKILL; collect pids from every lock file.
4. **A mid-run failure loses earlier results**: checks are reported only in the final `writeReport` (`m1-demo.ts:484`). Report each check as it completes, or include `checks` in the failure report.
5. **7a is weak** (`m1-demo.ts:400` accepts `idle` or `ended`). Assert that `assistantChars` stops growing, or that the "Stopped" note exists.
6. **6a only toggles the setting**; posting and suppression are covered only by `cargo test --test requests_notifications` (which does cover them). Keep the wording "daemon recording notifier".
7. **The demo posts real macOS banners** (`main.rs:84`, `CommandNotifier` via `osascript`). Say so in the doc.
8. **`m1-demo.ts` is statically imported** (`index.tsx:18`), like the other bench modules; ~500 lines in the production bundle. Gating is correct. Optional: dynamic `import()`.
9. **`cargo test` runs with no timeout** after the app section; a cold build can take minutes. Say so in the doc; keep `lockf` around the whole script.

### Nits
- `m1_demo.py:169-170` unlinks export files after `rmtree` (redundant).
- `m1-demo.ts:417-421` duplicates `formatErr` (line 94).

## Step assertions
| Step | Verdict |
|---|---|
| 1 | Real |
| 2a, 2b, 2c | Real (outcomes, tool status, `inQueue`, thread-scoped rule, no 4th approval) |
| 3a, 3b | Real |
| 4a, 4b | Real |
| 5 | Real (`⌘J` is implemented in `ui/jump.ts`) |
| 6a | Weak (finding 6) |
| 7a | Weak (finding 5) |
| 7b | Fails by design (finding 1) |
| 8 | Real |
| 9 | Real (mode 0600, JSONL parses, content present) |
| 10 | Real |

## 7b: a deliberate limit
Treat it as "not checkable with the fake agent"; don't change the daemon.
- `resume_thread` (`commands.rs:454-519`) rebuilds a native adapter from stored provider, model, effort and preset; the fake's launch parameters aren't persisted.
- `ThreadCard.tsx:158` hides the Resume button for `atrium-fake:` ids, so UI and daemon agree.
- Contract §6.2 describes resume only for native providers; the fake is a streaming and requests source only.
- Supporting fake resume would be a medium daemon and store change for test convenience only.

Real resume stays a human step with Claude Code and Codex (doc step 7), plus 7c.

## `docs/demos/M1-demo.md`
Every UI target named in the doc exists. Corrections:
- Step 7: the UI has no way to reach `Ended` (the automated run sends `Shutdown` over the protocol); fake threads can't be resumed and the Resume button is hidden for them.
- Step 8: same note for the fake thread; the live demo uses a real provider thread.
- Add the banner note (finding 7) and the cargo build time (finding 9).
- Step 6 claims notifications are suppressed for the focused chat; the reviewer didn't verify this in `upkeep.rs`. Verify or soften.
- Confirmed: no UI for Export, Archive or Shutdown; Stop sends `Interrupt` and the thread returns to Idle with a "Stopped" note.

## What the reviewer ran
In `~/atrium-wt/M1-21-demo`, read-only: `pnpm typecheck` clean; `pnpm test` 264/264; `py_compile` OK. The demo itself was not re-run.
