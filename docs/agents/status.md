# Status board

| Task | Model | Owner (driver) | State | Worktree / branch | Log (on which machine) |
|---|---|---|---|---|---|
| M0-00-smoke-claude | Sonnet 5.5 (Claude subagent) | Pedro | ✅ passed, worktree removed, archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-00-smoke-codex | GPT-6.1 Sol (`codex exec`) | Pedro | ✅ passed on run 2 (network enabled), archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-00-smoke-agy | Gemini 3.8 Flash (`agy -p`) | Pedro | ✅ passed on run 4 (policy hook + skip-permissions), file tool verified, archived | — | `.orchestration/archive/M0-00/` (Pedro's Mac) |
| M0-01-claude-adapter | Sonnet 5.5 | Pedro | ✅ merged into local `dev` (code reaches origin at handoff) · 63 tests, 20 live fixtures, `AllToolCalls` unless external Claude hooks exist (then Partial) · archived | removed | `.orchestration/archive/M0-01-claude-adapter/` (Pedro's Mac) |
| M0-02-codex-adapter | GPT-6.1 Sol | Pedro | ✅ merged into local `dev` · 54 tests; 13 scenarios live; read and execute denied by the gate · open: re-record `patch` (hit the usage limit), Codex didn't ask `question` live · archived | removed | `.orchestration/archive/M0-02-codex-adapter/` (Pedro's Mac) |
| M0-03-agy-adapter | Gemini 4 Argon → Sonnet 5.5 (D-015) | Pedro | ✅ merged into local `dev` as a spike (D-018) · review findings → `m1-must-fix.md` A1–A15 · archived | removed | `.orchestration/archive/M0-03-agy-adapter/` (Pedro's Mac) |
| M0-04-acp-adapter | Gemini 4 Argon → Sonnet 5.5 (D-015) | Pedro | ✅ merged into local `dev` as a spike (D-018) · review findings → `m1-must-fix.md` O1–O11 · Code Mode default pending · archived | removed | `.orchestration/archive/M0-04-acp-adapter/` (Pedro's Mac) |
| M0-05-daemon-socket | GPT-6.1 Sol | Pedro | ✅ merged into local `dev` · 25 tests; reconnect 3.5–10.3 ms ✅; immediate idle RSS ~89 MiB ❌ over 50 MB (60 s: 9–23 MiB) · archived | removed | `.orchestration/archive/M0-05-daemon-socket/` (Pedro's Mac) |
| M0-06-ui-shell | Sonnet 5.5 | Pedro | ✅ merged into local `dev` · 87 tests; TTI median <400 ms (worst warm 457 ms); footprint 125–131 MB ✅, ps RSS 176–215 MB · archived · CR-01 pending | removed | `.orchestration/archive/M0-06-ui-shell/` (Pedro's Mac) |
| M0-07-stream-perf | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014) · wave 2, after M0-05 merges | — | — |
| M0-08-browser-screencast | Sonnet 5.5 | Pedro | 🧊 contract frozen (D-014) · wave 2, when a Sonnet slot frees | — | — |
| M0-09-m0-findings | Gemini 3.8 Flash | Pedro | 🧊 contract frozen (D-014) · wave 3, after M0-01…04 merge | — | — |
| M0-00-hookcheck-1.4.7 | Gemini 3.8 Flash (`agy -p`) | Pedro | ✅ `agy` 1.4.7 policy re-check passed: `cargo` allowed, `.agents/` write and `git push` denied | removed | `.orchestration/logs/M0-00-hookcheck-1.4.7.*` (Pedro's Mac) |
