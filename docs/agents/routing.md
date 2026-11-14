# Routing scoreboard

Starting hypothesis: PLAN §8.2. Updated after every task and recalibrated each milestone.

| Model | Task type | Tasks | Passed first try | Passed ≤2 fix rounds | Accepted review findings | Notes |
|---|---|---|---|---|---|---|
| Sonnet 5.5 | smoke (M0-00) | 1 | 1 | 1 | n/a | ~31k tokens, 10 s |
| GPT-6.1 Sol | smoke (M0-00) | 1 | 0 (sandbox had no network) | 1 | n/a | Reported the failure accurately instead of working around it |
| Gemini 3.8 Flash | smoke (M0-00) | 1 | 0 (permissions) | 1 | n/a | Under the early, too-strict policy it tried many workarounds (~159k tokens); after the policy fix, ~21k tokens |
| Gemini 4 Argon | M0-03 agy adapter, M0-04 ACP adapter, M0-01 review | 2 + 1 review | 0 (both failed live on first try) | pending | review: approve with changes | Paused from 2026-10-25 (weekly limit, D-015). Strong offline output and thorough reports, but harnesses passed vacuously when the CLI never ran. |
| GPT-6.1 Sol | M0-06b UI shell (layout B) | 1 | 0 (harness broken; design fidelity low) | n/a | review: approve with changes, 13 findings | Pedro judged it underwhelming; handed to Opus (D-027). Weak on UI/design-heavy frontend work. |
| Gemini 4 Hydrogen | replaces Gemini 3.8 Flash from 2026-11-13 (D-068) | 0 | — | — | — | No task yet. Model id `gemini-4-hydrogen-medium`. |
| Gemini 4 Hydrogen | M1-14a fix round 1 close-out (perf harness fix, L8 re-measure, DoD, handoff) | 1 | 1 | 1 | review pending | 2026-11-14, `agy` 1.5.8, ~10 min, ~158k input tokens. Correct, in owned paths, clean report with raw output. 8 hook denials, all legitimate (`/proc/loadavg`, `ps`, `uptime`, `uname`, a sed range read as a path, its own brain dir, two unparsable commands); no unknown tool names. The run ended with status `ERROR` (an empty malformed function call) after the work and the final report were done. |
