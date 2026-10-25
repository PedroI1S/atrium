# gate-deny-execute

hand-made; app-server was blocked by worker sandbox before handshake. Protocol: codex-cli 0.163.0 app-server v2, generated JSON schema.

Model: `fixture-model` (synthetic). Command: not executed; hand-made app-server v2 trace.

Reproduce: `cargo run -p spike-agent-codex --example record -- gate-deny-execute`.
Prompt: Run python3 -c 'print(42)'. Do not retry if denied.

Hand-made traces demonstrate mapping only; they do not prove provider enforcement.
