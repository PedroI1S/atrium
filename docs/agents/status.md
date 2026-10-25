# Status board

| Task | Model | Owner (driver) | State | Worktree / branch | Log (on which machine) |
|---|---|---|---|---|---|
| M0-00-smoke-claude | Sonnet 5.5 (Claude subagent) | Pedro | ✅ passed, worktree removed, archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-00-smoke-codex | GPT-6.1 Sol (`codex exec`) | Pedro | ✅ passed on run 2 (network enabled), archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-00-smoke-agy | Gemini 3.8 Flash (`agy -p`) | Pedro | ✅ passed on run 4 (policy hook + skip-permissions), file tool verified, archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-01-claude-adapter | Sonnet 5.5 | Pedro | 🏃 running · wave 1 · dispatched 2026-10-24 | `../atrium-wt/M0-01-claude-adapter` · `agent/sonnet/M0-01-claude-adapter` | Claude subagent transcript (Pedro's Mac) |
| M0-02-codex-adapter | GPT-6.1 Sol | Pedro | 🔧 fix round 1/2 (Sol) · offline DoD verified 29/29, committed `83225db` · live run works but leaked hostname/installation id and user MCP/hook config → recorder fixes, then orchestrator records live | `../atrium-wt/M0-02-codex-adapter` · `agent/sol/M0-02-codex-adapter` | `.orchestration/logs/M0-02-codex-adapter.*` (Pedro's Mac) |
| M0-03-agy-adapter | Gemini 4 Argon | Pedro | 🏃 running · wave 1 · dispatched 2026-10-24 | `../atrium-wt/M0-03-agy-adapter` · `agent/argon/M0-03-agy-adapter` | `.orchestration/logs/M0-03-agy-adapter.*` (Pedro's Mac) |
| M0-04-acp-adapter | Gemini 4 Argon | Pedro | 🏃 running · wave 1 · dispatched 2026-10-24 | `../atrium-wt/M0-04-acp-adapter` · `agent/argon/M0-04-acp-adapter` | `.orchestration/logs/M0-04-acp-adapter.*` (Pedro's Mac) |
| M0-05-daemon-socket | GPT-6.1 Sol | Pedro | 🔍 in review (Sonnet 5.5) · DoD re-run by orchestrator: 11/11 tests, budgets met · committed `9fa0f4f` | `../atrium-wt/M0-05-daemon-socket` · `agent/sol/M0-05-daemon-socket` | `.orchestration/logs/M0-05-daemon-socket.*` (Pedro's Mac) |
| M0-06-ui-shell | Sonnet 5.5 | Pedro | 🏃 running · wave 1 · dispatched 2026-10-24 | `../atrium-wt/M0-06-ui-shell` · `agent/sonnet/M0-06-ui-shell` | Claude subagent transcript (Pedro's Mac) |
| M0-07-stream-perf | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014) · wave 2, after M0-05 merges | — | — |
| M0-08-browser-screencast | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014) · wave 2, when a Sonnet slot frees | — | — |
| M0-09-m0-findings | Gemini 3.8 Flash | Pedro | 🧊 contract frozen (D-014) · wave 3, after M0-01…04 merge | — | — |
| M0-00-hookcheck-1.4.7 | Gemini 3.8 Flash (`agy -p`) | Pedro | ✅ `agy` 1.4.7 policy re-check passed: `cargo` allowed, `.agents/` write and `git push` denied | removed | `.orchestration/logs/M0-00-hookcheck-1.4.7.*` (Pedro's Mac) |
