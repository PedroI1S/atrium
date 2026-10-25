# Status board

| Task | Model | Owner (driver) | State | Worktree / branch | Log (on which machine) |
|---|---|---|---|---|---|
| M0-00-smoke-claude | Sonnet 5.5 (Claude subagent) | Pedro | ✅ passed, worktree removed, archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-00-smoke-codex | GPT-6.1 Sol (`codex exec`) | Pedro | ✅ passed on run 2 (network enabled), archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-00-smoke-agy | Gemini 3.8 Flash (`agy -p`) | Pedro | ✅ passed on run 4 (policy hook + skip-permissions), file tool verified, archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-01-claude-adapter | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014), awaiting dispatch go · wave 1 | — | — |
| M0-02-codex-adapter | GPT-6.1 Sol | Pedro | 🧊 contract frozen (D-014), awaiting dispatch go · wave 1 | — | — |
| M0-03-agy-adapter | Gemini 4 Argon | Pedro | 🧊 contract frozen (D-014), awaiting dispatch go · wave 1 (needs the `agy` 1.4.7 hook re-check first) | — | — |
| M0-04-acp-adapter | Gemini 4 Argon | Pedro | 🧊 contract frozen (D-014), awaiting dispatch go · wave 1 (needs the `agy` 1.4.7 hook re-check first) | — | — |
| M0-05-daemon-socket | GPT-6.1 Sol | Pedro | 🧊 contract frozen (D-014), awaiting dispatch go · wave 1 | — | — |
| M0-06-ui-shell | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014), awaiting dispatch go · wave 1 | — | — |
| M0-07-stream-perf | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014) · wave 2, after M0-05 merges | — | — |
| M0-08-browser-screencast | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014) · wave 2, when a Sonnet slot frees | — | — |
| M0-09-m0-findings | Gemini 3.8 Flash | Pedro | 🧊 contract frozen (D-014) · wave 3, after M0-01…04 merge | — | — |
