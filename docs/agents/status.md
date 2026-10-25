# Status board

| Task | Model | Owner (driver) | State | Worktree / branch | Log (on which machine) |
|---|---|---|---|---|---|
| M0-00-smoke-claude | Sonnet 5.5 (Claude subagent) | Pedro | ✅ passed, worktree removed, archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-00-smoke-codex | GPT-6.1 Sol (`codex exec`) | Pedro | ✅ passed on run 2 (network enabled), archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-00-smoke-agy | Gemini 3.8 Flash (`agy -p`) | Pedro | ✅ passed on run 4 (policy hook + skip-permissions), file tool verified, archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-01-claude-adapter | Sonnet 5.5 | Pedro | ✅ merged into local `dev` (code reaches origin at handoff) · 63 tests, 20 live fixtures, `AllToolCalls` unless external Claude hooks exist (then Partial) · archived | removed | `.orchestration/archive/M0-01-claude-adapter/` (Pedro's Mac) |
| M0-02-codex-adapter | GPT-6.1 Sol | Pedro | ⏸ paused · fix round 1 verified (34 tests, `49e6b20`) · review: approve with changes, 11 findings → fix round 2 (Sol) NOT started (cancelled at wrap-up) · live recording still to do | `../atrium-wt/M0-02-codex-adapter` · `agent/sol/M0-02-codex-adapter` | `.orchestration/logs/M0-02-codex-adapter.*` (Pedro's Mac) |
| M0-03-agy-adapter | Gemini 4 Argon → Sonnet 5.5 (D-015) | Pedro | ⏸ paused · all 10 scenarios live and passing (`22e1208`) · Sol review NOT started (cancelled at wrap-up) | `../atrium-wt/M0-03-agy-adapter` · `agent/argon/M0-03-agy-adapter` | `.orchestration/logs/M0-03-agy-adapter.*` (Pedro's Mac) |
| M0-04-acp-adapter | Gemini 4 Argon → Sonnet 5.5 (D-015) | Pedro | ⏸ paused · Sonnet fix round 1 done (`d2e565b`, offline green): per-session `ask` config, model selection, timeouts · live re-record NOT done (commands in its handoff) | `../atrium-wt/M0-04-acp-adapter` · `agent/argon/M0-04-acp-adapter` | `.orchestration/logs/M0-04-acp-adapter.*` (Pedro's Mac) |
| M0-05-daemon-socket | GPT-6.1 Sol | Pedro | ⏸ paused · fix round 1 half done (WIP `c2ae8f9`, Codex limit) · resume NOT started (cancelled at wrap-up) | `../atrium-wt/M0-05-daemon-socket` · `agent/sol/M0-05-daemon-socket` | `.orchestration/logs/M0-05-daemon-socket.*` (Pedro's Mac) |
| M0-06-ui-shell | Sonnet 5.5 | Pedro | ⏸ paused · verified (65 tests) · Sol review NOT started (cancelled at wrap-up) · CR-01 pending · screenshots 10 MB to shrink at merge | `../atrium-wt/M0-06-ui-shell` · `agent/sonnet/M0-06-ui-shell` | Claude subagent transcript (Pedro's Mac) |
| M0-07-stream-perf | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014) · wave 2, after M0-05 merges | — | — |
| M0-08-browser-screencast | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014) · wave 2, when a Sonnet slot frees | — | — |
| M0-09-m0-findings | Gemini 3.8 Flash | Pedro | 🧊 contract frozen (D-014) · wave 3, after M0-01…04 merge | — | — |
| M0-00-hookcheck-1.4.7 | Gemini 3.8 Flash (`agy -p`) | Pedro | ✅ `agy` 1.4.7 policy re-check passed: `cargo` allowed, `.agents/` write and `git push` denied | removed | `.orchestration/logs/M0-00-hookcheck-1.4.7.*` (Pedro's Mac) |
