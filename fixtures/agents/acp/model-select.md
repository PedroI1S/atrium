# Fixture: `model-select`

- **Source**: `hand-made` (shaped after the live OpenCode v2.1.3 recording M0-04-live-1: `configOptions`, top-level tool `name`, permission options `once`/`always`/`reject`; the orchestrator replaces it with a live recording)
- **Provider**: ACP (`opencode acp`, v2.1.3)
- **Client capabilities**: `fs`, `terminal`, `elicitation.form` advertised (OpenCode ignores fs/terminal)
- **Injected config**: `permissions: [{* * ask}]` (see `spikes/agent-acp/src/opencode_config.rs`)
- **Prompt**: `"Reply with one word."` (model `opencode/big-pickle`, agent default `opencode/deepseek-v4.1-flash-free`)
- **Covers**: `SessionConfig::model` is applied: the `session/new` response shows another current model, the adapter sends `session/set_config_option {configId: model}`, and `SessionStarted` (with the requested model) is only emitted once the response confirms it.
