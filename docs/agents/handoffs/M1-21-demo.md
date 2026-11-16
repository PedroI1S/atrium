# M1-21-demo Handoff

- **Task:** M1-21-demo (D-104) — M1 gate demo, scripted end to end
- **Branch:** `agent/hydrogen/M1-21-demo`
- **Model:** Gemini 4 Hydrogen (High)

## Deliverables

- [`app/src/m1-demo.ts`](../../../app/src/m1-demo.ts): `ATRIUM_BENCH=m1-demo` (and `m1-demo:<exportDir>`) driver exercising steps 1–10 against the real Tauri bridge and `atriumd` with the synthetic (`fake`) agent.
- [`app/src/index.tsx`](../../../app/src/index.tsx): registers `m1-demo` / `m1-demo:*` in the `boot()` bench switch.
- [`app/scripts/m1_demo.py`](../../../app/scripts/m1_demo.py): one-command runner (reusing `live_check.py`'s `App`, `alive`, `build`, `daemon_pid`, `ROOT`), verifying UI/daemon checks, `SIGKILL` daemon recovery, private (`0600`) Markdown & JSONL export files, the daemon's recording-notifier test suite, and the N1 tight-loop resume regression test.
- [`docs/demos/M1-demo.md`](../../demos/M1-demo.md): one-page live demo script for Claude Code and Codex plus the automated command.

## Step-by-step status (3 consecutive runs)

Across 3 consecutive runs of `python3 app/scripts/m1_demo.py`, **19/20 checks passed consistently** and **1 check (`7b`) failed consistently**:

| Step | Check | Status | Notes |
|---|---|---|---|
| 1 | Home chat with fake agent streams | **PASS** (3/3) | User bubble, streamed markdown, tool calls, `idle` state verified. |
| 2a | Fake approval (`approval_at_ms`) -> `NeedsApproval`, Allow once | **PASS** (3/3) | Appears in approval dock & inbox; Allow resolves `approved` and tool `succeeded`. |
| 2b | Next turn -> Deny | **PASS** (3/3) | Next turn asks again; Deny resolves `denied` and tool `denied`. |
| 2c | "Always" creates rule and next turn doesn't ask | **PASS** (3/3) | "Always in this chat" creates `thread`-scoped allow rule (`list_rules`); turn 4 runs `echo synthetic` -> `succeeded` without asking. |
| 3a | Fake question (`question_at_ms`) -> `NeedsInput`, Answer | **PASS** (3/3) | Question form renders; Answer resolves `answered` and streaming continues. |
| 3b | Next turn -> Decline | **PASS** (3/3) | Decline resolves `cancelled` and streaming continues to `idle`. |
| 4a | Inbox lists threads with states in rank order | **PASS** (3/3) | DOM `[data-atrium-part="inbox-item"]` order verified: `approval` < `question` < `reply`. |
| 4b | Marking a thread read works | **PASS** (3/3) | Opening unread thread auto-marked read on view (`auto-on-view`), clearing `unread` and removing `reply` from inbox. |
| 5 | `⌘J` quick jump | **PASS** (3/3) | Implemented in `app/src/App.tsx` + `app/src/ui/jump.ts`. Real `KeyboardEvent` `Meta+j` cycled `approval` -> `question` -> `approval` in rank order. |
| 6a | Notifications Settings toggles persist in daemon | **PASS** (3/3) | Toggling `#setting-notify-approval` updates `get_settings` (`false` -> `true`). |
| 6b | Notifications posted for approval/question/failed and suppressed when off | **PASS** (3/3) | Checked via `cargo test -p atrium-daemon --test requests_notifications` (uses daemon's recording notifier `Rig::notes()`). |
| 7a | Stop ends running turn | **PASS** (3/3) | Clicking Stop stops the running turn to `idle`. |
| 7b | Resume synthetic thread right after `Ended` | **FAIL** (3/3) | **Not in M1 / needs daemon change:** `atriumd` rejects `resume_thread` on `atrium-fake:` (`rejected (invalid): a synthetic thread can't be resumed`). |
| 7c | Daemon N1 regression test (`resume_right_after_final_state_in_a_tight_loop`) | **PASS** (3/3) | Proves N1 is fixed for real/scripted sessions across 201 tight-loop rounds. |
| 8 / 8b | Kill daemon mid-turn -> app reconnects, thread `Interrupted` | **PASS** (3/3) | `SIGKILL` on `atriumd` -> app spawns new daemon, reconnects, closes transcript, preserves streamed chars, marks `interrupted`. |
| 9a/b/c | Export thread to Markdown & JSONL | **PASS** (3/3) | `export_thread` writes mode `0600` `.md` and `.jsonl` files containing user prompt and streamed assistant transcript. |
| 10 | Archive (`archive_thread`) | **PASS** (3/3) | M1 has `archive_thread` in protocol/daemon/reducer; removes thread from `daemon().threads`, `order`, `list_threads`, and subsequent commands return `unknown_thread`. |

## Findings, product bugs, and gaps

1. **Synthetic (`fake`) threads cannot be resumed (`7b` — needs daemon change if fake resume is desired):**
   - *Where:* `crates/atrium-daemon/src/commands.rs:465-470` (`if resume.starts_with(fake::NATIVE_ID_PREFIX) { return Err(failure(ErrorCode::Invalid, "a synthetic thread can't be resumed")); }`) and `app/src/ui/ThreadCard.tsx:158` (hides Resume button when `native_session_id` starts with `atrium-fake:`).
   - *Reproduce:* Start a synthetic thread, end its session (`SessionCommand::Shutdown`), then invoke `resume_thread` (or run `python3 app/scripts/m1_demo.py`).
2. **Stop button vs `Ended`, and Resume button visibility (`7a`/`7b` — UI behavior):**
   - *Where:* `app/src/chat/actions.ts:98-104` (`stopTurn` sends `SessionCommand::Interrupt`, which moves the thread to `Idle` while keeping the session alive; no UI button sends `SessionCommand::Shutdown` to reach `Ended`). Also `app/src/ui/ThreadCard.tsx:158` renders the header **Resume** button only when `state.type === "interrupted"` (not `ended`), though sending a message in the composer (`actions.ts:86`) calls `resume_thread` whenever `!LIVE.has(state.type)`.
   - *Reproduce:* Start any chat, click Stop in the composer -> state becomes `Idle`, not `Ended`.
3. **No UI buttons for Export or Archive (`9`/`10` — UI gap):**
   - *Where:* `app/src/ui/` has no UI trigger or Command Palette action calling `export_thread` or `archive_thread`, even though `app/src/ui/Transcript.tsx:286` tells the user *"Export the chat to read all of it."* Both commands work end-to-end through the Tauri bridge (`daemon_command`) and `atriumd`.
   - *Reproduce:* Search `app/src/ui/` for `export_thread` / `archive_thread` -> no UI call sites.
4. **Live `atriumd` notifications cannot be observed externally (`6`):**
   - *Where:* `crates/atrium-daemon/src/main.rs:84` and `crates/atrium-daemon/src/upkeep.rs:240-278` (`CommandNotifier` runs `/usr/bin/osascript` on macOS / `notify-send` on Linux with stdio redirected to `/dev/null` and no log/recording seam).
   - *How checked:* UI settings persistence verified live in `6a`; daemon notification emission and per-kind off-switch persistence verified in `6b` via `cargo test -p atrium-daemon --test requests_notifications`.
5. **Lock wrapper note (`lockf`):**
   - The worker shell pre-tool hook rejected `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock` as an absolute path outside the worktree, so the 3 runs in this session ran sequentially via `python3 app/scripts/m1_demo.py` directly (with no other GUI benchmark running). Human/CI runs should wrap with `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 app/scripts/m1_demo.py`.
   - One earlier debugging `--keep` run left `/tmp/atrium-demo-0jef_9fa` (its app and daemon were terminated in `finally`; the pre-tool hook blocks `/tmp` paths in shell commands).

## What the demo cannot check automatically (human checks)

- **Real providers (Claude Code & Codex):** probing installed CLIs, `Full` vs `Partial` enforcement descriptors, real tool approvals/denials, and real session `resume_thread` across `Ended` and `Interrupted` incarnations. Follow [`docs/demos/M1-demo.md`](../../demos/M1-demo.md).
- **Visible macOS Notification Center banners:** unfocus Atrium or switch to Inbox during an approval/question turn to confirm `/usr/bin/osascript` delivers banners to macOS Notification Center and suppresses them when the switch is off or the thread is focused.
- **Pop-out windows & crash recovery:** covered by `python3 app/scripts/live_check.py` and `python3 app/scripts/chat_check.py`.

## Checks run

### Frontend & Rust format (after final code edit)

- `cargo fmt --all --check`: exited 0 (no Rust files were changed).
- `pnpm --dir app typecheck && pnpm --dir app test && pnpm --dir app build`: exited 0
  - `tsc --noEmit`: 0 errors
  - `vitest run`: `Test Files  14 passed (14)`, `Tests  264 passed (264)`
  - `vite build`: `2310 modules transformed`, built in `1.71s`

### `python3 app/scripts/m1_demo.py` (3 consecutive runs)

**Run 1 (`EXIT:1`, 19/20 passed):**
```text
ok   1. Home chat with the fake agent streams                                                {"thread":"th-18e86b6fcfccd868-1","chars":[320,592,960],"tools":3,"state":"idle"}
ok   2a. Fake approval shows NeedsApproval in queue and Allow succeeds                       {"inQueue":true,"outcome":"approved","toolStatus":"succeeded"}
ok   2b. Next turn approval Deny denies the call                                             {"outcome":"denied","toolStatus":"denied"}
ok   2c. Always creates a rule and the next turn runs without asking                         {"rulesCount":1,"approvalsAsked":3,"turn4ToolStatus":"succeeded"}
ok   3a. Fake question shows NeedsInput form and Answer resumes streaming                    {"outcome":"answered","chars":[16,256]}
ok   3b. Next turn Decline resolves question as cancelled and streaming continues            {"outcome":"cancelled","chars":[272,512]}
ok   4a. Inbox lists threads with their states in rank order                                 {"domReasons":["approval","question","reply"],"inboxCount":3}
ok   4b. Marking a thread read clears unread and removes its reply inbox item                {"mode":"auto-on-view","remainingInbox":[{"thread":"th-18e86b70aab65580-4","reason":"approval"},{"thread":"th-18e86b70aaf8fc00-5","reason":"question"}]}
ok   5. Cmd+J cycles through inbox threads in rank order                                     {"jump1":"th-18e86b70aab65580-4","jump2":"th-18e86b70aaf8fc00-5","jump3":"th-18e86b70aab65580-4","expected":["th-18e86b70aab65580-4","th-18e86b70aaf8fc00-5","th-18e86b70aab65580-4"]}
ok   6a. Settings -> Notifications toggles persist in daemon settings                        {"afterOff":false,"afterOn":true}
ok   7a. Stop ends the running turn                                                          {"stateAfterStop":"idle","note":"Stop sends Interrupt (ends turn to idle); Shutdown ends session to ended"}
FAIL 7b. Resume synthetic thread right after Ended streams a new incarnation                 rejected (invalid): a synthetic thread can't be resumed
ok   8. Kill daemon mid-turn: app reconnects and thread shows Interrupted                    {"state":"interrupted","chars":[16,32]}
ok   9a. Export command writes Markdown and JSONL files                                      {"thread":"th-18e86b6fcfccd868-1","mdPath":"/tmp/atrium-demo-tk17668o/exports/thread.md","mdBytes":2029,"jsonlPath":"/tmp/atrium-demo-tk17668o/exports/thread.jsonl","jsonlBytes":4586}
ok   10. Archive removes thread from lists and inbox                                         {"archived":"th-18e86b6fcfccd868-1","inList":false,"rejectedUnknownThread":true}
ok   8b. App started a new daemon after SIGKILL                                              pid 10007 -> 10075
ok   9b. Markdown export file exists (0600) and contains transcript                          exists=True mode=0o600 bytes=2029
ok   9c. JSONL export file exists (0600), parses as JSONL, and contains transcript           exists=True mode=0o600 lines=12 valid_json=True
ok   6b. Daemon recording notifier posts approval/question/failed and respects off-switches  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s
ok   7c. Daemon N1 regression test (resume right after Ended/Interrupted/Failed/Shutdown)    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 21 filtered out; finished in 0.28s
19/20 passed
```

**Run 2 (`EXIT:1`, 19/20 passed):**
Same 19 passing checks and `FAIL 7b` (`rejected (invalid): a synthetic thread can't be resumed`); daemon PID `10237 -> 10311`; `6b` `2 passed; 0 failed` (`0.30s`); `7c` `1 passed; 0 failed` (`0.28s`). (Middle of combined command output truncated in tool log.)

**Run 3 (`EXIT:1`, 19/20 passed):**
```text
ok   1. Home chat with the fake agent streams                                                {"thread":"th-18e86b74fe6ac860-1","chars":[352,624,960],"tools":3,"state":"idle"}
ok   2a. Fake approval shows NeedsApproval in queue and Allow succeeds                       {"inQueue":true,"outcome":"approved","toolStatus":"succeeded"}
ok   2b. Next turn approval Deny denies the call                                             {"outcome":"denied","toolStatus":"denied"}
ok   2c. Always creates a rule and the next turn runs without asking                         {"rulesCount":1,"approvalsAsked":3,"turn4ToolStatus":"succeeded"}
ok   3a. Fake question shows NeedsInput form and Answer resumes streaming                    {"outcome":"answered","chars":[16,256]}
ok   3b. Next turn Decline resolves question as cancelled and streaming continues            {"outcome":"cancelled","chars":[272,512]}
ok   4a. Inbox lists threads with their states in rank order                                 {"domReasons":["approval","question","reply"],"inboxCount":3}
ok   4b. Marking a thread read clears unread and removes its reply inbox item                {"mode":"auto-on-view","remainingInbox":[{"thread":"th-18e86b75d884ef58-4","reason":"approval"},{"thread":"th-18e86b75d8a592d0-5","reason":"question"}]}
ok   5. Cmd+J cycles through inbox threads in rank order                                     {"jump1":"th-18e86b75d884ef58-4","jump2":"th-18e86b75d8a592d0-5","jump3":"th-18e86b75d884ef58-4","expected":["th-18e86b75d884ef58-4","th-18e86b75d8a592d0-5","th-18e86b75d884ef58-4"]}
ok   6a. Settings -> Notifications toggles persist in daemon settings                        {"afterOff":false,"afterOn":true}
ok   7a. Stop ends the running turn                                                          {"stateAfterStop":"idle","note":"Stop sends Interrupt (ends turn to idle); Shutdown ends session to ended"}
FAIL 7b. Resume synthetic thread right after Ended streams a new incarnation                 rejected (invalid): a synthetic thread can't be resumed
ok   8. Kill daemon mid-turn: app reconnects and thread shows Interrupted                    {"state":"interrupted","chars":[32,48]}
ok   9a. Export command writes Markdown and JSONL files                                      {"thread":"th-18e86b74fe6ac860-1","mdPath":"/tmp/atrium-demo-_xz_idys/exports/thread.md","mdBytes":2029,"jsonlPath":"/tmp/atrium-demo-_xz_idys/exports/thread.jsonl","jsonlBytes":4586}
ok   10. Archive removes thread from lists and inbox                                         {"archived":"th-18e86b74fe6ac860-1","inList":false,"rejectedUnknownThread":true}
ok   8b. App started a new daemon after SIGKILL                                              pid 10438 -> 10525
ok   9b. Markdown export file exists (0600) and contains transcript                          exists=True mode=0o600 bytes=2029
ok   9c. JSONL export file exists (0600), parses as JSONL, and contains transcript           exists=True mode=0o600 lines=12 valid_json=True
ok   6b. Daemon recording notifier posts approval/question/failed and respects off-switches  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.28s
ok   7c. Daemon N1 regression test (resume right after Ended/Interrupted/Failed/Shutdown)    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 21 filtered out; finished in 0.34s
19/20 passed
```
