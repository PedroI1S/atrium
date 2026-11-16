# Handoff: M1-28-ansi-c-words

- **Branch:** `agent/hydrogen/M1-28-ansi-c-words` (cut from `a3b2a60`)
- **Rule D-123:** This worker wrote code and tests only, and ran only `cargo fmt --all` and `cargo check -p <crate> --tests` on touched crates. Test suites, clippy, and `app/` `pnpm` checks were **not** run here and must be executed by the verification task (commands below).

---

## 1. What changed and where

### S7: ANSI-C quoted words (`$'…'`)
- **[`crates/atrium-core/src/policy/words.rs`](crates/atrium-core/src/policy/words.rs):**
  - Added `decode_ansi_c(chars: &[char], quote_idx: usize) -> AnsiCDecode` and `is_refused_control`.
  - Decodes all bash `$'…'` escapes: `\a`, `\b`, `\e`, `\E`, `\f`, `\n`, `\r`, `\t`, `\v`, `\\`, `\'`, `\"`, `\?`, `\nnn` (1–3 octal digits), `\xHH` (1–2 hex digits), `\uHHHH` (1–4 hex digits), `\UHHHHHHHH` (1–8 hex digits), and `\cX` (including `\c?` → DEL and `\c\\`).
  - Buffers `\nnn` / `\xHH` bytes to decode multi-byte UTF-8 sequences (e.g., `$'\xc5\xbf'` → `ſ`, preserving APFS `ſ` → `s` folding).
  - Fails closed by setting flags (`closed == false`, `control_char == true`, `bad_escape == true`) on unterminated `$'…'`, NUL bytes (which truncate the decoded word at NUL just as bash does so e.g. `$'.ssh/id_rsa\x00.txt'` still hits `.ssh/id_rsa`), refused control characters (`is_control()` excluding `\n`/`\t`/`\r`, plus U+2028/U+2029), invalid UTF-8 byte sequences, octal `> 0xff`, surrogates/out-of-range Unicode scalars, incomplete `\x`/`\u`/`\U`/`\c`, or unknown `\X` escapes.
- **[`crates/atrium-core/src/policy/shell.rs`](crates/atrium-core/src/policy/shell.rs):**
  - Updated the `$'` branch in the word reader (`lex_depth_uncached`) to call `super::words::decode_ansi_c(&chars, i + 1)`, set `quoted_prefix_empty = false` (so `$'if'` is not a keyword and `$'~'` is not tilde-expanded), append the decoded text to `word`, advance `i`, and push `"unterminated quote"`, `"control character"`, and/or `"malformed escape"` to `out.reasons` when signaled. Keeps `expansion = true` on `$` so ordinary `$'…'` lines keep `dev`'s exact verdict (`Ask` in Home/prefix, `Allow` in Any/FullAccess).
- **[`crates/atrium-core/tests/adversarial_policy.rs`](crates/atrium-core/tests/adversarial_policy.rs):**
  - Removed `#[ignore = "M1-13 finding 4"]` from `finding_4_ansi_c_quoted_words_are_decoded`.
- **Out-of-range observation (not edited, per owned-paths rule):** `scan_paren_n` (~line 210) and `scan_dollar_brace_n` (~line 385) in `shell.rs` are outside the word-reader range (~700–950); their single-quote loops do not yet treat `\'` inside `$'…'` specially when scanning nested `$(…)` / `${…}`.

