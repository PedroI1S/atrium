# Handoff: M1-06-client

Model: Claude Fable 5 (the orchestrator, inline; D-055) · Status: **partial** — written and tested, not reviewed, not merged (the shift ended here)

## Next step, for whoever continues
1. Run the Gemini 3.8 Flash review (D-056) on branch `agent/fable/M1-06-client`, commit `5a88cb8` or later. Nothing else is outstanding: the code is complete and every check passes.
   - `git worktree add --detach ../atrium-wt/M1-06-review agent/fable/M1-06-client`, write the prompt to a file, then `scripts/agent-run agy gemini-3.8-flash-medium M1-06-review <prompt-file>`. The review lands in that worktree at `docs/agents/reviews/M1-06-client.md`.
   - Ask for exact failing sequences, not a general opinion. On M1-11 a general prompt got "approve, no findings"; prompts that asked for a concrete input per claim found four real defects.
   - What to have it attack:
     - **Windows** (`state.rs`): a sequence of deliveries, takes and acknowledgements after which a window has a wrong or incomplete view (an event lost or applied twice across a snapshot, a snapshot that never comes, a window that stays stale for ever); the byte accounting of `waiting` and `in_flight`; whether one window can delay another or a command.
     - **Subscriptions** (`state.rs`): each `Phase` transition against `wire.rs`; late answers, late events and late `ReplayDone`s; reconnect from the cursor; `SubscriptionLagged`; the reload after a rejected cursor; whether any path can loop sending `Subscribe`.
     - **The cache** (`cache.rs`): is applying the folded entries always equal to applying the events (tool-call update merging, in-place `Usage`); can a segment boundary fall inside an open item; entry numbers after eviction; ids rising along the list.
     - **The connection** (`connection.rs`): which endings are retried and which are not; the socket checks and the window between the check and the connect; cancellation of the read task mid-frame; what a panic in a task does.
     - **Tests**: which rule has no test, and which test only mirrors the code.
2. Fix what it finds on the branch, ask it to look at the fix, then merge into `dev` (`git merge --no-ff`), copy the review files to `docs/agents/reviews/`, update the status row, and remove the worktrees and the branch.

## Done
- New crate `crates/atrium-client`, added to the workspace. No dependency that wasn't already in `Cargo.lock` (`rustix` for the effective uid; `specta` behind the optional `ts` feature).
- 26 unit tests (state machine, history cache, codec) and 10 tests over a real unix socket against a scripted daemon.

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
- dropped or stalled: reconnect at once, then with backoff (50 ms doubling to 1 s);
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
- A subscribe response must carry `replay_until` (a missing key is a protocol violation), and `ReplayDone.last` must equal it. The client only goes live on a `ReplayDone` that matches.
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
- `cargo test --workspace`: 219 passed, 0 failed, 1 ignored (`consistency_all_fixtures`, owned by M1-07 and M1-08).
- `cargo test -p atrium-client`: 26 unit and 10 socket tests, run three times in a row with the same result.
- No `unsafe`, and no `unwrap`, `expect` or `panic!` outside tests.
