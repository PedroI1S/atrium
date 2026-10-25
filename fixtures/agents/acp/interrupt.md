# Fixture: `interrupt`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Run the shell command `sleep 30`, then say done.`
- **Events**: 9
- **Tool calls**:
  - `call_00_svzwjyno158plddnahfphqgj` kind `execute` status `failed`: **did not reach the gate**
