# Handoff: M1-09b-home-chat (Opus 5.5) — status: **partial**

Stopped at the orchestrator's request (driver's shift ending). Branch `agent/opus/M1-09b-home-chat`,
rebased onto `dev` at 1d8d9ca (M1-09a merge). Not rebased again. No apps, daemons or
measurements were started in this session, so nothing is left running.

## Done (committed)

Pure modules with vitest tests (all pass, 241 tests in 12 files):
- `app/src/chat/limits.ts` (+ test that parses `crates/atrium-protocol/src/limits.rs`).
- `app/src/chat/markdown.ts` (parser to AST, depth cap 12), `links.ts` (link/image classification
  per §12: http/https/mailto open via `open_link`, `file://` reveal only, data raster images
  ≤ MAX_INLINE_IMAGE_BYTES, SVG never, remote https click-to-load), `render.ts` (DOM built with
  createElement/text nodes only). `markdown.test.ts` covers the hostile inputs from the brief
  (javascript: links, raw `<script>`, `<img onerror>`, SVG, huge data images, nested fences) and a
  source scan forbidding innerHTML & co. in render/markdown/Transcript/RequestCard/InboxCard.
- `app/src/chat/blocks.ts` (streaming block splitter), `transcript.ts` (reducer: deltas, tool
  output replace/append, incarnations, loaded-bytes bound, paging/prepend, detach), tested in
  `transcript.test.ts` including recorded ServerMessage fixture `app/src/chat/fixtures/approvals.json`
  (card states fresh / stale / resolved elsewhere).
- `app/src/chat/inbox.ts` (§8 order, groups, filters, ⌘J `nextJump`, J/K) tested in `inbox.test.ts`.
- `app/src/chat/requests.ts` (card state machine, "Always…" scopes worded per D-062, exact
  subject per D-054).
- `app/src-tauri/src/links.rs`: `check_reveal`/`reveal` (+3 tests); `lib.rs`: new command
  `reveal_path` (Home folder = $HOME). This is an M1-09a-owned file: new command only.
- `app/src/platform.ts`: `revealPath`. `app/package.json`: devDependency `happy-dom` (DOM env for
  render tests; dev only, no bundle cost).

WIP UI (committed in the wip commits, not yet type-clean):
- `app/src/chat/threads.ts` (holdThread: per-frame flush, paging, reattach; request commands with
  per-window local state), `app/src/chat/actions.ts` (start/send/stop/resume/markRead/reportFocus,
  agent choices from ProbeProviders, synthetic fake agent in debug builds).
- `app/src/ui/Transcript.tsx` (virtualized transcript, follow tail, paging at top, jump to latest),
  `app/src/ui/RequestCard.tsx` (approval + question cards), `app/src/ui/Composer.tsx` (stop button,
  busy), `app/src/ui/ThreadCard.tsx` (ThreadBody: live thread with transcript, request dock,
  composer; new-chat view with agent chip; Resume button for interrupted threads).

## Not done, by Definition of Done item

- vitest suites (markdown §12, reducer, bound/paging, inbox order, card states, ⌘J): **done**.
- UI wiring: InboxCard rewrite (filters, inline cards via held models, J/K/Enter/A/D),
  ContextPanel inbox preview + usage/context, App.tsx ⌘J via `nextJump`, Settings sections
  Agents / Approvals (list_rules/delete_rule) / Notifications: **not done**.
- `app/src/styles/chat.css` (tokens only) and adding new TS files to `TS_FILES` in
  `tokens-conformance.test.ts`: **not done**.
- Live run against real atriumd + fake agent (stream, second window, daemon killed mid-stream):
  **not done**.
- macOS screenshots light/dark (extend `app/scripts/screenshots.py`): **not done**.
- §14 streaming run (10 threads × 1k tok/s × 30 s): **not done** (no `stream_bench.py` yet).
- `startup_bench.py` gate: **not run**.
- `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test -p atrium-app`, `pnpm build`,
  `scripts/check-protocol-ts`: **not run this session** (links.rs tests passed earlier via cargo test).
- `pnpm typecheck`: **fails** (see below). `pnpm test`: **passes** (241/241).
- Change requests: **not yet appended** to `docs/agents/change-requests.md`. Planned:
  1. fake agent option raising one approval and one question (needed for live approval/question runs);
  2. daemon honoring per-kind notification settings (notifier ignores them today);
  3. a "decline question" command (the wire has none; the UI interrupts the turn instead).

## What I ran

- `pnpm test` → 12 files, 241 tests pass.
- `pnpm typecheck` → fails:
  - `daemon/types` does not export `RequestOutcome`, `ToolKind`, `ToolStatus`, `TurnOutcome`,
    `Usage`, `Value`, `QuestionAnswer`, `RememberScope` (used by transcript.ts, RequestCard.tsx,
    Transcript.tsx, requests.ts). They exist in the generated bindings; either re-export them from
    `app/src/daemon/types.ts` or import from the bindings path directly.
  - `state.ts` has no `windowFocused` (ThreadCard.tsx). Add a focus signal (window focus/blur) or
    use whatever M1-09a exposes for focus.
  - `Transcript.tsx` lines 107–111: possibly-undefined indexing (noUncheckedIndexedAccess); add guards.

## Gotchas

- Write tool turned ` ` escapes into literal characters in links.ts; edit those regexes via
  a script, not the Write tool.
- The fake agent emits no markdown and can't raise approvals/questions or be resumed; screenshots
  of markdown/approval/question scenes need seeded events in shots mode (or the CR).
- Transcript eviction only cuts where no open item spans the cut (documented in transcript.ts).
- Pending requests are docked above the composer as full cards (transcript rows are compact), so
  they stay actionable under virtualization and eviction.

## Exact next step

Fix `pnpm typecheck` (re-export the missing types from `app/src/daemon/types.ts`, add
`windowFocused` to `state.ts`, guard Transcript.tsx 107–111), then rewrite `ui/InboxCard.tsx` and
continue down the "Not done" list above.
