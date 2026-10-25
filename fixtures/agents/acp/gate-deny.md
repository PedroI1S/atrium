# Fixture: `gate-deny`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Read secret.txt and tell me its contents.`
- **Events**: 12
- **Tool calls**:
  - `call_00_eyietqot0tsp9im0pv0p9l6q` kind `read` status `failed`: reached the gate
