# Handoff: M1-09b-home-chat — status: **partial** (in progress, kept current)

Worker: Claude Fable 5.2 on rodrigo's machine (Linux), continuing Opus 5.5's branch `agent/opus/M1-09b-home-chat` (D-065, D-066). This note is rewritten as items finish, so a cut-off loses nothing: what is under "Done" is committed.

## Done (committed)

From the first worker (Opus 5.5), unchanged unless said:
- Pure modules with vitest tests: `app/src/chat/{limits,markdown,links,render,blocks,transcript,inbox,requests}.ts`, the recorded `ServerMessage` fixture `app/src/chat/fixtures/approvals.json`.
- `app/src-tauri/src/links.rs` (`check_reveal`/`reveal`) and the `reveal_path` command in `lib.rs` (an M1-09a file: one new command, same rules as `open_link`); `app/src/platform.ts`: `revealPath`.
- `app/package.json`: devDependency `happy-dom` (a DOM for the render tests; not in the bundle).
- UI: `app/src/chat/{threads,actions}.ts`, `app/src/ui/{Transcript,RequestCard,Composer,ThreadCard}.tsx`.

This worker:
- `e2b09af` `pnpm typecheck` is clean: the protocol types the chat uses are re-exported from `app/src/daemon/types.ts`; `state.ts` has `windowFocused` (window focus/blur).
- `655825e` the rest of the UI:
  - **Inbox** (`ui/InboxCard.tsx`): groups (approvals and questions, failed, ready for you), filter (All · Approvals and questions · Replies), approval and question cards on the item (the first 12 asks view their thread for it), J/K/arrows, ↵ opens, A allows once, D denies, a footer with the keys.
  - **⌘J / Ctrl+J** (`ui/jump.ts`, `App.tsx`, the palette): the next item in inbox order after the chat being looked at; storage-full opens the inbox; nothing waiting says so. Main window only (a pop-out can't change its thread).
  - **Panel** (`ui/ContextPanel.tsx`): beside the inbox, a preview of the picked item with "Open chat"; beside a chat, Details plus Usage (context occupancy, tokens, cost) from the thread's events.
  - **Settings** (`ui/Settings.tsx`, through M1-09a's registry, plus an `action` control kind): Agents (`ProbeProviders`: installed or not, version, tested or not, Full/Partial with the reasons), Approvals (`ListRules`, each rule's exact command or glob, scope "in any folder" per D-062, origin, `DeleteRule`), Notifications (three switches stored with `SetSetting`; the section says the daemon doesn't read them yet, CR-14).
  - `chat/rules.ts` + `rules.test.ts`: how a rule reads (argv quoted so two argvs never read the same).
  - `styles/chat.css` (tokens only; new literals are tokens in `base.css`), and every new TS module is in `TS_FILES` of `tokens-conformance.test.ts`.
  - Approval card safety: a card that just appeared ignores ↵/esc for 600 ms and never takes focus from a field being typed in; ↵ on a focused button presses that button (before, ↵ on "Deny" allowed).
- Change requests filed in `docs/agents/change-requests.md`: **CR-13** (fake agent option that raises one approval and one question), **CR-14** (daemon reads per-kind notification settings), **CR-15** (declining a question).

Third worker (Claude Opus 5.5, rodrigo's machine, 2026-11-14), the escalation for §14:
- `f27fe0d` **§14 fix** (`ui/Transcript.tsx`, `chat/threads.ts`): transcript rows keyed by row key instead of position; the virtualizer's options, the follow-the-tail scroll and the tool and user rows follow the rows' structure and item versions instead of every applied batch. Bench (`stream-bench.ts`, `stream_bench.py`): per-frame work (`frameWork`), a 5 s idle baseline (`idleFrames`), the share over 16.7 ms past the 1 ms timestamp resolution, `apply`/`view`/`measure` counters, and `--mode stream-one` for diagnosis. Numbers under "Results".
- `62ee3ac` **screenshot fixes**: the Inbox tile said "4 need you" while the inbox said "2 need you" (it counted every item): it now counts what needs you (`chat/inbox.ts` `needsYouCount`, tested), else "N to read" with a ready dot; an approval card that takes focus drew a 2 px ink frame (the focus ring), now its own border turns the needs-you colour (neutral once it can't be acted on); a partial session's reasons (eleven "… calls: Unknown" lines for the fake agent) show three and "N more"; remembered rules name the tool in brackets ("every file edit (Edit) of", not "by Edit of").

## Results so far (each written when it was measured)

- **Live check, `python3 app/scripts/chat_check.py` (debug build, real `atriumd`, fake agent): 12/12 passed** (2026-11-13): a Home chat starts from the composer and streams (model and DOM grow, tool calls show, the view follows the tail); a second window shows the same thread, streaming; the daemon is killed mid-stream (pid 1338871 → 1339140), the thread shows as interrupted, both windows show the same closed transcript (3,296 characters, last event 216); a new chat streams after the reconnect; Stop ends its turn.
- **Screenshots on Linux, `python3 app/scripts/screenshots.py <dir>`: 28 pictures, all three runs report `pass`**, in `/home/rodrigo/atrium-wt/M1-09b-home-chat/.orchestration/M1-09b-screenshots/` (git-ignored; re-run the script to get them elsewhere). The chat scenes are seeded in the window (`app/src/chat-shots.ts` says what is seeded and what is the daemon's).
- **§14 streaming run, first measurement: FAILS THE GATE BY A WIDE MARGIN** (2026-11-13, `python3 app/scripts/stream_bench.py --runs 3`, `perf` profile, Linux/WebKitGTK, rAF timestamps, window visible in all three runs, load average about 10 at the start because the build had just finished):

  | run | tokens/s | fps | frame p50 / p95 / p99 / max (ms) | frames in the run | web content peak | app + webviews peak |
  |---|---:|---:|---|---:|---:|---:|
  | 1 | 7,507 over 39.9 s | 0.3 | 850 / 8,980 / 15,285 / 15,285 | 13 | 798 MiB | 920 MiB |
  | 2 | 8,933 over 33.5 s | 0.4 | 1,019 / 8,885 / 13,979 / 13,979 | 13 | 741 MiB | 853 MiB |
  | 3 | 9,651 over 31.1 s | 0.4 | 935 / 11,017 / 11,430 / 11,430 | 12 | 765 MiB | 885 MiB |

  The window drew about a dozen frames in 30 s: the webview's main thread is blocked for seconds at a time. All 300k tokens arrive and are applied (nothing is lost), but nothing like 60 fps. Cause not yet found when this was written; see below for what was tried after.
- **§14 streaming run after one fix: 59 fps, but STILL NOT WITHIN THE GATE (marginal). Escalated, not tuned further.** (2026-11-14, same command, `perf` profile, Linux/WebKitGTK on a 60 Hz display, rAF timestamps, window visible in all runs, load average 3.6–4.0 from other workers.)

  Cause of the first result, both found with the work counters now in the report: (1) `ThreadBody` keyed its view on the thread's *summary object*, which changes with every event, so the whole view was torn down, re-viewed in the client (a fresh snapshot) and redrawn at the event rate; it is keyed on the thread id now. (2) `MarkRead` was sent for every summary update of the chat being looked at (each delta sets `unread` again); it is sent at most once a second per thread now, and only while the chat is still shown and the window focused.

  | run | tokens/s | fps | frame p50 / p95 / p99 / max (ms) | frames | over 16.7 ms | over 1.5 × refresh ("long") | dropped | web content peak | app + webviews peak |
  |---|---:|---:|---|---:|---:|---:|---:|---:|---:|
  | 1 | 9,990 | 59.1 | 17 / 19 / **34** / 50 | 1,771 | 57.7 % | 26 (1.5 %) | 29 | 188.0 MiB | 284.8 MiB |
  | 2 | 9,988 | 59.0 | 17 / 18 / **35** / 50 | 1,769 | 58.9 % | 26 (1.5 %) | 29 | 186.9 MiB | 288.7 MiB |
  | 3 | 9,980 | 59.0 | 17 / 18 / **33** / 51 | 1,770 | 58.9 % | 27 (1.5 %) | 31 | 186.5 MiB | 283.4 MiB |

  Against the gate (at most 1 % of frames over 16.7 ms, p99 at most 33 ms):
  - **p99: 34, 35, 33 ms. One run of three is within, two are over by 1–2 ms.**
  - **Frames over 16.7 ms: 58 % as written, which says nothing**: WebKitGTK rounds rAF timestamps to 1 ms and a 60 Hz frame is 16.67 ms, so an on-time frame reads 16 or 17 (p50 reads 17; M0-07 saw the same). The share that skipped a display update (over 1.5 × the refresh interval, M0-07's "long" frames) is **1.5 %**, about one frame a second, each one or two updates late (max 50 ms). That is over 1 % too.
  - WebContent peak: **187–188 MiB** (web content process; app with all webview processes 283–289 MiB). M0-07 saw 1.05–1.39 GB on macOS at 10k tokens/s.
  - All 299.5k events were delivered and applied in each run (9,980–9,990 tokens/s).

  Where the main thread's time goes in 30 s (run 1; counters in the report's `work`): applying the per-frame batches of all ten threads, including the visible one's reactive update, virtualizer and follow-the-tail layout, **5.5 s over 17,689 flushes** (0.31 ms each, 18 % of the thread); markdown parse and DOM of the open block 0.2 s (1,974 renders); receiving 299,555 event deliveries 0.14 s; 138 thread-list updates 0.02 s; 29 `MarkRead` commands. Nothing was tuned after the fix. What I would look at next: the roughly once-a-second long frame (a row closing every 4,096 characters mounts and measures a new row, and the tool rows every 4 s), and the cost of one flush per thread per frame (ten rAF callbacks; one shared flush would do).
- **§14 streaming run after the escalation fix (Opus 5.5, 2026-11-14, `f27fe0d`): p99 and skipped updates WITHIN the gate; the literal "over 16.7 ms" share is not a usable number here (below).** `flock /tmp/atrium-gui-measure.lock python3 app/scripts/stream_bench.py --runs 3`, `perf` profile, Linux/WebKitGTK, rAF timestamps, window visible in all runs. Also running: load average 3.7–3.9 (another worker's agent and reviewer sessions for M1-14a; `ps` just before the runs showed no build or test running).

  | run | tokens/s | fps | frame p50 / p95 / p99 / max (ms) | frames | over 16.7 ms as read | over 17.7 ms | long (> 1.5 × p50) | web content peak | app + webviews peak |
  |---|---:|---:|---|---:|---:|---:|---:|---:|---:|
  | 1 | 9,993 | 59.9 | 17 / 19 / **22** / 37 | 1,794 | 58.3 % | 15.4 % | **5 (0.3 %)** | 179.4 MiB | 275.7 MiB |
  | 2 | 9,999 | 59.9 | 17 / 18 / **20** / 32 | 1,795 | 58.9 % | 13.7 % | **2 (0.1 %)** | 182.7 MiB | 283.8 MiB |
  | 3 | 9,991 | 59.8 | 17 / 18 / **20** / 78 | 1,791 | 57.3 % | 14.1 % | **2 (0.1 %)** | 182.6 MiB | 277.0 MiB |

  Before (`c884a76`): p99 33–35 ms, 1.5 % long frames, 187–188 MiB.

  - **Cause.** The per-frame counters added to the report (`frameWork`: per frame, main-thread time to after its rendering, batches applied, row changes, measurements) showed `measure` 8,675 times in 30 s (about 5 per frame) and frames with a row change taking 133 ms of main-thread time in a loaded run. The transcript rendered its rows with Solid's `Index` over the virtualizer's items, so an element was tied to a *position* in the rendered range. While following a streaming reply the range moves down by a row many times a second; every move handed every element the next row: its markdown re-rendered, tool rows rebuilt, every element measured again (forced layouts). Rows are now keyed by row key (`For` over the keys): a move mounts the row that enters and drops the one that leaves (`measure` 37 times in 30 s).
  - **Second cost, removed too:** the visible thread's view update ran 3.2 ms per frame (`view` 5.7 s in 30 s). The virtualizer's options effect re-ran on every batch (its `count` read the rows memo, which changes every batch), re-reconciling the rendered range; the follow-the-tail effect set `scrollTop` (a layout) every batch; each tool row rebuilt its icon every batch. Now the options follow the rows' structure only, the tail is followed when the total height changes, and rows read their item through its version. `view` is 0.78 s in 30 s (0.44 ms per frame); `flush` 5.8 s → 0.9 s.
  - **Reading the gate.** This display is **144 Hz** (`xrandr`: 2560×1440 at 143.87; the earlier "60 Hz display" was inferred from p50, wrongly). The idle window, sampled 5 s before each run, ticks at p50 7 ms. While streaming WebKitGTK paces rendering updates at 60 Hz (59.8–59.9 fps, mean frame 16.7 ms), with timestamps rounded to 1 ms, so an on-time frame reads 16 or 17 and **57–59 % "over 16.7 ms" says nothing**. Frames over 17.7 ms (past the resolution) are 14–15 %: they are paired with early ones (p50 17, mean 16.7), timer jitter of a 60 Hz cadence, not a missed update. The share that skipped an update (over 1.5 × the frame interval) is **0.1–0.3 %** and p99 **20–22 ms**. A 60 Hz display (macOS shift, or this machine set to 60 Hz) would give a cleaner reading.
  - **What is left on the main thread.** Per frame about 5 ms busy, of which the app's own work (`flush` incl. markdown) is about 0.5 ms; the rest is receiving the deliveries: Tauri 2 delivers each `Channel` message under 8 KiB with one `webview.eval` (tauri `ipc/channel.rs`), so 10,000 events/s are 10,000 script evaluations, which the counters can't see (`deliver:event` times only our handler: 0.17–0.22 s). The remaining long frames have under 3 ms of our work and 200–370 deliveries. Batching deliveries in `bridge.rs`'s `forward` (several frames per channel message) would cut this, but that is M1-09a's bridge, not a new command: left for the orchestrator.
  - Diagnosis mode: `python3 app/scripts/stream_bench.py --mode stream-one` holds only the shown thread (not the §14 run).

- **Release builds can't be used by the harness scripts** (found while measuring): a release build ignores `ATRIUM_HOME` (`lifecycle::default_paths` honours it only with debug assertions), and a release `atriumd` refuses every app ("no app identity is known") until packaging compiles the identity in. So `startup_bench.py` as M1-09a wrote it (release) measured an app that never connects, used the user's real Atrium folders, and left its daemon running there. Both bench scripts now build with the workspace's `perf` profile (optimized, debug assertions on), which honours `ATRIUM_HOME` and connects. `stop_daemon` in `startup_bench.py` now also finds a daemon by its `ATRIUM_HOME` and kills one that ignores SIGINT.
- **Side effect of my first two streaming attempts (release build): files outside the worktree.** They created `~/.local/share/atrium/` (`atrium.db`, `logs/atriumd.log`, empty `app/`, `snapshots/`) and `$XDG_RUNTIME_DIR/atrium/daemon.lock` on 2026-11-13 08:48, and left a release `atriumd` running (the orchestrator stopped pid 1349294). I did not delete them (outside my worktree); they hold no chats (the app never connected). The orchestrator decides whether to remove them.

## Not done yet (in this order)

1. Harness code: seeded screenshot scenes (`app/src/chat-shots.ts`), the live check against `atriumd` (`app/src/chat-live.ts`, `app/scripts/chat_check.py`), the §14 streaming run (`app/src/stream-bench.ts`, `app/scripts/stream_bench.py`). None of the new UI has been seen running yet: expect fixes from the first screenshots.
2. Run them on this machine's display under `flock /tmp/atrium-gui-measure.lock`: live check, screenshots (light and dark), §14 run, `startup_bench.py` (Linux numbers, private PSS).
3. The DoD command list: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-app`; `pnpm typecheck`; `pnpm test`; `pnpm build`; `scripts/check-protocol-ts`.
4. Final handoff with the outputs.

## What I ran so far

- `pnpm typecheck` → clean (after `e2b09af`, and again after `655825e`).
- `pnpm test` → 13 files, 258 tests pass (after `655825e`).
- `pnpm build` and `cargo build -p atrium-app --features custom-protocol -p atrium-daemon` (debug, on the host) → built.

## Owed to a macOS shift (Pedro)

The macOS screenshots (light and dark) and the macOS `startup_bench.py` gate. The §14 streaming gate is stated for the platform's presentation timestamps or rAF; only Linux/WebKitGTK (rAF) can be measured here.

## Needs a real provider (M1-07)

Approvals and questions end to end (card → ledger → provider), "Always…" creating a rule from a live request, the stale refusal from the daemon, Resume of an interrupted thread, markdown from a real model, usage/context numbers, notifications.

## Gotchas

- The agent's shell is a container without `git`, `node`, `python3`: host commands go through `distrobox-host-exec`.
- The Write tool once turned escape sequences in a regex in `app/src/chat/links.ts` into literal characters; they were checked again (they are escapes now). Check any regex after writing it.
- The fake agent emits no markdown and can't raise approvals or questions or be resumed (CR-13).
- Transcript eviction only cuts where no open item spans the cut (documented in `transcript.ts`).
- Pending requests are docked above the composer as full cards; their transcript rows are compact, so they stay actionable under virtualization and eviction.

## Exact next step

Write `app/src/chat-shots.ts` and call it from `runShots` in `app/src/roundtrip.ts`; run `flock /tmp/atrium-gui-measure.lock python3 app/scripts/screenshots.py <dir>` on the host and look at the pictures.
