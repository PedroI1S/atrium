# Contract change requests

Append only. Format: `## CR-<nn> — <task-id> — <date>`, then what to change, why, and who is affected.

## CR-01 — M0-06-ui-shell — 2026-10-25

Non-blocking; the spike continued with component-token stopgaps.

**Change**: add to the required token set in `docs/contracts/m0.md` §4, or state explicitly that they are component tokens a theme may omit:
- `--scrim` and `--shadow-overlay` (overlay shadow). PLAN §3.20 says every shadow is a custom property; §4's semantic list has none. The spike defines both as component-layer defaults in `base.css`.
- `--syntax-*` (keyword, string, number, comment, function, type, variable, property, punctuation). §4 says Shiki/CodeMirror themes come from `tokenColors`, but nothing says where non-Shiki surfaces (the file preview, inline code) get syntax colours. The spike uses `--syntax-*` component tokens, filled from `tokenColors` by the VS Code importer.
- `--glass-blur` (blur radius for `--surface-glass` panels). The spike defines it as a component token.

**Why**: components may only use semantic and component tokens, and these values are needed by every surface that floats or shows code.

**Affected**: M1 UI crates (token names), the theme loader's required-set validation. No Rust contract is touched.

## CR-02 — design-ui-direction — 2026-10-25

Proposal from the design session (`docs/design/ui-direction.md`, Pedro chose layout B). Non-blocking; nothing is implemented yet.

**Change** to `docs/contracts/m0.md` §4:
1. **Runtime window tokens, set by the app from user settings, never by a theme**: `--window-alpha-chrome`, `--window-alpha-content`, `--window-opacity`, `--window-blur`. A theme that defines one of these names is a load error (same rule as name collisions).
2. **Surface channel tokens**: for `surface-0`, `surface-1`, `surface-2` and `surface-overlay`, the loader also emits `--<name>-rgb` (`"R G B"`). Components paint translucent surfaces with `rgb(var(--surface-0-rgb) / var(--window-alpha-chrome))`. This works on WKWebView and WebKitGTK without relative colour syntax. Theme surfaces must therefore be opaque hex, which the contrast check already assumes.
3. **`surface-glass`** stays for overlays that are always translucent (popover backdrops). Window translucency no longer uses it.
4. **New semantic tokens** for overlays that must stay correct on a translucent surface: `--hover`, `--selected`, plus `--state-needs-you-bg`, `--state-failed-bg`, `--state-ready-bg`, `--state-running-bg`. The draft themes define them as component tokens (`hover-bg`, `selected-bg`, `state-*-bg`) until this is decided.
5. **Fold CR-01 in**: make `shadow-overlay`, `scrim` and `glass-blur` required; the draft themes define them, plus `shadow-card` and `shadow-window`.
6. **Contrast check**: add the four state colours as text on `surface-0..2` (they are used as text in the inbox and pills).

**Why**: D-023 makes window opacity a core setting, with a background-only mode and a whole-window mode (Pedro, design session). Opacity is a user setting that applies across every theme, so it can't live in theme files.

**Affected**: the theme loader (emit channel tokens, reject runtime names, extra contrast pairs), the M1 UI components (surface painting), and the draft themes in `docs/design/themes/`. No Rust contract is touched. The macOS side needs `macOSPrivateApi` (already noted in M0-06 §1), which is a distribution decision.

## CR-03 — M0-07-stream-perf — 2026-10-26

Non-blocking for M0 (the spike forwarded events as they are). Two separate problems, one proposal for the first.

**1. Wire size (what the spike measured).** `MessageCompleted.text` repeats the concatenation of all the message's deltas. A 5-minute stream at 10k tokens/s is one 12 MB message, within 4.8 MB of `MAX_FRAME_LEN` (16 MiB), and every consumer (client, bridge, webview) receives and decodes it again although the UI already holds the text.

**2. Lifecycle-fold memory (not fixed by item 1).** The M0-05 daemon peaked at 487 MB footprint at session close for this stream because `close_thread` materializes the whole event history (about 3 million envelopes, `spikes/daemon-socket/src/daemon.rs`), not because of the 12 MB completion text. Omitting that text does not remove this cost. It needs the M0-05 follow-up already listed (incremental fold or persisted open-object projections) and is out of scope here.

