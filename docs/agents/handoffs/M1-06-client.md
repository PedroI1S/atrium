# Handoff: M1-06-client

Model: Claude Fable 5 (the orchestrator, inline; D-055) · Status: **partial** — written and tested, not reviewed, not merged (the shift ended here)

## Review (done, 2026-11-10, rodrigo's shift)
Gemini 3.8 Flash, two rounds: **approve with changes**, then **approve** (`docs/agents/reviews/M1-06-client.md`, `M1-06-client-round2.md`). Fixed on the branch (`328a128`):
- From the review:
  - A `ReplayDone` while replaying that doesn't end at `replay_until`, or comes before the events up to it, is a protocol violation. Before, the view never loaded.
  - The peer's uid is checked on the connected socket, not only on the path.
  - A folded tool call is counted at what it holds (text added, content replaced), not at its frames' sizes.
- Found by the orchestrator while reading:
  - `Usage`, `ContextUpdated` and `QuotaUpdated` fold only within the last segment. Before, a later value was folded into the first entry wherever it was, so a tail snapshot could lack the latest one.
  - Connections that drop within 2 s of connecting, twice or more in a row, are followed by a wait (100 ms doubling to 1 s). `Client::reconnect` clears the count.
  - The aborted read task is awaited before the state hears the connection ended, so no message of an old connection is applied after the next one subscribed.
- Declined: a command timeout in the client. A command has one response and is never retried, and dropping the `Client::command` future is safe, so **the bridge (M1-09a) sets its own deadline per command**. A dropped command stays in the client's table until its answer or the connection's end.
- Not a defect, but worth knowing: an item that never closes within an incarnation (a tool call with no `ToolCallFinished`) keeps its segment open, and an open segment is never evicted. The adapters' output caps bound it.

## Done
- New crate `crates/atrium-client`, added to the workspace. No dependency that wasn't already in `Cargo.lock` (`rustix` for the effective uid; `specta` behind the optional `ts` feature).
- 28 unit tests (state machine, history cache, codec) and 11 tests over a real unix socket against a scripted daemon.

## How it is built
| File | What it holds |
|---|---|
| `lib.rs` | The public API: `Client`, `Window`, `WindowHandle`, `ClientOptions`. |
| `state.rs` | Everything the client knows and decides, behind one mutex, with no I/O. Unit-tested without a socket. |
| `cache.rs` | One thread's history, folded. |
| `connection.rs` | The socket: checks, handshake, reconnecting, the read and write tasks. |
| `codec.rs` | Frames. |
| `delivery.rs` | What windows receive (`Delivery`), `ConnectionState`, `CommandError`. |

**Connection.** Before each connect the client checks that the socket's folder is a real directory, owned by the user and closed to others, and that the socket is a socket of the user's. What ends a connection decides what follows:
- dropped or stalled: reconnect at once; from the second connection in a row that dropped within 2 s, after a wait (100 ms doubling to 1 s);
- nothing listening: the start hook runs on the first failed attempt and every 20th after, and attempts continue;
- `Incompatible`, an unsafe socket, or a protocol violation: stop until `Client::reconnect`. None of these runs the start hook.

**Commands.** One response each, never sent twice. A command with no connection fails with `NotConnected` (it is not queued for later). One in flight when the connection drops fails with `UnknownOutcome`. One over `MAX_CLIENT_FRAME_LEN` fails with `TooLarge` before anything is sent.

**Threads.** The client subscribes to a thread once, while some window views it, and keeps its history folded: a message's deltas are one delta, a tool call's updates one update, and running usage keeps the latest. A message of a million deltas costs its text. Applying the folded entries gives the same state as applying the events.
- Entries are grouped into segments that start where no message, tool call or request is open. Snapshots, pages and eviction use whole segments, so a view never starts with a tool result without its call.
- After a reconnect or `SubscriptionLagged`, the client subscribes from the cache's last event. A cursor the daemon rejects reloads the thread from its start; a second failure fails the thread for its viewers.

