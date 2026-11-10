# M1-06-client: `atrium-client`

Model: Claude Fable 5 (the orchestrator, inline; D-055) · Reviewer: Gemini 3.8 Flash (D-056) · Depends on: contract (M1 rev 3)
Branch: `agent/fable/M1-06-client` · Worktree: `../atrium-wt/M1-06-client`

## Goal
The app's one connection to `atriumd` and the only state authority in the app process (contract §11, D-028). It keeps the connection alive, replays what was missed, holds what each viewed thread needs, and delivers to every window through that window's own bounded queue, so a hung or hidden window can't stall another window or a command.

## Context
- `docs/contracts/m1.md` §4 (roles), §6.2 (wire, lanes, lagged subscriptions), §10 (starting the daemon, upgrades), §11 (client and windows), §14 (reconnect budget).
- `crates/atrium-protocol/src/wire.rs` (framing, session, cursors and retries), `limits.rs` (`MAX_CLIENT_FRAME_LEN`, `MAX_WINDOW_QUEUE_BYTES`, `MAX_LOADED_TRANSCRIPT_BYTES`).
- Prior art: `spikes/daemon-socket/src/client.rs` (M0-05). It had one event stream for the whole app; this task replaces that with per-window delivery.
- `docs/agents/m1-must-fix.md`: "Per-window delivery/ACK state", "Streaming memory".

## Owned paths
- `crates/atrium-client/**` (new crate)
- root `Cargo.toml` and `Cargo.lock`: the new member (the orchestrator's files; this task is the orchestrator's)
- `docs/agents/handoffs/M1-06-client.md`

## What to build
- **Connection.** Checks that the socket's folder is private and ours before connecting. `Hello` with the role, `Welcome` or `Incompatible`. Reconnects with backoff; a hook starts the daemon when nothing is listening. An unsafe socket, a protocol violation and `Incompatible` are never retried on their own and never start a daemon.
- **Commands.** One response each, never retried. A command in flight when the connection drops is reported as "unknown outcome". A frame over `MAX_CLIENT_FRAME_LEN` is refused before it is sent.
- **Thread state.** For each thread some window views, the client subscribes once and keeps the thread's history folded (a message's deltas joined, streamed tool output joined), so memory follows the transcript's size, not the event count. Reconnects and `SubscriptionLagged` resubscribe from what the client has; a cursor the daemon rejects reloads the thread.
- **Windows.** Each window has its own queue, bounded by `MAX_WINDOW_QUEUE_BYTES` over what it has not acknowledged. A window over the bound loses its queue and is marked as needing a resync; when it acknowledges again it gets the current state: connection, inbox, and a snapshot of each thread it views. A new view of a thread starts from a snapshot (the tail, at most `MAX_LOADED_TRANSCRIPT_BYTES`), then live events. Older history is paged from the client.
- **Inbox.** Thread summaries and the inbox, kept current and delivered to the windows that ask.

## Definition of Done
- [ ] Tests without a socket for the state machine: snapshot then live events; two windows on one thread; a window that never acknowledges doesn't delay another window or a command, and resyncs to a consistent state; reconnect continues from the cursor with nothing lost or repeated; rejected cursor; lagged subscription; stale events and replay markers; unknown thread; window closed and reopened.
- [ ] Tests for the history cache: folding, page boundaries that never split an item, the tail and older pages, eviction, a message of millions of deltas.
- [ ] Tests over a real unix socket against a scripted daemon: handshake, `Incompatible`, wrong version, reconnect, unknown outcome, the start hook, an unsafe socket folder, an oversized command, an invalid frame.
- [ ] No `unsafe`, no panic on anything the daemon sends.
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- [ ] Handoff with the commands' output, what M1-05 and M1-09a need to know, and anything the frozen wire can't express.

## Out of scope
The Tauri bridge and windows (M1-09a), the daemon (M1-05), draft recovery (M1-09a/b), the upgrade flow's signals (M1-09a with `atrium-host`).

## Escalate if
The wire contract can't express something the client needs: write a change request and stop that part.
