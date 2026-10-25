# m0-00-smoke (seed fixture from live M0-00 smoke run)

- Source: the M0-00 launcher smoke test, `agy` run 4 (`.orchestration/archive/M0-00/`). Converted to the fixture format by the orchestrator in the M0 contract phase; golden `m0-00-smoke.events.json` added in M0-03.
- CLI: `agy` 1.4.5, model `gemini-3.8-flash-medium`, `-p … --mode accept-edits --dangerously-skip-permissions --output-format stream-json`, with the worker policy hook active (so some tool calls end in `ERROR` because the hook denied them).
- Covers: `init`, many `step_update` records (`run_command`, `write_file`, denied calls), `result` with usage.
- Timing is unknown (`ms` = 0). Home paths and user names are sanitized. The final `exit` line was added in the format revision after the Astra review.
- Golden events file: `m0-00-smoke.events.json`.
