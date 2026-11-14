# Review: M1-07-claude-adapter, round 3 (fix round 2)
Reviewer: Gemini 4 Argon (read-only) · 2026-11-14

## Verdict
**rework**: Round 2's fixes for canonical `gate_key` JSON, inline `--settings` `enabledPlugins`, duplicate `MessageCompleted` after `close_messages`, `Req::Hook`/`Req::CanUse` separation, and the five additional helper command keys hold, as does pre-`init` session establishment save for oversize `can_use_tool` inputs, but `Glob`/`Grep` subject extraction in `tools.rs` still lets brace-interleaved character classes (`[{a],.}][{a],.}]/.ssh/id_ed25519`), `~user`/`~*` prefixes (`~bob/*`, `~a*/.ssh/id_ed25519`), and non-leaving relative credential globs (`path: ".."`, `pattern: ".ssh/id_ed25519"`) bypass `Preset::Home` containment or `HARD_DENY_DEFAULTS` under `Preset::FullAccess`, breaks ordinary character ranges (`*.[a-z]`), and `Proc::drop` outside a runtime skips process-group cleanup when the group leader has already exited into an unreaped zombie.

## Round 2 findings
- **Round 1 finding 2 & Round 2 finding 1 (`Glob`/`Grep` subjects in `tools.rs`)**: **not closed** — the author's new tests in `crates/atrium-agents/tests/claude_policy_e2e.rs:225-272` and `crates/atrium-agents/tests/claude_review.rs:665-675` fail on `17a70d2` and pass at `HEAD` for Round 2's exact vectors, but running `classes_are_plain` before `expand_braces`, allowing `~user`/`~*` prefixes, omitting `join(glob)` when `leaves == false`, and overwriting `prev` on `-` in `classes_are_plain` leave three policy bypasses and break character ranges (`*.[a-z]`) (New findings 1–4).
- **Round 2 finding 2 (Held events before `SessionStarted` in `normalizer.rs`)**: **closed with a gap** — `r2_records_before_any_init_establish_the_session_from_the_record`, `r2_an_approval_or_question_before_init_is_shown_at_once`, `r2_a_result_before_init_resolves_the_request_and_keeps_turns_in_order`, and `r2_what_waits_for_the_session_is_bounded` (`crates/atrium-agents/tests/claude_review.rs:733-888`) fail on `17a70d2` and pass at `HEAD`; only `can_use_tool`'s oversize branch (`crates/atrium-agents/src/claude/normalizer.rs:927-935`) omits `establish` and `ensure_turn` before calling `oversize_call` (New finding 6).
- **Round 2 finding 3 (`gate_key` canonical JSON in `tools.rs`)**: **closed** — `r2_a_ticket_survives_the_input_being_reserialized_with_other_key_order` and `r2_our_canonical_text_is_the_policys` (`crates/atrium-agents/tests/claude_review.rs:680-728`) fail on `17a70d2` and pass at `HEAD`, and `canonical_json` (`crates/atrium-agents/src/claude/tools.rs:131-159`) produces the exact same string as `atrium_core::policy::canonical_input` (`crates/atrium-core/src/policy/mod.rs:29-59`) for every `serde_json::Value`.
- **Round 2 finding 4 (Inline `--settings` `enabledPlugins` in `settings.rs`)**: **closed** — `plugins_enabled_by_an_inline_settings_argument_are_listed` (`crates/atrium-agents/src/claude/settings.rs:637-681`) fails on `17a70d2` and passes at `HEAD`; `inspect` (`crates/atrium-agents/src/claude/settings.rs:201-207`) extends `enabled` with the plugins returned by `inspect_text` on inline `--settings` JSON.
- **Round 2 finding 5 (Late `assistant` record after `close_messages` in `normalizer.rs`)**: **closed** — `r2_a_late_assistant_record_does_not_complete_a_closed_message_again` (`crates/atrium-agents/tests/claude_review.rs:893-915`) fails on `17a70d2` and passes at `HEAD`; `complete_message` (`crates/atrium-agents/src/claude/normalizer.rs:575-577`) returns early when `self.done_msgs.contains(&id)`.
- **Round 2 finding 6 (`Proc::drop` outside a runtime in `adapter.rs`)**: **closed with a gap** — `a_start_dropped_outside_a_runtime_kills_the_process_group` (`crates/atrium-agents/tests/claude_adapter.rs:1530-1551`) fails on `17a70d2` and passes at `HEAD` when the group leader is still running at drop time, but `supervise::kill_recorded` (`crates/atrium-host/src/supervise.rs:171-208`) returns `Cleanup::Unverified` without signalling the group when the leader has already exited into an unreaped zombie while descendants remain alive (New finding 5).
- **Round 2 finding 7 (`Actor::live` namespace and checking `live` before `feed` in `adapter.rs`)**: **closed** — `r2_a_hook_call_and_a_can_use_request_with_one_id_are_separate` (`crates/atrium-agents/tests/claude_adapter.rs:1769-1805`) and the three unit tests in `crates/atrium-agents/src/claude/adapter.rs:1855-1931` fail on `17a70d2` and pass at `HEAD`; `Req::Hook` and `Req::CanUse` separate the namespaces in `live` and `inflight`, and `on_internal` checks `self.live` before `self.feed` across `Gate`, `Redeem`, and `Question`.
- **Item 8 (Helper keys `gcpAuthRefresh`, `policyHelpers`, `processWrapper`, `proxyAuthHelper`, `subagentStatusLine` in `settings.rs`)**: **closed** — `the_other_helper_keys_are_command_keys_too` (`crates/atrium-agents/src/claude/settings.rs:701-734`) fails on `17a70d2` and passes at `HEAD`, and all 11 entries of `COMMAND_KEYS` (`crates/atrium-agents/src/claude/settings.rs:46-58`) emit `ExternalHooks` across managed, user, project, local, `--settings` file, and inline `--settings` sources.

