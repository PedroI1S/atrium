# M1-09a-app-shell: the app shell (layout B, tokens, windows, tray, bridge, content security)

Model: Claude Fable 5 (the orchestrator, inline; D-057, D-059) · Reviewer: Gemini 3.8 Flash, read-only, with screenshots · Depends on: M1-06 (merged), M1-05 (merged)
Branch: `agent/fable/M1-09a-app-shell` · Worktree: `../atrium-wt/M1-09a-app-shell`

## Goal
Turn the M0-06/M0-06b spike into the real app under `app/`: a Tauri 2 + SolidJS shell in layout B that connects to `atriumd` through `atrium-client`, with the token contract of §11.1, pop-out windows, the tray item, and the security rules of §12 that belong to the shell. M1-09b fills it with the Home chat, the inbox and the approval UI.

## Context
- `docs/contracts/m1.md` §4 (the `App` role), §10 (starting the daemon), §11 and §11.1 (client, windows, tokens), §12 (rendered content, CSP), §14 (budgets).
- `docs/design/ui-direction.md` (layout B, opacity, visual language) and `docs/design/themes/atrium-calm-{light,dark}.toml`.
- `crates/atrium-client/src/lib.rs` ("For the bridge"), `docs/agents/handoffs/M1-06-client.md` ("For M1-09a"), `docs/agents/handoffs/M1-05-daemon.md` ("For M1-09a").
- The spike: `spikes/ui-shell/` (report `docs/spikes/M0-06b-ui-shell-layout-b.md`), and its Linux results in `docs/spikes/M0-linux-checks.md`.
- `docs/agents/m1-must-fix.md`: L1, "Per-window delivery/ACK state", "Rendered agent content security contract".
- Decisions: D-022 (thin UI), D-024 (layout B, Calm), D-028 (one client, pop-outs), D-034 (`macOSPrivateApi`), D-042 (60 fps), D-059 (this go, and the §9 answers below).

