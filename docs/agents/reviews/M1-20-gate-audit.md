# M1 Gate Audit — `dev` at `b8ebe26`

- **Task:** `M1-20-gate-audit` ([D-104](../decisions.md))
- **Date:** 2026-11-16
- **Audited commit:** `b8ebe26` (`dev` at worktree dispatch, `docs/agents/status.md` baseline)
- **Specs checked against:** `PLAN.md` (M1 ~L727 and referenced §3.x, §4, §8), `docs/contracts/m1.md` (contract rev 3 + rev 3.1), `docs/agents/m1-must-fix.md` (M0 gate promotions + open M1 items), `docs/agents/decisions.md` (D-001..D-104), `docs/agents/change-requests.md` (CR-01..CR-15)
- **In-flight work excluded from gap counts (marked `in flight`):**
  - `M1-18`: CR-12 (credential stores that ask), `P1` (escaping search globs), `K1–K4` (`SessionGate` / policy minors), and `engine_never_panics_on_garbage` runtime fix.
  - `M1-19`: `N3` (zombie reap race in `kill_recorded`), `N6` (`judge_exe_read` `PF_EXITING` / `num_threads` on Linux), `N10` (macOS `EPERM` race on `terminate`).
  - `M1-08L`: live Codex checks `X1` (patch file-change), `X2` (question / `DeclineQuestion`), `X3` (`ProviderGated` MCP/hooks matrix).

> [!CAUTION]
> **Security escalation check (open M1-13 `#[ignore]` findings on `b8ebe26`):**
> No *new* unrecorded Allow-on-credentials or delete-outside-workspace bugs were found in code or tests on `b8ebe26`, and every active test in `policy`, `adversarial_policy`, `adversarial_ordinary`, `adversarial_gate`, `adversarial_ledger`, `adversarial_fuzz`, and `adversarial_regression` passes. However, **four open credential/Atrium-dir gaps from `M1-13` remain unfixed and `#[ignore]`d on `dev`** (`S5`, `S7`, `S8`, `S11` in `docs/agents/m1-must-fix.md`), plus `finding_6` (`S1` / CR-12, `in flight` in `M1-18`):
> - `S7` (`crates/atrium-core/tests/adversarial_policy.rs:977`, `finding_4_ansi_c_quoted_words_are_decoded`) — ANSI-C `$'...'` quoted words reaching credentials.
> - `S8` (`crates/atrium-core/tests/adversarial_policy.rs:1065`, `finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked`) — recursive search over a parent folder containing a credential file.
> - `S11` (`crates/atrium-core/tests/adversarial_gate.rs:571`, `finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked`) — glob traversal through a symlink into credentials.
> - `S5` (`crates/atrium-core/tests/adversarial_gate.rs:582`, `finding_11_a_glob_inside_atriums_folder_names_is_blocked`, and `crates/atrium-core/tests/adversarial_fuzz.rs:422`, `random_spellings_of_sensitive_paths_are_always_hard_denied`) — globs/case spellings reaching Atrium's own folders or sensitive paths.
>
> Release readiness requires gate sign-off on either fixing or explicitly accepting/waiving `S5`, `S7`, `S8`, `S11` before promoting M1.

---

## Verdicts Legend

- `delivered`: Code present and covered by passing test(s) or a recorded check script report.
- `delivered, untested`: Code present in the audited commit, but no passing test or recorded live check verifies it.
- `partial`: Part of the promise is delivered; the missing piece is named explicitly.
- `missing`: Promised by PLAN / contract / M0 promotion, absent on `b8ebe26`.
- `in flight`: Active open task (`M1-18`, `M1-19`, `M1-08L`).
- `out of M1`: Explicitly deferred beyond M1 by `PLAN.md`, `docs/contracts/m1.md`, or `docs/agents/decisions.md`.

---

## 1. Summary Audit Table by Area

### 1.1 Architecture, Daemon Lifecycle & IPC (`PLAN.md` §0/M1, `m1.md` §§1, 4, 5, 12, 13, 15)

