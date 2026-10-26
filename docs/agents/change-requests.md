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

Non-blocking; the spike forwarded the events as they are. (Number may need renumbering at merge if another CR-02 exists.)

**Change** (`crates/atrium-protocol` agent contract, M1 revision): stop repeating the full text in `MessageCompleted` for messages whose deltas were stored. Options: `text` becomes optional and absent when the deltas are in the store, or it carries `{len, sha256}` so consumers can verify the concatenation; whole-message providers keep sending `text`.

**Why**: `MessageCompleted.text` is the concatenation of all deltas. A 5-minute stream at 10k tokens/s is one 12 MB message, close to `MAX_FRAME_LEN` (16 MiB), and building it made the M0-05 daemon peak at 487 MB at session close (M0-07 memory run). Every consumer (bridge, webview) pays for it again, and the UI already holds the text.

**Affected**: `atrium-agents` adapters (the closing-order rule in `agent.rs` still holds), the daemon's lifecycle fold, UI reducers. No M0 spike needs it.