### Added scope: M1-13 finding 11 (`cd_effect` globs)
- **[`crates/atrium-core/src/policy/words.rs`](crates/atrium-core/src/policy/words.rs):**
  - Updated `cd_effect` plus added `cd_glob_matches`, `normalize_cd_glob`, `cd_comps_match`, `cd_comp_matches`, `leading_class_matches_dot`, `SENSITIVE_CD_DIRS`, and `SENSITIVE_CD_TREES`.
  - When `cd` targets a glob (`*`, `?`, `[`), `cd_effect` normalizes `./`, `//`, and `literal/..` (and `..` after wildcards/`**`) relative to each current `cand`, enforces shell dotfile rules (wildcards do not match a leading `.` unless the pattern component starts with `.` or a positive `[...]` class matching `.`, or uses `**`), matches case-insensitively with `ſ` → `s` folding against credential/Atrium directories and trees, resolves matches through `resolve` (keeping gate and engine list shapes identical), and always returns `Cd::Lost(matches)` (`glob_cd = true`) so the directory remains unknown (`Ask` in Home/prefix, `HardDeny` where the plain path is hard-denied, never `Allow` on sensitive paths).
- **[`crates/atrium-core/tests/adversarial_fuzz.rs`](crates/atrium-core/tests/adversarial_fuzz.rs):**
  - Removed `#[ignore = "M1-13 finding 11"]` from `random_spellings_of_sensitive_paths_are_always_hard_denied`.

### M1-22 minors & nits
- **Login-shell `PATH` discovery ([`crates/atrium-daemon/src/lib.rs`](crates/atrium-daemon/src/lib.rs), [`crates/atrium-daemon/src/main.rs`](crates/atrium-daemon/src/main.rs)):**
  - Added `LOGIN_SHELL_PATH_TIMEOUT` (2 s), `is_fish_shell`, `login_shell_path_command` (`string join : -- $PATH` for `fish`, `"$PATH"` for POSIX shells), `extract_marked_path` (sentinel markers ignoring MOTD/banner noise and preserving non-UTF-8 bytes), `discover_login_shell_path` (runs `$SHELL -ilc ...` with `Stdio::null()` stdin/stderr, bounded 64 KiB stdout, kill/reap on timeout, never fails startup), and `provider_path_with_shell`.
  - `provider_path_with_shell` merges in order: existing `PATH` entries first → `$HOME`-relative `DEFAULT_PATH_EXTRAS` (preserving user-local priority for tests and `~/.local/bin`) → absolute entries from the login shell's `PATH` (for `nvm`/`asdf`/`mise`/custom init) → absolute `DEFAULT_PATH_EXTRAS` (`/opt/homebrew/bin`, `/usr/local/bin`), deduplicated and filtered by `exists`.
  - `main.rs` `extend_provider_path` skips all `PATH` extension when `ATRIUM_NO_PATH_EXTRAS=1`.
- **Daemon piped stderr (`already fixed on dev`):**
  - Verified in `dev`: [`crates/atrium-daemon/tests/binary_adapters.rs`](crates/atrium-daemon/tests/binary_adapters.rs) already writes stderr to `atriumd.stderr` (`Stdio::from(stderr_file)`, lines 34–45), and [`crates/atrium-host/src/lifecycle.rs`](crates/atrium-host/src/lifecycle.rs) `spawn_detached_wait` writes to `atriumd.log` (lines 336–345; only uses `Stdio::piped()` if the log cannot open, killing the child if it stays alive).
- **M1-22 nits in `binary_adapters.rs`:** `binary_adapters.rs` is outside owned paths so left untouched; our new daemon test in [`crates/atrium-daemon/tests/M1-28-followups.rs`](crates/atrium-daemon/tests/M1-28-followups.rs) uses `wait_for_listen` (`UnixStream::connect`).

### M1-24 finding 4 (`client.ts` `receiveFrame`)
- **[`app/src/daemon/client.ts`](app/src/daemon/client.ts):**
  - Updated `receiveFrame` so the first error in a batch is saved and rethrown, while every subsequent error in the batch is logged via `console.error("batch delivery error", err)`.
- **[`app/src/daemon/client.test.ts`](app/src/daemon/client.test.ts):**
  - Added unit test verifying later errors in a batch are logged via `console.error` while the first is rethrown and non-throwing items still apply and acknowledge the final `seq`.

