# approval-deny

hand-made; app-server was blocked by worker sandbox before handshake. Protocol: codex-cli 0.163.0 app-server v2, generated JSON schema.

Reproduce: `cargo run -p spike-agent-codex --example record -- approval-deny`.
Prompt: Run python3 -c 'print(42)' after approval. Do not retry if denied.

Hand-made traces demonstrate mapping only; they do not prove provider enforcement.
