# tool-read

- Status: `live`
- CLI: `agy 1.4.7`
- Model: `gemini-3.8-flash-medium`
- Command: `agy --print= --input-format stream-json --output-format stream-json --mode accept-edits --dangerously-skip-permissions --print-timeout 0s`
- Prompt: `Use read_file to read hello.txt and state its contents.`
- Scenario: `tool-read` (`docs/contracts/m0.md` §3.3)
- Paths: the scratch working directory is recorded as `/scratch`, other temp-dir roots as `/scratch-tmp`, the home directory as `~`.