### M1-25 finding 1 (damaged + open message export test)
- **[`crates/atrium-core/tests/M1-28-ansi-c-words.rs`](crates/atrium-core/tests/M1-28-ansi-c-words.rs):**
  - Added `m1_25_export_with_damaged_and_still_open_message`.

### M1-26 minors & nit
- **[`crates/atrium-agents/src/claude/adapter.rs`](crates/atrium-agents/src/claude/adapter.rs) & [`crates/atrium-agents/src/codex/adapter.rs`](crates/atrium-agents/src/codex/adapter.rs):**
  - Replaced sequential per-ticket withdrawal loops in `drain_withdrawals` with a single `tokio::task::JoinSet` bounded by one overall deadline (`self.options.gate_deadline` in Claude, `REDEEM_TIMEOUT` in Codex).
  - In `claude/adapter.rs`, added `Internal::Withdraw(ApprovalTicket)` and `OrphanGuard` inside `spawn_gate` / `spawn_question` so any ticket opened by `gate.check` or `gate.question_ticket` before `clear_requests` (`self.gates.abort_all()`) or `forget_native` (`task.abort()`) aborts the outer task is never leaked, plus `while self.gates.join_next().await.is_some() {}` before `drain_internal_withdrawals` in `finish()` and at the end of `run()`.
  - *(Note: `codex/adapter.rs` `spawn_gate`/`spawn_question` has a similar `abort_inflight` / `tx.send(...).await` pattern; left untouched because owned paths scoped the `clear_requests` fix to `claude/adapter.rs`.)*
- **[`crates/atrium-agents/tests/M1-28-followups.rs`](crates/atrium-agents/tests/M1-28-followups.rs):**
  - Added fake-provider tests for Claude `tool_result`-without-`can_use_tool` withdrawal, Claude and Codex concurrent `drain_withdrawals` under hung gates, and Claude `clear_requests` racing with an in-flight `gate.check`.

### M1-21 confirmation (`fdabc1b8`)
- Confirmed against `fdabc1b8` and current `HEAD`:
  - **Major 1 (`7b`):** fixed in `app/src/m1-demo.ts` (asserts `resume_thread` refusal `code === "invalid"` and `"can't be resumed"`, reported as a passing guard check labelled a fake-agent limit).
  - **Major 2 (`run_cargo_test` & `7c`):** fixed in `app/scripts/m1_demo.py` (parses `(\d+)\s+passed\b` across `test result:` lines and requires `passed >= min_passed`).
  - **Minor 3 (cleanup/SIGTERM):** fixed in `app/scripts/m1_demo.py` (`all_daemon_pids`, 3 s `SIGINT` poll + `SIGKILL` fallback, `SIGTERM` handler, removed redundant unlinks).
  - **Minor 4 (mid-run failure reporting):** fixed in `app/src/m1-demo.ts` and `app/scripts/m1_demo.py` (`m1-demo-check` streamed per check and printed on failure).
  - **Minor 5 (`7a`):** fixed in `app/src/m1-demo.ts` (asserts `"Stopped"` note and `charsAfterStopWait === charsAtStop`).
  - **Minors 6–9 & nits:** all addressed in `fdabc1b8` (`m1_demo.py`, `m1-demo.ts`, `app/src/index.tsx` dynamic `import("./m1-demo")`, `docs/demos/M1-demo.md`). Nothing left open.

---

## 2. Tests added and what each pins

1. **`crates/atrium-core/tests/adversarial_policy.rs` (un-ignored):**
   - `finding_4_ansi_c_quoted_words_are_decoded`: pins `cat $'.ssh/\x69d_rsa'` and `cat $'\056ssh/id_rsa'` → `HardDeny` everywhere.
2. **`crates/atrium-core/tests/adversarial_fuzz.rs` (un-ignored):**
   - `random_spellings_of_sensitive_paths_are_always_hard_denied`: pins 30,000 random spellings including `shape_pick == 8` (`cd <glob>; <reader> <file>`) → `HardDeny` everywhere in `< 40s`.
