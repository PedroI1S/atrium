# M1-05-daemon: `atriumd`

Model: Claude Fable 5 (the orchestrator, inline; D-055, D-057) · Reviewer: Gemini 3.8 Flash (D-056) · Depends on: M1-02, M1-03, M1-04, M1-10, M1-11 (merged), M1-06 (merged; the client it must work with)
Branch: `agent/fable/M1-05-daemon` · Worktree: `../atrium-wt/M1-05-daemon`

## Goal
The daemon: the one process that owns the store, the agent sessions and the approval ledger, and serves them to the app over the unix socket (PLAN §3.1). It wires the merged parts together (`SqliteStore`, `DefaultPolicy`, `Ledger`, `SessionGate`, `attention`, `history`, `atrium-host`) behind the frozen wire.

## Context
- `docs/contracts/m1.md` §4 (authority), §5 (approvals: who calls the ledger when), §6.2 (wire, lanes, quotas, replaced subscriptions), §7 (store: read-only, snapshots), §8 (inbox, notifications), §10 (lifecycle, recovery, upgrades), §13 (export), §14 (reconnect and idle-memory budgets).
- `crates/atrium-protocol/src/wire.rs` (the module docs are the rules), `limits.rs`, `agent.rs` (closing order).
- `crates/atrium-core/src/lib.rs` (`Store`, `ApprovalLedger`, `Signal`, `OpenItems`), `crates/atrium-agents/src/lib.rs` (`AgentAdapter`, `Session`, `SessionConfig`), `crates/atrium-host/src/{lifecycle,peer,supervise}.rs`.
- Handoffs with notes for this task: `M1-02-store.md`, `M1-03-policy.md`, `M1-06-client.md` ("For M1-05" and "After the merge"), `M1-11-export.md` ("For M1-05").
- Prior art: `spikes/daemon-socket/src/daemon.rs` (M0-05). `docs/agents/m1-must-fix.md`: G2, X4, L8, "Lifecycle projections", "Resource exhaustion".
- Decisions: D-045, D-046, D-058 (CR-07).

## Owned paths
- `crates/atrium-daemon/**` (new crate: a library plus the `atriumd` binary)
- root `Cargo.toml` and `Cargo.lock`: the new member and its dependencies (the orchestrator's files; this task is the orchestrator's)
- `docs/agents/handoffs/M1-05-daemon.md`

## What to build
- **Start-up.** Private directories (`secure_dir`), the instance lock before a stale socket is removed, the socket at mode 0600, the store. Then recovery, before any client is accepted: every recorded live process is checked and killed with `supervise::kill_recorded`, its incarnation's closing events are stored from `Store::open_items` (never from the history), the thread gets `Signal::Recovered`, and the ledger expires everything.
- **Connections.** `Hello` within `HELLO_TIMEOUT`; `MAX_CONNECTIONS`; frames over `MAX_CLIENT_FRAME_LEN`, unknown kinds and invalid JSON close the connection. Roles as in contract §4: `App` only after `peer::verify_app_peer`, `Dev` only in a debug build started with `--dev-socket`, `Agent` refused; a failed check is `Incompatible`, never a lesser role. Every command's authority is checked first.
- **Two lanes.** Responses, `ThreadUpdated`, `ThreadRemoved`, `InboxUpdated` and `SubscriptionLagged` are written before events. The event lane is bounded by `MAX_CONNECTION_QUEUE_BYTES`; a live subscription that would overflow it is dropped with `SubscriptionLagged`.
- **Subscriptions.** The response carries `replay_until`; then the stored events after the cursor in pages of `REPLAY_PAGE_EVENTS`, `ReplayDone { last = replay_until }`, then live events, with nothing skipped or repeated at the boundary. A cursor beyond the thread's last event is `Invalid`; an unknown thread `UnknownThread`. After a `Subscribe` is answered or an `Unsubscribe` processed, nothing of the earlier subscription is sent, queued frames included (D-058). `MAX_SUBSCRIPTIONS_PER_CONNECTION`.
- **Inbox stream.** `SubscribeInbox` answers with every thread and the ranked inbox, then `ThreadUpdated`, `ThreadRemoved` (archive) and `InboxUpdated` as they change. Streaming text alone doesn't produce an update per batch.
- **Sessions.** `StartThread` and `ResumeThread` begin an incarnation, build its `SessionGate`, start the adapter (recording the process from `on_spawn`), and pump its events into the store in batches. After storing a batch the daemon drives the ledger (contract §5): `RequestResolved { cancelled | expired }` → `withdraw`, `TurnCompleted` → `expire_turn`, `SessionEnded` → `expire_incarnation`. A session whose event stream ends without `SessionEnded`, or whose adapter fails to start, is closed by the daemon in the contract's order. `AgentLaunch::Fake` is a built-in synthetic agent. Real adapters are registered by M1-07 and M1-08; until then `Native` answers `Invalid`.
- **Commands.** All of `DaemonCommand`. `ResolveApproval` and `AnswerQuestion` go to the ledger first, and only an accepted one reaches the session; every ledger refusal is `Expired`. `ExportThread` reads the thread from its first event and deletes the file on any error. `ShutdownDaemon` as in contract §10.
- **Storage full.** When the store goes read-only: `Signal::StorageReadOnly` for threads with a running turn, their sessions are stopped, new turns are refused with `StorageReadOnly`, and the inbox shows the storage-full item until space returns.
- **Housekeeping.** A snapshot once a day when no turn is running. Notifications by `attention::should_notify`, suppressed for a thread an App connection has in focus. `SIGTERM`: exit once no turn is running. `SIGINT`: stop sessions (resumable) and exit now.

## Definition of Done
- [ ] Tests over a real socket in a temporary `ATRIUM_HOME`, with a scripted adapter, for each item above. In particular:
  - replay then live with events arriving during the replay: every event once, in order, `ReplayDone` where announced;
  - resubscribing while a replay is queued: nothing of the old subscription after the answer;
  - a client that doesn't read: its live subscription lags, a command on the same connection is still answered, another connection is unaffected;
  - a stale approval (resolved twice, older incarnation, after the turn ended) is `Expired` and never reaches the session;
  - a session that dies without closing events is closed in order; a daemon killed with a live process recovers it at the next start (process gone, thread `Interrupted`, requests expired, closing events stored once);
  - role checks: `Dev` refused without `--dev-socket`, `Agent` refused, `Hello` timeout, oversized frame, too many connections.
- [ ] `atrium-client` (M1-06) against the real daemon: handshake, inbox, view a thread, reconnect from the cursor.
- [ ] Contract §14, reported with the commands: reconnect with a cursor 1,000 events behind (p95 of 20), full replay of 100k events, idle RSS 60 s after a 3M-token `Fake` session was closed (10 samples), on Linux.
- [ ] No `unsafe`, no panic on anything a client or an adapter sends.
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`
- [ ] Handoff: the commands' output, what M1-07, M1-08, M1-09a and M1-13 need to know, anything the frozen contract can't express.

## Out of scope
The adapters (M1-07, M1-08), the app (M1-09a/b), adversarial tests (M1-13), the MCP server and `Agent` connections (later milestone), `Store::events_before` and `ReadEvents` (CR-07 point 1, put off by D-058).

## Escalate if
A frozen trait or type can't express what the daemon needs: write a change request and stop that part.