| Promise / Requirement | Verdict | Evidence & Notes |
| --- | --- | --- |
| Daemon-first process split (`atriumd` owns SQLite, adapters, sessions, approval ledger; Tauri shell is a pure client) | `delivered` | `crates/atrium-daemon/src/lib.rs`, `app/src-tauri/src/bridge.rs`, `app/src/daemon/client.ts`; `crates/atrium-daemon/tests/socket.rs`, `crates/atrium-client/tests/socket.rs` (11 passed) |
| **`atriumd` binary registers native Claude & Codex adapters** (`m1.md` §1, §6) | **`missing` (BLOCKER)** | `crates/atrium-daemon/src/main.rs:68-86` constructs `Options::new(paths.clone(), home)` and leaves `options.adapters` as `Vec::new()` (`crates/atrium-daemon/src/lib.rs:133`). The real `atriumd` binary never registers `ClaudeAdapter` or `CodexAdapter`, so `ProbeProviders` returns no real agent and `AgentLaunch::Native` fails. Tests pass only because they inject adapters directly into `Options`. |
| Single-daemon lockfile, socket & protocol `v2` handshake (`m1.md` §§4, 5, 13, PLAN §3.16) | `delivered` | `crates/atrium-protocol/src/lib.rs:22` (`PROTOCOL_VERSION = 2`), `crates/atrium-daemon/src/server.rs`, `crates/atrium-client/src/lib.rs`; `crates/atrium-daemon/tests/socket.rs`, `tests/start_refusal.rs` (5 passed), `app/src/ui/ConnectionBanner.tsx` |
| Peer identity, same-UID enforcement & client roles (`App` / `Dev` / `Harness`, `m1.md` §4) | `delivered` | `crates/atrium-host/src/peer.rs`, `crates/atrium-protocol/src/wire.rs`, `crates/atrium-daemon/src/conn.rs`; `crates/atrium-host/tests/peer.rs` (3 passed), `crates/atrium-daemon/tests/adversarial_socket.rs` (7 passed) |
| Release daemon app-identity verification (`AppIdentitySource`, `m1.md` §4) | `partial` | `crates/atrium-daemon/src/main.rs:79-83` uses `AppIdentitySource::File` in `cfg!(debug_assertions)` and `AppIdentitySource::None` in release until M8 packaging compiles the release bundle identity. Tracked as packaged-build work (`M8` / `N5` Linux note). |
| Settings → Security disclosure of same-UID peer boundary (`m1.md` §4: *"must state clearly"*) | `missing` | `rg -n -e 'same-UID' -e 'same UID' app/src/` returned 0 matches; `app/src/ui/Settings.tsx` has no Security disclosure explaining that same-UID processes can talk to `atriumd`. |
| Protocol wire & delivery shapes + TS binding freshness (`m1.md` §5, §15, M0 promotion) | `delivered` | `crates/atrium-protocol/bindings/protocol.ts`, `app/src-tauri/tests/delivery_shapes.rs` (2 passed), `crates/atrium-protocol/tests/serde_shape.rs` (13 passed), `./scripts/check-protocol-ts` exits 0 (`specta v2.0.0-rc.26`). |
| Unauthenticated local HTTP dev / test server disabled by default; gated behind harness flag | `delivered` | `app/src-tauri/src/lib.rs` only starts harness hooks when `harness` flag is active; verified in `app/scripts/live_check.py`, `app/scripts/chat_check.py`. |

### 1.2 Storage, Snapshots, Redaction & Export (`PLAN.md` M1, §3.x, `m1.md` §§10, 13, 14)

| Promise / Requirement | Verdict | Evidence & Notes |
| --- | --- | --- |
| SQLite store in WAL mode, `synchronous = NORMAL`, bounded `cache_size`, migrations rollback (`m1.md` §10, §14) | `delivered` | `crates/atrium-core/schema/0001_init.sql`, `crates/atrium-core/src/store/mod.rs:223-229`, `crates/atrium-core/src/store/reader.rs:34`, `crates/atrium-core/src/store/migrate.rs`; `crates/atrium-core/tests/store.rs::failed_migration_rolls_back_and_refuses_to_open` |
| Event log + atomic projection writes + SHA-256 hash verification + closing batches on low disk (`m1.md` §10, §13) | `delivered` | `crates/atrium-core/src/store/writer.rs`; `store.rs::append_updates_events_and_projections_atomically`, `integrity_mismatch_is_stored_but_counted`, `low_disk_makes_store_read_only_and_recovers`, `read_only_store_still_accepts_closing_batches_and_signals`, `three_million_token_stream_appends_and_closes` |
| Compact snapshots, private `0600` files, symlink refusal & orphan snapshot pruning (`m1.md` §10) | `delivered` | `crates/atrium-core/src/store/snapshot.rs`; `store.rs::snapshot_is_private_and_pruned`, `symlinks_are_refused`, `crates/atrium-daemon/tests/gaps_snapshot.rs` (2 passed), `gaps_full_storage.rs` (2 passed) |
| Export JSONL/Markdown, secret/home redaction & atomic `0600` cleanup on failure — backend (`m1.md` §10) | `delivered` | `crates/atrium-core/src/history/{export.rs,redact.rs,file.rs}`, `crates/atrium-daemon/src/commands.rs`; `crates/atrium-core/tests/history.rs` (25 passed), `crates/atrium-daemon/tests/sessions.rs::an_export_is_private_and_removed_when_it_fails` |
| Export thread UI action (`PLAN.md` M1 / §3.7, `m1.md` §10) | `missing` | `rg -n -e export_thread app/src/` returned 0 hits; no export button, palette command, or bridge call exists in `app/src/`. |