3. **`crates/atrium-core/tests/M1-28-ansi-c-words.rs`:**
   - `s7_ansi_c_hidden_paths_commands_and_payloads_are_hard_denied`: pins `\xHH`, `\nnn` (1–3 digits & `>0377`), `\uHHHH`, `\UHHHHHHHH`, APFS `ſ` via `\u017f` and `\xc5\xbf`, `\cI`/`\cJ`, `\t`/`\n`/`\'`/`\"`/`\?`, mixed quoting, hidden command names (`cat`, `env`, `rm`, `sh`, `bash`), `-c` payloads, `echo $'...' | sh`, and NUL-truncated paths (`$'.ssh/id_rsa\x00.txt'`).
   - `s7_ordinary_and_fail_closed_ansi_c_keep_safe_verdicts`: pins `printf $'a\tb\n'`, `IFS=$'\n'`, `echo $'a\tb\rc'`, `cat $'notes\t1.txt'`, refused control chars (`\e`, `\E`, `\a`, `\b`, `\f`, `\v`, `\cC`, `\c?`, `\c\\`), unknown/malformed escapes (`\z`, `\x`, `\u`, `\U`, `\ud800`, `\U00110000`, `\xff`, `\c`, `\c\'`), NUL (`\x00`, `\u0000`, `\U00000000`, `\c@`), and unterminated `$'…` → `["Ask", "Ask", "Allow", "Allow"]`.
   - `s7_ansi_c_quoting_is_not_treated_as_keyword_or_tilde`: pins `$'~'/.ssh/id_rsa` and `$'if' true` → `Ask` (not expanded/parsed as keyword).
   - `finding_11_cd_glob_reaching_sensitive_dirs_is_caught`: pins `cd <glob>; cat <file>` across `.s?h`, `.a*s`, `.local/sh*re/atrium`, `.gnupg/priv*te-keys-v1.d`, `.gnupg/sub*`, `~/x/../.s?h//`, `L*brary/Keychains`, `[.]ssh`, `.s?h/../.aws` → `HardDeny`, and `cd .s?h && cat id_rsa.pub` / `cd .c*rgo && cat credentials.toml` / ordinary glob `cd` → `["Ask", "Ask", "Allow", "Allow"]`.
   - `m1_25_export_with_damaged_and_still_open_message`: pins Markdown and JSONL `export_thread_report` with 1 damaged message (`damaged: 1`, fragments withheld, placeholder written) plus 1 still-open streamed message (`deltas` preserved in both formats).
4. **`crates/atrium-daemon/src/lib.rs` (unit tests):**
   - `provider_path_with_shell_merges_and_deduplicates_in_order`: pins ordering (`existing` → `$HOME`-relative defaults → login shell `PATH` → absolute defaults), relative-entry filtering, and deduplication.
   - `fish_shell_detection_and_marker_extraction`: pins `fish` vs POSIX detection, marker extraction past MOTD noise, non-UTF-8 byte preservation, and empty/missing marker handling.
5. **`crates/atrium-daemon/tests/M1-28-followups.rs`:**
   - `discover_login_shell_path_handles_noise_failures_and_timeouts`: pins noisy stdout, failing shell, hanging shell timeout, missing shell, and `None`.
   - `atriumd_merges_login_shell_path_and_skips_under_no_path_extras`: spawns `atriumd` with a fake `zsh` exporting an `nvm` bin dir containing a fake `claude`, verifies `ProbeProviders` discovers `9.8.7` and `ATRIUM_NO_PATH_EXTRAS=1` skips it (`None`).
   - `atriumd_discovers_fish_login_shell_path_and_never_fails_on_bad_shell`: spawns `atriumd` with a fake `fish` checking `string join : -- $PATH` (`9.8.8`), and verifies a failing `$SHELL` does not block `atriumd` startup.
