# M1-07-claude-adapter: Claude Code adapter (production)

Model: Sonnet 5.5 · Reviewer: Gemini 3.8 Flash (D-062; Argon is out of usage) · Depends on: contract (M1 rev 3), M1-10 merged (it is)
Go: D-062 (Pedro, 2026-11-12). Remembered command rules are global (D-062): `rememberable` needs no folder.
Branch: `agent/sonnet/M1-07-claude-adapter` · Worktree: `../atrium-wt/M1-07-claude-adapter`

**On rodrigo's shifts (D-065, D-066):** a Sonnet 5.5 worker finishes the task from `docs/agents/handoffs/M1-07-claude-adapter.md` (status partial, "Next step"); the reviewer is Gemini 4 Argon (contract §15). The live fixtures are recorded: don't re-record them unless a test proves one wrong. The agent's shell runs in a container without `git` and `python3`: run those with `distrobox-host-exec <command>`; `cargo` works in the container.

## Goal
Fold the M0-01 spike into `atrium-agents` as the production Claude Code adapter, on the M1 contract, with every M1 must-fix item for Claude closed.

## Context
- `docs/contracts/m1.md` §3, §5, §6.1, §10. M0 contract §3 (fixtures and required scenarios) still applies.
- `crates/atrium-agents/src/lib.rs` (rev 3), `crates/atrium-protocol/src/{agent,policy,limits}.rs`.
- The spike: `spikes/agent-claude/` and its report `docs/spikes/M0-01-claude-adapter.md`.
- `docs/agents/reviews/M0-gate.md` §2 Claude row and §3 item 3; `m1-must-fix.md`: "Claude" line, process groups, credential-safe Debug, X6.
- Decisions: D-017 (Claude keeps its native settings; external hooks make enforcement Partial), D-044, D-046.

## Owned paths
- `crates/atrium-agents/src/claude/**` (declare `pub mod claude;` in a one-line change to `lib.rs`, the only edit allowed there)
- `crates/atrium-agents/tests/claude*.rs`, `crates/atrium-agents/examples/claude-*.rs`
- `crates/atrium-agents/Cargo.toml`: dependency lines only, with `# M1-07`
- `fixtures/agents/claude/**`
- `docs/agents/handoffs/M1-07-claude-adapter.md`

## Contract to implement
- `claude::ClaudeAdapter: AgentAdapter` and `claude::ClaudeNormalizer: Normalizer`.
- `probe` and the session descriptor (built only with `EnforcementDescriptor::evaluate`): inspect user, project, local and managed settings, plugin directories, `--settings` and `--plugin-dir` files and `enabledPlugins`. Plugin hooks, and any existing file you can't read or parse, are `inspected: false` (the spike silently skipped unreadable files: `spikes/agent-claude/src/settings.rs:133`). Hooks and command-running keys (`apiKeyHelper`, `awsAuthRefresh`, `awsCredentialExport`, `otelHeadersHelper`, `statusLine`) → `ExternalHooks`. `tested_versions` lists the versions your live fixtures were recorded with.
- Every `ToolCheck` carries the extracted `ToolSubject` (Bash → `Command`, Read/Edit/Write/NotebookEdit → `Paths`, Grep/Glob → `Paths` with `glob` and `recursive`, WebFetch → `Fetch`, MCP → `Mcp`; anything else `Unknown`) and your own turn counter.
- Approvals: `ApprovalRequested.request_id` and `rememberable` come from the gate's ticket. On `ResolveApproval { Allow }`, call `gate.redeem` immediately before answering `can_use_tool`, with a `ToolCheck` built from that `can_use_tool` payload (never a cached one); anything but `Allow` answers deny. Record each redeem as `AdapterInput::Redeem`. Questions use `gate.question_ticket(turn)`, recorded as `AdapterInput::QuestionTicket`.
- Spawn through `atrium_host::supervise::spawn_group`, call `SessionConfig::on_spawn` right after, before the handshake, and set `Session::process`. Never spawn a second process within one session: the spike's resume-respawn on a hung interrupt (`spikes/agent-claude/src/adapter.rs:32`) becomes "end the session" (the daemon resumes in a new incarnation).
- `MessageBody` (`Streamed` with chars/deltas/sha256; `Inline` only for short completion-only messages), `ContextUpdated` for context figures, `Error::details` for pre-session diagnostics, output caps and record limits from `limits.rs`.
- Shutdown: soft stop, then `atrium_host::supervise::terminate` on the process group. Closing events go through a bounded terminal tail (`TERMINAL_TAIL_EVENTS`) and are never dropped after 300 ms (`spikes/agent-claude/src/adapter.rs:696`).