**Change proposed for item 1** (`crates/atrium-protocol/src/agent.rs`, M1 revision; versioned):
- Replace `MessageCompleted.text: String` by `body: MessageBody` where `MessageBody` is `Inline { text }` (whole-message providers, and short messages) or `Streamed { chars: u64, deltas: u64, sha256: String }` (the text is the in-order concatenation of the stored `MessageDelta`s of this `message_id`; `chars` is its length in Unicode scalar values, `sha256` is over its UTF-8 bytes).
- Reconstruction rule: a consumer that needs the full text concatenates the stored deltas and checks `chars` and `sha256`; a mismatch is a hard error (it indicates a lost or reordered delta), not something to patch up. Deltas of a message with a `Streamed` completion must be retained by the store at least as long as the completion (history export and resume need them).
- Protocol version bump in `Hello`/`Welcome`; the daemon keeps writing `Inline` for events recorded before the change and a reader accepts both. During one release the daemon may send `Inline` to clients that announce an older version.
- Adapters: the closing-order rule in `agent.rs` ("open messages first, `MessageCompleted` with the text so far") stays; the adapter emits `Streamed` when it has emitted deltas and `Inline` otherwise.
- Update together: UI reducers (the spike's `model.ts` reads `text.length` and would break on absent text; its `completedMismatch` check becomes `chars`/`sha256` verification), the daemon's lifecycle fold, and the concatenation fixtures in `fixtures/` (add: streamed completion with matching and non-matching hash, completion-only provider, replay across a reconnect).

**Why now**: the choice affects every adapter and reducer in M1, and the wire limit is within reach of a long single message.

**Affected**: `atrium-protocol`, `atrium-agents` adapters, daemon fold and store, UI reducers, fixtures.

## CR-04 — M0-06b-ui-shell-layout-b — 2026-10-26

Feedback on CR-02 and three small amendments to `docs/design/ui-direction.md`, from building and measuring layout B (fix round 1). Non-blocking; the spike implements what is described here.

**Change**:
1. **ui-direction §5, whole window:** replace "the window's CSS `opacity` is `a`" with "the native window alpha is `a` (`NSWindow.alphaValue` on macOS)". CSS `opacity` on the root cannot fade the native traffic lights or the window shadow, and it forces a compositing layer over the whole page. CR-02's `--window-opacity` stays as the runtime token that carries `a`; components never read it.
2. **ui-direction §3 Settings, Advanced → "Blur strength (0–48 px)":** macOS's `NSVisualEffectView` has no public radius, so the native blur behind the window is not adjustable. Either drop the row or keep it for the CSS blur of in-window overlays only (palette, popovers), which is what `--window-blur` drives in the spike. The spike shows it disabled with "macOS sets the blur radius; not adjustable".
3. **CR-02 point 2:** state that surfaces must be **six-digit** opaque hex (the loader rejects shorthand and alpha), and that `--<surface>-rgb` names are reserved derived outputs (a theme defining one is a load error, like the runtime names).
4. **Component tokens for layout B** that the Calm themes should own (the spike defines them in `base.css` for now): `advanced-width` (400 px), `panel-width-editor` (380 px), and the drag bounds `sidebar-min-width`/`sidebar-max-width`, `panel-min-width`/`panel-max-width`.
5. **Accessibility (ui-direction §5):** add "Atrium observes the OS setting (`NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`) and repaints every window when it changes". The spike does this; re-checking on focus is not needed.

**Why**: (1) and (2) describe what the platform can do; (3)–(5) close gaps the spike hit. Details and measurements: `docs/spikes/M0-06b-ui-shell-layout-b.md` ("CR-02 feedback").

**Affected**: `docs/design/ui-direction.md` §3 and §5, the Calm theme files' `[component]` tables, CR-02's text. No Rust contract is touched.

## CR-05 — M1-10-host (orchestrator, from review) — 2026-11-09

The M1 contract (§4, and `crates/atrium-host/src/peer.rs` doc comments) names `SecCodeCreateWithAuditToken` for macOS peer verification. Sonnet's M1-10 review found it isn't in the public Security headers: it's exported, but undocumented, and linking it directly would make the daemon fail to load on a macOS that lacks it.

**Change**: in §4 and on `verify_app_peer`, "audit token → `SecCodeCreateWithAuditToken`" becomes "audit token → `SecCodeCopyGuestWithAttributes` with `kSecGuestAttributeAudit`" (the documented route to the same code object). Also make explicit that the unsigned dev-build cdhash comes from the **running** (dynamic) code object, which §4 already intends ("never a hash of the file on disk"). No signature or behaviour change. macOS 11 is the stated minimum.

**Why**: an undocumented symbol is an ABI and availability risk on the security-critical path.

**Affected**: `docs/contracts/m1.md` §4 wording, doc comments in `crates/atrium-host/src/peer.rs` (M1-10 updates them in its fix round). **Approved by Pedro (driver), 2026-11-09 (D-052)**; applied to `docs/contracts/m1.md` §4.

## CR-06 — M1-03-policy (orchestrator, from the Opus security review) — 2026-11-09

Two contract-level findings from `docs/agents/reviews/M1-03-policy.md`. They change security defaults, so they need Pedro's approval (D-013). **Approved by Pedro (driver), 2026-11-09, option (a) (D-054)**; applied to `atrium-protocol` and contract §5 and §9.

**1. `date` guard (finding 3).** On GNU `date` (Linux, and Homebrew's coreutils on macOS), `-f FILE` prints every line of FILE in an error message. In `HOME_READONLY_COMMANDS` (`crates/atrium-protocol/src/policy.rs`), the `date` entry's `forbidden` goes from `["-s", "--set"]` to `["-s", "--set", "-f", "--file"]`. BSD `date -f fmt` would then ask too. The worker's fix (resolving every suffix of a short-flag cluster) already closes the reported bypass. This is defence in depth.

**2. Remembered command rules match extra arguments (finding 12).** "Always allow `rm -rf build`" stores `CommandPrefix ["rm","-rf","build"]`, and that rule also allows `rm -rf build ~/Documents`. The options:
- (a) remembered rules match the whole argv exactly (a new `RuleMatcher::CommandExact`), and prefix rules can only be created in the rules editor. **Recommended.**
- (b) keep the prefix match and word the UI "commands starting with …".

**Affected**: `atrium-protocol` policy (`HOME_READONLY_COMMANDS`, and `RuleMatcher` under (a)), contract §5 and §9, M1-03's engine and tables, and the M1-07/M1-08 approval UI wording.

## CR-07 — M1-06-client (orchestrator) — 2026-11-10

Non-blocking. `atrium-client` is built against the wire as frozen and works with it; these are three places where the wire can't say what the client needs. Each has a stated workaround. They change `wire.rs`, so they need the driver's approval. **Decided by rodrigo (driver), 2026-11-10 (D-058): points 2 and 3 approved, point 1 put off until after the M1 gate.** Points 2 and 3 are applied to `wire.rs`, the serde shape test, `bindings/protocol.ts`, contract §6.2 and `atrium-client` (which no longer infers a removal from its own `ArchiveThread`).

**1. No way to read a range of a thread's history.** A thread is loaded by `Subscribe { after: None }`, which replays every event from its start. The client folds that into its cache and serves snapshots and older pages from there (contract §11, "paged back from the client"). Two costs:
- Opening a large thread means replaying all of it, however little the view shows.
- The cache is capped at 64 MiB per thread (`atrium_client::MAX_CACHED_THREAD_BYTES`). Past that the oldest history is dropped and can't be paged back: snapshots and pages say `truncated`. The history is still in the store and in exports.

**Change proposed:** `DaemonCommand::ReadEvents { thread_id, before: Option<EventId>, limit: u32 }` (value: events before `before`, newest first), with `Store::events_before` behind it. The client would then keep only a tail and page the rest from the daemon. Affects `wire.rs`, the `Store` trait and `SqliteStore`, M1-05, and the client's cache.

**2. A thread that is archived leaves no trace on the control lane.** `ThreadUpdated` carries a `ThreadSummary`, which has no "archived" field, and no message says a thread left the list. The client removes the thread and tells its windows (`Delivery::ThreadRemoved`) when its own `ArchiveThread` succeeds. A thread archived by anything else stays in the app's list until the next reconnect.

**Change proposed:** `ServerMessage::ThreadRemoved { thread_id }` on the control lane for inbox subscribers. Affects `wire.rs`, M1-05, and one line in the client.

**3. What follows a replaced subscription.** `wire.rs` says subscribing again "replaces the earlier subscription and replays from the new cursor". The client relies on a stronger reading: once the daemon has answered a `Subscribe`, it sends no event and no `ReplayDone` of an earlier subscription to that thread on that connection, including frames it had already queued. Without that, a late event of the old subscription can be taken for the first event of the new replay. The client drops everything for the thread between sending `Subscribe` and its answer, which is safe only with this rule. The same applies after `Unsubscribe` followed by a new `Subscribe`.

**Change proposed:** state that rule in `wire.rs` ("Session", step 2), and say which `ErrorCode` a cursor beyond the thread's last event gets (the client treats any error but `UnknownThread` on a subscribe that carried a cursor as "reload the thread from its start"). Affects `wire.rs` docs and M1-05; M1-13 should test it.

## CR-08 — M1-05-daemon (orchestrator) — 2026-11-10

Non-blocking; it needs the driver's approval because it adds a method to the frozen `Store` trait.

**`ResumeThread` can't read back how a thread was launched.** `NewThread` carries `effort` and `preset`, and the store keeps both (`threads.effort`, `threads.preset`), but nothing in `Store` returns them: `ThreadSummary` has the provider and the model only. A resumed incarnation needs both for its `SessionConfig` and its `SessionGate`.

**Workaround in M1-05 until this is decided:** a resumed thread runs with the Home preset (M1's only preset in use, contract §9) and no effort setting, so a thread started with an effort level loses it when resumed.

**Change proposed:** `Store::launch(&ThreadId) -> StoreResult<Option<NewThread>>` (the row as it was created, with the current title), implemented in `SqliteStore`'s reader. Affects `crates/atrium-core/src/lib.rs`, `store/`, and one function in the daemon.


## CR-09 — M1-05-daemon (orchestrator, from the Codex review) — 2026-11-11

Needs the driver's approval, and Pedro's, since it changes what a thread is shown as (contract §8, §10). Found by the review of M1-05 (`reviews/M1-05-daemon-codex.md`, finding 4).

**A thread that was stopped can't be shown as "cleanup unverified" afterwards.** Contract §10 says a thread whose process can't be verified "is never shown as stopped". Contract §8 and `attention::next_state` say `Ended`, `Failed` and `Interrupted` are sticky until the next start, and that a user's stop is `Ended` at once. The two meet in this order:
1. The user stops a thread (`Signal::Stopped`): it is `Ended`.
2. The adapter then fails to end its session, and the daemon's check of the provider process comes back `Unverified` (the leader is gone and its group isn't empty, or the process couldn't be inspected).
3. The daemon sends `Signal::Recovered { cleanup: Unverified }`, which `next_state` ignores because `Ended` is sticky.

The thread stays `Ended` while something of it may still be running. The same happens from `Interrupted { Verified }` (full storage, then a failed cleanup) and when the daemon is killed between the user's stop and the session's end.

**What M1-05 does until this is decided:** it sends the `Unverified` signal before the closing events, so the case is right whenever the thread was still in a live state. From a state that is already sticky, the thread keeps that state; the `SessionEnded` the daemon stores says `"the daemon restarted"` or the session's own reason, and nothing else records the doubt.

**Change proposed:** in `next_state`, `Signal::Recovered { cleanup: Unverified }` moves `Ended` and `Interrupted { cleanup: Verified }` to `Interrupted { cleanup: Unverified }` (not `Failed`, which keeps its message). Affects `crates/atrium-core/src/attention/mod.rs`, its transition table test, and the wording of contract §8.

## CR-10 — M1-05-daemon (orchestrator, from the Codex review) — 2026-11-11

Non-blocking; it adds a method to the frozen `Store` trait. Found by the review of M1-05 (finding 2).

**Recovery can't find an unfinished incarnation of an archived thread.** At its start the daemon closes every incarnation a previous daemon left unfinished (contract §5, §10). It finds them through `Store::live_processes` (incarnations with a recorded process) and `Store::threads` (which leaves archived threads out). `ArchiveThread` stops the session and archives the thread at once, so a daemon killed in between leaves an archived thread whose session never ended. If it had no recorded process (a synthetic session, or the record was already cleared), recovery never sees it.

**What M1-05 does until this is decided:** the thread's requests are expired with all the others at start, so nothing of it can be approved. Its open messages, tool calls and turn stay open in the store, and its history has no `SessionEnded`. The thread is hidden, so only an export shows it.

**Change proposed:** `Store::unfinished_incarnations() -> StoreResult<Vec<(ThreadId, u32)>>`: every thread's current incarnation that has events and no `SessionEnded`, archived or not, from the sessions table. Recovery would then use it instead of reading each listed thread's last event. Affects `crates/atrium-core/src/lib.rs`, `store/`, and `upkeep::recover`.

The same method would let recovery clear a process record whose incarnation already ended (left by a daemon killed between storing `SessionEnded` and clearing the record). `Store::live_processes` leaves ended incarnations out, so today nothing lists such a record; nothing uses it either (round 2 of the review, finding 8).

## CR-11 — M1-09a-app-shell (orchestrator) — 2026-11-12

Non-blocking for the review; needed before the merge. The M1-09a brief keeps `unsafe` denied and asks for a change request if the macOS window code can't be written without it. It can't, and the code referred to this CR without anyone writing it. Written on Pedro's shift, when M1-09a first compiled and ran on macOS.

**What needs `unsafe`.** Only `app/src-tauri/src/macos.rs`, which has `#![allow(unsafe_code)]`; the workspace lint stays `deny` everywhere else. It has four `unsafe` blocks, each with a `// SAFETY:` comment:
- three dereferences of the `NSWindow` pointer Tauri hands out (`ns_window()` returns `*mut c_void`): to set the window alpha in `set_alpha` and in `apply_to_new_window`, and to read `windowNumber` in `window_number` for the harness screenshots;
- registering the accessibility notification observer (`addObserverForName_object_queue_usingBlock`, `unsafe` in `objc2-foundation`), for "Reduce transparency" and "Increase contrast" (contract §11.1).

Nothing else in the app uses `unsafe`; `window-vibrancy` does its own.

**Change proposed:** allow `unsafe_code` in that one module (on macOS only, since it is `cfg(target_os = "macos")`), with a `// SAFETY:` comment on every block, and an Opus 5.5 audit of the four blocks before the merge, as for M1-10 (D-053). Any new `unsafe` in the app goes through a change request.

## CR-12 — M1-13-adversarial (orchestrator, from the worker's finding 6) — 2026-11-12

**Security default; Pedro decides (D-013).** In Home with no rules, the policy auto-allows reading credential stores that aren't on the contract's hard-deny list (§9, `HARD_DENY_DEFAULTS`). The worker's examples: `cat ~/.cargo/credentials.toml`, `~/.pypirc`, `~/.pgpass`, `~/.vault-token`, `~/.azure/accessTokens.json`, `~/.aws/sso/cache/*`, `~/.config/git/credentials`, shell history files, `~/.env`, and `git show HEAD:.npmrc`. They are tested in `crates/atrium-core/tests/adversarial_*.rs` on `agent/sonnet/M1-13-adversarial`, ignored as "M1-13 finding 6".

**Change proposed:** add these to the hard-deny credential list, with the same identity and hard-link handling as the existing entries. Also make `git show`/`git cat-file` of a path matching a credential name ask rather than auto-allow. Affects `atrium-protocol`/`atrium-core` policy defaults, contract §9, and the Settings text that lists what is protected.

**Related, for the same decision:** findings 1 and 2 (a `cd` inside a subshell or pipeline is not followed; a failing `cd` is assumed to succeed). With two ordinary prefix rules (`cd`, `cat`), `cd nope; cat .ssh/id_rsa` reaches Allow. These are engine fixes rather than contract changes, but they reach Allow on a hard-denied path, so they should be fixed before the M1 gate (worker's handoff has proposed fixes for 1, 2, 7 and 8).