### 1.3 Thread States, Attention, Notifications & Tray (`PLAN.md` §3.1, §3.4, §3.10, `m1.md` §§2, 8, M0 promotions)

| Promise / Requirement | Verdict | Evidence & Notes |
| --- | --- | --- |
| Thread state machine (`Starting`, `Running`, `NeedsApproval`, `NeedsInput`, `Idle`, `Paused`, `Failed`, `Interrupted(Verified/Unverified)`, `Ended`, incl. CR-09 sticky unverified cleanup) (`m1.md` §2) | `delivered` | `crates/atrium-protocol/src/thread.rs`, `crates/atrium-core/src/attention/mod.rs`, `app/src/daemon/reduce.ts`; `crates/atrium-core/tests/attention.rs` (9 passed), `app/src/daemon/reduce.test.ts` |
| Attention inbox ranking & urgency sorting (`approval > question > failed > interrupted > ready`) (`m1.md` §2, PLAN §3.4) | `delivered` | `crates/atrium-core/src/attention/mod.rs` (`rank_inbox`), `app/src/chat/inbox.ts`; `attention.rs::approval_outranks_question_outranks_failed`, `app/src/chat/inbox.test.ts` |
| OS desktop notifications via `CommandNotifier`, rate-limiting & focus suppression (`SetFocus`) (`m1.md` §8, M0 promotion) | `delivered` | `crates/atrium-core/src/attention/mod.rs` (`should_notify`), `crates/atrium-daemon/src/upkeep.rs:230-240` (`CommandNotifier`), `crates/atrium-daemon/src/hub.rs:81,216-276`, `crates/atrium-daemon/src/commands.rs:329-330`, `app/src/chat/actions.ts:143`; `attention.rs::notifications_are_rate_limited_and_suppressed_when_focused`, `crates/atrium-daemon/tests/requests_notifications.rs` (2 passed) |
| System tray icon (`@tray` logical client), live counts tooltip & hide-on-close (`PLAN.md` §3.1, `m1.md` §8) | `partial` | `app/src-tauri/src/presence.rs`, `app/src-tauri/src/lib.rs:718-925`; `app/scripts/live_check.py` (macOS 12/12). On Linux GNOME Shell 51.1, `docs/spikes/M1-linux-checks.md:107-111` records that the tray indicator was not verified (`libayatana-appindicator3` present, top bar showed no icon). |

### 1.4 Process Supervision & Cleanup (`PLAN.md` M1, `m1.md` §§2, 12, `m1-must-fix.md` N1–N10)

| Promise / Requirement | Verdict | Evidence & Notes |
| --- | --- | --- |
| Process group isolation (`setsid`), start-time identity capture, orphan sweep, startup refusal on unverified leftover, crash recovery (`m1.md` §12) | `delivered` | `crates/atrium-host/src/{supervise.rs,lifecycle.rs}`, `crates/atrium-daemon/src/upkeep.rs`; `crates/atrium-host/tests/supervise.rs` (15 passed), `lifecycle.rs` (5 passed), `crates/atrium-daemon/tests/recovery.rs` (7 passed), `adversarial_recovery.rs` (3 passed), `start_refusal.rs` (5 passed) |
| Live-process kill verification fixes `N3` (zombie reap race), `N6` (`PF_EXITING`/`num_threads`), `N10` (macOS `EPERM` race) | `in flight` | Tracked in `M1-19` per `docs/agents/status.md` (`m1-must-fix.md` N3, N6, N10); `crates/atrium-host/tests/adversarial_supervise.rs:380-384` remains macOS `#[ignore]` (`kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries`). |
| Linux root-owned packaged app-identity check (`N5`, `m1-must-fix.md`) | `out of M1` / open minor | Requires root-owned packaged install (Linux M8 packaging / follow-up note in `m1-must-fix.md`). |

### 1.5 Policy Engine, Approval Ledger & Rules (`PLAN.md` M1, §§3.12, 3.13, `m1.md` §§3, 7, 9, `m1-must-fix.md` S/K/P)

