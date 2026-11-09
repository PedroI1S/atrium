# Review: M1 contracts (revision 1, draft)

Reviewer: Gemini 4 Argon (read-only, D-043) · 2026-11-09

## Verdict

**rework**

Five blockers require contract/schema signature changes before freezing; workers cannot resolve them inside their owned paths.

## Findings

1. **[blocker] `ApprovalLedger::resolve` cannot create "remember" rules from `Binding`.**
   - Where: `crates/atrium-core/src/lib.rs:236-245`, `318-323`; `crates/atrium-core/src/policy/mod.rs:56-64`; `crates/atrium-core/schema/0001_init.sql:93-108`; `docs/agents/briefs/M1-03-policy.md:28`.
   - Wrong: `Ledger` holds only `Arc<dyn RequestStore>`. `resolve(key, Allow, Some(scope))` must create a `CommandPrefix { argv }` or canonicalized `PathGlob { glob }` rule, yet `Binding` and `requests` store only one-way `input_sha256: String` and optional display `title: Option<String>` (which is `None` for Codex `fileChange`, regex pattern for Claude `Grep`/`Glob`, and lacks `cwd` to resolve relative paths/symlinks).
   - Fix: Add `remember_matcher: Option<RuleMatcher>` to `Binding` and a nullable JSON column `remember_matcher TEXT` to `requests` in `0001_init.sql`, computed by `SessionGate::check` when opening the approval (where `ToolCheck` and canonical `cwd` are available). Expose `rememberable: bool` on `ApprovalRequested` so the UI does not offer "Always…" when no rule can be created.

2. **[blocker] `requests` state transitions conflict across `Store::append` and `ApprovalLedger`, leaving cancel/redeem races and broken `PolicyContext` checks.**
   - Where: `crates/atrium-core/src/lib.rs:271-299`, `318-338`; `crates/atrium-core/schema/0001_init.sql:91-110`; `docs/agents/briefs/M1-02-store.md:27`; `docs/contracts/m1.md:77-83`.
   - Wrong:
     1. `M1-02` brief (`:27`) tells `Store::append` that "`RequestResolved` moves a row to its final status", while `Ledger` mutates the same table via `RequestStore`. If `Store::append` sets `'approved'` on `RequestResolved { Approved }`, an async `append` arriving after `ToolGate::redeem` resets `'redeemed' → 'approved'` (re-enabling a single-use grant). If `Store::append` sets `'redeemed'`, `append` arriving before `redeem` causes `redeem` to fail.
     2. `lib.rs:296-297` omits `approved → cancelled`, and `RequestStore::transition` (`:271`) takes a single `from: &str`. If a provider withdraws or cancels a request after `resolve(Allow)` (`status = 'approved'`) and before `redeem`, `withdraw` cannot cancel the `'approved'` row, violating D-046.
     3. `Binding.context: PolicyContext` validation at `redeem` is unspecified. `approve_with_rule` (`:278`) increments `rules_revision` in the same transaction as `resolve(Allow)`, so checking `current.rules_revision == binding.context.rules_revision` at `redeem` denies every "Allow + remember" action; ignoring `context` allows an unredeemed grant to bypass a `Deny` rule added before `redeem`.
     4. Calling `ledger.expire_all()` at daemon startup (`m1.md:82`) sets all rows to `'expired'` before crash recovery calls `Store::open_items` (`lib.rs:166`), losing track of which `ApprovalRequested`/`QuestionAsked` events were emitted without a matching `RequestResolved`.
   - Fix:
     1. Give `ApprovalLedger` sole ownership of `requests.status` transitions (`Store::append` must never overwrite `'approved'` or `'redeemed'`; on `RequestResolved { Cancelled | Expired }`, `TurnCompleted`, and `SessionEnded`, expire/cancel both `'pending'` and `'approved'` rows, never `'redeemed'`). Change `RequestStore::transition` to accept `from: &[&str]` (allowing `&["pending", "approved"] → "cancelled"`).
     2. Specify `SessionGate::redeem`: require `turn == current_turn` and canonical input hash match; if `current_context != binding.context`, re-run `PolicyEngine::evaluate` and reject iff the new verdict is `Deny` or `HardDeny`.
     3. Track open transcript requests in an `open_requests` table (updated only on `ApprovalRequested`/`QuestionAsked` and deleted on `RequestResolved`), separate from ledger `requests`.

