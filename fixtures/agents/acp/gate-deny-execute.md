# Fixture: `gate-deny-execute`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Run the shell command `cat secret.txt`.`
- **Events**: 11
- **Tool calls**:
  - `call_00_rjjuokvk0d26emvaa6fe41qy` kind `execute` status `failed`: reached the gate