| Promise / Requirement | Verdict | Evidence & Notes |
| --- | --- | --- |
| Three presets (`Home` read-only, `AlwaysAsk`, `FullAccess`), hard-deny precedence over rules & presets (`m1.md` §§3, 9) | `delivered` | `crates/atrium-protocol/src/policy.rs`, `crates/atrium-core/src/policy/{engine.rs,deny.rs,gate.rs}`; `crates/atrium-core/tests/policy.rs` (44 passed), `adversarial_ledger.rs` (14 passed), `adversarial_ordinary.rs` (16 passed), `adversarial_regression.rs` (57 passed) |
| Single-use exact-input approval tickets (`ApprovalLedger`), turn/incarnation expiry, atomic remember-rule creation (`m1.md` §7) | `delivered` | `crates/atrium-core/src/policy/ledger.rs`; `crates/atrium-core/tests/ledger.rs` (20 passed), `adversarial_ledger.rs` (14 passed) |
| CR-12 (credential stores that ask), `P1` (escaping search globs), `K1–K4`, `engine_never_panics_on_garbage` slow fuzz speedup | `in flight` | Tracked in `M1-18` per `docs/agents/status.md` and `docs/agents/decisions.md` (D-104); `engine_never_panics_on_garbage` still takes `119.54s` on `b8ebe26`. |
| Open `M1-13` security findings `S5`, `S7`, `S8`, `S11` | `missing` (open minor/security) | 5 active `#[ignore]` tests in `atrium-core` (excluding CR-12 `finding_6` in flight in `M1-18`); see Security warning above and §2 below. |
| `K5` (tool rule matching vs `bash -c` nuance) & `P2` (`SessionGate` ticket withdrawal on cancelled Ask) | `missing` (open minor) | Tracked in `docs/agents/m1-must-fix.md` (`K5`, `P2`), not resolved on `b8ebe26`. |
| Rule creation/editing UI in Settings (`CreateRule`, `CommandPrefix` rules editor) (`PLAN.md` §3.13, `m1.md` §9, CR-06) | `missing` | `app/src/ui/Settings.tsx:74-81` only calls `list_rules` and `delete_rule`. `rg -n create_rule app/src/` hits only `app/src/chat-shots.ts:182-184` (screenshot fixture). `CommandPrefix` rules cannot be created anywhere in the UI. |
| "Always blocked (for calls Atrium sees)" hard-deny list + `git config` known limit in Settings (`m1.md` §§3, 9, `PLAN.md` §3.12) | `missing` | `app/src/ui/Settings.tsx` does not display the hard-deny list and `rg -n 'git config' app/src/` returned 0 matches. (CR-12 credential stores list in Settings awaits `M1-18` per D-104.) |

### 1.6 Agent Adapters (`PLAN.md` M1, §§3.12, 3.16, `m1.md` §§6, 7, 13)

| Promise / Requirement | Verdict | Evidence & Notes |
| --- | --- | --- |
| Claude Code adapter (`stream-json`, permission prompt tool + hooks, `--strict-mcp-config`, normalizer, SessionGate, replay, usage/cost, external-hooks `Partial`) | `partial` | Backend & suite pass (`atrium-agents`: unit 35, `claude_adapter` 38, `claude_golden` 20, `claude_normalizer` 61, `claude_policy_e2e` 5, `claude_review` 28). Two gaps: (1) `ClaudeAdapter` is not wired into `atriumd` `main.rs` (Blocker above); (2) live `claude` CLI continuation after `DeclineQuestion` (`deny_can_use`) is verified only on mocked/normalized fixtures (`claude_normalizer::a_question_can_be_declined_without_ending_the_turn`), per `status.md:42` & `reviews/M1-16.md`. |
| Codex adapter (`app-server` JSON-RPC over stdio, normalizer, policy gating, replay, `ProviderGated` flags) | `partial` / `in flight` | Mocked/unit/golden suites pass (`codex_adapter` 16, `codex_golden` 7, `codex_normalizer` 10, `codex_policy_e2e` 3, `consistency` 12). Not wired into `atriumd` `main.rs` (Blocker above), and live `codex` CLI checks `X1–X3` are `in flight` under `M1-08L`. |
| Daemon-owned MCP server list (`m1.md` §6.2) | `delivered` | `crates/atrium-daemon/src/sessions.rs:350-352` supplies `mcp_servers: Vec::new()` ("MCP servers are the daemon's to add, never the client's; M1 has none"); adapters enforce config when supplied (`claude_adapter::temp_mcp_config_is_private_and_removed_when_the_session_ends`). |
| Enforcement descriptor & per-session enforcement banner (`m1.md` §6, `PLAN.md` §3.12) | `partial` | `EnforcementDescriptor` (`Full` / `Partial` / `ProviderGated`) is computed and displayed as badge + reasons (`app/src/ui/Settings.tsx:161-162`, `app/src/ui/ContextPanel.tsx:101-102`). **Missing**: PLAN §3.12's *"details" link* to the enforcement matrix (`coverage` per `ToolClass`, `config_sources`, and separate coverage/policy/isolation presentation). |

### 1.7 Chat Shell, Multi-Window, Transcript & Themes (`PLAN.md` M1, §§3.2–3.15, `m1.md` §§2, 3, 5, 11, 13, M0 promotions)