6. **`app/src/daemon/client.test.ts`:**
   - `logs every later error in a batch via console.error and rethrows the first`: pins `console.error` calls for 2nd/3rd errors while rethrowing the 1st and acknowledging the final `seq`.
7. **`crates/atrium-agents/tests/M1-28-followups.rs`:**
   - `claude_tool_result_without_can_use_tool_withdraws_hook_ticket_before_turn_end`: pins immediate withdrawal on `user` `tool_result` before `result` is emitted.
   - `claude_hung_withdrawals_finish_within_one_deadline`: pins 5 hung withdrawals completing within ~1 `gate_deadline` (`< 450 ms` for `150 ms` deadline).
   - `codex_hung_withdrawals_finish_within_one_deadline`: pins 3 hung withdrawals completing within ~1 `REDEEM_TIMEOUT` (`< 16 s` for `10 s` deadline).
   - `claude_clear_requests_withdraws_in_flight_gate_ticket`: races `result` / `Shutdown` (`clear_requests`) against `gate.check` across 8 rounds, pinning `withdrawn == asked` and `open_count == 0`.

---

## 3. Expected verdicts table (deduced — not yet executed under D-123)

*(Settings columns: `[Home, Prefix(cd/cat/printf/echo/ls), Any, FullAccess]`)*

| Line / Case | Expected `dev` | Expected now | Notes |
|---|---|---|---|
| `cat $'.ssh/\x69d_rsa'`, `cat $'\056ssh/id_rsa'` | Ask/Ask/Allow/Allow | Hard/Hard/Hard/Hard | Finding 4 |
| `$'\163h' -c 'rm -rf ~'`, `sh -c $'cat ~/.ssh/\x69d_rsa'`, `echo $'rm -rf ~' \| sh` | Ask/Ask/Allow/Allow (unless raw hit) | Hard/Hard/Hard/Hard | S7 hidden command/payload |
| `cat $'.ssh/id_rsa\x00.txt'` | Ask/Ask/Allow/Allow | Hard/Hard/Hard/Hard | S7 NUL truncation |
| `printf $'a\tb\n'`, `IFS=$'\n'`, `echo $'a\tb\rc'`, `cat $'notes\t1.txt'` | Ask/Ask/Allow/Allow | Ask/Ask/Allow/Allow | Ordinary `$'…'` |
| `printf $'\e[31mhi\e[0m'`, `cat $'\zfoo'`, `cat $'ok\x00'`, `cat $'unterminated` | Ask/Ask/Allow/Allow | Ask/Ask/Allow/Allow | Fail-closed control/bad escapes |
| `cd .s?h; cat id_rsa`, `cd .a*s; cat credentials`, `cd .local/sh*re/atrium; cat atrium.db` | Ask/Ask/Allow/Allow | Hard/Hard/Hard/Hard | Finding 11 (`cd_effect`) |
| `cd .s?h && cat id_rsa.pub`, `cd src*; cat README.md`, `cd *; cat README.md` | Ask/Ask/Allow/Allow | Ask/Ask/Allow/Allow | Ordinary/non-hard-denied glob `cd` |

---

## 4. Tests expected to fail on `dev`'s code

- `atrium-core`:
  - `adversarial_policy::finding_4_ansi_c_quoted_words_are_decoded`
  - `adversarial_fuzz::random_spellings_of_sensitive_paths_are_always_hard_denied`
  - `M1-28-ansi-c-words::s7_ansi_c_hidden_paths_commands_and_payloads_are_hard_denied`
  - `M1-28-ansi-c-words::finding_11_cd_glob_reaching_sensitive_dirs_is_caught`
  - *(Note: `m1_25_export_with_damaged_and_still_open_message` pins existing M1-25 behavior that `dev` already implements, so it should pass on `dev` too.)*
