# Fixture: `question`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Ask me which target environment to use (staging or production) before proceeding. Use the question tool.`
- **Events**: 20
- **Tool calls**:
  - `call_00_jyoiaabyr0s3plcz6h3uednz` kind `other` status `completed`: reached the gate