| Promise / Requirement | Verdict | Evidence & Notes |
| --- | --- | --- |
| Thread sidebar, active/attention/closed grouping, composer, new thread creation & provider probe | `delivered` | `app/src/ui/{Sidebar.tsx,Composer.tsx,ThreadCard.tsx}`, `app/src/chat/{threads.ts,actions.ts:31}`; `app/scripts/chat_check.py` (12/12 on Linux), `app/scripts/screenshots.py` (28 screenshots). |
| Thread archive & rename controls in UI (`DaemonCommand::ArchiveThread` / `RenameThread`, PLAN §3.2) | `missing` | Backend & reducer handle updates (`crates/atrium-daemon/src/commands.rs`, `app/src/daemon/reduce.test.ts:57`), but `rg -n -e archive_thread -e rename_thread app/src/` returned 0 hits. No UI triggers exist to rename or archive a thread. |
| Streaming Markdown transcript, syntax highlight, tool call cards, request cards, open file/URL links, truncation limits | `delivered` | `app/src/ui/{Transcript.tsx,RequestCard.tsx}`, `app/src/chat/{transcript.ts,markdown.ts,render.ts,links.ts,limits.ts}`; `transcript.test.ts`, `markdown.test.ts`, `limits.test.ts`, `requests.test.ts`. |
| Approval Inbox, `⌘J` next-item jump, `A` allow / `D` deny / `1–4` quick answer | `partial` | `app/src/App.tsx:18` (`⌘J` → `jumpToNext`), `app/src/ui/InboxCard.tsx` (`A`/`D` single item), `app/src/ui/RequestCard.tsx`. **Missing vs PLAN §3.4**: (1) `⌘⇧J` previous-item shortcut (`App.tsx:18` sends `⇧⌘J` to `jumpToNext`); (2) batch approve/deny in the approval queue (`InboxCard.tsx` acts only on one selected request at a time). |
| Command Palette (`⌘K`) & Pop-out windows with cross-window `Sync` + reconnect banner (M0 promotion) | `delivered` | `app/src/ui/{Palette.tsx,ConnectionBanner.tsx}`, `app/src/{windows.ts,sync.ts}`; `app/src/sync.test.ts`, `app/src/window-model.test.ts`, `app/scripts/live_check.py`. |
| VS Code theme loader, state×surface readability contrast (19 pairs, M0 promotion), `tokens-conformance` & font settings | `delivered` | `app/src/theme/{loader.ts:331-337,vscode.ts}`, `app/src/styles/tokens-conformance.test.ts`; `app/src/theme/loader.test.ts:240` (checks 12 state×surface pairs + 7 UI pairs = 19), `pnpm --dir app test` (14 files, 264 tests passed). |
| macOS vibrancy / Reduce transparency & Linux opaque window `L1` (`m1.md` §11.1, M0 promotion) | `delivered` | `app/src-tauri/src/native.rs:31-58` (`translucency_supported() = cfg!(target_os = "macos")`, `native_target` forces opaque when `!supported || reduce || (popout && Opaque)`), `app/src-tauri/src/macos.rs`; verified in `docs/spikes/M1-linux-checks.md` (`L1`). |

### 1.8 Out of M1 (Verified Scope Boundaries)

The following items from `PLAN.md` and `docs/agents/m1-must-fix.md` are explicitly **out of M1**:
- **M2 workspace / worktree / git / diff / review & multi-provider expansion:** `PLAN.md` §5 M2+ (`docs/contracts/m1.md` §1 limits M1 to Home directory); `m1-must-fix.md` items `A1–A15` and `O1–O11`.
- **Kernel sandbox (`sandbox-exec` / `bwrap` / Landlock) & OS ACLs:** `docs/contracts/m1.md` §3, `m1-must-fix.md` `S10` (deferred to M2/M8 hardening).
- **MCP server tool exposure by Atrium:** `docs/contracts/m1.md` §6.2 & `crates/atrium-daemon/src/sessions.rs:350-352` ("M1 has none").
- **Full packaged installer / release signing / root-owned Linux bundle:** M8 (`crates/atrium-daemon/src/main.rs:79-83`, `m1-must-fix.md` `N5`).

---

## 2. Gate Process Items (`docs/agents/m1-must-fix.md`, "Process (for M1)")

### 2.1 Ignored Rust Tests & Skipped Vitest Tests

**Vitest:** `0` skipped tests across all 14 test files (`264` passed; `rg` for `.skip`, `xit(`, `xtest(`, `xdescribe(` returned 0 matches).

**Rust `#[ignore]` tests (`7` total on macOS / `6` cross-platform):**

