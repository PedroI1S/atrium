# M1-24-bridge-batching: deliver daemon events to the webview in batches

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-111) · Reviewer: Sonnet 5.5 (read-only, cross-provider) · Depends on: nothing open
Go: D-111 (Pedro: batching in `bridge.rs` now, in M1) · Branch: `agent/hydrogen/M1-24-bridge-batching` · Worktree: `../atrium-wt/M1-24-bridge-batching`

## Goal
Tauri 2 delivers each `Channel` message under 8 KiB with its own `webview.eval`, so 10,000 events/s are 10,000 script evaluations: about 5 ms of each frame on the webview's main thread (`handoffs/M1-09b-home-chat.md`, "What is left on the main thread"). Batch the deliveries so a busy stream costs a few evaluations per frame, without adding visible latency or changing order, acknowledgement or back-pressure.

## Context
- `app/src-tauri/src/bridge.rs` (`Sink`, `frame`, `forward`, the tests), and where `forward` is wired to a `Channel` in `app/src-tauri/src/lib.rs`.
- `app/src/daemon/client.ts` (`channel.onmessage`, `applied`, `bridge_ack`, the per-animation-frame acknowledgement).
- `crates/atrium-client` (`Window`, `recv`, the per-window queue and acknowledgements), read only.
- `handoffs/M1-09b-home-chat.md` and contract `docs/contracts/m1.md` §14 (streaming at 60 fps, p99 ≤ 33 ms).

## Owned paths
- `app/src-tauri/src/bridge.rs` and the lines of `app/src-tauri/src/lib.rs` that wire `forward` to the channel.
- In `app/src/daemon/client.ts`, only the receive path (`onmessage`, applying a batch, acknowledging). Another task (M1-23, the UI gaps) is editing other app files at the same time: touch nothing else in `app/src`.
- `docs/agents/handoffs/M1-24-bridge-batching.md`.
- Not yours: `atrium-client`, the daemon, the protocol.

## What to do
1. **Batch in `forward`.** After the first delivery arrives, take every delivery already queued, up to a bound (e.g. 256 KiB or 512 deliveries), and send one frame holding them all, in order: e.g. `{"batch":[{"seq":…,"delivery":…},…]}`. Never wait for more to arrive: a lone delivery goes out at once, so idle latency doesn't change. If a time window helps (≤ 4 ms), measure it and justify it.
2. **Receive in `client.ts`.** Apply each delivery of a batch in order, exactly as single ones are applied (same `applied`, same `timed`, same acknowledgement per animation frame). Keep accepting the single-delivery frame shape too, or remove it everywhere in one change.
3. **Keep the guarantees:** order, no loss, no duplicates, a gone sink ends forwarding, replacing a window ends the old task, acknowledgements still drive the client's back-pressure.
4. **Tests.** Rust: batching keeps order and sequence numbers; a lone delivery is not delayed; the bound is respected; a gone sink still ends forwarding. TS (vitest): a batch applies in order and acknowledges the last seq; a mix of batch and single frames.
5. **Measure without the GUI.** Don't run the GUI benches (another task uses the screen; the orchestrator runs `stream_bench.py` after). Instead, give a Rust or vitest micro-measurement of frames per 10,000 deliveries before and after.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] In `app/`: `pnpm typecheck`, `pnpm test`, `pnpm build`.
- [ ] Handoff: the frame shape, the bound, the tests, the micro-measurement, and what the orchestrator should check with `stream_bench.py`.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase.
- Never run a real provider. Don't run GUI scripts.