## Decided for this task (ui-direction §9; D-059)
- Fonts: Instrument Sans and Geist Mono, bundled, with their OFL notices.
- Home is a chat like any other. The sidebar has the Inbox and Home tiles and **one list of threads** (all M1 threads are Home threads). Topic groups (§4) are not built in M1: the store has no topics, and they arrive with workspaces.
- Linux: no server-side decorations. The window draws its own controls (minimize, maximize, close) on the right of the frame's top strip; the strip is the drag region.
- The right panel exists with its minimize behaviour; in M1 its only tab is *Details* (the thread's enforcement, provider and session), filled by M1-09b.

## Owned paths
- `app/**` (new)
- root `Cargo.toml`, `Cargo.lock`: the new member and `default-members` (the orchestrator's files)
- `.gitignore`: lines for `app/`
- `docs/agents/handoffs/M1-09a-app-shell.md`

## What to build
- **Project.** `app/` (pnpm, Vite, SolidJS, TypeScript) and `app/src-tauri` (crate `atrium-app`, a workspace member that is not a default member, so `cargo test` without `--workspace` still runs anywhere; `--workspace` needs WebKitGTK's development files on Linux). `unsafe` stays denied. The macOS window alpha and accessibility observer need AppKit calls: if they can't be written without `unsafe`, they go in one module with `// SAFETY:` comments and a change request, not silently.
- **Bridge** (`app/src-tauri/src/bridge.rs`).
  - One `Client` for the process, role `App`. The start hook runs `atrium_host::lifecycle::spawn_detached` on the `atriumd` next to the app's executable. A debug build writes `Paths::app_identity` (0600) before connecting and may start the daemon with `--dev-socket` off.
  - For each webview: `Client::open_window(label)` and a task that forwards `Window::recv()` over a Tauri channel as `{seq, json}`, unchanged. The webview acknowledges after applying.
  - Commands: `daemon_command` takes a `DaemonCommand` (deserialized by serde, so nothing but a real command passes) and returns the value or a typed error. Each command has a deadline set here (M1-06 handoff); a deadline that passes is reported as "unknown outcome", never retried.
  - `view`, `unview`, `show_inbox`, `page_before`, `ack`, `reconnect`, and the connection state.
  - Window labels are made by Rust from a counter. No string from the daemon or an agent ever becomes a label, a command name or a capability (§12).
- **Windows.** The main window and pop-outs. A pop-out shows one thread; the thread's place in the main window shows "Open in its own window · Bring back". Closing a pop-out, or a pop-out that dies, gives the view back. Per-view state that must survive a move or a crash (the draft, the scroll anchor) lives in Rust and is written to disk debounced, so a draft survives a forced close of its window and an app crash. Pop-out positions are remembered and restored after the main window is interactive.
- **Tokens and themes** (§11.1). The theme loader with: the required semantic set (the new names included), reserved runtime and `-rgb` names as load errors, six-digit opaque hex surfaces, `-rgb` channel outputs, and the contrast check with the four state colours as text on `surface-0..2`. The Calm themes live in `app/themes/` and are checked by a test. Components use semantic and component tokens only (the conformance test from the spike).
- **Appearance settings**, app-local (they describe windows, not the daemon): theme (light, dark, system), window opacity with both modes, keep reading areas solid, blur, overlay blur strength, pop-outs opaque or not, text size. Whole-window opacity is the native window alpha. The OS accessibility options (reduce transparency, increase contrast) force opaque surfaces and repaint every window when they change. On Linux windows are opaque and the notes say so (L1): "Atrium uses opaque windows on Linux."
- **Settings card** with the section registry (`essentials[]`, `advanced[]`, search over both) and the Advanced card. Sections in this task: General (keep agents running after quitting, through `SetSetting`) and Appearance. M1-09b adds Agents, Approvals and Notifications.
- **Tray.** An item with the running and needs-you counts from the thread list, "Show Atrium" and "Quit". With a tray, closing the main window hides it; without one (no tray support on the machine), closing it quits the app. The daemon is never stopped by quitting.
- **Content security.** The CSP of §12, exactly. `withGlobalTauri` off. Capabilities list only what the windows use. A Rust command opens links, accepting `http`, `https` and `mailto` only.
- **Connection states in the UI.** Connecting, connected, `Incompatible` (with what §10's upgrade needs: version, running turns, and the two choices) and failed, as a quiet banner in the frame. The upgrade's signals themselves are M1-09b's if they need the thread list UI; the banner and `ShutdownDaemon` call are here.
- **Harness hooks.** Startup timings (`main` → JS → mounted → first paint → first input), a report command, and pinned settings for harness runs, as in the spike. The measurement scripts are ported so that M1-14 can extend them.

## Definition of Done
- [ ] Rust tests for: the forwarding task (order, acknowledgements, a replaced window), command deadlines, the settings patch and file rules, view ownership and pop-out moves, draft persistence (a forced close and a simulated crash both keep the draft), link scheme checks.
- [ ] Frontend tests (vitest) for: the theme loader's rules above, token conformance, the window model, the delivery reducer for `Connection`, `Inbox`, `ThreadUpdated`, `ThreadRemoved` and `InboxUpdated`, and the settings registry and its search.
- [ ] The app starts on Linux against a real `atriumd` it started itself, shows the thread list of that daemon (threads created with `AgentLaunch::Fake` by a dev command), survives the daemon being killed (reconnects, list intact), and a pop-out round trip keeps a draft. Screenshots of: main light and dark, sidebar hidden, panel minimized, Settings with Advanced, a pop-out, the `Incompatible` banner.
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace`; `pnpm typecheck`; `pnpm test`; `pnpm build`; `scripts/check-protocol-ts`.
- [ ] No `unwrap`, `expect` or `panic!` outside tests in `app/src-tauri`.
- [ ] TTI and idle memory on Linux reported with the ported harness (report only; the gates are macOS's, contract §14).
- [ ] Handoff: what was run and where, what was not run (macOS, the tray if the machine has none), and what M1-09b and M1-14 need.

## Out of scope
The transcript, composer behaviour, inbox list, approval and question UI, markdown rendering, ⌘J, notifications settings (M1-09b). Topic groups, tints, card gap, font pickers, theme file import UI, keybindings file, named layout presets (later milestones). macOS runs (Pedro's shift). Packaging and signing (M8).

## Escalate if
The bridge needs something `atrium-client` or the wire can't do: write a change request and stop that part. Anything that would weaken §12 or the `App` role check.
