# Fixture: `startup-error`

- **Source**: `hand-made` (shaped after the live OpenCode v2.1.3 recording M0-04-live-1: `configOptions`, top-level tool `name`, permission options `once`/`always`/`reject`; the orchestrator replaces it with a live recording)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Client capabilities**: `fs`, `terminal`, `elicitation.form` advertised (OpenCode ignores fs/terminal)
- **Injected config**: `permissions: [{* * ask}]` (see `spikes/agent-acp/src/opencode_config.rs`)
- **Prompt**: *(none: fails during session creation)*
- **Covers**: A model the agent rejects (`session/set_config_option` returns an error): pre-session failure lifecycle, `Error {fatal: true}` then `SessionEnded`, **no** `SessionStarted`. The exact OpenCode error text is not known yet; the live recording replaces this.
