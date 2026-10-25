# Fixture: `tool-fetch`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Fetch https://example.com and tell me the page title.`
- **Events**: 14
- **Tool calls**:
  - `call_00_qdlsrhckl67lydsu3lg9gpb5` kind `fetch` status `completed`: reached the gate
