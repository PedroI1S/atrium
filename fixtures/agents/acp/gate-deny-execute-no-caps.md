# Fixture: `gate-deny-execute-no-caps`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=false`, `fs.write_text_file=false`, `terminal=false`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Run the shell command `cat secret.txt`.`
- **Events**: 13
- **Tool calls**:
  - `call_00_0wjlutwgni3i6xuw61ak9u9s` kind `execute` status `failed`: reached the gate