3. **[blocker] Policy and hard-deny bypasses via provider-assigned `ToolKind`, untyped `ToolCheck.input`, undefined Home Bash allowlist, and no-I/O symlink contradiction.**
   - Where: `crates/atrium-agents/src/lib.rs:235-241`; `crates/atrium-core/src/lib.rs:195-226`; `crates/atrium-protocol/src/policy.rs:262-265`; `docs/contracts/m1.md:121-125`; `docs/agents/briefs/M1-03-policy.md:23-25`.
   - Wrong:
     1. Codex classifies `commandExecution` with `commandActions` of `read`/`search` as `ToolKind::Read`/`Search` (`spikes/agent-codex/src/normalize.rs:60-70`). Under `M1-03` (`:23, :25`), Home auto-allows `Read`/`Search` and runs command hard-deny on `Execute`, so a Codex shell command labeled `Read`/`Search` bypasses `parse_command`, the Bash allowlist, and command hard-deny.
     2. Claude's `Grep` (`ToolKind::Search`) is auto-allowed in Home (`cwd = ~`) and reads arbitrary file contents (`path: "~/.ssh/id_ed25519"` or `glob: ".aws/**"`).
     3. `ToolCheck` exposes only opaque `input: Value` (`file_path` vs `notebook_path` vs `path`/`glob` vs Codex `changes`/`command`), with no contract across M1-03, M1-07, and M1-08 for extracting commands and paths. Unrecognized path shapes fail open on path hard-denies.
     4. Home's "read-only Bash allowlist" (`m1.md:121`) is an approval default (D-013) but is undefined in the contract, leaving a worker to choose auto-allowed host commands in `~` (e.g. `env`, `git -c …`, `find -exec`, `sed`, or `cat ~/.ss?/id_*` with shell globs/`$HOME` bypassing static `credential-read` matching). `m1.md:124` and `policy/mod.rs:30` also omit `\n` as a shell command separator (`"ls\nsudo id"`).
     5. `PolicyEngine::evaluate` is documented `"Pure: no I/O, no clock"` (`atrium-core/src/lib.rs:222`), contradicting `policy.rs:263` and `tests/policy.rs:35` (symlink resolution requires filesystem I/O, including deepest-existing-ancestor `canonicalize` when a target file does not yet exist). `PolicyInput` also lacks `home: &'a Path` and Atrium's protected paths (`runtime_dir`, `data_dir`, config dir, LaunchAgent/systemd unit, app binary), while `std::env::set_var` is `unsafe` (denied) in Rust 2024 tests.
   - Fix:
     1. Require every shell/command invocation to use `ToolKind::Execute` (never `Read`/`Search`), and add normalized `command: Option<String>` and `paths: Vec<PathBuf>` fields (or a typed `ToolSubject` enum) to `ToolCheck`, failing closed (`Ask`, after raw-string hard-deny) when a tool's subject cannot be extracted.
     2. Apply `credential-read` and `atrium-self` to `Search` (`path` and `glob`) as well as `Read`/`Edit`/`Delete`/`Move`/`Execute`; in Home (`cwd = ~`), require `Ask` for recursive `Grep` rooted at `~` without a non-sensitive subpath.
     3. Define `HOME_READONLY_COMMANDS` in `atrium-protocol::policy` for Pedro's D-013 approval; reject `\n`, `$` expansion, shell globs (`*?[]`), brace expansion, and `git -c`/`--config-env`/`--exec-path` from auto-allow and `CommandPrefix` matches.
     4. Add `home: &'a Path` and `atrium_paths: &'a [PathBuf]` to `PolicyInput`, and resolve paths (deepest-existing-ancestor `canonicalize`) in `SessionGate` before calling pure `PolicyEngine::evaluate`.