| # | Location | Test Function | Ignore Attribute / Reason | Tracking Status |
| --- | --- | --- | --- | --- |
| 1 | `crates/atrium-core/tests/adversarial_policy.rs:977` | `finding_4_ansi_c_quoted_words_are_decoded` | `#[ignore = "M1-13 finding 4"]` | Open (`S7`) |
| 2 | `crates/atrium-core/tests/adversarial_policy.rs:1065` | `finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked` | `#[ignore = "M1-13 finding 5"]` | Open (`S8`) |
| 3 | `crates/atrium-core/tests/adversarial_policy.rs:1102` | `finding_6_other_credential_stores_are_hard_denied` | `#[ignore = "M1-13 finding 6"]` | `in flight` (`M1-18`, CR-12; note test asserts `HardDeny` while D-082 chose `Ask`) |
| 4 | `crates/atrium-core/tests/adversarial_fuzz.rs:422` | `random_spellings_of_sensitive_paths_are_always_hard_denied` | `#[ignore = "M1-13 finding 11"]` | Open (`S5`) |
| 5 | `crates/atrium-core/tests/adversarial_gate.rs:571` | `finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked` | `#[ignore = "M1-13 finding 10"]` | Open (`S11`) |
| 6 | `crates/atrium-core/tests/adversarial_gate.rs:582` | `finding_11_a_glob_inside_atriums_folder_names_is_blocked` | `#[ignore = "M1-13 finding 11"]` | Open (`S5`) |
| 7 | `crates/atrium-host/tests/adversarial_supervise.rs:380-384` | `kill_recorded_setsid_grandchild_and_unreaped_zombie_leader_boundaries` | `#[cfg_attr(not(target_os = "linux"), ignore = "M1-13: macOS /usr/bin/python3 is a re-execing stub, so kill_recorded can't verify the leader")]` | Platform-expected on macOS; runs on Linux |

### 2.2 Pinned §14 Performance & GUI Measurement Evidence

Measurement evidence on `dev` comes from two recorded spike reports (not re-run during this audit per brief rules):

1. **macOS (`docs/spikes/M1-macos-checks.md`, measured on `dev` `a688f5c`, `perf` profile, busy machine):**
   - **Startup (`30` launches):** median `330.9 ms`, p90 `341.8 ms`, mean `332.9 ms` (**passes `<350 ms`**; report notes machine was busy).
   - **Idle memory (`9` launches):** median `phys_footprint` `84.6–91.5 MiB`, max `92.9 MiB` (**passes `<150 MB`**).
   - **Streaming (`3` runs):** `9993–9995 tok/s`, `59.9–60 fps`; `frame_ms` p99 `19, 19, 19 ms`, max `19, 19, 61 ms`, long frames `0, 0, 2 (0.1%)`, updates dropped `0, 0, 4`.
   - **Font preload A/B (`5` launches each):** ±`6 ms` (`334.2 ms` on vs `340.1 ms` off).
2. **Linux (`docs/spikes/M1-linux-checks.md`, `M1-14a` daemon + `M1-14b` desktop + `M1-17` re-checks):**
   - **Daemon (`M1-14a`, `L8`):** idle RSS `18.4, 18.6, 18.6 MiB`; reconnect p95 `3.23, 3.74, 3.81 ms`; `100k` event replay `157.6, 191.1, 166.9 ms` (`177k–185k` events).
   - **Desktop idle memory (`M1-14b`) — FAILS `<150 MB` GATE:** `151.4, 152.5 MiB` private PSS (`158.7, 160.0 MB`), RSS `244.6 MiB` vs minimal WebKitGTK floor `124.5 MiB` (`+27–28 MiB` over floor). Pop-out adds `+71.2 MiB` private PSS (`223.7 MiB` total). Raw M1-14b artifact files were also lost with worktree cleanup (`docs/agents/status.md:35`).
   - **Linux TTI:** `377 / 391 ms` at load 2.0 (pre-D-098) and `345 / 362 ms` with font preload; no quiet-machine re-check (`<350 ms` borderline on load).
   - **Linux streaming (`stream_bench.py`, 3 runs):** `57.2–59.7 fps`, `p99 21–22 ms`, max `29–106 ms`, long frames `0.2–0.4%`.
   - **Linux GUI scripts:** `live_check.py` `12/12`, `chat_check.py` `12/12`, `screenshots.py` `28` screenshots captured cleanly; **tray icon** not verified on GNOME Shell 51.1 (`docs/spikes/M1-linux-checks.md:107-111`).

### 2.3 `scripts/doctor` D-030 Preflight Coverage

