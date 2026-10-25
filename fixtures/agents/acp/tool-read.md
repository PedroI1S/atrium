# Fixture: `tool-read`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Read Cargo.toml and tell me the package name.`
- **Events**: 12
- **Tool calls**:
  - `call_00_xjkia2tjex0mbahdhshducqa` kind `read` status `completed`: reached the gate
