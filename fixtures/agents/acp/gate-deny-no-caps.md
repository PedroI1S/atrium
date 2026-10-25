# Fixture: `gate-deny-no-caps`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=false`, `fs.write_text_file=false`, `terminal=false`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Read secret.txt and tell me its contents.`
- **Events**: 13
- **Tool calls**:
  - `call_a3af8a28a34b4bb4a37dbae0` kind `read` status `failed`: reached the gate
