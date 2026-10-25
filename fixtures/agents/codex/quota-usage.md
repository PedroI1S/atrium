# quota-usage

hand-made; app-server was blocked by worker sandbox before handshake. Protocol: codex-cli 0.163.0 app-server v2, generated JSON schema.

Model: `fixture-model` (synthetic). Command: not executed; hand-made app-server v2 trace.

Reproduce: `cargo run -p spike-agent-codex --example record -- quota-usage`.
Prompt: Reply with exactly hello.

Hand-made traces demonstrate mapping only; they do not prove provider enforcement.

Includes a retried error, terminal notification and duplicate turn completion error; exactly one RateLimited with cached reset time is expected.
