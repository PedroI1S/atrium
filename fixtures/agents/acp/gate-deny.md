# Fixture: `gate-deny`

- **Source**: `hand-made` (shaped after the live OpenCode v2.1.3 recording M0-04-live-1: `configOptions`, top-level tool `name`, permission options `once`/`always`/`reject`; the orchestrator replaces it with a live recording)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Client capabilities**: `fs`, `terminal`, `elicitation.form` advertised (OpenCode ignores fs/terminal)
- **Injected config**: `permissions: [{* * ask}]` (see `spikes/agent-acp/src/opencode_config.rs`)
- **Prompt**: `"Read secret.txt and tell me its contents."`
- **Covers**: The read goes through `session/request_permission` (the only path that exists on OpenCode). The gate returns `Deny`, the adapter answers `reject`, the call finishes `denied`, and the canary never appears in the stream. The live recording must show the same; if OpenCode instead reads natively, the recorder reports `GATE BYPASSED`.
