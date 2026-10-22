# Routing scoreboard

Starting hypothesis: PLAN §8.2. Updated after every task and recalibrated each milestone.

| Model | Task type | Tasks | Passed first try | Passed ≤2 fix rounds | Accepted review findings | Notes |
|---|---|---|---|---|---|---|
| Sonnet 5.5 | smoke (M0-00) | 1 | 1 | 1 | n/a | ~31k tokens, 10 s |
| GPT-6.1 Sol | smoke (M0-00) | 1 | 0 (sandbox had no network) | 1 | n/a | Reported the failure accurately instead of working around it |
| Gemini 3.8 Flash | smoke (M0-00) | 1 | 0 (permissions) | 1 | n/a | Under the early, too-strict policy it tried many workarounds (~159k tokens); after the policy fix, ~21k tokens |
