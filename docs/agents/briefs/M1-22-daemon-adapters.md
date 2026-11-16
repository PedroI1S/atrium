# M1-22-daemon-adapters: `atriumd` must register the Claude and Codex adapters

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-106) · Reviewer: Sonnet 5.5 (read-only, cross-provider) · Depends on: nothing open
Go: D-106 (Pedro's D-104 go for pending tasks; a blocker found by M1-20) · Branch: `agent/hydrogen/M1-22-daemon-adapters` · Worktree: `../atrium-wt/M1-22-daemon-adapters`

## Goal
M1-20's gate audit found, and the orchestrator confirmed, that the built daemon has no agent adapters:
- `crates/atrium-daemon/src/main.rs` `run()` builds `Options::new(…)` and never fills `options.adapters`;
- `Options::new` starts with `adapters: Vec::new()` (`lib.rs:133`).

Only the tests set adapters (`tests/common/mod.rs:392`). So in the real app:
- `ProbeProviders` answers with no providers;
- `StartThread` with `AgentLaunch::Native` fails for Claude Code and Codex alike.

Every test passes because none runs the real `atriumd` binary with a native launch.

## Context
- `crates/atrium-daemon/src/{main.rs,lib.rs,commands.rs}` (`probe`, launch);
- `crates/atrium-agents/src/claude/adapter.rs` (`ClaudeAdapter::new(ClaudeOptions)`, `impl Default for ClaudeOptions`);
- `crates/atrium-agents/src/codex/adapter.rs` (`CodexAdapter::new()`);
- contract `docs/contracts/m1.md` §3 (enforcement per session, probing), §6 (`AgentLaunch::Native`), §10 (lifecycle);
- `docs/agents/reviews/M1-20-gate-audit.md` (blocker 1).

## Owned paths
- `crates/atrium-daemon/src/main.rs`; `crates/atrium-daemon/src/lib.rs`, only if a small helper (e.g. `Options::with_default_adapters`) belongs there;
- a new `crates/atrium-daemon/tests/binary_adapters.rs`;
- `docs/agents/handoffs/M1-22-daemon-adapters.md`.

## What to do
1. **Register both adapters in `run()`:** `ClaudeAdapter::new(ClaudeOptions::default())` and `CodexAdapter::new()`. Read both adapters' constructors and options first:
   - where each finds its CLI (PATH, an override);
   - whether `ClaudeOptions::default()` is what a real daemon should use.

   If a constructor needs configuration the daemon has (paths, the home), pass it, and say why in the handoff.
2. **A test that runs the real binary.** Start the built `atriumd` in a throwaway `ATRIUM_HOME`, with `HOME` pointing at a temp folder. Use `CARGO_BIN_EXE_atriumd`, following how the existing daemon tests or `start_refusal.rs` start it.
   - Send `ProbeProviders` over the socket with the test client (`tests/common/`).
   - Assert that the answer lists Claude Code and Codex. Being listed is enough: neither CLI need be installed, and "not installed" or "not found" is a fine status.
   - Assert that `StartThread` with `AgentLaunch::Native` for each provider reaches the adapter. A missing CLI may fail with the adapter's own "not installed" error; the test fails only on "no adapter for this provider".

   Never run a real provider. If a CLI is installed on this machine, the test must not start a real session: probe only, or set PATH so the CLIs aren't found.
3. **Check for other gaps like this one.** Are there other `Options` fields that tests set but `main.rs` doesn't (notifier, service, identity, limits)? List each, and fix it if it's a gap.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] The new test fails on `dev` (adapters missing) and passes with the fix. Show both runs.
- [ ] It passes 20 runs in a row.
- [ ] Handoff: the change, the test, the `Options` comparison, the outputs.

## Rules
- Work only in your worktree, and commit on your branch only. Never push, merge or rebase.
- Stop every daemon a test starts.
- Never run a real provider.
