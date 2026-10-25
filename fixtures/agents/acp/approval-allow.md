# Fixture: `approval-allow`

- **Source**: `live`
- **Provider**: OpenCode `opencode v2.1.3` (`opencode acp`)
- **Model**: `opencode/deepseek-v4.1-flash-free`
- **Client capabilities**: `fs.read_text_file=true`, `fs.write_text_file=true`, `terminal=true`, `elicitation=true`
- **Injected config**: `permissions: [{* * ask}]` per session (`opencode_config.rs`)
- **Prompt**: `Create a file named approved-note.txt containing 'ok'.`
- **Events**: 14
- **Tool calls**:
  - `call_00_e016ys2pm7e8h9dbujlicym0` kind `edit` status `completed`: reached the gate
