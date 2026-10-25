# approval-allow

live. Protocol: atrium_m0/0.163.0 (Mac OS 27.0.1; arm64) unknown (atrium_m0; 0.0.0), app-server v2.
Model: "gpt-5.6-luna".
Command argv (private values redacted): `["codex","app-server","--listen","stdio://","-c","features.hooks=false"]`.
Thread parameters: `{"cwd":"/scratch","model":"gpt-5.6-luna","sandbox":"read-only","approvalPolicy":"untrusted","approvalsReviewer":"user"}`.

Reproduce: `cargo run -p spike-agent-codex --example record -- approval-allow`.
Prompt: Run python3 -c 'print(42)' after approval. Do not retry if denied.

Live transcript; enforcement claims require the raw-stream PASS checks and matrix qualifications.
