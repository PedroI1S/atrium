# M1-23-ui-gaps: the M1 UI the gate audit found missing

Model: Claude Opus 5.5 (Agent, D-109) · Reviewer: Gemini 4 Argon `high` (read-only, cross-provider) · Depends on: nothing open (it runs on `dev`)
Go: D-109 (Pedro, 2026-11-16: "do it now, launch an opus agent") · Branch: `agent/opus/M1-23-ui-gaps` · Worktree: `../atrium-wt/M1-23-ui-gaps`

## Goal
The M1-20 gate audit (`docs/agents/reviews/M1-20-gate-audit.md`, majors 1-5 and minor 1, and the `missing` rows) found UI that PLAN and contract rev 3 promise for M1 but `dev` doesn't have. The daemon and protocol already support all of it. Build that UI, in the app's existing style, so M1 delivers what it promises.

## Context
- `PLAN.md` §3.2 (threads), §3.4 (inbox, approval queue, `⌘J`/`⌘⇧J`, batch approve/deny), §3.12 (enforcement levels, the "details" link, hard-deny list "Always blocked (for calls Atrium sees)"), §3.13 (rules), §3.18 (export), and "M1: Daemon + Home + attention" (around line 727).
- Contract `docs/contracts/m1.md`: §3 (enforcement), §4 (the same-UID peer boundary Settings must "state clearly"), §8 (attention), §9 (rules, the hard-deny list, the `git config` known limit), §10 (lifecycle, archive), §13 (export).
- Protocol: `crates/atrium-protocol/src/wire.rs` (`RenameThread`, `ArchiveThread`, `ThreadCommand` with `Shutdown`, `CreateRule`, `ExportThread`, `ResolveApproval`), `policy.rs` (`EnforcementDescriptor`, `RuleDraft`, `HARD_DENY_DEFAULTS`).
- The app: `app/src/ui/*` (`Settings.tsx`, `ThreadCard.tsx`, `InboxCard.tsx`, `RequestCard.tsx`, `ContextPanel.tsx`, `Palette.tsx`, `Sidebar.tsx`), `app/src/App.tsx`, `app/src/ui/jump.ts`, `app/src/daemon/*`, `app/src/settings-registry.ts`, `app/src-tauri/src/*`.
- Design: `docs/design/ui-direction.md`, `docs/design/themes/`, and the components in `app/src/ui/controls.tsx`. Match what is there: no new UI library.
- The demo `app/scripts/m1_demo.py` + `app/src/m1-demo.ts`, and `docs/demos/M1-demo.md`.

## Owned paths
- `app/src/**` (UI, daemon client, settings registry, tests).
- `app/src-tauri/src/**`, only for what the UI can't do without it: a native save dialog for export, and a command returning `HARD_DENY_DEFAULTS` (or an equivalent read-only source) to the webview.
- `app/scripts/m1_demo.py`, `app/src/m1-demo.ts`, `docs/demos/M1-demo.md`: extend the demo to cover what you add.
- `docs/agents/handoffs/M1-23-ui-gaps.md`.
- Not yours: daemon, protocol, policy, store and agent crates. If a gap needs a change there, stop that item and report it.

## What to do
1. **Thread actions.** Rename, Archive, Export (Markdown and JSONL, through a native save dialog; `ExportThread` writes the file), and End session (`ThreadCommand` `Shutdown`, so a thread can reach `Ended` from the UI). Put them in the thread header (a menu is fine) and in the Palette. Archive and End session confirm first. Errors show in the UI.
2. **Approval queue: batch approve and deny.** Select several pending approvals (and "select all"), then Allow once or Deny them together. Each goes through `ResolveApproval`; show per-item failures (e.g. `Expired`). No batch "Always": rules stay one at a time.
3. **`⌘⇧J`** jumps to the previous item that needs you (`App.tsx:18` sends it to "next" today). Add `jumpToPrev` in `jump.ts` with tests.
4. **Rules editor in Settings → Approvals.** Create rules with `CreateRule`: command prefix, exact command, path glob, and the other `RuleDraft` matchers; allow or deny; scope as `RuleDraft` allows. Validate input before sending. Listing and deleting already exist.
5. **Settings safety disclosures.**
   - "Always blocked (for calls Atrium sees)": the `HARD_DENY_DEFAULTS` list, read-only, with PLAN §3.12's wording that it isn't a machine-wide prohibition.
   - The `git config` known limit (contract §9).
   - The same-UID peer boundary (contract §4): any process running as the same user can talk to `atriumd`.
   - Leave a clearly marked spot for the CR-12 credential-stores list; it lands after M1-18 merges.
6. **Enforcement "details".** From the enforcement badge (thread header, Context panel, Settings → Agents), open a details view showing `EnforcementDescriptor`: the level, per-tool-class coverage, config sources, and reasons, with coverage, policy and isolation kept visibly separate (PLAN §3.12).
7. **The demo.** Add steps to `m1-demo.ts`/`m1_demo.py` for: rename, export through the new action (bypass the native dialog in bench mode), archive from the UI, end session, batch approve/deny, `⌘⇧J`, and create a rule from Settings. Update `docs/demos/M1-demo.md`.

## Definition of Done
- [ ] In `app/`: `pnpm typecheck`, `pnpm test` (new tests for jump, batch, rule validation, and any new reducer or state logic), `pnpm build`.
- [ ] If Rust changed: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `python3 app/scripts/m1_demo.py` passes 3 runs in a row, under `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock`.
- [ ] Screenshots of each new surface in light and dark, saved under `docs/agents/handoffs/M1-23-ui-gaps/` (reuse `app/scripts/screenshots.py` or `chat-shots.ts` if they fit).
- [ ] Startup and idle memory don't regress: run the macOS TTI and idle-memory checks used in `docs/spikes/M1-macos-checks.md` once and report the numbers.
- [ ] Handoff: what was added where, the demo results, the measurements, and anything left.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase.
- Never run a real provider. The demo uses the fake agent.
- Stop every app and daemon you start.
- Keep the app fast: no heavy dependencies, nothing that loads on the startup path unless it's needed there.

## Escalate if
- an item needs a daemon or protocol change;
- PLAN and the contract disagree about what an item should do.

Write it down in the handoff, and go on with the other items.
