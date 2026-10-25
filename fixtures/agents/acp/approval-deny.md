# Fixture: `approval-deny`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Write 'blocked' to production.conf.`
- **Events**: 14
- **Tool calls**:
  - `call_00_qamw8ulq4f46r2dpzwpwuvab` kind `execute` status `failed`: reached the gate