`scripts/doctor` (`110` lines) covers D-030/D-036 preflight checks:
- Tool availability (`git`, `gh`, `claude`, `codex`, `agy`, `cargo`, `node`, `pnpm`, `python3`), `gh auth status` & repo read access;
- Synced-folder check (`is_synced`) on `pwd -P`, `git-common-dir`, and `atrium-wt`;
- `git fsck --connectivity-only --no-dangling`;
- Sync conflict copies (`*conflicted copy*`, `*[1-9].*`);
- Tested `agy` version pin (`1.5.9`) + `scripts/agy-policy.py` self-test;
- `.agents/hooks.json` existence check in checkout root.
- **Note:** On worker worktrees (where `agent-run` installs `.agents/hooks.json`), `./scripts/doctor` exits `1` specifically on that final hook check (`scripts/doctor:103-105`), while all D-030/D-036 checks pass.

---

## 3. Ranked Gaps & Smallest Fixes

### Blockers

1. **`atriumd` registers zero agent adapters (`crates/atrium-daemon/src/main.rs:68-86`)**
   - **Impact:** In real execution (outside tests that manually populate `Options.adapters`), `ProbeProviders` returns `[]` and `StartThread(Native { .. })` fails immediately with no registered `Claude` or `Codex` adapter.
   - **Smallest fix:** Construct `Arc::new(ClaudeAdapter::new())` and `Arc::new(CodexAdapter::new())` (matching `atrium-agents` defaults) and push them to `options.adapters` in `crates/atrium-daemon/src/main.rs`, plus add a smoke test on the built `atriumd` binary calling `ProbeProviders`.
2. **Linux desktop idle memory misses the `<150 MB` §14 budget (`151.4–152.5 MiB` / `158.7–160.0 MB` private PSS, `docs/spikes/M1-linux-checks.md:183-199`)**
   - **Impact:** Directly misses the §14 `<150 MB` Linux desktop idle gate (`M1-14b`), with WebKitGTK floor at `124.5 MiB`; raw M1-14b measurement files are also missing from the repo.
   - **Smallest fix:** Gate reviewers must either waive/amend the Linux §14 desktop PSS ceiling (via a decision doc entry accounting for WebKitGTK baseline) or slim WebKitGTK/frontend footprint and re-record Linux §14 artifacts (`startup` + `idle`) on a quiet machine.

### Majors

1. **Missing UI for `ExportThread` (`app/src/`, `PLAN.md` M1 / §3.7, `m1.md` §10)**
   - **Smallest fix:** Add an "Export thread..." action (thread header menu / Palette) calling `command({ type: "export_thread", ... })` via a native save dialog.
2. **Missing Rules editor UI (`CreateRule`, `CommandPrefix`) & Settings Safety disclosures (`app/src/ui/Settings.tsx`, `PLAN.md` §§3.12–3.13, `m1.md` §§3, 4, 9, CR-06)**
   - **Smallest fix:** Add rule creation form (`CreateRule` supporting `CommandPrefix`/`CommandExact`/`PathGlob`), a read-only "Always blocked (for calls Atrium sees)" section from protocol defaults, and the `git config` + same-UID peer boundary disclosures (and CR-12 stores notice once `M1-18` lands).
3. **Missing UI for `ArchiveThread` & `RenameThread` (`app/src/`, `PLAN.md` §3.2)**
   - **Smallest fix:** Wire rename & archive actions on `ThreadCard` / Palette calling `archive_thread` and `rename_thread`.
4. **Missing Enforcement Matrix `"details"` view (`app/src/ui/{Settings.tsx,ContextPanel.tsx}`, `PLAN.md` §3.12)**
   - **Smallest fix:** Add a disclosure/drawer from the enforcement badge displaying `EnforcementDescriptor.coverage` (`ToolClass` grid) and `config_sources`, keeping coverage/policy/isolation distinct.
5. **Missing batch approve/deny in Inbox (`app/src/ui/InboxCard.tsx`, `PLAN.md` §3.4)**
   - **Smallest fix:** Add multi-select / "Approve all" / "Deny all" pending-approval controls on `InboxCard.tsx`.

### Minors

1. **`⌘⇧J` previous-item shortcut broken (`app/src/App.tsx:18`, `PLAN.md` §3.4):** `if (k === "j")` does not check `e.shiftKey`, so `⌘⇧J` runs `jumpToNext` instead of `jumpToPrev`. Smallest fix: branch on `e.shiftKey` to jump backward.
2. **Open `M1-13` policy `#[ignore]` tests (`S5`, `S7`, `S8`, `S11`) + `K5`, `P2` (`docs/agents/m1-must-fix.md`):** Decide before gate close whether `S5/S7/S8/S11/K5/P2` ship as known limits or get a pre-gate fix commit.
3. **Live CLI verification gaps (`ClaudeAdapter::DeclineQuestion` + Linux GNOME tray + quiet macOS/Linux §14 re-runs + release `AppIdentitySource::None`):** Document or run live verification scripts where applicable (`ClaudeAdapter::DeclineQuestion` real CLI test, Linux GNOME AppIndicator check).
4. **`scripts/doctor` exits `1` in worker worktrees (`scripts/doctor:103-105`):** Smallest fix: detect linked worktree vs main checkout or flag clearly as worker-expected.
5. **`atrium-core::tests::policy::engine_never_panics_on_garbage` (`119.54s` debug runtime):** Already being fixed in `M1-18`.

