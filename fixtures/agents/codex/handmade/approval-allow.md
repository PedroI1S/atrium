# approval-allow

hand-made; app-server was blocked by worker sandbox before handshake. Protocol: codex-cli 0.163.0 app-server v2, generated JSON schema.

Model: `fixture-model` (synthetic). Command: not executed; hand-made app-server v2 trace.

Reproduce: `cargo run -p spike-agent-codex --example record -- approval-allow`.
Prompt: Run python3 -c 'print(42)' after approval. Do not retry if denied.

Hand-made traces demonstrate mapping only; they do not prove provider enforcement.
Atrium M1 (`f0b1aab`) rewrote the `"dir":"gate"` metadata line to add `check.subject` (with `cwd: null`, whereas M1 `CodexAdapter` populates `/scratch`), `check.turn`, and `decision.ticket`; recorded `stdin`/`stdout` wire lines were untouched.
