# Review: M1-07-claude-adapter, round 2 (fix round 1)...`). Per the workspace rules, I am reporting the block and stopping without attempting to work around the hook.

Below is the complete review content for `docs/agents/reviews/M1-07-claude-adapter-round2.md`:

---

# Review: M1-07-claude-adapter, round 2 (fix round 1)
Reviewer: Gemini 4 Argon (read-only) · 2026-11-13
## Verdict
**rework**: Finding 2 remains open (`glob_target` still lets brace/character-class patterns leave the base as `Ok(None)` and strips wildcard filenames on relative/tilde globs so `DefaultPolicy` misses `Preset::Home` escapes and `credential-read` hard-denies), and Fix Round 1 introduced three major defects (`ClaudeNormalizer::held` deadlocking pre-`init` approvals/questions and dropping `RequestResolved`, non-canonical `gate_key` hashing under `serde_json`'s `preserve_order`, and inline `--settings` discarding `enabledPlugins`).

## Round 1 findings
- **Finding 1 (In-flight `gate.check` after `control_cancel_request` or `"result"`)**: **closed with a gap** — `f1_*` (`crates/atrium-agents/tests/claude_adapter.rs:1205-1298`) fail on `daf3bb5` and pass at `HEAD` because `Internal::Gate` (`crates/atrium-agents/src/claude/adapter.rs:1240`) checks `self.live.contains(reply.request_id())` and `forget_native`/`clear_requests` abort `inflight`/`gates`; however, `Internal::Redeem` (`crates/atrium-agents/src/claude/adapter.rs:1256-1263`) and `Internal::Question` (`crates/atrium-agents/src/claude/adapter.rs:1282-1288`) still call `self.feed(...)` before checking `self.live`, and `self.live` shares one flat ID namespace across `hook_callback` and `can_use_tool` (New finding 7).
- **Finding 2 (`Glob`/`Grep` subjects in `tools.rs`)**: **not closed** — `f2_*` (`crates/atrium-agents/tests/claude_review.rs:521-650`) fail on `daf3bb5`, but `glob_target` (`crates/atrium-agents/src/claude/tools.rs:166-204`) still returns `Ok(None)` for brace/character-class escapes (`"{a,}/home/ana/.ssh/id_ed25519"`, `".{.,.}/.ssh/id_ed25519"`, `"[.][.]/.ssh/id_ed25519"`) and strips the wildcarded component on relative/tilde globs (`"../.aws/cred*"`, `"~/.ss*/id_ed25519"`), bypassing `Preset::Home` and `HARD_DENY_DEFAULTS` `credential-read` (New finding 1).
- **Finding 3 (Oversize `can_use_tool` arguments)**: **closed** — `f3_*` (`crates/atrium-agents/tests/claude_review.rs:130-199`, `crates/atrium-agents/tests/claude_adapter.rs:1303-1349`) fail on `daf3bb5` and pass at `HEAD`; `on_can_use` (`crates/atrium-agents/src/claude/adapter.rs:1079-1086`) denies inputs over `MAX_PARTIAL_ARGS_BYTES` without calling the gate and `can_use_tool` (`crates/atrium-agents/src/claude/normalizer.rs:902-910`) fails the call once with empty input.
- **Finding 4 (Events before `SessionStarted` and outside any turn)**: **closed with a gap** — `f4_*` (`crates/atrium-agents/tests/claude_review.rs:228-339`) fail on `daf3bb5` and pass at `HEAD`, but holding pre-establishment events in `self.held` (`crates/atrium-agents/src/claude/normalizer.rs:1372-1374`) deadlocks any approval/question that arrives before `system/init`, drops `RequestResolved` and mismatches turn numbers when `result` precedes `system/init`, and grows `self.held` without bound when a session never establishes (New finding 2).
- **Finding 5 (Non-closing flood during `stop_process` consuming `TERMINAL_TAIL_EVENTS`)**: **closed** — `f5_thousands_of_records_during_shutdown_do_not_drop_the_closing_events` (`crates/atrium-agents/tests/claude_adapter.rs:1354-1415`) fails on `daf3bb5` and passes at `HEAD`; `self.finishing = true` is set after `stop_process` (`crates/atrium-agents/src/claude/adapter.rs:940-944`), and non-closing stdout events during the grace period are bounded by `self.pending.len() < PENDING_LIMIT` (`4096`, `crates/atrium-agents/src/claude/adapter.rs:969`).
- **Finding 6 (Settings inspection: `fileSuggestion`, unparsed `hooks`, `display_path`)**: **closed with a gap** — the three unit tests (`crates/atrium-agents/src/claude/settings.rs:621-690`) fail on `daf3bb5` and pass at `HEAD`, and no `hooks` shape with a handler parses as inspected-and-clean, but inline `--settings '{...}'` (`crates/atrium-agents/src/claude/settings.rs:184-190`) discards the `enabledPlugins` vector returned by `inspect_text` (New finding 4).
- **Finding 7 (`gate_key` and FIFO ticket queues for `hook_asked` / `asked`)**: **closed with a gap** — `f7_*` (`crates/atrium-agents/tests/claude_review.rs:344-421`, `crates/atrium-agents/tests/claude_adapter.rs:1419-1499`) fail on `daf3bb5` and pass at `HEAD`, but `gate_key` (`crates/atrium-agents/src/claude/tools.rs:118-121`) formats `serde_json::Value` with `preserve_order` enabled (`Cargo.toml:40`), so key reordering between `hook_callback` and `can_use_tool` misses `hook_asked` and asks the gate a second time (New finding 3).
- **Finding 8 (Late `content_block_delta` after `assistant` completion)**: **closed with a gap** — `f8_a_late_delta_after_the_assistant_record_does_not_complete_the_message_twice` (`crates/atrium-agents/tests/claude_review.rs:426-459`) fails on `daf3bb5` and passes at `HEAD`, but `complete_message` (`crates/atrium-agents/src/claude/normalizer.rs:546-569`) does not check `self.done_msgs`, so a late `assistant` record after `close_messages` closed `"m1:0"` still emits a second `MessageCompleted` (New finding 5).
- **Finding 9 (`Session.descriptor` vs `SessionStarted.enforcement`; `plugin-hook` 80-char clip)**: **closed** — `f9_a_descriptor_that_changes_before_the_session_starts_is_reported_as_a_change` (`crates/atrium-agents/tests/claude_review.rs:464-505`) fails on `daf3bb5` and passes at `HEAD`, and `a_long_plugin_path_is_cut_to_80_characters_in_the_reason` (`crates/atrium-agents/src/claude/settings.rs:695-719`) confirms `.../plugin-hook/plugi` is `clip(id, 80)` at `crates/atrium-agents/src/claude/settings.rs:380`.
- **Dropped `start` (`a_start_dropped_during_the_handshake_kills_the_process_group`)**: **closed with a gap** — the test (`crates/atrium-agents/tests/claude_adapter.rs:1504-1525`) fails on `daf3bb5` and passes at `HEAD`, and `stop_process` takes `self.child` first (`crates/atrium-agents/src/claude/adapter.rs:708`) so normal shutdown never double-terminates, but `Proc::drop` (`crates/atrium-agents/src/claude/adapter.rs:414-420`) only spawns an async task on `Handle::try_current()`, leaving the process group alive if dropped outside a Tokio runtime or during runtime shutdown (New finding 6).

## New findings
1. [blocker] **`glob_target` returns `Ok(None)` on brace/character-class escapes (`{a,}/…`, `.{.,.}/…`, `[.][.]/…`) and strips the wildcard filename on relative/tilde globs (`../.aws/cred*`, `~/.ss*/id_ed25519`), bypassing `Preset::Home` and `HARD_DENY_DEFAULTS` (`credential-read`)**
   - Where: `crates/atrium-agents/src/claude/tools.rs:172-203` (`glob_target`), interacting with `crates/atrium-core/src/policy/deny.rs:840-854`, `crates/atrium-core/src/policy/deny.rs:1061-1088` (`Ctx::check`, `Ctx::search_glob_hit`) and `crates/atrium-core/src/policy/engine.rs:564-578` (`Home::evaluate`):
     ```rust
     let first = glob.find(WILDCARDS);
     let (prefix, rest) = glob.split_at(first.unwrap_or(glob.len()));
     // After the first wildcard, nothing may climb out or restart at a root
     // (`{/etc/passwd,x}`, `**/../..`).
     let climbs = |text: &str| text.split(['/', '{', '}', ',']).any(|seg| seg == "..");
     if climbs(rest)
         || rest.contains("{/")
         || rest.contains(",/")
         || rest.contains("{~")
         || rest.contains(",~")
     {
         return Err(());
     }
     let leaves = prefix.starts_with('/') || prefix.starts_with('~') || climbs(prefix);
     if !leaves {
         return Ok(None);
     }
     // No wildcard: the whole thing is the target. With one, its directory.
     let dir = if first.is_none() {
         prefix
     } else {
         match prefix.rfind('/') {
             Some(i) => &prefix[..=i],
             None if prefix.starts_with('~') => "~",
             None => "",
         }
     };
     ```
   - Sequence:
     1. **Case A (Brace/character-class escape in `Preset::Home`)**: Construct `DefaultPolicy::new(PolicyConfig::new(Preset::Home))` with `home = "/home/ana"` and `cwd = "/home/ana/work"`. Evaluate `tool_check(Some("t1"), "Glob", &json!({"path": "/home/ana/work", "pattern": "{a,}/home/ana/.ssh/id_ed25519"}), 1)` (or `"pattern": ".{.,.}/.ssh/id_ed25519"`, or `"pattern": "[.][.]/.ssh/id_ed25519"`, or `"pattern": "{a,}/etc/passwd"`).
     2. **Case B (Relative/tilde wildcard credential read)**: Construct `DefaultPolicy` with `Preset::FullAccess` (or `Preset::Home`), `home = "/home/ana"`, `cwd = "/home/ana/work"`. Evaluate `tool_check(Some("t1"), "Glob", &json!({"path": "/home/ana/work", "pattern": "../.aws/cred*"}), 1)` (or `"pattern": "~/.aws/*"`, `"pattern": "../.ss*/id_ed25519"`, or `"pattern": "~/.ss*/id_ed25519"`).
   - Result:
     - In Case A, `first` is `Some(0)` (or `Some(1)` for `".{.,.}/..."`), `prefix` is `""` (or `"."`), `climbs(rest)` is `false` (`".{.,.}/.ssh/id_ed25519".split(['/', '{', '}', ','])` yields `["", ".", ".", "", ".ssh", "id_ed25519"]`, and `"[.][.]/.ssh/id_ed25519"` has `"[.][.]"`), and `rest` does not contain `"{/"`, `",/"`, `"{~"`, or `",~"` (for `"{a,}/..."`, the substring is `",}/"`). `leaves` is `false`, so `glob_target` returns `Ok(None)`, `paths` is `vec!["/home/ana/work"]`, `search_glob_hit` ignores probes outside `/home/ana/work`, and `Home::evaluate` returns **`PolicyVerdict::Allow`** for reading `~/.ssh/id_ed25519` or `/etc/passwd`.
     - In Case B, `leaves` is `true` and `first.is_some()`, so `glob_target` strips `"cred*"` (or `".ss*/id_ed25519"`) at `prefix.rfind('/')` (`crates/atrium-agents/src/claude/tools.rs:193-194`) and puts `"/home/ana/work/../.aws/"` (or `"/home/ana/work/../"`) in `paths`. In `deny::Ctx::check` (`crates/atrium-core/src/policy/deny.rs:841-850`), `u.word` has no wildcard (`globby == false`), so `self.s.glob_hit` is never called; `self.s.hit("/home/ana/.aws", Role::Read)` and `self.s.hit("/home/ana", Role::Read)` return `None` (`.aws/credentials` is in `CRED_FILES`, not `CRED_TREES`), and `is_ssh_dir` is `false`. In `Ctx::search_glob_hit` (`crates/atrium-core/src/policy/deny.rs:1061-1088`), `GlobBuilder::new("../.aws/cred*")` (or `"../.ss*/id_ed25519"`) matches neither `name` (`"credentials"` / `"id_ed25519"`) nor `probe` (`"/home/ana/.aws/credentials"` / `"/home/ana/.ssh/id_ed25519"`). `hard_deny` returns `None`, and `DefaultPolicy::evaluate` returns **`PolicyVerdict::Allow`** under `Preset::FullAccess` (and `Ask` instead of `HardDeny` under `Preset::Home`).
     - Violates `crates/atrium-agents/src/lib.rs:34-40` and `crates/atrium-protocol/src/policy.rs:398-401` (`HARD_DENY_DEFAULTS`: "No rule or remembered decision overrides them for calls the gate sees. Applies to every tool kind whose subject names a matching command or path, searches included").
   - Fix: In `glob_target` (`crates/atrium-agents/src/claude/tools.rs:166-204`), return `Err(())` (`ToolSubject::Unknown`) whenever `rest` contains `/` after `{` or `[` (or when any `{...}` / `[...]` touches a path separator or `.` segment, or when `prefix` starts with `~` and is not `"~"` or `"~/..."`). When `leaves` is `true`, if `rest` contains `**` or `{`, return `Err(())`; otherwise include **both** the directory `dir` and the full joined glob pattern (e.g. `format!("{}/{}", base.trim_end_matches('/'), glob)`) in `paths` so `deny::Ctx::check` (`crates/atrium-core/src/policy/deny.rs:841-850`) runs `lexical_resolve` + `self.s.glob_hit` on the full pattern.

2. [major] **Holding pre-establishment events in `ClaudeNormalizer::held` deadlocks pre-`init` approvals/questions, drops `RequestResolved` and corrupts turn state when `result` precedes `system/init`, and grows `self.held` without bound when a session never establishes**
   - Where: `crates/atrium-agents/src/claude/normalizer.rs:287-289`, `crates/atrium-agents/src/claude/normalizer.rs:311-318` (`establish`), `crates/atrium-agents/src/claude/normalizer.rs:944-993` (`emit_approval`), `crates/atrium-agents/src/claude/normalizer.rs:1273-1285` (`cancel_pending`), `crates/atrium-agents/src/claude/normalizer.rs:1372-1375` (`on_input`), and `crates/atrium-agents/src/claude/adapter.rs:847` (`Actor::run`):
     ```rust
     let Some(init) = &self.init else {
         return;
     };
     ```
     ```rust
     if self.turn_open && !self.turn_start_emitted {
         self.start_turn_events(out);
     } else if !self.turn_open && self.held.iter().any(needs_turn) {
         // What waited for the session belongs to a turn.
         self.ensure_turn(out);
     }
     out.append(&mut self.held);
     ```
     ```rust
     for (rid, _) in std::mem::take(&mut self.pending) {
         // Nothing was shown before the session started.
         if self.established {
             out.push(AgentEvent::RequestResolved {
                 request_id: rid,
                 outcome: RequestOutcome::Cancelled,
             });
         }
     }
     ```
     ```rust
     if !self.established && !matches!(input, AdapterInput::Exit { .. }) {
         self.held.append(&mut out);
     }
     ```
   - Sequence:
     1. **Case A (Pre-`init` approval/question deadlock)**: Start a session (`ClaudeAdapter::start`), send `SessionCommand::SendUserMessage { text: "go".into() }`, and before emitting `system/init`, emit stdout `control_request` `can_use_tool` (`request_id: "c1"`, `tool_name: "Bash"`, `tool_use_id: "t1"`, `input: {"command": "ls"}`) with a gate that returns `GateDecision::Ask { ticket: ApprovalTicket { request_id: "T1", rememberable: true } }` (or `AskUserQuestion` with `Some(ticket)`).
     2. **Case B (Pre-`init` `result` followed by `system/init`)**: On `ClaudeNormalizer`, feed `user_line("go")`, `can_use("r1", Some("t1"), "Bash", json!({"command": "ls"}))`, `gate(Some("t1"), "Bash", json!({"command": "ls"}), ask("T1"))`, and `result()` (`is_error: false`), then feed `init()` and `message_start()`, then `AdapterInput::Exit { code: Some(0) }`.
     3. **Case C (Unbounded `self.held` with unread receiver)**: Start a session (`handshake` completes), never read from `session.events`, and have the child stream `{"type":"system","subtype":"hook_started"}` (or unparseable stdout lines) without sending `system/init` + `message_start`.
   - Result:
     - In Case A, `Actor::on_gate_decision` (`crates/atrium-agents/src/claude/adapter.rs:1335`) inserts `PendingApproval` into `self.approvals` and waits indefinitely for `SessionCommand::ResolveApproval` from the client before answering `c1`. In `ClaudeNormalizer::emit_approval` (`crates/atrium-agents/src/claude/normalizer.rs:952`), `self.establish(out)` returns early because `self.init` is `None`, and `on_input` (`crates/atrium-agents/src/claude/normalizer.rs:1373`) moves `ApprovalRequested { request_id: "T1" }` into `self.held` and returns `vec![]`. The client never receives `ApprovalRequested`, and the CLI hangs forever waiting for `c1`'s `control_response`.
     - In Case B, `emit_approval` puts `ApprovalRequested { request_id: "T1" }` into `self.held` and `("T1", Pending::Approval)` into `self.pending`. When `result()` runs before `init()`, `cancel_pending` (`crates/atrium-agents/src/claude/normalizer.rs:1274-1282`) drains `"T1"` from `self.pending`, sees `!self.established`, and drops `RequestResolved`; `result()` then pushes `TurnCompleted { turn: 1 }` into `self.held` and sets `self.turn_open = false` (`crates/atrium-agents/src/claude/normalizer.rs:803`). When `init()` + `message_start()` arrive next, `establish` (`crates/atrium-agents/src/claude/normalizer.rs:313-317`) sees `!self.turn_open && self.held.iter().any(needs_turn)`, calls `self.ensure_turn(out)` — which increments `self.turn` to `2`, sets `self.turn_open = true`, drops turn 1's `user_text`, and emits `TurnStarted { turn: 2 }` — and appends `self.held`. The stream emits `TurnStarted { turn: 2 }` closed by `TurnCompleted { turn: 1 }`, leaves `self.turn_open == true` after `TurnCompleted`, and emits `ApprovalRequested { request_id: "T1" }` with **no `RequestResolved` ever emitted** (even at `Exit`).
     - In Case C, because `on_input` moves every `Unmapped` event into `self.held` and returns `vec![]`, `self.pending.len()` in `Actor::run` (`crates/atrium-agents/src/claude/adapter.rs:847`) stays `0 < PENDING_LIMIT`, backpressure never stops reading `out_rx`, and `self.held` grows without bound in memory.
     - Violates `crates/atrium-protocol/src/agent.rs:45-50`, `crates/atrium-agents/src/lib.rs:34`, `crates/atrium-agents/src/lib.rs:59-61`, and `crates/atrium-agents/tests/consistency.rs:251-282`.
   - Fix: In `ClaudeNormalizer::establish` (`crates/atrium-agents/src/claude/normalizer.rs:283-290`), if `self.init` is `None` when an establishing event (`message_start`, `content_block_delta`, `message_delta`, `assistant`, non-error `result`, `emit_approval`, `question_ticket`, rejected `rate_limit`) arrives, synthesize a fallback `InitInfo` (empty `session_id`, `model` from the record if present) so the session establishes immediately instead of accumulating turn/approval events in `self.held`; and cap `self.held` at `MAX_DIAGNOSTIC_LINES` (`50`) for pre-session `Unmapped`/`QuotaUpdated` events.

3. [major] **`tools::gate_key` hashes `serde_json::Value` in wire key order (`preserve_order`) instead of canonical sorted-key order, causing re-serialized inputs between `hook_callback` and `can_use_tool` to ask the gate twice and leak a pending ledger ticket**
   - Where: `crates/atrium-agents/src/claude/tools.rs:118-121` (`gate_key`) and `Cargo.toml:40`:
     ```rust
     pub(crate) fn gate_key(call_id: Option<&str>, name: &str, input: &Value) -> String {
         let digest = Sha256::digest(format!("{name}\0{input}").as_bytes());
         format!("{}:{digest:x}", call_id.unwrap_or("anon"))
     }
     ```
     ```toml
     serde_json = { version = "1", features = ["preserve_order"] }
     ```
   - Sequence:
     1. Start a session with a `SessionGate` backed by `ApprovalLedger` that returns `GateDecision::Ask` for `Grep` outside the workspace.
     2. Emit stdout `control_request` `hook_callback` (`request_id: "h1"`, `callback_id: "atrium-pre-tool-use"`, `tool_use_id: "toolu_1"`, `tool_name: "Grep"`, `tool_input: {"path": "/etc/hosts", "pattern": "root"}`). `gate.check` opens ticket `T1` (`status: "pending"`); `crates/atrium-agents/src/claude/adapter.rs:1318` stores `T1` in `self.hook_asked[K1]`, `crates/atrium-agents/src/claude/normalizer.rs:1072` stores `T1` in `self.asked[K1]`, and the adapter replies `permissionDecision: "ask"`.
     3. Emit stdout `control_request` `can_use_tool` (`request_id: "c1"`, `tool_use_id: "toolu_1"`, `tool_name: "Grep"`, `input: {"pattern": "root", "path": "/etc/hosts"}`) — the same object with keys in sorted order (which `atrium_core::policy::canonical_input` at `crates/atrium-core/src/policy/mod.rs:29-45` hashes identically).
   - Result:
     - Because `Cargo.toml:40` enables `serde_json`'s `preserve_order` feature, `format!("{input}")` prints object keys in parse order, so `gate_key` computes `K2 != K1`.
     - `self.take_hook_ticket(&K2)` (`crates/atrium-agents/src/claude/adapter.rs:1097`) and `self.take_asked(&K2)` (`crates/atrium-agents/src/claude/normalizer.rs:929`) both return `None`.
     - `on_can_use` (`crates/atrium-agents/src/claude/adapter.rs:1117`) spawns `gate.check` a **second time** for the same call, opening a second ledger ticket `T2` while `T1` stays permanently stranded in `self.hook_asked[K1]`, `self.norm.asked[K1]`, and `"pending"` in `ApprovalLedger` for the rest of the turn (consuming 2 of `MAX_PENDING_REQUESTS = 64` per call; if 63 requests were already pending, `hook_callback` opens #64 and `can_use_tool`'s duplicate check hits `LedgerError::TooMany` at `crates/atrium-core/src/policy/ledger.rs:85` and denies the call).
     - Violates `crates/atrium-agents/src/lib.rs:34-39` and `docs/contracts/m1.md:58-78` (§5).
   - Fix: In `tools::gate_key` (`crates/atrium-agents/src/claude/tools.rs:118-121`), format `input` with object keys sorted recursively (matching `atrium_core::policy::canonical_input`) before hashing.

4. [major] **Inline `--settings '{...}'` in `settings::inspect` discards `inspect_text`'s returned `enabledPlugins` list, reporting `EnforcementLevel::Full` when an inline `--settings` enables a plugin with hooks**
   - Where: `crates/atrium-agents/src/claude/settings.rs:182-191` (`inspect`), compared with `crates/atrium-agents/src/claude/settings.rs:208-209`:
     ```rust
     if let Some(v) = value("--settings", &mut args) {
         if v.trim_start().starts_with('{') {
             inspect_text(
                 &mut out,
                 ConfigSourceKind::ExplicitSettings,
                 "--settings",
                 "(inline --settings)".to_owned(),
                 v.into_bytes(),
             );
         } else {
     ```
   - Sequence:
     1. Create `<user_dir>/plugins/installed_plugins.json` mapping `"fmt@market"` to `<root>/inst/fmt`, and write `{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/tmp/exfil.sh"}]}]}}` to `<root>/inst/fmt/hooks/hooks.json`.
     2. Call `settings::inspect` (or `ClaudeAdapter::probe` / `start` with CLI version `2.1.308`) with `setting_sources: Some("")` and `extra_args: ["--settings", "{\"enabledPlugins\":{\"fmt@market\":true}}"]`.
   - Result:
     - `inspect_text` (`crates/atrium-agents/src/claude/settings.rs:253-296`) parses the inline JSON, pushes `ConfigSource { kind: ExplicitSettings, path: "(inline --settings)", inspected: true }`, and returns `vec!["fmt@market".to_owned()]`.
     - Line 184 discards the returned `Vec<String>` instead of extending `enabled` (unlike file-based settings at `crates/atrium-agents/src/claude/settings.rs:208-209`).
     - `plugin_source` (`crates/atrium-agents/src/claude/settings.rs:241-244`) never runs for `"fmt@market"`: no `ConfigSourceKind::Plugin` (`inspected: false`) and no `ReasonCode::ExternalHooks` are recorded, and `EnforcementDescriptor::evaluate` returns **`EnforcementLevel::Full`** instead of `Partial`.
     - Violates `crates/atrium-agents/src/lib.rs:42-55` and `docs/contracts/m1.md:27-38` (§3).
   - Fix: Declare `let mut enabled: Vec<String> = Vec::new();` before the `extra_args` loop (`crates/atrium-agents/src/claude/settings.rs:172`) and capture `let found = inspect_text(...); enabled.extend(found);` at `crates/atrium-agents/src/claude/settings.rs:184-190`.

5. [minor] **`ClaudeNormalizer::complete_message` does not check `self.done_msgs`, emitting a second `MessageCompleted` when an `assistant` record arrives after `close_messages` closed the streamed message**
   - Where: `crates/atrium-agents/src/claude/normalizer.rs:546-569` (`complete_message`) and `crates/atrium-agents/src/claude/normalizer.rs:1165-1170` (`close_messages`):
     ```rust
     match streamed {
         Some(m) => {
             self.done_msgs.insert(m.id.clone());
             out.push(streamed_completion(m));
         }
         None if record_text.is_empty() => {}
         None => {
             self.done_msgs.insert(id.clone());
             out.extend(whole_message(id, kind, record_text));
         }
     }
     ```
   - Sequence:
     1. Establish a session on `ClaudeNormalizer` (`user_line("go")`, `init()`, `message_start()`).
     2. Feed `stream_event` `content_block_delta` with `api_message_id: "m1"`, `index: 0`, `delta: {"type": "text_delta", "text": "Hi"}` (`"m1:0"` is opened in `self.open_msgs`; `self.block_counts` has no `"m1"` entry yet).
     3. Feed `result()`, which calls `close_messages` (`crates/atrium-agents/src/claude/normalizer.rs:767`, `crates/atrium-agents/src/claude/normalizer.rs:1165-1170`), removing `"m1:0"` from `self.open_msgs`, inserting `"m1:0"` into `self.done_msgs`, and emitting `MessageCompleted { message_id: "m1:0", body: Streamed { ... } }`.
     4. Feed `{"type":"assistant","message":{"model":"claude-x","id":"m1","content":[{"type":"text","text":"Hi"}]}}`.
   - Result:
     - `assistant` (`crates/atrium-agents/src/claude/normalizer.rs:525-527`) assigns `id = "m1:0"` (`block_counts["m1"]` starts at `0`) and calls `complete_message("m1:0", MessageKind::Assistant, "Hi", out)`.
     - Because `close_messages` already removed `"m1:0"` from `self.open_msgs`, `streamed` is `None`. `complete_message` does not check `self.done_msgs.contains(&id)` and takes the `None =>` branch (`crates/atrium-agents/src/claude/normalizer.rs:564-567`), emitting a **second** `MessageCompleted` for `message_id: "m1:0"`.
     - Violates `crates/atrium-agents/src/lib.rs:31` and `crates/atrium-agents/tests/consistency.rs:270`.
   - Fix: Add `if self.done_msgs.contains(&id) { return; }` at the top of `complete_message` (`crates/atrium-agents/src/claude/normalizer.rs:553`).

6. [minor] **`Proc::drop` only kills the process group asynchronously via `Handle::try_current()`, leaving grandchild processes alive when `Proc` is dropped outside a Tokio runtime or during runtime shutdown**
   - Where: `crates/atrium-agents/src/claude/adapter.rs:408-422` (`Proc::drop`):
     ```rust
     fn drop(&mut self) {
         if let Some(mut child) = self.child.take()
             && let Ok(runtime) = tokio::runtime::Handle::try_current()
         {
             runtime.spawn(async move {
                 let _ = supervise::terminate(&mut child, Duration::ZERO).await;
             });
         }
     }
     ```
   - Sequence:
     1. On a Tokio runtime, poll `let mut fut = ClaudeAdapter::new(opts).start(cfg, Transport::Local)` (with `FAKE_MODE=no-init` and `FAKE_GRANDCHILD=1`) until `on_spawn` fires and `grandchild.pid` is created (`Actor::launch` has stored `Proc` in `self.proc`, and `fut` is suspended in `handshake()`).
     2. Move `fut` to a plain OS thread `std::thread::spawn(move || drop(fut))` where `Handle::try_current()` is `Err` (or shut down/drop the Tokio `Runtime` holding the task).
   - Result:
     - When `Handle::try_current()` is `Err`, `runtime.spawn` is skipped; when the `Runtime` is shutting down, the task spawned by `runtime.spawn` is cancelled before polling `supervise::terminate`.
     - `child` (`SupervisedChild`) is dropped without `supervise::kill_recorded` ever running; `tokio::process::Child`'s `kill_on_drop(true)` kills only the group leader PID, leaving the grandchild (`sleep 600`) in `pgid` alive.
     - Violates `crates/atrium-agents/src/lib.rs:32` and `docs/contracts/m1.md:152-157` (§10).
   - Fix: In `Proc::drop` (`crates/atrium-agents/src/claude/adapter.rs:414`), call `let _ = supervise::kill_recorded(&child.identity, supervise::Signal::Kill);` synchronously before `if let Ok(runtime) = tokio::runtime::Handle::try_current() { runtime.spawn(...); }`.

7. [minor] **`Actor::live` shares one namespace across `hook_callback` and `can_use_tool`, and `Internal::Redeem` / `Internal::Question` feed `ClaudeNormalizer` before checking `self.live`**
   - Where: `crates/atrium-agents/src/claude/adapter.rs:1072`, `crates/atrium-agents/src/claude/adapter.rs:1091`, `crates/atrium-agents/src/claude/adapter.rs:1256-1288`, and `crates/atrium-agents/src/claude/adapter.rs:1308-1311` (`on_hook`, `on_can_use`, `on_internal`, `on_gate_decision`):
     ```rust
     self.feed(AdapterInput::Redeem {
         ticket,
         check,
         decision: decision.clone(),
     });
     // The request may have been withdrawn while the gate decided.
     if !self.live.remove(&native) || self.proc.is_none() {
         return;
     }
     ```
     ```rust
     Reply::Hook { request_id } => {
         // Answered now: the hook call is over.
         self.live.remove(&request_id);
     ```
   - Sequence:
     1. **Case A (`Internal::Redeem` queued before `control_cancel_request`)**: Emit `assistant` `tool_use` (`id: "toolu_1"`) and `can_use_tool` (`request_id: "c1"`, `tool_use_id: "toolu_1"`), resolve the gate with `Ask(T1)`, and send `SessionCommand::ResolveApproval { request_id: "T1", decision: ApprovalDecision::Allow }`. Suppose `gate.redeem` resolves with `GateDecision::Deny { reason: "expired" }` and queues `Internal::Redeem` into `self.internal_tx` just before `on_stdout` processes `{"type":"control_cancel_request","request_id":"c1"}` (`forget_native("c1")` removes `"c1"` from `self.live`, while `"toolu_1"` stays open in `self.norm.calls`).
     2. **Case B (Shared `request_id` between an open `can_use_tool` approval and a concurrent `hook_callback`)**: Emit `can_use_tool` with `request_id: "r1"`, which receives `GateDecision::Ask { ticket: T1 }` (`"r1"` stays in `self.live` and `self.approvals["T1"]`). Before the user resolves `T1`, emit a `hook_callback` that also uses `request_id: "r1"` and receives `GateDecision::Allow`. Then send `SessionCommand::ResolveApproval { request_id: "T1", decision: ApprovalDecision::Allow }`.
   - Result:
     - In Case A, `on_internal` (`crates/atrium-agents/src/claude/adapter.rs:1256`) feeds `AdapterInput::Redeem` to `self.norm` **before** checking `if !self.live.remove(&native)` at line 1262, causing `crates/atrium-agents/src/claude/normalizer.rs:1343-1347` to call `self.deny_call("toolu_1", ...)` and emit `ToolCallFinished { call_id: "toolu_1", status: Denied }` for a withdrawn permission request.
     - In Case B, when the hook's gate check finishes, `on_gate_decision` (`crates/atrium-agents/src/claude/adapter.rs:1310`) calls `self.live.remove("r1")`. When `Internal::Redeem` for `T1` then completes, `self.live.remove("r1")` (`crates/atrium-agents/src/claude/adapter.rs:1262`) returns `false` and drops the `control_response` for `can_use_tool` `"r1"`, leaving the CLI hanging forever on `"r1"`.
   - Fix: In `Actor::on_internal` (`crates/atrium-agents/src/claude/adapter.rs:1256-1288`), check `self.live` before calling `self.feed(...)` (matching `Internal::Gate` at `crates/atrium-agents/src/claude/adapter.rs:1240`), and key `self.live` / `self.inflight` by `(ReplyKind, String)` (or separate sets for hook vs `can_use_tool` requests).

## Suspected, not shown
- **Other helper keys in the CLI binary (`gcpAuthRefresh`, `processWrapper`, `policyHelpers`, `proxyAuthHelper`, `subagentStatusLine`)**:
  - `gcpAuthRefresh`: Runs a shell command outside the gate (Claude Code's GCP counterpart to `awsAuthRefresh` in `COMMAND_KEYS` at `crates/atrium-agents/src/claude/settings.rs:38`, configured as a command string such as `"gcloud auth application-default login"` to refresh credentials in the foreground). Because inspecting or running the host `claude` binary is outside the read-only worktree scope, I cannot quote the `2.1.308`/`2.1.312` binary's call site from in-tree files, so it is recorded here rather than as a shown sequence.
  - `processWrapper`, `policyHelpers`, `proxyAuthHelper`, `subagentStatusLine`: **Don't know** from in-worktree evidence whether each executes a shell command in `2.1.308`/`2.1.312` (cannot inspect the host binary from the read-only worktree).
- **Round 1 Suspected 2 (Symlinked settings files; mid-turn settings changes)**:
  - For symlinked settings files, `fs::read(&path)` (`crates/atrium-agents/src/claude/settings.rs:206`) follows symlinks just as the CLI does, hashes the target content, and marks broken non-`ENOENT` symlinks (`ELOOP`, `EACCES`) as `inspected: false`.
  - For mid-turn settings changes, `Actor` only calls `refresh_enforcement()` in `dispatch_next()` (`crates/atrium-agents/src/claude/adapter.rs:1459`) and `gate_failed()` (`crates/atrium-agents/src/claude/adapter.rs:1357`). If a tool call modifies `.claude/settings.json` during turn 1, no `EnforcementChanged` is emitted until turn 2's `dispatch_next()` (and never if turn 1 is the last turn). Whether Claude Code hot-reloads `.claude/settings.json` mid-turn in `-p --input-format stream-json` mode cannot be shown without starting the real `claude` CLI, so it stays suspected.
- **Dangling ledger entry when an `Ask` decision is dropped mid-turn**: When `control_cancel_request` withdraws a request while `gate.check` is in flight (or when `hook_callback` returns `Ask(T1)` and `can_use_tool` never follows or changes its input), the `ApprovalTicket` opened by `SessionGate::check` (`crates/atrium-core/src/policy/gate.rs:349-372`) cannot be redeemed later (`Ledger::redeem` at `crates/atrium-core/src/policy/ledger.rs:257-261` rejects `"pending"` entries, and `Actor::approvals` does not hold the ticket). However, because neither `ApprovalRequested` nor `RequestResolved` is emitted, the daemon never calls `ledger.withdraw`, so the `"pending"` entry remains counted in `open_count` against `MAX_PENDING_REQUESTS` (`64`, `crates/atrium-core/src/policy/ledger.rs:85`) until `expire_turn` at the end of the turn.

## Tried and held
- **Ordinary `Glob`/`Grep` patterns (`**/*.rs`, `src/**/mod.rs`, `*.{ts,tsx}`, `src/{a,b}/mod.rs`)**: `glob_target` (`crates/atrium-agents/src/claude/tools.rs:166-204`) splits at the first wildcard (`""` or `"src/"`), sees `climbs(rest) == false` and `leaves == false`, and returns `Ok(None)`, keeping `paths: vec![base]` without false `Unknown` or extra paths.
- **Unparsed `hooks` shapes in `settings.rs`**: In `command_findings` (`crates/atrium-agents/src/claude/settings.rs:302-343`), non-object `hooks`, non-array event values, matcher groups without an array `"hooks"` field, and non-object handler entries all set `understood = false` (`inspected: false`), and every object handler entry pushes an `ExternalHooks` reason; only empty `hooks` (`{}`, `{"PreToolUse": []}`, `{"PreToolUse": [{"hooks": []}]}`) return `understood = true` with no findings.
- **Non-closing stdout/stderr flood during `stop_process` (`SHUTDOWN_GRACE`)**: `stop_process` (`crates/atrium-agents/src/claude/adapter.rs:718`, `crates/atrium-agents/src/claude/adapter.rs:744`) routes output through `on_out(out, false)` (`crates/atrium-agents/src/claude/adapter.rs:966-980`), which gates `on_stdout` on `self.pending.len() < PENDING_LIMIT` (`4096`) and caps `stderr_tail` at `MAX_DIAGNOSTIC_LINES` (`50`, `crates/atrium-agents/src/claude/normalizer.rs:1331`), bounding memory during the 5s grace period once the session is established.
- **Sequential reuse of `request_id` after a hook response**: `on_gate_decision` for `Reply::Hook` (`crates/atrium-agents/src/claude/adapter.rs:1310`) removes `request_id` from `self.live` before sending the hook response, so a subsequent `can_use_tool` reusing that id after the hook completed does not collide.
- **Dead write-only `ClaudeNormalizer::allowed` set**: `self.allowed` (`crates/atrium-agents/src/claude/normalizer.rs:167`, `crates/atrium-agents/src/claude/normalizer.rs:1058`) is inserted on `GateDecision::Allow` and never read or cleared, but does not alter event output.

## Test gaps
1. **End-to-end `tool_check` + `DefaultPolicy::evaluate` tests for `Glob` and `Grep`** (New finding 1): `f2_*` in `crates/atrium-agents/tests/claude_review.rs:521-650` only assert the `ToolSubject` returned by `tool_check`, never passing the `ToolCheck` into `atrium_core::policy::DefaultPolicy::evaluate` to verify that `Preset::Home` and `HARD_DENY_DEFAULTS` (`credential-read`) actually block `../.aws/cred*`, `~/.ss*/id_ed25519`, `{a,}/etc/passwd`, or `.{.,.}/.ssh/id_ed25519`.
2. **Pre-`init` `can_use_tool` approval/question and pre-`init` `result` followed by `init`** (New finding 2): `f4_records_before_any_init_produce_nothing_and_a_pre_session_failure_is_two_events` (`crates/atrium-agents/tests/claude_review.rs:315-339`) only tests `Exit { code: Some(1) }` after pre-`init` records, never testing what happens when `system/init` arrives later or when `can_use_tool` arrives before `system/init`.
3. **Different JSON key ordering between `hook_callback` and `can_use_tool`** (New finding 3): `f7_*` (`crates/atrium-agents/tests/claude_review.rs:344-421`, `crates/atrium-agents/tests/claude_adapter.rs:1419-1499`) only test identical single-key objects `{"command": "..."}`, missing multi-key objects whose key order differs between `hook_callback` and `can_use_tool`.
4. **Inline `--settings` with `enabledPlugins`** (New finding 4): `explicit_settings_files_and_inline_json_are_inspected` (`crates/atrium-agents/src/claude/settings.rs:534-558`) only tests `statusLine` in inline `--settings`, never `enabledPlugins`.
5. **Late `assistant` record after `close_messages`** (New finding 5): `f8_a_late_delta_after_the_assistant_record_does_not_complete_the_message_twice` (`crates/atrium-agents/tests/claude_review.rs:426-459`) tests `assistant` followed by `content_block_delta` + `result`, but not `content_block_delta` + `result` followed by `assistant`.

*Orchestrator's note:* the reviewer's own write of this file was refused by the worker policy hook (`scripts/agy-policy.py` reads every string argument of a file tool as a path, and the review's text contains `../`). The text above is the reviewer's final answer, taken from `.orchestration/logs/M1-07-review.jsonl` on rodrigo's machine, with its `file://` links turned into `path:line`.