- `atrium-daemon`:
  - `lib::tests::provider_path_with_shell_merges_and_deduplicates_in_order` / `fish_shell_detection_and_marker_extraction` (symbols don't exist on `dev`)
  - `M1-28-followups::discover_login_shell_path_handles_noise_failures_and_timeouts` (symbol doesn't exist on `dev`)
  - `M1-28-followups::atriumd_merges_login_shell_path_and_skips_under_no_path_extras`
  - `M1-28-followups::atriumd_discovers_fish_login_shell_path_and_never_fails_on_bad_shell`
- `app`:
  - `client.test.ts`: `"logs every later error in a batch via console.error and rethrows the first"`
- `atrium-agents`:
  - `M1-28-followups::claude_hung_withdrawals_finish_within_one_deadline` (5 × 150 ms > 450 ms on `dev`)
  - `M1-28-followups::codex_hung_withdrawals_finish_within_one_deadline` (3 × 10 s ≈ 30 s > 16 s on `dev`)
  - `M1-28-followups::claude_clear_requests_withdraws_in_flight_gate_ticket` (races on `dev`)
  - *(Note: `claude_tool_result_without_can_use_tool_withdraws_hook_ticket_before_turn_end` pins existing M1-26 behavior on `dev`.)*

---

## 5. Checks run by this worker (D-123)

- `cargo fmt --all`: exited 0
- `cargo check -p atrium-core --tests`: exited 0 (0 warnings)
- `cargo check -p atrium-daemon --tests`: exited 0 (0 warnings)
- `cargo check -p atrium-agents --tests`: exited 0 (0 warnings)

---

## 6. Exact Definition of Done commands for the verification task

Run in fish from the worktree root:

```fish
# 1. Format, clippy, and full workspace tests
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast

# 2. atrium-core 3x timing check (adversarial_fuzz and policy)
for i in 1 2 3
    echo "=== atrium-core run $i ==="
    cargo test -p atrium-core --test adversarial_fuzz -- --nocapture
    cargo test -p atrium-core --test policy -- --nocapture
    cargo test -p atrium-core
end

# 3. Frontend checks (M1-24)
pnpm --dir app typecheck
pnpm --dir app test
pnpm --dir app build

# 4. Owned-paths diff check
git diff a3b2a60 --stat

# 5. Verify new/un-ignored tests fail on dev's src (restore afterwards)
git checkout a3b2a60 -- crates/atrium-core/src crates/atrium-daemon/src crates/atrium-agents/src app/src/daemon/client.ts
cargo test -p atrium-core --test adversarial_policy finding_4_ansi_c_quoted_words_are_decoded
cargo test -p atrium-core --test M1-28-ansi-c-words
cargo test -p atrium-daemon --test M1-28-followups
cargo test -p atrium-daemon --test adversarial_socket flooding_unread_list_threads_past_8_mib_control_lane_closes_connection
cargo test -p atrium-agents --test M1-28-followups
pnpm --dir app test -- client.test.ts
git checkout HEAD -- crates/atrium-core/src crates/atrium-daemon/src crates/atrium-agents/src app/src/daemon/client.ts
```

---

## 7. Rework round 1

1. **`M1-28-ansi-c-words.rs:173` (`s7_ansi_c_quoting_is_not_treated_as_keyword_or_tilde`):**
   - **Wrong test expectation fixed:** `cat $'~'/.ssh/id_rsa` already returns `Hard(credential-read)` on `dev` because `deny::raw_hit` (`crates/atrium-core/src/policy/deny.rs:733-743`) strips `'"'`, `'\''`, and `'\\'` from the raw line (`cat $~/.ssh/id_rsa`) and matches `"~/.ssh/id_rsa"`. In addition, `crates/atrium-core/tests/adversarial_policy.rs:383` (`SENSITIVE_HARD`) explicitly requires `"cat $'~/.ssh/id_rsa'"` (along with `"cat '~'/.ssh/id_rsa"`, `"cat \"~/.ssh/id_rsa\""`, `"cat ~\"/.ssh/id_rsa\""`) to be `HardDeny` everywhere. Making `cat $'~'/.ssh/id_rsa` return `Ask` would violate the brief's policy rule ("no line more permissive than `dev`") and require modifying `deny.rs` outside owned paths.
   - Updated `s7_ansi_c_quoting_is_not_treated_as_keyword_or_tilde` in [`crates/atrium-core/tests/M1-28-ansi-c-words.rs`](crates/atrium-core/tests/M1-28-ansi-c-words.rs) to assert `HardDeny` for `cat $'~'/.ssh/id_rsa` (matching `dev`), and `["Ask", "Ask", "Allow", "Allow"]` for `cat $'~'/notes.txt`, `cat $'~'root/notes.txt`, `$'if' true`, and `$'FOO'=1 cat notes.txt`.

2. **`adversarial_fuzz.rs:422` (`random_spellings_of_sensitive_paths_are_always_hard_denied`):**
   - **Investigation:** The engine **does** fold case (and APFS `ſ` → `s`) for Atrium's own folders on both macOS and Linux: `deny::lower` (`deny.rs:378-387`) runs unconditionally without `cfg!(target_os)`, `data` (`atrium_data_paths`, `deny.rs:480`) and `protected` (`atrium_protected_paths`, `deny.rs:481`) are lowercased, `hit` (`deny.rs:506`) and `glob_hit` (`deny.rs:595`) lowercase the candidate path, and `crates/atrium-core/tests/adversarial_gate.rs:152` (`"cat .Local/Share/Atrium/atrium.db"`) requires case-folding for Atrium's folders.
   - **Actual cause of iteration 1 failure (`FOO=1 less -- $HOME/.l""ocal/SH?RE//ATRIUM/atrium.db` -> `Ask`):** Even in lowercase (`sh?re/atrium/atrium.db`), `deny::Sensitive::glob_hit` (`deny.rs:594-619`) stops its literal `prefix` at `/h/u/.local` (before `sh?re`) and only checks `probes()` (`deny.rs:586-590`: `{data}` and `{data}/x`), which neither matches `.../atrium.db` nor matches wildcards inside tree directory names. That exact `deny.rs` fix (`probes` / `glob_hit`, `deny.rs:560-815`) belongs to **`M1-30-tree-wildcards`** (not yet merged on `dev`) and is explicitly outside M1-28's owned paths ("If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it").
   - **Action:** Left `deny.rs` untouched, restored `#[ignore = "M1-13 finding 11 (requires M1-30 deny.rs glob_hit)"]` on [`crates/atrium-core/tests/adversarial_fuzz.rs`](crates/atrium-core/tests/adversarial_fuzz.rs#L422) so M1-28 passes verification on `dev` without crossing into `M1-30`'s owned paths, and added `cd $HOME/.l""ocal/SH?RE//ATRIUM; less atrium.db` and `cd .l""ocal/SH?RE/ATRIUM; cat atrium.db` to `finding_11_cd_glob_reaching_sensitive_dirs_is_caught` in [`crates/atrium-core/tests/M1-28-ansi-c-words.rs`](crates/atrium-core/tests/M1-28-ansi-c-words.rs) to pin `cd_effect` case-folding on Atrium's own folders.

3. **`crates/atrium-agents/tests/M1-28-followups.rs`:**
   - **`codex_hung_withdrawals_finish_within_one_deadline`:** Changed `fake-codex-3.py`'s `thread/start` sandbox response from `"readOnly"` to `"workspaceWrite"` to match `PermissionPolicy::Ask` (`"workspace-write"`), matching `examples/codex-fake.rs:170`.
   - **`claude_hung_withdrawals_finish_within_one_deadline` (and Codex shutdown wait):** Both fake scripts (`fake-claude-5.py` and `fake-codex-3.py`) ended with `time.sleep(60)`, which prevented them from seeing stdin EOF when `stop_process` closed stdin, forcing `stop_process` to wait the full `SHUTDOWN_GRACE` (5 s) before `drain_withdrawals` even started. Meanwhile, `shutdown()` (and the Codex shutdown loop) used a `2 s` per-`recv` timeout (`Duration::from_secs(2)`) that returned early before `stop_process(5 s)` or `REDEEM_TIMEOUT(10 s)` + `drain_withdrawals` ran, leaving `withdrawn().len() == 0`. Removed `time.sleep(60)` so the fakes exit immediately on stdin EOF, added `wait_for_approvals` so all `Internal::Gate` tickets are confirmed in `approvals` before `Shutdown`, and changed `shutdown()` and the Codex wait loop to await `SessionEnded` against the remaining overall deadline (`SHUTDOWN_GRACE + 10 s` / `20 s`).

4. **N10 (`crates/atrium-daemon/tests/adversarial_socket.rs` and `crates/atrium-daemon/src/conn.rs`):**
   - **Root causes found:**
     1. In [`crates/atrium-daemon/tests/adversarial_socket.rs`](crates/atrium-daemon/tests/adversarial_socket.rs), Phase 1 had a `RenameThread("phase-1-done")` barrier before reading, but Phase 2 immediately entered `while flooded.read().await.is_some()` right after writing the Phase 2 commands. On Linux, the client drained responses concurrently while the daemon was still running the Phase 2 `ListThreads` queries, keeping `outbox.control_bytes` below `8 MiB` so all 17 responses arrived in 0.6 s.
     2. In [`crates/atrium-daemon/src/conn.rs`](crates/atrium-daemon/src/conn.rs), `write_loop` awaited `tokio::time::timeout(WRITE_STALL, writing)` (`WRITE_STALL = 30 s`) without selecting on `conn.closed()`. When a client didn't read and `push_control` tripped `MAX_CONTROL_BYTES` (`conn.close()`), `write_loop` remained blocked in `writer.writable().await` for up to 30 s instead of closing the socket promptly (which would exceed the test's `SOON = 20 s` timeout once Phase 2 stops draining early).
   - **Fixes:**
     - In [`crates/atrium-daemon/src/conn.rs`](crates/atrium-daemon/src/conn.rs), added `tokio::select! { biased; () = conn.closed() => break, res = tokio::time::timeout(WRITE_STALL, writing) => ... }` in `write_loop` so exceeding `MAX_CONTROL_BYTES` closes the socket immediately.
     - In [`crates/atrium-daemon/tests/adversarial_socket.rs`](crates/atrium-daemon/tests/adversarial_socket.rs), sized `phase2 = (LIMIT + 4 * 1024 * 1024) / size + 2` from the 8 MiB lane limit, appended `RenameThread("phase-2-done")`, and raced `setup.thread_when(..., "phase-2-done")` against non-draining `Unsubscribe` probe writes until the daemon closes `flooded`, only draining `flooded` afterwards to assert `received < sent` and `closed`.

5. **`app/src-tauri` `bridge::tests::batch_bounds_are_respected_for_count_and_bytes`:**
   - `git diff dev -- app/src-tauri/` is empty (no launcher or Rust app code was modified on this branch), and `batch_bounds_are_respected_for_count_and_bytes` uses an in-process `Scripted` daemon without spawning `atriumd`, so none of our changes in `atrium-core`, `atrium-daemon`, `atrium-agents`, or `app/src/daemon/client.ts` can affect it.

6. **Checks run after rework round 1 (D-123):**
   - `cargo fmt --all`: exited 0
   - `cargo check -p atrium-core --tests`: exited 0 (0 warnings)
   - `cargo check -p atrium-agents --tests`: exited 0 (0 warnings)
   - `cargo check -p atrium-daemon --tests`: exited 0 (0 warnings)
