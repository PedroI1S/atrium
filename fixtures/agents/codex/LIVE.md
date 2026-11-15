# Codex live verification runs (X1 - X3)

These checks require a real logged-in `codex` binary and must **not** be run
from offline agent workers. Before accepting any `EnforcementDescriptor`
upgrade or deleting M0 hand-made fixtures, run them on a developer machine:

## List available scenarios

```fish
cargo run -p atrium-agents --example codex-record -- --list
```

## X1 - Per-class interception probes (untested in offline fixtures)

Every tool class currently marked `Unknown`, `ProviderGated`, or `NotIntercepted`
must be probed against a live Codex session to verify whether Codex sends an
app-server JSON-RPC approval request *before* taking the side effect and
honors a `decline` reply:

```fish
cargo run -p atrium-agents --example codex-record -- x1-read
cargo run -p atrium-agents --example codex-record -- x1-edit
cargo run -p atrium-agents --example codex-record -- x1-delete
cargo run -p atrium-agents --example codex-record -- x1-move
cargo run -p atrium-agents --example codex-record -- x1-search
cargo run -p atrium-agents --example codex-record -- x1-fetch
cargo run -p atrium-agents --example codex-record -- x1-mcp
cargo run -p atrium-agents --example codex-record -- x1-subagent
```

Check each recorded fixture:
- Confirm `ATRIUM-7Q2X-CANARY` does not appear in any `.jsonl` or `.events.json`.
- Inspect `ToolCheck` vs actual effect in `--keep` scratch dir.
- Only upgrade a class in `crates/atrium-agents/src/codex/normalizer.rs` (`coverage()`) when backed by a checked-in recorded denial fixture showing no leak.

## X2 - Full-access mode vs Atrium policy

Confirm behavior under `PermissionPolicy::FullAccess` (`approvalPolicy = "never"`):

```fish
cargo run -p atrium-agents --example codex-record -- x2-fullaccess
```

Verify whether `approvalPolicy: "never"` suppresses app-server approval requests (keeping Codex enforcement `Partial` with `ProviderGated` / `NotIntercepted` warnings accurate).

## X3 - Repeated command approvals without execpolicy amendment bypass

Confirm `approvalPolicy = "untrusted"` with `approvalsReviewer = "user"` asks for
each command execution individually:

```fish
cargo run -p atrium-agents --example codex-record -- x3-exec-amendment
```

## Re-sanitizing or replacing hand-made fixtures

After recording live `patch`, `question`, `interrupt`, and `quota-usage` runs,
copy/re-sanitize verified fixtures and run:

```fish
cargo run -p atrium-agents --example codex-record -- --resanitize all
cargo test -p atrium-agents --test codex_golden
cargo test -p atrium-agents --test consistency -- --ignored
```
