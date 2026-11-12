# M1-09b-home-chat: Home chat, inbox, approval queue, question forms, ⌘J

Model: Opus 5.5 (contract §15, D-048) · Reviewer: Gemini 3.8 Flash, read-only, with screenshots · Depends on: M1-09a (written and checked, being reviewed; not yet merged)
Go: D-059 (rodrigo), D-062 (Pedro) · Branch: `agent/opus/M1-09b-home-chat`, cut from `agent/fable/M1-09a-app-shell` · Worktree: `../atrium-wt/M1-09b-home-chat`

**On rodrigo's shifts (D-065, D-066):** the worker is Claude Fable 5.2, on the same branch. Start from `docs/agents/handoffs/M1-09b-home-chat.md` (status partial) and its "Exact next step". The machine is Linux:
- Where this brief says macOS (screenshots, `startup_bench.py`), run on Linux and report the Linux numbers (private PSS, D-040). The macOS screenshots and the macOS startup gate stay owed to a Pedro shift; say so in the handoff.
- The GUI lock on Linux is `flock /tmp/atrium-gui-measure.lock <command>` (there is no `lockf`).
- The agent's shell runs in a container without `git`, `node`, `python3` or the WebKitGTK development files. Run `git`, `pnpm`, `python3` and every `cargo` command that builds `atrium-app` on the host: `distrobox-host-exec <command>` from the worktree.
- M1-09a is merged; `dev` is merged into this branch at dispatch. Don't rebase.

## Goal
Fill the M1-09a shell with what a user does in M1: chat with Home, see what needs them (inbox), approve or deny tool calls, answer questions, and jump to the next thing with ⌘J. All of it on daemon data through the existing bridge.

## Context
- `docs/contracts/m1.md` §5 (approvals, remember scopes), §6.1 (events, `MessageBody`), §8 (states and attention), §9 (policy, what an approval shows), §10 (upgrade), §11 (windows, streaming memory), §12 (rendered content: **read it twice**), §14 (streaming budget).
- `docs/design/ui-direction.md` (layout B, Calm, the inbox, the approval and question cards).
- M1-09a: `docs/agents/briefs/M1-09a-app-shell.md` and `docs/agents/handoffs/M1-09a-app-shell.md` on your branch. Use its bridge, window model, sync reducer, settings registry, tokens and harness scripts (`app/scripts/live_check.py`, `screenshots.py`, `startup_bench.py`) as they are.
- `crates/atrium-client/src/lib.rs` ("For the bridge"), `bindings/protocol.ts`.
- The spike's streaming renderer: `spikes/stream-perf/` and `docs/spikes/M0-07-stream-perf.md` (virtualizer, markdown to DOM nodes).
- Decisions: D-054 (approvals show the exact command; "always" creates an exact rule), D-059 (no real provider yet: build and test against `AgentLaunch::Fake`), D-062 (remembered rules are global: the approval UI says "in any folder").

## Owned paths
- `app/src/**` and `app/scripts/**`, except the M1-09a files below, which you may change only where this task needs it, and say so in the handoff: `app/src-tauri/**` (only new commands the UI needs, behind the same rules as M1-09a's), `app/src/bridge*`, `app/src/windows.ts`, `app/src/themes*`.
- `app/package.json`, `app/pnpm-lock.yaml`: dependencies, each one justified in the handoff (keep the bundle small).
- `docs/agents/handoffs/M1-09b-home-chat.md`
- Not yours: `crates/**`, `bindings/**`, the contract. If the daemon or the wire can't do something you need, append a change request and build around it.

## What to build
- **Transcript.** User and agent messages, tool calls with their state and capped output, turn boundaries, errors, session start/end, usage and context. Streaming deltas render as they arrive at 60 fps. Markdown to DOM nodes per §12 (no `innerHTML` of model text, raw HTML as text, links per §12 with the URL on hover, `file:` links revealed not opened, images per §12). A view keeps at most `limits::MAX_LOADED_TRANSCRIPT_BYTES` rendered; older parts are paged back from the client on scroll (§11).
- **Composer.** Send (`SendUserMessage`), interrupt, the draft from M1-09a's per-view state, a new Home chat from the composer of an empty view.
- **Inbox.** The list of what needs the user (approvals, questions, failures, finished turns), from the daemon's inbox, ordered as §8 says. Selecting an item opens its thread.
- **Approval cards** (in the transcript and the inbox): tool, the exact command or paths (D-054), why it asks, Allow / Deny, and "Always…" with the scopes the request's `rememberable` allows (this thread / Home / always), worded "in any folder" (D-062). A stale approval (the turn moved on) can't be allowed: the daemon refuses and the card says so.
- **Question forms**: the provider's question, its options or free text, answer and cancel.
- **⌘J** jumps to the next item that needs the user, from anywhere (the inbox footer already names it).
- **Settings sections**: Agents (`ProbeProviders`: what is installed, its enforcement), Approvals (remembered rules: list and delete), Notifications (on/off per kind). Use M1-09a's registry.
- **Upgrade** (§10): what the `Incompatible` banner needs from the thread list (running turns), if M1-09a left it.

## Definition of Done
- [ ] Frontend tests (vitest) for: markdown rendering against §12 (each rule, with hostile inputs: `javascript:` links, raw `<script>`, `<img onerror>`, SVG, huge data images, nested fences), the transcript reducer (deltas, tool output replace/append, incarnations), the loaded-bytes bound and paging, the inbox order, approval card states (fresh, stale, resolved elsewhere), ⌘J order.
- [ ] Against a real `atriumd` with the fake agent: a Home chat streams, a second window shows the same thread, the daemon is killed mid-stream and the view recovers. Approvals and questions: the fake agent can't raise them, so test them in vitest against recorded `ServerMessage` sequences, and append a change request for a fake-agent option that raises one approval and one question (so M1-13 and the gate can test them live).
- [ ] Screenshots on macOS (`app/scripts/screenshots.py`, extended), light and dark: a streaming chat with markdown and a tool call, an approval card, a question form, the inbox with items, Settings → Agents and Approvals.
- [ ] §14 streaming run (10 threads × 1k tok/s for 30 s) with the fake agent: frame statistics and the WebContent peak, reported (§11).
- [ ] `startup_bench.py` still within the gate on macOS.
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test -p atrium-app`; `pnpm typecheck`; `pnpm test`; `pnpm build`; `scripts/check-protocol-ts`.
- [ ] Handoff: what was run, the numbers, the change requests, and what needs a real provider (M1-07) to check.

## Rules
- GUI runs and screenshots are exclusive: wrap them in `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock <command>` (docs/agents/README.md). Other workers may be measuring.
- Stop every app and daemon you started before finishing.
- After M1-09a is merged into `dev`, the orchestrator will tell you to rebase; until then commit on your branch only.

## Out of scope
Topics, workspaces, the browser, M2 states, packaging. Real providers (M1-07, M1-08).

## Escalate if
Anything would weaken §12; the bridge or the wire can't express something (change request); the streaming budget can't be met.
