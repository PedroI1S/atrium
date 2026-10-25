# Fixture: `tool-execute`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Use your execute tool to run JavaScript that computes 6 * 7 and returns the result. If you have no execute tool, say so.`
- **Events**: 29
- **Tool calls**:
  - `call_00_227lwavarij16nx4u3hmiya0` kind `execute` status `completed`: reached the gate
