# tool-read

live. Protocol: atrium_m0/0.163.0 (Mac OS 27.0.1; arm64) unknown (atrium_m0; 0.0.0), app-server v2.
Model: "gpt-5.6-luna".
Command argv (private values redacted): `["codex","app-server","--listen","stdio://","-c","features.hooks=false"]`.
Thread parameters: `{"cwd":"/scratch","model":"gpt-5.6-luna","sandbox":"read-only","approvalPolicy":"untrusted","approvalsReviewer":"user"}`.

Reproduce: `cargo run -p spike-agent-codex --example record -- tool-read`.
Prompt: Read public.txt and report its content.

Live transcript; enforcement claims require the raw-stream PASS checks and matrix qualifications.
Atrium M1 (`f0b1aab`) rewrote the `"dir":"gate"` metadata line to add `check.subject` (with `cwd: null`, whereas M1 `CodexAdapter` populates `/scratch`) and `check.turn`; recorded `stdin`/`stdout` wire lines were untouched.
