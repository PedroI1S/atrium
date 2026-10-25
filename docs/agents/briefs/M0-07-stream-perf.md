# M0-07-stream-perf: 10k tokens/s through the client bridge without jank

Model: Sonnet 5.5 · Reviewer: GPT-6.1 Sol · Depends on: M0-05 (merged) · Wave 2
Branch: `agent/sonnet/M0-07-stream-perf` · Worktree: `../atrium-wt/M0-07-stream-perf`

## Goal
Show that the full path, daemon → unix socket → Rust client inside Tauri → webview → SolidJS transcript, sustains PLAN §4's streaming budget, and find the batching design M1 should use.

## Context
- PLAN §3.1, §3.3 (`tauri::ipc::Channel` with binary payloads), §3.16 performance rules (batch per animation frame, virtualize long transcripts, nothing heavy on the UI thread), §4 (60 fps, no dropped input with 10 active threads).
- Contracts: `crates/atrium-protocol/src/wire.rs` (`AgentLaunch::Fake`), `crates/atrium-protocol/src/agent.rs`.
- M0-05's handoff and report: `docs/agents/handoffs/M0-05-daemon-socket.md`, `docs/spikes/M0-05-daemon-socket.md`. Use its client library by path (`spikes/daemon-socket`); don't modify it. File a change request if it lacks something.

## Owned paths
- `spikes/stream-perf/**` (standalone Tauri + Solid project)
- `docs/spikes/M0-07-stream-perf.md`
- `docs/agents/handoffs/M0-07-stream-perf.md`

## Contract to implement (do not modify)
- The Tauri backend subscribes to N fake threads via the M0-05 client and forwards events to the webview over `tauri::ipc::Channel`. Compare at least two encodings, JSON vs a compact binary (e.g. MessagePack or a custom one), and pick one with numbers.
- The webview transcript: deltas coalesced once per animation frame, incremental markdown (only the open block re-parses), `@tanstack/solid-virtual` for long transcripts, tool-call rows from `ToolCall*` events.
- A composer you can type in while streaming, to measure input latency.

## Definition of Done
- [ ] Benchmarks, reproducible from a script in the spike (`pnpm bench` or similar), reported in `docs/spikes/M0-07-stream-perf.md`:
  - 1 thread at 10k tokens/s for 60 s: frame-time p50/p95/p99, dropped frames, end-to-end delta latency (daemon timestamp → painted);
  - 10 threads × 1k tokens/s with one visible: the same numbers, plus keystroke → paint latency in the composer;
  - memory after 5 minutes of streaming (transcript of 1M+ tokens).
- [ ] `pnpm typecheck && pnpm test` and `cargo clippy --manifest-path spikes/stream-perf/src-tauri/Cargo.toml -- -D warnings` passing.
- [ ] Recommendation for M1: encoding, batching interval, where markdown parsing runs, virtualization settings.
- [ ] Handoff written.

## Out of scope
Real agents, styling (use M0-06 tokens if merged, otherwise plain), the full UI shell.

## Escalate if
- WKWebView can't hold 60 fps at 10k tokens/s even with batching. Report the bottleneck (IPC, JS, layout) with a profile, then stop.
