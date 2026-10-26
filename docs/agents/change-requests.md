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
