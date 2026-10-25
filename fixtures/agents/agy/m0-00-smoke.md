# m0-00-smoke (seed fixture, no golden file yet)

- Source: the M0-00 launcher smoke test, `agy` run 4 (`.orchestration/archive/M0-00/`, Pedro's Mac). Converted to the fixture format by the orchestrator in the M0 contract phase.
- CLI: `agy` 1.4.5, model `gemini-3.8-flash-medium`, `-p … --mode accept-edits --dangerously-skip-permissions --output-format stream-json`, with the worker policy hook active (so some tool calls end in `ERROR` because the hook denied them).
- Covers: `init`, many `step_update` records (`run_command`, `write_file`, denied calls), `result` with usage.
- Timing is unknown (`ms` = 0). Home paths and user names are sanitized. The final `exit` line was added in the format revision after the Astra review.
- Not one of the required scenarios: it's real data to start from while the live harness is built. M0-03 owns this directory and may replace or extend it.