- User messages: right after `TurnStarted` (after `SessionStarted` if the provider starts lazily), emit the turn's user message as `MessageKind::User` from the `SendUserMessage` you sent, with the inline/delta rules.
- Record the descriptor you build (at start and on every change) as `AdapterInput::Enforcement`, so fixture replay emits the same `SessionStarted.enforcement`/`EnforcementChanged`. The sanitizer already cleans it and every `ToolCheck.subject`.

## Definition of Done
- [ ] All nine M0 scenarios re-recorded **live** with the current `claude` CLI and the cheapest model, sanitized, `Sanitizer::is_clean` asserted; goldens regenerated on the rev-3 event shapes.
- [ ] New fixtures: `plugin-hook` (a plugin with a hook → Partial with the reason), `stale-approval` (an Allow that arrives after the turn ended is never executed).
- [ ] Saturation test (receiver never read): `Interrupt` and `Shutdown` work within `SHUTDOWN_GRACE`, the whole process group is gone, and `SessionEnded` is delivered once the receiver drains.
- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p atrium-agents`
- [ ] Handoff with outputs, the enforcement matrix (M0 contract §3.1) for the tested version, and the descriptor printed for your own machine's setup (sanitized).

## Out of scope
The policy decisions (the gate is given to you; tests use a scripted gate), the daemon, the Codex adapter, MCP injection beyond keeping `SessionConfig::mcp_servers` working.

## Escalate if
The CLI's control protocol changed in a way the contract can't express; a must-fix item can't be closed as written; two fix rounds without passing.

## Escalation (2026-11-14, D-071): Gemini 4 Argon, reviewed by GPT-6.1 Sol
Three reviews (`docs/agents/reviews/M1-07-claude-adapter.md`, `-round2.md`, `-round3.md`) show that expanding a glob and then trying to bound it keeps leaking. Read `-round3.md` in full first, then the handoff's "Fix round 1" and "Fix round 2". Same branch, same owned paths. Do these, each with a test that fails first (end to end through `atrium_core::policy::DefaultPolicy`, `Preset::Home` and `Preset::FullAccess`, as `tests/claude_policy_e2e.rs` does):
1. **The subject of `Glob` and `Grep` follows a small grammar; anything outside it is `ToolSubject::Unknown`.**
   - A backslash anywhere: `Unknown` (as now).
   - `~`: the base or the pattern may start with `~` only as exactly `~` or `~/…`. `~user`, `~*`, `~?…`: `Unknown` (blocker 2).
   - Braces `{…}`: their content may hold only ASCII letters, digits, `_`, `-`, `.` that is not a whole alternative, and `,`; no nested braces, no empty alternative, no alternative that is `.` or `..`. Classes `[…]`: only ASCII letters, digits, `_`, and ranges within one of `a-z`, `A-Z`, `0-9`; no negation (`!`, `^`), no `[:class:]`. Any other `{`, `}`, `[` or `]`: `Unknown`. Check the raw pattern with these rules before any expansion; then interleaving (blocker 1) can't parse. The 32-expansion cap stays.
   - Every expansion that contains a `/`, joined to the base, goes into `paths` as well as the base (the major: `.ssh/id_ed25519` from `..`, `.aws/cred*` from home), so the policy's own checks see the full pattern. Climbing and absolute expansions keep what fix round 2 gives them.
2. **Ordinary patterns keep their verdict**: `**/*.rs`, `src/**/mod.rs`, `*.{ts,tsx}`, `src/{a,b}/mod.rs`, `*.[ch]`, `*.[a-z]`, `file-[0-9].rs` stay Allow in `Preset::Home` with base `cwd` (minor 4).
3. **An exhaustive test**: enumerate every pattern up to length 5 over the alphabet `{ } [ ] , . / ~ * a -` (plus a fixed suffix `/.ssh/id_ed25519` and none), with base `cwd` and base `..`; for each one the policy allows in `Preset::Home` or `Preset::FullAccess`, assert that no credential probe of the test layout matches the pattern joined to the base (use `globset`, as the policy does). Keep its run time under 30 seconds in a debug build (if the full set is too slow, drop `-` from the alphabet before shortening the length, and say what you dropped); say how many patterns it checks.
4. **Minors 5 and 6** of round 3: an oversize `can_use_tool` is inside an established session and an open turn; `Proc::drop` with an exited, unreaped leader still kills the group, if that can be done safely inside `crates/atrium-agents/src/claude/**` (the leader is unreaped, so its pid and pgid can't be reused yet); if it needs `atrium-host`, leave a finding for N6's task instead.
5. The full Definition of Done (`cargo build -p atrium-agents --examples` first), and an "Escalation" section in the handoff: one row per round-3 finding and per item above, the test names, the exhaustive test's count and time, the DoD outputs.
- **Not yours:** `crates/atrium-core/**` (the policy's side is P1 in `docs/agents/m1-must-fix.md`), the contract, goldens (only regenerated from recorded `.jsonl`).
- Worker rule 6: hostile patterns are Rust string literals in tests, written with the file tools, never in a shell command or heredoc.
