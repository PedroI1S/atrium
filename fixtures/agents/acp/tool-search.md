# Fixture: `tool-search`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Use grep to find the word 'sensitive' in the files here and tell me which file has it.`
- **Events**: 17
- **Tool calls**:
  - `call_00_uaausn2qrpc8u8idujqhvxhz` kind `search` status `completed`: reached the gate
