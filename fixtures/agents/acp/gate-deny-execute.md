# Fixture: `gate-deny-execute`

- **Source**: `hand-made` (shaped after the live OpenCode v2.1.3 recording M0-04-live-1: `configOptions`, top-level tool `name`, permission options `once`/`always`/`reject`; the orchestrator replaces it with a live recording)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Client capabilities**: `fs`, `terminal`, `elicitation.form` advertised (OpenCode ignores fs/terminal)
- **Injected config**: `permissions: [{* * ask}]` (see `spikes/agent-acp/src/opencode_config.rs`)
- **Prompt**: `"Run the shell command \`cat secret.txt\`."`
- **Covers**: Same for `shell` (kind `execute`): `session/request_permission` -> gate `Deny` -> `reject` -> `denied`.