---

## 4. For the Gate Reviewers — Where to Look Hardest

1. **`atriumd` entrypoint (`crates/atrium-daemon/src/main.rs:68-86`):** This is the highest-leverage finding — all adapter/daemon tests construct `Options` in Rust tests with mock or explicit adapters, hiding that the compiled `atriumd` executable has `adapters = []`. Fix this before any live dogfooding or M1 sign-off.
2. **Linux §14 memory gate & missing M1-14b artifacts (`docs/spikes/M1-linux-checks.md:183-199`, `docs/agents/status.md:35`):** Decide explicitly on the WebKitGTK `151–152 MiB` private PSS number (`>150 MB` limit) and whether Linux GNOME tray indicator behavior needs a live check on a tray-enabled DE.
3. **Open `M1-13` security items (`S5`, `S7`, `S8`, `S11`) + `M1-18` merge:** Once `M1-18` lands (`CR-12`, `P1`, `K1–K4`, fast fuzz), verify `S5/S7/S8/S11` triage: ANSI-C `$'...'` decoding (`S7`), folder-level recursive search (`S8`), symlink-across-glob (`S11`), and Atrium dir globs (`S5`) are concrete bypass shapes with ignored regression tests already written.
4. **In-flight live provider validation (`M1-08L`, `M1-19`, plus Claude `DeclineQuestion`):** M1-18, M1-19, and M1-08L must land cleanly and pass live CLI checks (`X1–X3` for Codex; `DeclineQuestion` for Claude if tested live) so live behavior matches golden mocks.
5. **UI/spec alignment on missing Settings & Shell actions:** Decide which missing UI surfaces (`CreateRule`, `ExportThread`, `ArchiveThread`/`RenameThread`, `"Always blocked"` + `git config` + same-UID disclosures, enforcement `"details"` drawer, batch approve/deny, `⌘⇧J`) must land before M1 tag vs move into M2 via a formal CR.

---

## 5. Checks Run on `b8ebe26`

All automated Rust, TypeScript, vitest, and protocol checks below ran directly in `/Users/pedro/atrium-wt/M1-20-gate-audit` (no GUI scripts or benches were executed, per brief):

| Check | Outcome | Exact Output Summary |
| --- | --- | --- |
| `./scripts/check-protocol-ts` | Passed (`0`) | `crates/atrium-protocol/bindings/protocol.ts is up to date.` (`specta v2.0.0-rc.26`) |
| `pnpm --dir app test && pnpm --dir app typecheck` | Passed (`0`) | Vitest: `14` files, `264` passed, `0` skipped, `0` failed; `tsc --noEmit`: `0` errors |
| `cargo test -p atrium-core --no-fail-fast` (`task-179` + `-q` summary) | Passed (`0`) | `250` passed, `0` failed, `6` ignored (`unit` 9, `adversarial_fuzz` 2/1i, `adversarial_gate` 9/2i, `adversarial_ledger` 14, `adversarial_ordinary` 16, `adversarial_policy` 15/3i, `adversarial_regression` 57, `attention` 9, `history` 25, `ledger` 20, `policy` 44 (`engine_never_panics_on_garbage` 119.54s), `store` 30) |
| `cargo test --workspace --exclude atrium-core --no-fail-fast -- -q` (`task-284`) | Passed (`0`) | `451` passed, `0` failed, `1` ignored (`atrium-agents` 235: `lib` 35 + `claude_adapter` 38 + `claude_golden` 20 + `claude_normalizer` 61 + `claude_policy_e2e` 5 + `claude_review` 28 + `codex_adapter` 16 + `codex_golden` 7 + `codex_normalizer` 10 + `codex_policy_e2e` 3 + `consistency` 12; `atrium-app` 47: `lib` 45 + `delivery_shapes` 2; `atrium-client` 40: `lib` 29 + `socket` 11; `atrium-daemon` 81: `lib` 15 + 11 int suites; `atrium-host` 35/1i: `lib` 5 + `adversarial_supervise` 7/1i + `lifecycle` 5 + `peer` 3 + `supervise` 15; `atrium-protocol` 13) |
| `./scripts/doctor` | Exited `1` | All tool, `gh`, synced-folder, `git fsck`, conflict-copy, `agy 1.5.9`, and `agy-policy.py` self-test checks passed; failed solely on final check `✗ .agents/hooks.json exists in the main checkout` (expected in worker worktrees). |