4. **[blocker] `Session` omits `ProcessIdentity`, preventing daemon crash-recovery persistence.**
   - Where: `crates/atrium-agents/src/lib.rs:106-122`; `crates/atrium-core/src/lib.rs:116-121, 172`; `docs/contracts/m1.md:134`.
   - Wrong: `Store::set_process` requires `atrium_host::supervise::ProcessIdentity`, yet `atrium_agents::Session` contains only `descriptor`, `events`, and `commands`. `atriumd` cannot obtain the spawned child's `ProcessIdentity` without modifying `atrium-agents/src/lib.rs`. In addition, spawning before a 30 s `STARTUP_TIMEOUT` handshake leaves an unrecorded orphan if `atriumd` crashes mid-handshake, and carrying over `spikes/agent-claude`'s internal `--resume` respawn on a hung interrupt (`spikes/agent-claude/src/adapter.rs:32-33`) would spawn a new OS process without a new incarnation or updated `ProcessIdentity`.
   - Fix: Add `pub process: Option<atrium_host::supervise::ProcessIdentity>` to `Session` plus an optional `on_spawn: Option<Arc<dyn Fn(&ProcessIdentity) + Send + Sync>>` callback on `SessionConfig` invoked immediately after `spawn_group` (before handshake). Forbid adapters from respawning a second OS process inside one `Session`; if `Interrupt` escalates to kill, end the session so the daemon resumes in a new incarnation.

5. **[blocker] Missing `AdapterInput` for `ToolGate::question_ticket` and turn-number race on `ToolGate`.**
   - Where: `crates/atrium-agents/src/lib.rs:208-241`, `273-336`; `docs/agents/briefs/M1-03-policy.md:27`.
   - Wrong:
     1. `Normalizer::on_input` is pure and shared by live runs and fixture replay (`lib.rs:14-15`), yet `AdapterInput` has no variant recording the `Option<ApprovalTicket>` returned by `ToolGate::question_ticket()`. `Normalizer` cannot emit `QuestionAsked` with the ledger ticket id (or suppress it on `None`) deterministically during replay.
     2. `ToolCheck` and `ToolGate::question_ticket` lack `turn: u32`. If `SessionGate` tracks `turn` from daemon-consumed `session.events`, an adapter calling `gate.check` at the start of Turn `N+1` races ahead of `TurnCompleted(N)`/`TurnStarted(N+1)` in `session.events` and binds the request to Turn `N`, which `expire_turn(N)` immediately expires.
   - Fix: Add `AdapterInput::QuestionTicket { id: String, ticket: Option<ApprovalTicket> }`, add `pub turn: u32` to `ToolCheck`, and pass `turn: Option<u32>` to `ToolGate::question_ticket(&self, turn: Option<u32>)`.