## New findings
1. [blocker] **`glob_targets` runs `classes_are_plain` before `expand_braces`, allowing interleaved `[` and `{...}` (`"[{a],.}][{a],.}]/.ssh/id_ed25519"`, `"[{a],/}]/etc/passwd"`, `"[{a],~}]/.ssh/id_ed25519"`) to synthesize `[.]`, `[/]`, and `[~]` after brace expansion and return `GateDecision::Allow` under both `Preset::Home` and `Preset::FullAccess`**
   - Where: `crates/atrium-agents/src/claude/tools.rs:216-226` (`glob_targets`), `crates/atrium-agents/src/claude/tools.rs:230-270` (`classes_are_plain`), and `crates/atrium-agents/src/claude/tools.rs:329-359` (`glob_target`):
     ```rust
     fn glob_targets(base: &str, glob: &str) -> Option<Vec<String>> {
         // On Unix a backslash escapes: too subtle to bound.
         if glob.contains('\\') || !classes_are_plain(glob) {
             return None;
         }
         let mut out = Vec::new();
         for e in expand_braces(glob)? {
             out.extend(glob_target(base, &e)?);
         }
         Some(out)
     }
     ```
     ```rust
     fn classes_are_plain(glob: &str) -> bool {
         let mut chars = glob.chars().peekable();
         while let Some(c) = chars.next() {
             if c != '[' {
                 continue;
             }
             let mut closed = false;
             let mut first = true;
             let mut prev: Option<char> = None;
             let mut in_range = false;
             for c in chars.by_ref() {
                 if c == ']' && !first {
                     closed = true;
                     break;
                 }
                 if first && matches!(c, '!' | '^') {
                     return false;
                 }
                 if matches!(c, '.' | '/' | '~' | '[' | ':') {
                     return false;
                 }
     ```
   - Sequence:
     1. In `crates/atrium-agents/tests/claude_policy_e2e.rs`'s `Layout::new()` (`home = "<tmp>/home/ana"`, `cwd = "<tmp>/home/ana/work"`), construct a `SessionGate` with `Preset::Home` and a second `SessionGate` with `Preset::FullAccess`.
     2. Call `tool_check(Some("t1"), "Glob", &json!({"path": cwd, "pattern": "[{a],.}][{a],.}]/.ssh/id_ed25519"}), 1)` (or `"pattern": "[{a],/}]/etc/passwd"`, or `"pattern": "[{a],~}]/.ssh/id_ed25519"`, or the same string in `Grep`'s `"glob"` field) and pass it to `gate.check(c, &cwd)`.
   - Result:
     - In `classes_are_plain("[{a],.}][{a],.}]/.ssh/id_ed25519")`, the first `[` opens a class; inside the inner loop, `'{'` and `'a'` pass line 248 (`!matches!(c, '.' | '/' | '~' | '[' | ':')`), and the `']'` inside the first brace branch (`"a]"`) closes the class at line 241 (`closed = true; break;`). The outer loop (`if c != '[' { continue; }`, line 233) then skips `",.}]"` because it is outside `[...]`. The second `"[{a],.}]"` behaves identically, so `classes_are_plain` returns `true`.
     - Next, `expand_braces` (`crates/atrium-agents/src/claude/tools.rs:274-324`) expands the two brace groups into 4 strings: `"[a][a]/.ssh/id_ed25519"`, `"[a][.]/.ssh/id_ed25519"`, `"[.][a]/.ssh/id_ed25519"`, and `"[.][.]/.ssh/id_ed25519"` (or `"[/]/etc/passwd"`, `"[~]/.ssh/id_ed25519"`).
     - `glob_targets` passes each expanded string `e` to `glob_target` without re-checking `classes_are_plain(&e)`. In `glob_target` (`crates/atrium-agents/src/claude/tools.rs:331-340`), `first` is `Some(0)` (`'['`), `prefix` is `""`, `climbs(rest)` is `false` (`"[.][.]"` is not `".."`), and `leaves` is `false`, returning `Some(vec![])`.
     - `tool_subject` returns `ToolSubject::Paths { paths: vec![cwd], glob: Some("[{a],.}][{a],.}]/.ssh/id_ed25519"), recursive: true }`, and `SessionGate::check` returns **`GateDecision::Allow`** under both `Preset::Home` and `Preset::FullAccess`.
     - Under `docs/contracts/m1.md` §3 (`atrium-agents` / `claude::tools`: "Anything that cannot be conservatively bounded -> `ToolSubject::Unknown`") and §10 (`Preset::Home` containment and `HARD_DENY_DEFAULTS`), these patterns must produce `ToolSubject::Unknown` (so `Preset::Home` returns `GateDecision::Ask` rather than `Allow`, and any pattern that can expand to `[.]`, `[/]`, `[~]`, or `[!^]` is rejected).
   - Fix: In `glob_targets` (`crates/atrium-agents/src/claude/tools.rs:216-226`), run `classes_are_plain(&e)` on every expanded string `e` returned by `expand_braces(glob)?` (and in `classes_are_plain` line 248, also reject `'{'` and `'}'` inside `[...]`).

2. [blocker] **`glob_target` and `search_subject` accept `~user` and `~*` prefixes (e.g. `"~bob/*"`, `"~a*/.ssh/id_ed25519"`), which `atrium_core::policy::words::expand_home` leaves unexpanded so policy resolves them under `cwd` and returns `GateDecision::Allow` under `Preset::Home` and `Preset::FullAccess`**
   - Where: `crates/atrium-agents/src/claude/tools.rs:177-181` (`search_subject`) and `crates/atrium-agents/src/claude/tools.rs:338-358` (`glob_target`), interacting with `crates/atrium-core/src/policy/words.rs:260-276` (`expand_home`):
     ```rust
     let leaves = prefix.starts_with('/') || prefix.starts_with('~') || climbs(prefix);
     if !leaves {
         return Some(vec![]);
     }
     let join = |p: &str| {
         if p.starts_with('/') || p.starts_with('~') {
             p.to_owned()
         } else {
             format!("{}/{}", base.trim_end_matches('/'), p)
         }
     };
     if first.is_none() {
         // No wildcard: the whole thing is the target.
         return Some(vec![join(glob)]);
     }
     let dir = match prefix.rfind('/') {
         Some(i) => &prefix[..=i],
         None if prefix.starts_with('~') => "~",
         None => "",
     };
     Some(vec![join(dir), join(glob)])
     ```
     and `crates/atrium-core/src/policy/words.rs:260-276`:
     ```rust
     pub(crate) fn expand_home(raw: &str, home: Option<&Path>) -> String {
         let Some(home) = home else {
             return raw.to_string();
         };
         if raw == "~" {
             return home.to_string_lossy().into_owned();
         }
         if let Some(rest) = raw.strip_prefix("~/") {
             return format!("{}/{rest}", home.to_string_lossy());
         }
         if let Some(rest) = raw.strip_prefix('~') {
             let (user, tail) = rest.split_once('/').unwrap_or((rest, ""));
             if !user.is_empty() && home.file_name().and_then(|n| n.to_str()) == Some(user) {
                 if tail.is_empty() {
                     return home.to_string_lossy().into_owned();
                 }
                 return format!("{}/{tail}", home.to_string_lossy());
             }
         }
         raw.to_string()
     }
     ```
   - Sequence:
     1. In `Layout::new()` (`home = "<tmp>/home/ana"`, `cwd = "<tmp>/home/ana/work"`), evaluate `Glob` with `{"path": cwd, "pattern": "~bob/*"}` (or `{"path": "~bob", "pattern": "*.rs"}`) through `SessionGate::check` under `Preset::Home`.
     2. In the same layout, evaluate `Glob` with `{"path": cwd, "pattern": "~a*/.ssh/id_ed25519"}` (or `"pattern": "~?na/.ssh/id_ed25519"`) through `SessionGate::check` under `Preset::FullAccess` and `Preset::Home`.
   - Result:
     - Case 1 (`"~bob/*"` under `Preset::Home`): `glob_target` sees `prefix = "~bob/"`, sets `leaves = true`, `dir = "~bob/"`, and produces `paths: [cwd, "~bob/", "~bob/*"]`. In `words::expand_home`, `user` is `"bob"` which does not match `home.file_name()` (`"ana"`), so `expand_home` returns `"~bob/"` and `"~bob/*"` unchanged. Both `fs_resolve` and `lexical_resolve` (`crates/atrium-core/src/policy/engine.rs:514-548`) see a relative path (`!p.is_absolute()`) and join it onto `cwd` (`"<tmp>/home/ana/work/~bob"`), which is inside `home`, so `Preset::Home` returns **`GateDecision::Allow`** for another user's home directory (`~bob`).
     - Case 2 (`"~a*/.ssh/id_ed25519"` under `Preset::FullAccess` and `Preset::Home`): `first` is `Some(2)` (`'*'`), `prefix` is `"~a"`, `prefix.rfind('/')` is `None`, so line 355 sets `dir = "~"` while `join(glob)` is `"~a*/.ssh/id_ed25519"`. `expand_home` expands `dir` (`"~"`) to `"<tmp>/home/ana"` (which is a directory, not a credential file, and satisfies `Home::root_ok`), while `"~a*/.ssh/id_ed25519"` has `user = "a*" != "ana"`, so `expand_home` leaves it unexpanded and `lexical_resolve` resolves it to `"<tmp>/home/ana/work/~a*/.ssh/id_ed25519"`, missing `Sensitive::glob_hit` on `"<tmp>/home/ana/.ssh/id_ed25519"`. `SessionGate::check` returns **`GateDecision::Allow`** under `Preset::FullAccess` (and `GateDecision::Ask` instead of `GateDecision::Deny` `"Always blocked (credential-read)"` under `Preset::Home`).
     - Under `docs/contracts/m1.md` §3 and §10, any `~` prefix other than `"~"` or `"~/..."` cannot be bounded to `home` and must return `ToolSubject::Unknown`.
   - Fix: Define `fn tilde_ok(s: &str) -> bool { !s.starts_with('~') || s == "~" || s.starts_with("~/") }` in `crates/atrium-agents/src/claude/tools.rs`, return `ToolSubject::Unknown` in `search_subject` if `!tilde_ok(&base)`, and return `None` in `glob_target` if `!tilde_ok(glob)`.

3. [major] **Multi-component relative globs that stay inside `base` (e.g. `{"path": "..", "pattern": ".ssh/id_ed25519"}`, `{"path": "/home/ana", "pattern": ".aws/cred*"}`, `{"path": "/home/ana/.config", "pattern": "gh/hosts.yml"}`, or `{"pattern": ".ssh/id_ed25519"}` when `cwd == home`) have `leaves == false`, omitting `join(glob)` from `paths` so `HARD_DENY_DEFAULTS` (`credential-read`) is bypassed**
   - Where: `crates/atrium-agents/src/claude/tools.rs:338-341` (`glob_target`), interacting with `crates/atrium-core/src/policy/deny.rs:840-854` and `crates/atrium-core/src/policy/deny.rs:1061-1088` (`Ctx::check`, `Ctx::search_glob_hit`):
     ```rust
     let leaves = prefix.starts_with('/') || prefix.starts_with('~') || climbs(prefix);
     if !leaves {
         return Some(vec![]);
     }
     ```
     and `crates/atrium-core/src/policy/deny.rs:1076-1087`:
     ```rust
     fn search_glob_hit(&self, pattern: &str) -> bool {
         let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).build() else {
             return true;
         };
         let matcher = glob.compile_matcher();
         self.sensitive.probes.iter().any(|probe| {
             let name_hit = probe
                 .file_name()
                 .and_then(|n| n.to_str())
                 .is_some_and(|name| matcher.is_match(name));
             name_hit || matcher.is_match(probe)
         })
     }
     ```
   - Sequence:
     1. In `Layout::new()` (`home = "<tmp>/home/ana"`, `cwd = "<tmp>/home/ana/work"`), evaluate `Glob` (or `Grep` with `"glob"`) on:
        - `{"path": "..", "pattern": ".ssh/id_ed25519"}`
        - `{"path": "..", "pattern": ".aws/cred*"}`
        - `{"path": home, "pattern": ".aws/cred*"}`
        - `{"path": "<tmp>/home/ana/.config", "pattern": "gh/hosts.yml"}`
        - or, with `cwd = home` (`"<tmp>/home/ana"`), `{"pattern": ".ssh/id_ed25519"}` / `{"pattern": ".aws/cred*"}`
        through `SessionGate::check` under `Preset::FullAccess` and `Preset::Home`.
   - Result:
     - In `glob_target` (`crates/atrium-agents/src/claude/tools.rs:338-340`), `prefix` is `".ssh/id_ed25519"`, `".aws/cred"`, or `"gh/hosts.yml"`, which does not start with `'/'`, `'~'`, or contain `".."`. Therefore `leaves` is `false` and `glob_target` returns `Some(vec![])`.
     - `ToolSubject::Paths` is constructed with `paths: vec![base]` (just `".."`, `home`, `"<tmp>/home/ana/.config"`, or `"."`) and `glob: Some(pattern)`.
     - In `deny::Ctx::check` (`crates/atrium-core/src/policy/deny.rs:840-854`), `base` resolves to `"<tmp>/home/ana"` or `"<tmp>/home/ana/.config"`, neither of which is itself a credential path in `Sensitive`, and `has_glob_meta(base)` is `false`.
     - `deny::Ctx::search_glob_hit(pattern)` (`crates/atrium-core/src/policy/deny.rs:1076-1087`) then compiles `pattern` (`".ssh/id_ed25519"`, `".aws/cred*"`, `"gh/hosts.yml"`) with `literal_separator(true)` and tests it against each sensitive probe's `file_name()` (`"id_ed25519"`, `"credentials"`, `"hosts.yml"`, none of which contain `'/'`) and full absolute path (`"<tmp>/home/ana/.ssh/id_ed25519"`, which starts with `'/'`). Because `pattern` is a multi-component relative path (containing `'/'` without a leading `'/'` or `"**/"`), `matcher.is_match` is `false` on both!
     - Consequently, `hard_deny` returns `None`: `SessionGate::check` returns **`GateDecision::Allow`** under `Preset::FullAccess` and **`GateDecision::Ask`** (instead of `GateDecision::Deny` `"Always blocked (credential-read)"`) under `Preset::Home`.
     - Violates `docs/contracts/m1.md` §10 (`HARD_DENY_DEFAULTS` apply in every preset, including `FullAccess`) and `crates/atrium-agents/tests/claude_policy_e2e.rs:15-16`.
   - Fix: In `glob_target` (`crates/atrium-agents/src/claude/tools.rs:338-358`), whenever `glob` contains `'/'` (or touches a dot-segment / `Library`), include `join(glob)` (and `join(dir)` when `leaves` is `true`) in the returned `Vec<String>` so `deny::Ctx::check` evaluates `Sensitive::glob_hit` and `Sensitive::contains` on `join(glob)` resolved against `base`. (Single-component workspace patterns like `"*.rs"`, `"*.[ch]"`, `"*.{ts,tsx}"` have no `'/'` and are already matched against `probe.file_name()` by `search_glob_hit`, while multi-component workspace patterns like `"**/*.rs"`, `"src/**/mod.rs"`, `"src/{a,b}/mod.rs"` stay inside `cwd` so adding `join(glob)` keeps them `GateDecision::Allow` under both `Preset::Home` and `Preset::FullAccess`.)

4. [minor] **`classes_are_plain` overwrites `prev` with `Some('-')` when entering a range, rejecting every ordinary character range (`"*.[a-z]"`, `"file-[0-9].rs"`) as `ToolSubject::Unknown` and failing to reject ranges that span `'~'`**
   - Where: `crates/atrium-agents/src/claude/tools.rs:251-263` (`classes_are_plain`):
     ```rust
     if in_range {
         if let Some(lo) = prev
             && lo <= '/'
             && c >= '.'
         {
             return false;
         }
         in_range = false;
     } else if c == '-' && prev.is_some() {
         in_range = true;
     }
     prev = Some(c);
     first = false;
     ```
   - Sequence:
     1. In `Layout::new()`, call `tool_check(Some("t1"), "Glob", &json!({"path": cwd, "pattern": "*.[a-z]"}), 1)` (or `"pattern": "file-[0-9].rs"`) and evaluate under `Preset::Home`.
     2. Call `tool_check(Some("t2"), "Glob", &json!({"path": cwd, "pattern": "[}-\u{7f}]/.ssh/id_ed25519"}), 1)`.
   - Result:
     - In step 1 (`"*.[a-z]"`), on `'a'` line 262 sets `prev = Some('a')`. On `'-'`, line 260 sets `in_range = true`, and line 262 immediately overwrites `prev` with `Some('-')` (ASCII 45). On `'z'` (ASCII 122), `in_range` is `true` and `lo` is `'-'` (45 `<= '/'` = 47) while `c` is `'z'` (122 `>= '.'` = 46), so lines 252-256 return `false`! Every `[a-z]`, `[A-Z]`, and `[0-9]` range is rejected as `ToolSubject::Unknown`, causing `Preset::Home` to return `GateDecision::Ask` instead of `GateDecision::Allow`.
     - In step 2 (`"[}-\u{7f}]/.ssh/id_ed25519"`), if `prev` is preserved as `'}'` (ASCII 125), `lo <= '/'` is `false`, so a range spanning `'~'` (ASCII 126) is not rejected because lines 252-255 only check `'.'`/`'/'` and not `'~'` (nor reverse ranges where `lo > c`).
   - Fix: In `classes_are_plain` (`crates/atrium-agents/src/claude/tools.rs:251-263`), when `c == '-' && prev.is_some()`, set `in_range = true; first = false; continue;` without overwriting `prev`; when `in_range` is `true`, reject if `lo > c || (lo <= '/' && c >= '.') || (lo <= '~' && c >= '~')`, then set `in_range = false; prev = None; first = false; continue;`.

5. [minor] **`Proc::drop` outside a Tokio runtime (or during runtime shutdown) relies on `supervise::kill_recorded`, which skips signalling the process group when the leader has already exited into an unreaped zombie**
   - Where: `crates/atrium-agents/src/claude/adapter.rs:431-451` (`Proc::drop`), calling `crates/atrium-host/src/supervise.rs:171-208` (`kill_recorded`) and `crates/atrium-host/src/supervise.rs:554-563` / `469-475` (`platform_identify`):
     ```rust
     impl Drop for Proc {
         fn drop(&mut self) {
             if let Some(mut child) = self.child.take() {
                 let identity = child.identity.clone();
                 let kill_now = move |mut child: supervise::SupervisedChild| {
                     let _ = supervise::kill_recorded(&identity);
                     let _ = child.child.try_wait();
                 };
                 match tokio::runtime::Handle::try_current() {
                     Ok(h) => {
                         let jh = h.spawn(async move {
                             let _ = supervise::terminate(&mut child, Duration::ZERO).await;
                         });
                         if jh.is_finished() {
                             let _ = supervise::kill_recorded(&identity);
                         }
                     }
                     Err(_) => kill_now(child),
                 }
             }
         }
     }
     ```
     and `crates/atrium-host/src/supervise.rs:180-188`:
     ```rust
     match identify(recorded.pid)? {
         None => {
             // Leader is gone. If the group is gone too, cleanup is complete; if
             // processes remain in `pgid`, we can't prove they're ours.
             if group_is_empty(pgid) {
                 Ok(Cleanup::Verified)
             } else {
                 Ok(Cleanup::Unverified)
             }
         }
     ```
   - Sequence:
     1. Spawn a session where the CLI process spawns a background child in the same process group (`pgid == pid`) and the leader itself exits (`exit(0)`) before answering `initialize` while the background child keeps stdout/stderr open (or right before the caller drops `fut`).
     2. Drop `fut` on a plain `std::thread` outside any Tokio runtime (`Handle::try_current()` is `Err(_)`).
   - Result:
     - Because `self.child` (`SupervisedChild`) has not been reaped yet, the exited leader is an unreaped zombie (`state == 'Z'` on Linux at `crates/atrium-host/src/supervise.rs:559`; `pbi_status == MACOS_SZOMB` on macOS at `crates/atrium-host/src/supervise.rs:473`).
     - `identify(recorded.pid)` returns `Ok(None)` for zombies, so `supervise::kill_recorded(&identity)` enters the `None` branch (`crates/atrium-host/src/supervise.rs:181-188`) and returns `Ok(Cleanup::Unverified)` **without sending `SIGKILL` to `pgid`**.
     - Next, `let _ = child.child.try_wait();` (line 448) reaps the zombie leader, leaving the background child running in `pgid`.
     - Violates `crates/atrium-host/src/supervise.rs:93-98` ("Also kills the group when the leader exited on its own but descendants remain. The caller must **not** reap `child.child` itself first (adapters call `terminate`, never `wait` or `try_wait`)") and `docs/contracts/m1.md` §3 (`atrium-host::supervise`).
   - Fix: In `Proc::drop` (`crates/atrium-agents/src/claude/adapter.rs:431-451`), when `Handle::try_current()` is `Err(_)` (or `jh.is_finished()`), build a `tokio::runtime::Builder::new_current_thread().enable_all().build()` (or spawn a helper `std::thread` that builds one) and run `rt.block_on(supervise::terminate(&mut child, Duration::ZERO))`, which sends `SIGKILL` to `pgid` before reaping `child.child` even when the leader is already a zombie.

6. [minor] **`ClaudeNormalizer::can_use_tool` does not call `establish` and `ensure_turn` before `oversize_call`, emitting `ToolCallStarted` / `ToolCallFinished` outside a turn or letting `hold` split the pair**
   - Where: `crates/atrium-agents/src/claude/normalizer.rs:923-938` (`can_use_tool`):
     ```rust
     fn can_use_tool(&mut self, req_id: String, msg: &Value, out: &mut NormalizedLine) {
         let name = str_field(msg, "tool_name").unwrap_or("").to_owned();
         let id = str_field(msg, "tool_use_id")
             .map(str::to_owned)
             .unwrap_or_else(|| format!("req:{req_id}"));
         let input = msg.get("input").cloned().unwrap_or_else(empty_obj);
         if !within_limit(&input) {
             self.oversized.insert(id.clone());
             self.oversize_call(&id, &name, out);
             out.replies.push(allow_deny(
                 &req_id,
                 false,
                 Some(OVERSIZE_REASON),
                 Some(input),
             ));
             return;
         }
     ```
   - Sequence:
     - Variant A (between turns): On an established `ClaudeNormalizer` after `result()` closes turn 1 (`self.turn_open == false`), feed `can_use("r1", Some("toolu_big"), "Edit", big_input())`.
     - Variant B (before `SessionStarted` with `hold` overflow): On a fresh `ClaudeNormalizer` (`self.established == false`), feed `user_line("go")`, then `can_use("r1", Some("toolu_big"), "Edit", big_input())` (which pushes `ToolCallStarted` and `ToolCallFinished` into `self.held`), then 49 unmapped records (`{"type":"system","subtype":"noise_0"}` .. `noise_48`, each pushing 1 diagnostic into `self.held`), and finally `message_start()`.
   - Result:
     - In Variant A, `ToolCallStarted` and `ToolCallFinished` are emitted while `self.turn_open == false` (outside any `TurnStarted`..`TurnCompleted`), violating `crates/atrium-protocol/src/agent.rs:51-55`.
     - In Variant B, `self.held` reaches 51 items (`MAX_DIAGNOSTIC_LINES = 50` at `crates/atrium-agents/src/claude/normalizer.rs:1291`), so `self.hold` drains the first item (`ToolCallStarted { call_id: "toolu_big" }`) while keeping `ToolCallFinished { call_id: "toolu_big" }`. When `message_start()` arrives, it emits `SessionStarted`, `TurnStarted`, and an orphan `ToolCallFinished { call_id: "toolu_big" }` with no preceding `ToolCallStarted`, violating `crates/atrium-agents/tests/consistency.rs:212-220`.
   - Fix: Call `self.establish(out); self.ensure_turn(out);` immediately before `self.oversize_call(&id, &name, out);` at `crates/atrium-agents/src/claude/normalizer.rs:930`.

## Suspected, not shown
- **Dangling `"pending"` ledger entry when an `Ask` decision is dropped mid-turn (`crates/atrium-agents/src/claude/adapter.rs:1164-1197`, `1262-1288` vs `crates/atrium-core/src/policy/gate.rs:43`, `232-279`)**: Still suspected (depends on daemon-side wiring outside M1-07). When `gate.check` records a `"pending"` ledger entry and returns `GateDecision::Ask`, and either (a) `control_cancel_request` withdraws the request before `Internal::Gate` is processed (`!self.live.contains(&req)` at line 1264 drops it before `feed`), or (b) `hook_callback` parks `Ask(T1)` in `self.tickets` and `can_use_tool` never follows or arrives with a modified input, the adapter emits neither `ApprovalRequested` nor `RequestResolved` for `T1`. Because `SessionGate::redeem` (`crates/atrium-core/src/policy/gate.rs:297`) only calls `self.ledger.withdraw(&stale)` when redeeming a *second* `Ask` on the same `(call_id, tool_name)`, `T1`'s `"pending"` entry remains counted against `MAX_PENDING_REQUESTS` (`64`, `crates/atrium-core/src/policy/gate.rs:43`) until `expire_turn` runs at turn end.
- **Mid-turn settings changes (`crates/atrium-agents/src/claude/adapter.rs:943-973`)**: Still suspected (design choice). `refresh_enforcement()` runs in `dispatch_next()` and `gate_failed()`, not after mid-turn tool executions, so a tool call that edits `.claude/settings.json` during turn 1 is reported via `EnforcementChanged` at the start of turn 2 rather than mid-turn.

## Tried and held
- **`gate_key` canonical JSON (`crates/atrium-agents/src/claude/tools.rs:131-159`)**: Compared `canonical_json` line by line against `write` in `atrium_core::policy::canonical_input` (`crates/atrium-core/src/policy/mod.rs:30-58`). Both sort `Value::Object` keys by `String::cmp` (`BTreeMap` / `Vec::sort_by`), recurse into nested objects and arrays while preserving array order, and format primitives via `serde_json::Value::to_string()` (so numbers, booleans, `null`, and unicode string escapes are byte-for-byte identical).
- **Brace expansion cap (`crates/atrium-agents/src/claude/tools.rs:208`, `274-324`)**: Checked nested and chained brace groups (`"{a,b}{c,d}{e,f}{g,h}{i,j}{k,l}"`, `"{a,{b,{c,d}}}"`, unbalanced `"{"` and `"}"`, empty alternatives `"{a,}"`). `expand_braces` expands the leftmost group and recursively expands each branch, checking `if all.len() > MAX_EXPANSIONS { return None; }` after each branch and propagating `None` via `?`, bounding work and returning `ToolSubject::Unknown` past 32 alternatives.
- **Ordinary workspace globs (`crates/atrium-agents/src/claude/tools.rs:172-204`, `329-359`)**: Verified that `"**/*.rs"`, `"src/**/mod.rs"`, `"*.{ts,tsx}"`, `"src/{a,b}/mod.rs"`, and `"*.[ch]"` produce `ToolSubject::Paths` with `paths: vec![base]` and return `GateDecision::Allow` under both `Preset::Home` and `Preset::FullAccess`.
- **Session establishment without `init` (`crates/atrium-agents/src/claude/normalizer.rs:1254-1324`)**: Verified that `establish_with` sets `self.established = true` before emitting `SessionStarted` and draining `self.held`, so `SessionStarted` is never emitted twice even when a late `system/init` arrives after `message_start`, `assistant`, `can_use_tool`, or `result`. Turn numbers (`next_turn_number`, `current_turn`, `turn_open`) remain aligned across pre-`init` and post-`init` turns, and `self.held` is bounded at `MAX_DIAGNOSTIC_LINES` (`50`) and drained in FIFO order after `SessionStarted` and (when `prompt_sent`) `TurnStarted`.
- **Request kinds (`crates/atrium-agents/src/claude/adapter.rs:702-712`, `868-899`, `1020-1034`, `1255-1328`)**: Verified that `live: BTreeSet<Req>` and `inflight: BTreeMap<Req, JoinHandle<()>>` isolate `Req::Hook(id)` from `Req::CanUse(id)` when the CLI reuses a `request_id` across `hook_callback` and `can_use_tool`, while `forget_native` (on `control_cancel_request`) removes and aborts both `Req::Hook(id)` and `Req::CanUse(id)`, drops any parked ticket, and feeds `control_cancel_request` to the normalizer only when a `CanUse` request was live. When cancelled by the CLI, no reply is written (the CLI itself cancelled the request); when a live request is not cancelled, its reply is never dropped by a sibling request of the other kind.
- **Helper command keys (`crates/atrium-agents/src/claude/settings.rs:46-58`, `180-213`, `269-368`)**: Verified that all 11 keys in `COMMAND_KEYS` (`apiKeyHelper`, `awsAuthRefresh`, `awsCredentialExport`, `fileSuggestion`, `gcpAuthRefresh`, `otelHeadersHelper`, `policyHelpers`, `processWrapper`, `proxyAuthHelper`, `statusLine`, `subagentStatusLine`) are checked by `command_findings` via `inspect_text` across all 6 settings sources (`ManagedPolicy`, `UserSettings`, `ProjectSettings`, `LocalSettings`, `--settings <file>`, and inline `--settings <json>`).

## Test gaps
- `crates/atrium-agents/tests/claude_policy_e2e.rs`: Add end-to-end policy tests for brace-interleaved character classes (`"[{a],.}][{a],.}]/.ssh/id_ed25519"`, `"[{a],/}]/etc/passwd"`, `"[{a],~}]/.ssh/id_ed25519"`), `~user` / `~*` prefixes (`"~bob/*"`, `"~a*/.ssh/id_ed25519"`), and multi-component relative credential globs (`{"path": "..", "pattern": ".ssh/id_ed25519"}`, `{"path": home, "pattern": ".aws/cred*"}`, `{"path": "<home>/.config", "pattern": "gh/hosts.yml"}`) under both `Preset::Home` and `Preset::FullAccess`.
- `crates/atrium-agents/tests/claude_review.rs`: Add a unit test checking that `tool_subject("Glob", &json!({"path": "/home/ana/work", "pattern": "*.[a-z]"}))` produces `ToolSubject::Paths` (not `ToolSubject::Unknown`), and that `"[}-\u{7f}]"` is `ToolSubject::Unknown`.
- `crates/atrium-agents/tests/claude_adapter.rs`: Add a test for `Proc::drop` outside a Tokio runtime where the group leader exits immediately after spawning a background grandchild in its process group, verifying that the grandchild is killed when `start` is dropped.
- `crates/atrium-agents/tests/claude_review.rs`: Add a normalizer test feeding an oversize `can_use_tool` input before `init` and between turns, asserting via `consistency::check_event_stream` that `ToolCallStarted` and `ToolCallFinished` are always inside an open turn after `SessionStarted`.