**Windows.** Each window has a queue bounded by `MAX_WINDOW_QUEUE_BYTES` over what it has not acknowledged: deliveries waiting to be taken plus those taken and not yet acknowledged.
- A live delivery that doesn't fit empties the waiting part and marks what the window shows as stale. Stale state is sent again in full when there is room, which is when the window acknowledges: the connection state, the inbox, and a snapshot of each thread it views.
- A new view of a thread starts with a snapshot (the newest segments, about `MAX_LOADED_TRANSCRIPT_BYTES`), then live events. A second window on the same thread gets its snapshot from the cache with no new subscription.
- Live events, `ThreadUpdated` and `InboxUpdated` reach windows as the daemon's own JSON, not re-encoded.
- The reader task never waits on a window, so a hung window delays neither another window nor a command. The socket test for this pushes 40 MiB at a window that never acknowledges while another window receives all of it in order.

## Numbers
- Reconnect, then 1,000 events to catch up, against the scripted daemon (release build, 20 runs): median 3.2 ms, p95 5.7 ms. This is the client's share only; contract §14 gates the figure against the real daemon (p95 under 100 ms).

## For M1-05 (`atriumd`)
- The client depends on the three points in **CR-07**. Point 3 matters most: after answering a `Subscribe`, send nothing of an earlier subscription to that thread, queued frames included.
- A subscribe response must carry `replay_until` (a missing key is a protocol violation). `ReplayDone.last` must equal it and must come after every event up to it: anything else is a protocol violation and closes the connection.
- The client always sends `SubscribeInbox` first after `Welcome`. A refused or unreadable answer to it is a protocol violation.
- A response with a request id the client didn't use is a protocol violation, and so is any `Stream` frame in M1.

## For M1-09a (the bridge)
- `Client::start` needs a Tokio runtime and returns at once. Keep a `Client` alive: when the last one is dropped the connection closes.
- One `Client::open_window(label)` per webview, and a task that forwards `Window::recv()` to it (`seq` and the JSON string). Opening a label again replaces the old window, which is what a reloaded webview needs.
- The webview must acknowledge (`WindowHandle::ack(seq)`) as it applies deliveries. Without acknowledgements a window falls behind after 16 MiB.
- `Delivery::Snapshot` replaces the view's state for that thread and `Delivery::Inbox` replaces the thread list and inbox. Either can arrive at any time.
- A snapshot's events are folded and may start after the thread's first event (`has_older`). `WindowHandle::page_before` returns the segments before a given event.
- The start hook is where the app calls `atrium_host::lifecycle::spawn_detached`.
- TypeScript types for `Delivery`, `ConnectionState` and `Page` come from the crate's `ts` feature (`specta::Type` derives). Nothing exports them to a file yet.
- `ConnectionState::Incompatible` carries what the upgrade flow of contract §10 needs (pid, version, running turns); the signals are the app's to send.

## Not done, and why
- **Draft recovery and forced-close tests** (contract §11) belong to the app; the client's part is that a closed or reopened window frees its queue and its views, which is tested.
- **A thread's cache is freed when its last viewer leaves.** Switching back to a thread loads it again from the daemon. Keeping recently viewed threads warm would be a small addition if M1-09b wants it.
- **Sequence gaps are counted, not acted on** (`ThreadCache::gaps`). The envelope's `seq` should rise by one within an incarnation; the client applies events in id order regardless.

## Definition of Done results
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean, also with `--features ts` for this crate.
- `cargo test --workspace`: 222 passed, 0 failed, 1 ignored (also on Linux, rodrigo's machine) (`consistency_all_fixtures`, owned by M1-07 and M1-08).
- `cargo test -p atrium-client`: 28 unit and 11 socket tests, run three times in a row with the same result.
- No `unsafe`, and no `unwrap`, `expect` or `panic!` outside tests.