6. **[major] `HARD_DENY_DEFAULTS` has unimplementable match criteria and concrete security gaps.**
   - Where: `crates/atrium-protocol/src/policy.rs:319-348`.
   - Wrong:
     - `force-push-protected` (`:324-327`): a pure `PolicyEngine` cannot determine "the remote's default branch", and matching branch names in argv misses bare `git push --force`, `git push -f origin`, `+HEAD`, `--force-if-includes`, and `src:dst` refspecs (`HEAD:refs/heads/main`) when checked out on `main`/`dev`.
     - `pipe-to-shell` (`:332-335`): covers pipes (`curl … | sh`) but omits command/process substitution (`bash -c "$(curl …)"`, `sh <(curl …)`, `eval "$(wget …)"`), which parse as `Unparseable` and degrade to `Ask` (violating `m1.md:84`) or `Allow` under `Trusted`/`FullAccess`.
     - `credential-read` (`:328-331`): omits agent/CLI tokens (`~/.claude/.credentials.json`, `~/.claude.json`, `~/.codex/auth.json`, `~/.gemini/`, `~/.config/gcloud/`, `~/.git-credentials`, `~/.npmrc`), non-`id_*` keys in `~/.ssh/`, and writes to `~/.ssh/config` or `authorized_keys`.
     - `atrium-self` (`:336-339`): blocks only writes, leaving Home's auto-allowed `Read` free to read Atrium's SQLite DB/WAL/snapshots/logs (containing other threads' transcripts), and omits `~/.config/atrium`, LaunchAgent/systemd unit files, and the Atrium app bundle/binary.
     - `privilege-escalation` (`:340-343`): misses Linux `pkexec` and systemd `run0` (present on the target CachyOS platform) and macOS `osascript … with administrator privileges`.
     - `recursive-delete-outside-cwd` (`:320-323`): in Home (`cwd == ~`), must explicitly block `.` (resolves to `~`), ancestors of `~` (`/Users`, `/home`), and `~/*` / `/*`.
   - Fix: Tighten all six entries in `policy.rs` and `m1.md` §9: hard-deny force-pushes lacking an explicit non-protected destination refspec (include `dev`/`develop`/`trunk` in protected names and parse `src:dst` destinations); include `sh`/`bash`/`zsh`/`eval`/`source` with `$(curl|wget …)` or `<(curl|wget …)` in `pipe-to-shell`; expand `credential-read`, `atrium-self` (reads of Atrium data/logs + writes to config/units/binary), `privilege-escalation`, and `recursive-delete-outside-cwd`.

7. **[major] `Sanitizer` private fields and literal-only matching block `M1-11` export redaction.**
   - Where: `crates/atrium-agents/src/fixture.rs:80-117`, `189-194`; `crates/atrium-core/src/history/mod.rs:31-39`; `crates/atrium-core/tests/history.rs:11`; `docs/agents/briefs/M1-12-protocol-ts.md:20, 29`.
   - Wrong: `M1-11` must pass `export_redacts_secrets_split_across_deltas_and_escaped` using `&Sanitizer` without editing `fixture.rs`. `Sanitizer`'s `secrets`/`home`/`user` fields are private, and `Sanitizer::text`/`value` do only literal `.replace` — they do not decode `\uXXXX` or nested JSON strings inside `ToolCallFinished.output` (while `M1-12` updates `is_clean` to fail on them), nor do they redact common credential patterns (`sk-ant-*`, `sk-*`, `gh[pousr]_*`, `github_pat_*`, `AKIA*`, PEM private keys, `Bearer`) when exporting real sessions where secrets are not pre-registered test canaries (`M0-gate.md:52`).
   - Fix: Add escaped/nested-JSON secret redaction and standard token-pattern redaction directly to `Sanitizer` (or expose a dedicated `Redactor` in `atrium-core::history` / `Sanitizer::event(&self, &mut AgentEvent)` in the contract before freeze, and decide whether user history exports should rewrite the user's own username to `"user"`).

8. **[major] Daemon upgrade deadlock, undefined `$ATRIUM_HOME`, and peer-verification TOCTOU details.**
   - Where: `docs/contracts/m1.md:66-69`, `110`, `133`; `crates/atrium-host/src/peer.rs:11-26`, `47-58`; `crates/atrium-host/src/lifecycle.rs:22-30`; `crates/atrium-protocol/src/wire.rs:271-274`, `328-331`.
   - Wrong:
     1. Upgrading (or rebuilding) the app changes `(dev, ino)` on Linux, `cdhash` on `MacUnsigned`, or `PROTOCOL_VERSION` on `MacSigned`. The running old daemon rejects `Hello` with `Incompatible` before `ShutdownDaemon` can be sent; `Incompatible` omits `daemon_pid` and `running_turns`; `InstanceLock` blocks starting the new daemon.
     2. `$ATRIUM_HOME` (`m1.md:67, 110`; `peer.rs:12`) is not defined in `lifecycle::Paths` (`runtime_dir`, `data_dir`). Relying on a mutable `app-identity.json` for `MacSigned` weakens verification and fails on first launch if `atriumd` starts before the file is written.
     3. On `MacUnsigned`, hashing the peer's path on disk is a TOCTOU; on Linux, reading `/proc/<pid>/exe` uses numeric `pid` rather than the `pidfd`, so if the peer exited after connect its `pid` could be recycled while `pidfd` stays open.
   - Fix:
     1. Include `daemon_pid: u32` and `running_turns: u32` in `ServerMessage::Incompatible`, store the daemon's `ProcessIdentity` in the lock file, and handle `SIGTERM` in `atriumd` for graceful idle exit so an upgraded app can hand over across binary/protocol changes. Also specify `ShutdownDaemon { force: false }`'s immediate return when `running_turns > 0`.
     2. Define `app_identity` and `snapshots_dir` on `lifecycle::Paths` (under `data_dir`, honoring `$ATRIUM_HOME` override); compile the `MacSigned` designated requirement into `atriumd` instead of reading `app-identity.json`.
     3. Document in `peer.rs`: `MacUnsigned` reads `kSecCodeInfoUnique` (`cdhash`) from `SecCodeCreateWithAuditToken`; Linux obtains `SO_PEERPIDFD`, runs `stat("/proc/<pid>/exe")`, and then checks `pidfd_send_signal(pidfd, 0) == 0` to prove the peer was still alive throughout `/proc/<pid>/exe` inspection.

9. **[major] `supervise::kill_recorded` marks live orphaned descendants `Cleanup::Verified` when the leader already exited.**
   - Where: `crates/atrium-host/src/supervise.rs:53-74`.
   - Wrong: `supervise.rs:68-69` returns `Cleanup::Verified` when `recorded` "was killed or is already gone". After a daemon crash, the leader often exits on stdin `EOF` and is reaped by init while tool descendants in `recorded.pgid` continue running (`M0-gate.md:41-42`). Returning `Verified` when `identify(recorded.pid)` is `None` without checking `kill(-pgid, 0) == ESRCH` falsely reports execution stopped. In `terminate` (`:56`), reaping `child` before `kill(-pgid, SIGKILL)` also allows `pgid` reuse in the gap.
   - Fix: In `kill_recorded`, when `identify(recorded.pid)` is `None`, return `Cleanup::Verified` only if `kill(-pgid, 0)` fails with `ESRCH`; if processes remain in `pgid` and cannot be verified (`start_time >= recorded.start_time`), return `Cleanup::Unverified`. In `terminate`, send `SIGKILL` to `-pgid` before reaping the zombie leader (or poll exit with `WNOWAIT` first).

10. **[major] `EnforcementDescriptor::evaluate` edge cases, missing `ProbeProviders` command, and Claude/Codex coverage gaps.**
    - Where: `crates/atrium-protocol/src/policy.rs:80`, `157-213`; `crates/atrium-protocol/src/wire.rs:194-275`; `docs/contracts/m1.md:54-56`, `121`; `docs/agents/briefs/M1-07-claude-adapter.md:25`; `docs/agents/briefs/M1-08-codex-adapter.md:26`.
    - Wrong:
      1. `evaluate` (`policy.rs:166`) yields `EnforcementLevel::Full` if passed `cli_version: None, version_tested: true`, and `coverage.iter().find(...)` (`:177`) inspects only the first entry per `ToolClass`, ignoring duplicate weaker entries. `m1.md:55` ("every tool class ... is `Coverage::Intercepted`") contradicts `policy.rs:181` (`Intercepted | NotApplicable`).
      2. `DaemonCommand` has no command calling `AgentAdapter::probe`, leaving the UI unable to render enforcement descriptors in the provider picker or *Settings → Agents* (`m1.md:54`, `PLAN.md:323`).
      3. `M1-07`: `spikes/agent-claude/src/settings.rs:133-138` ignored existing settings files on read/parse errors (`PermissionDenied`, invalid JSON) and ignored command-executing settings keys (`apiKeyHelper`, `awsAuthRefresh`, `awsCredentialExport`, `otelHeadersHelper`, `statusLine`), `--plugin-dir`, and `enabledPlugins`. Also, `M1-07` (`:26`) must build `redeem`'s `ToolCheck` from `can_use_tool`'s payload, not a cached pre-hook `ToolCheck`.
      4. `M1-08` (`:26`) and `m1.md:121` specify `approval_policy=on-request` for Codex Home, whereas M0-02's live fixtures and `gate-deny` proof used `approvalPolicy="untrusted"` (`docs/spikes/M0-02-codex-adapter.md:46, 390`). Under `read-only` + `on-request`, Codex reads `~/.ssh` without asking Atrium's gate and existing fixture `Stdin` lines diverge.
    - Fix: Require `cli_version.is_some()` for `Full` and fold all matching `ClassCoverage` entries taking the weakest coverage in `evaluate`; align `m1.md:55` on `NotApplicable`; add `DaemonCommand::ProbeProviders { preset: Preset }`; require `M1-07` to treat unreadable/unparseable existing configs as `inspected: false` and command-executing settings as `ExternalHooks`, and to pass `can_use_tool`'s input to `redeem`; reconcile Codex `untrusted` vs `on-request` explicitly.

11. **[major] `attention::rank_inbox` lacks required inputs on `ThreadSummary`, `next_state` loses `Failed`/`Paused`/`Ended` distinctions, and disk-full blocks cleanup writes.**
    - Where: `crates/atrium-core/src/attention/mod.rs:22-36`; `crates/atrium-protocol/src/thread.rs:30-105`; `crates/atrium-core/src/lib.rs:39-42`, `69-79`, `137-141`, `269`; `crates/atrium-core/schema/0001_init.sql:13-30`.
    - Wrong:
      1. `rank_inbox(threads: &[ThreadSummary], ...)` must output `InboxItem.detail` ("the approval's title, the question's first prompt") and `InboxItem.since_ms` ("when the item started needing the user"), neither of which exists on `ThreadSummary` or `threads` (`updated_at_ms` changes on any event; `open_question` takes no prompt/title).
      2. `next_state` cannot distinguish user-requested `Shutdown` (Claude exits `1` or `None` after `SIGKILL` → `Ended`) from a crashed provider (`Interrupted { cleanup: Verified }`), and trailing `SessionEnded`/`Signal::Exited` after `Error { fatal: true }` or `RateLimited` + `TurnCompleted { Failed }` will overwrite `Failed`/`Paused` unless terminal/paused precedence is defined.
      3. Refusing *every* store write below `MIN_FREE_DISK_BYTES` (`M1-02` brief `:31`) makes `Store::signal(StorageReadOnly)`, `RequestStore::expire`, and `set_process(..., None)` fail with `StoreError::ReadOnly`, preventing the daemon from interrupting turns and expiring approvals on disk-full (`m1.md:108`).
    - Fix: Add `attention_since_ms: Option<u64>` and `attention_detail: Option<String>` to `ThreadSummary` and `threads` (and `prompt: Option<String>` to `open_question` or populate from `QuestionAsked`); add `Signal::Stopped` and document sticky `Failed`/`Paused` transitions in `attention/mod.rs`; exempt lifecycle/cleanup mutations (`signal`, `expire`/`withdraw`/deny, `set_process(..., None)`, closing events) from the soft `min_free_disk_bytes` check.

12. **[major] `MAX_EVENT_BYTES` is smaller than allowed adapter inputs, and wire errors/commands have gaps.**
    - Where: `crates/atrium-protocol/src/limits.rs:16-36`; `crates/atrium-core/src/lib.rs:48-50`, `129-134`; `crates/atrium-protocol/src/wire.rs:180-275`, `361-366`.
    - Wrong:
      1. `MAX_PARTIAL_ARGS_BYTES` (4 MiB) and `MAX_PROVIDER_RECORD_BYTES` (8 MiB) allow valid `ToolCallStarted.input`, `ApprovalRequested.input`, and `Unmapped.raw` events exceeding `MAX_EVENT_BYTES` (2 MiB). `StoreError::EventTooLarge` (`lib.rs:49`) also contradicts `limits.rs:35` ("replaced by a fatal `Error`").
      2. `Store::append` does not specify rejecting stale incarnations (`incarnation != threads.incarnation`), allowing a late task from an older incarnation to corrupt thread projections.
      3. `CommandResult::Err { message: String }` has no structured error code (`Expired`, `AlreadyResolved`, `StaleIncarnation`, `StorageReadOnly`, `TurnsRunning`, `UnknownThread`), forcing fragile client string matching. `RenameThread` (promised in `thread.rs:62`) is missing from `DaemonCommand` and `Store`, `CreateRule` passes dummy `id`/`origin`/`created_at_ms`, and `SubscribeInbox`'s `Response` payload/ordering is unspecified.
    - Fix: Raise `MAX_EVENT_BYTES` above `MAX_PROVIDER_RECORD_BYTES` (e.g. 10 MiB, under `MAX_FRAME_LEN = 16 MiB`) or cap event `input`/`raw` below `MAX_EVENT_BYTES` and reconcile `EventTooLarge`; reject stale incarnations in `Store::append`; add `code: WireErrorCode` to `CommandResult::Err`, add `RenameThread`, sanitize `CreateRule` server-side, and define `SubscribeInbox`'s `Response`.

13. **[major] Wave-1 dispatch order, owned-path omissions, `specta` `u64`/`Value` traps, and `sha2` BLOB assumption.**
    - Where: `docs/contracts/m1.md:48`, `180-191`; `docs/agents/briefs/M1-07-claude-adapter.md:31-33`; `docs/agents/briefs/M1-08-codex-adapter.md:30-32`; `docs/agents/briefs/M1-12-protocol-ts.md:17-28`; `crates/atrium-core/schema/0001_init.sql:79`.
    - Wrong:
      1. `m1.md:191` schedules `M1-07` and `M1-08` in Wave 1 alongside `M1-10`, while `atrium_host::supervise::spawn_group` is `unimplemented!("M1-10")` — panicking every live recording (`M1-07` DoD #1) and subprocess test in both adapters, while `#[ignore = "needs M1-10"]` (`M1-07:33`) violates `m1.md:48` ("none may be ignored at its merge").
      2. `M1-12` (`:28`) tells the worker to add `sha2` to `crates/atrium-agents/Cargo.toml`, which is omitted from `M1-12`'s `Owned paths` (`:17-22`). `M1-12` (`:25`) also references `spikes/ui-shell/src-tauri/Cargo.lock` for `specta` versions (that lockfile does not contain `specta`) and omits configuring `BigIntExportBehavior::Number` (without which `specta` rejects all `u64` fields) and `specta`'s `serde_json` feature.
      3. `M1-13` (`m1.md:188`) tests socket roles and ledger races but lists only `M1-03, M1-10` in `Depends on`, missing `M1-02` and `M1-05`.
      4. `0001_init.sql:79` and `M1-02:20, 26` suggest persisting incremental `sha256_state BLOB` with `sha2`, whose `Sha256` type does not expose serializable internal state in `0.10`.
    - Fix: Sequence `M1-10` before `M1-07`/`M1-08` (pull `M1-04` or `M1-06` into Wave 1 instead); add `crates/atrium-agents/Cargo.toml` to `M1-12` owned paths and specify exact `specta`/`specta-typescript` version + `BigIntExportBehavior::Number` + `serde_json` feature; add `M1-02, M1-05` to `M1-13` dependencies; note in `M1-02` to keep live `Sha256` hashers in the writer task (re-hashing from stored deltas only on crash recovery) or serialize explicit SHA-256 state words.

14. **[major] Unclosed M0 gate and must-fix items (`streaming memory`, `PLAN §6` vs D-012, `scripts/doctor`, CR-01/02/04 & L1).**
    - Where: `docs/agents/m1-must-fix.md:73, 77, 81, 87`; `docs/contracts/m1.md:44, 158-170, 183`; `PLAN.md:788`; `scripts/doctor:1-69`; `docs/agents/change-requests.md:5-32, 53-66`.
    - Wrong:
      1. Streaming memory (`m1-must-fix.md:73`, `M0-gate.md:82`): neither `limits.rs` nor `m1.md` §11/§14 bounds retained in-webview transcript/virtualizer state or reports WebContent peak memory under the §14 10×1k tok/s streaming benchmark (1.05–1.39 GB in M0-07).
      2. `PLAN.md:788` still says `(if a release breaks it, agy threads only run inside the sandbox)`, contradicting D-012 (`m1-must-fix.md:77`).
      3. `scripts/doctor` (`m1.md:44`) lacks every D-030/D-036 preflight check (`realpath` of checkout/worktree/git-dir outside iCloud/cloud-sync folders, `git fsck`, `* [0-9]*` conflict-copy scan), still hardcodes `tested="1.4.7"` (`:47`, vs `agy` 1.5.1 in D-043), and fails hard if `codex` is absent on Pedro's machine (`:25`).
      4. CR-01, CR-02, and CR-04 (including CR-04 #2's unresolved choice on the "Blur strength" row and L1's Linux wording) are referenced in `m1.md:183` without recording their accepted specification in `m1.md`.
    - Fix: Add a window transcript bound to `limits.rs` and a streaming WebContent memory reporting column to `m1.md` §14; update `PLAN.md:788` to report Partial rather than forcing sandbox; add D-030/D-036 checks and updated tool rules to `scripts/doctor`; record the accepted CR-01/02/04 + L1 token/UI spec in `m1.md`.

15. **[minor] Private log mode in `spawn_detached`, `MAX_PENDING_REQUESTS` enforcement point, and inline SVG / `file:` reveal hardening.**
    - Where: `crates/atrium-host/src/lifecycle.rs:52`; `crates/atrium-protocol/src/limits.rs:13`, `25-27`; `docs/contracts/m1.md:148-150`.
    - Wrong:
      1. `lifecycle::spawn_detached` does not specify creating `log` mode `0600` without following symlinks (`O_NOFOLLOW`), risking world-readable daemon logs (`m1-must-fix.md:71`).
      2. Enforcing `MAX_PENDING_REQUESTS` (`limits.rs:25-27`) in the adapter *after* `ToolGate::check`/`question_ticket` leaks an unexposed `'pending'` row in `requests`.
      3. `m1.md:148-150` allows `data:` images without excluding `data:image/svg+xml`, omits `referrerpolicy="no-referrer"` on clicked remote images, and should state `file:` links inside `cwd` (`~` in Home) reveal in file manager only (never OS-open/execute) after resolving symlinks. `agent.rs:36` should specify splitting oversized inline messages into `MAX_INLINE_MESSAGE_BYTES` chunks on UTF-8 char boundaries.
    - Fix: Require `0600` + `O_NOFOLLOW` on `log` in `spawn_detached`; enforce `MAX_PENDING_REQUESTS` inside `SessionGate`/`ApprovalLedger` before inserting; restrict inline `data:` images to raster MIME types (`png`/`jpeg`/`gif`/`webp`), set `referrerpolicy="no-referrer"`, specify reveal-only on resolved `file:` paths, and specify chunk size for split completion-only messages.

## Checked and Sound

- Workspace builds and passes existing unit/shape tests (`cargo check --workspace --all-targets`, `cargo test --workspace`).
- Workspace-wide `unsafe_code = "deny"` with exception isolated to `atrium-host` (`undocumented_unsafe_blocks = "deny"`).
- Credential-safe `Debug` implementations on `SessionConfig` and `McpServer` (`crates/atrium-agents/src/lib.rs:142-154, 186-196`), plus `Sanitizer` covering `AdapterInput::Command` and ordering secrets before path/username replacements.
- Separation of interception coverage, policy decisions, and OS isolation in `PLAN.md` §3.1/§3.12 and `m1.md` §3/§4, including explicit documentation of the same-UID non-isolation boundary (G1, G2, D-044, D-045).
- Amended CR-03 (`MessageBody::Streamed` with `chars`/`deltas`/`sha256` and no oversized `Inline` fallback), `ContextUpdated` separated from per-turn `Usage`, `Error::details` preserving the two-event pre-session failure lifecycle, bounded terminal event tail (`TERMINAL_TAIL_EVENTS`), two-lane socket framing with per-subscription `SubscriptionLagged`, per-window delivery/resync isolation, and G5 statistical budgets (warm TTI median/p90, 60 s idle footprint/private-PSS medians, 60 s post-close daemon RSS).
