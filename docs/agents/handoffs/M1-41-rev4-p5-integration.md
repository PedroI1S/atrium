# Handoff: M1-41-rev4-p5-integration (P5)

- **Status:** Complete (ready for orchestrator verification and review)
- **Branch:** `agent/neon/M1-41-rev4-p5-integration` (cut from `dev` tip `5c4d154f`)
- **Model:** Gemini 4 Neon (`gemini-4-neon-xhigh`, D-158, D-161)
- **Contract:** `docs/contracts/m1-policy-rev4.md` (§1, §2, §8, §9, §10, §11 items 1–15 confirmed in D-157, §12, §13 step 4)
- **Decisions applied:** D-133, D-135, D-138, D-139, D-141, D-142, D-144–D-150, D-153, D-154, D-156, D-157
- **Behaviour change:** **Yes — first rev 4 task that changes live behaviour.** Policy rev 4 now decides every call through `SessionGate`, `DefaultPolicy`, and `parse_command`; `dev`'s evaluator has been removed. No protocol schema or adapter/daemon call site changed beyond populating P4's existing `Binding::unread` / `ApprovalRequested.unread` field.

---

## 1. Commits on branch (base `5c4d154f`)

| Commit | Summary |
|---|---|
| `e99896e9` | Implement policy rev 4 evaluator (`v4/evaluate.rs`, `v4/evaluate/best_effort.rs`, `v4/evaluate/tests.rs`) |
| `27266513` | Wire policy rev 4 into `SessionGate`, `DefaultPolicy`, and `parse_command` |
| `0bc0d405` | Remove retired `dev` policy evaluator (`shell.rs`, `words.rs`, legacy `deny.rs`/`engine.rs`/`gate.rs`) |
| `01f5a388` | Add policy rev 4 §10 normative example table test suite (`tests/policy_rev4_section10.rs`) |
| `a9fd77dd` | Add policy rev 4 §13 boundary property tests (`tests/policy_rev4_properties.rs`) |
| `50aa78a4` | Migrate `crates/atrium-core/tests/policy.rs` to policy rev 4 (and fix MCP/kv-alias evaluation) |
| `fe9d3754` | Wire `build_units_with_hook` / `UnitReasonSink`, MCP `raw_text_checks`, and budget guards in `v4/evaluate.rs` |
| `987959c7` | Point `app/src/ui/security.test.ts` at live `v4/paths/sensitive.rs`, gate `deny.rs` to `#[cfg(test)]`, and precheck JSON size caps in `SessionGate` |
| `9dc64108` | Refine best-effort pass: `grep -r/-f`, stdin-as-code, `cd` pool/compounds/`env -C`, glob probes, and search-glob widening |
| `9c44df93` | Port compound-frame `cwd` tracking and `env -C` unioning to best-effort pass |
| `5a743bed` | Migrate `adversarial_policy.rs`, `adversarial_ordinary.rs`, and `adversarial_ledger.rs` to rev 4 |
| `0cfbba5c` | Add recursive-search `ssh_linked_dirs` ancestor check and wrapper option alt-readings in best-effort pass |
| `10f23838` | Migrate `adversarial_regression.rs` to policy rev 4 |
| `451c0020` | Migrate `adversarial_m1_18.rs`, `adversarial_gate.rs`, and `adversarial_fuzz.rs` to rev 4 and refine best-effort search-glob/`env -S` checks |
| `a203687a` | Give `build_protected_set` its own `ComputeBudget` alongside its own `FsCaps` in `SessionGate` |
| *(this commit)* | Add handoff `docs/agents/handoffs/M1-41-rev4-p5-integration.md` |

---

## 2. What was implemented, by Goal item

### Goal 1 — `crates/atrium-core/src/policy/v4/evaluate.rs` (+ `v4/evaluate/best_effort.rs`, `v4/evaluate/tests.rs`)
Implemented all three P0 stubs matching their frozen signatures in `policy/model.rs` and `policy/v4/evaluate.rs`:

- **`run_best_effort` (§2, §8):**
  - Runs budgeted `dev`-equivalent hard-deny checks on raw command text (`raw_text_checks`: `raw_hit` and `raw_pipe_to_shell`) and on every unit produced by P1's tolerant lexer (`BestEffortRecord::units`, including derived `-c`/`eval`/`env -S`/`script -c`/`flock -c`/`find -exec`/`$(...)`/heredoc/piped `echo|sh` units).
  - Covers all 7 `HARD_DENY_DEFAULTS` families: privilege escalation, disk wipe, force-push to protected branches, recursive delete outside `cwd`, pipe-to-shell, credential/browser paths and glob probes (`Sensitive::glob_hit`, `search_glob_hit`, `ssh_linked_dirs` ancestors), and Atrium self-protection paths.
  - Preserves compound-frame lexical `cd` tracking, `env -C` directory unioning, wrapper option alt-readings, `grep -r`/`-f`, stdin-as-code checks, and N11 bounded brace expansion (`v4::lex::brace_expand_budgeted`).
  - A hit produces `PartOutcome::HardDenyHit` with `HitSource::BestEffort`; a miss produces nothing. Pre-exhaustion hits stand, and exhaustion never creates a hit (§8).
- **`decide` (§1, §2):**
  - Pure 6-step decision procedure in strict contract order:
    1. Any `PartOutcome::TooLarge(_)` → `PolicyVerdict::Deny { reason: "too large to check", rule: None }`.
    2. Any `PartOutcome::HardDenyHit(_)` (model or best-effort) → `PolicyVerdict::HardDeny { id }` (deterministic `HARD_DENY_DEFAULTS` list precedence).
    3. Winning Deny rule (`rule_coverage.deny`) → `PolicyVerdict::Deny { reason, rule }`.
    4. `outcome.multiply_linked_file` → `PolicyVerdict::Ask` in every setting.
    5. Any `PartOutcome::Exhausted(_)` → `PolicyVerdict::Ask` in every setting.
    6. §2 authorization table: unscoped `Any` Allow and `Preset::FullAccess` authorize `Clear`, `StoreHit`, and `Unread`; `Home` / `Careful` / `Normal` / `Trusted` require every `StoreHit` to be covered by an Allow rule, every `Unread(Program)` to be covered by a rule pinning that program, zero plain `Unread` parts, and either every slot covered by an Allow rule or (for non-`Careful`) `home_readonly_all`.
  - Over-approximation never creates a hard-deny hit (except the §1 store-probe exception in P2).
- **`evaluate_call` (§1, §2, §8, §9, D-150):**
  - Enforces structural caps **before** charging budgets (`MAX_BYTES`, `MAX_UNITS`, `MAX_USES`, `MAX_PATHS`, JSON string/depth caps for `Mcp`/`Unknown`).
  - Orchestrates P1 (`build_units_with_hook` with P3's `CodeRunnerHook` + `UnitReasonSink`, plus `lex_best_effort`), P3 (`classify_unit`, `check_git_selection_into`), P2 (resolver-backed `resolve_command_targets` and `resolve_subject_paths`, applying D-147), P5 best-effort pass (`run_best_effort_into`, `raw_text_checks`, `check_search_glob_best_effort`), P3 rules (`evaluate_rules` with full `RuleEvalContext::outcomes` per CR-P3-3 / D-153, and `remember_matcher`), and `decide`.
  - Handles every `ToolSubject` variant (`Command`, `Paths`, `Fetch`, `Mcp`, `None`, `Unknown`) and derives P4's `Option<UnreadNotice>` via `unread_notice_for_outcome` (`Exhausted` beats `Unread`; `None` when neither occurred).
- **Unit tests (`v4/evaluate/tests.rs`):** covers §1 step ordering, §2 table across presets and rules, compute/allocation budget sweeps verifying exhaustion never creates a hit and preserves prior hits, `ToolSubject::Unknown`/`Mcp`/`Fetch`/`None`, `UnreadNotice` mapping, and §13 boundary properties.

### Goal 2 — `SessionGate`, `DefaultPolicy`, and `parse_command` wiring
- **`crates/atrium-core/src/policy/gate.rs`:**
  - `SessionGate::evaluate_sync` prechecks structural size limits (`MAX_BYTES`, `MAX_USES`, JSON caps for `Mcp`/`Unknown`), builds `ProtectedSet` via `v4::paths::sensitive::build_protected_set` using `v4::paths::FsResolver` with **its own `FsCaps` and `ComputeBudget`** (so builder discovery/protected-root walks stay bounded and never starve the call's caps or compute budget), then runs `v4::evaluate::evaluate_call(&input, rules, &protected, &resolver)`.
  - `ProtectedSet` is rebuilt per `check`/`redeem` rather than cached across calls so symlink/file mutations between `check` and `redeem` (and between consecutive checks in the same session) can never read stale identities.
  - Populates P4's `Binding::unread` from `unread_notice_for_outcome(&eval.outcome)` (clearing `remember_matcher` whenever `unread.is_some()`), propagating `UnreadNotice::{Unread, Exhausted}` into `ApprovalTicket` and `AgentEvent::ApprovalRequested` with no adapter, daemon, or protocol changes.
  - Keeps `SessionGate`'s ledger open/redeem/withdraw lifecycle, `redeem` re-evaluation, and public `fs_resolve` helper intact.
- **`crates/atrium-core/src/policy/engine.rs`:**
  - `DefaultPolicy::evaluate` runs `v4::evaluate::evaluate_call` over a pure `VirtualResolver` (zero filesystem access, contract §5).
- **`crates/atrium-core/src/policy/mod.rs`:**
  - `parse_command` delegates to `v4::lex::lex_exact`.

### Goal 3 — Removal of `dev`'s evaluator: deletions vs kept items

| Item / File | Status | Reason |
|---|---|---|
| `crates/atrium-core/src/policy/shell.rs` | **Deleted** | Replaced by P1 `v4/lex.rs`, `v4/lex/best_effort.rs`, and `v4/units.rs`. |
| `crates/atrium-core/src/policy/words.rs` | **Deleted** | Replaced by P1 `v4/lex/best_effort.rs::brace_expand_budgeted`, P2 `v4/paths/*`, and P3 `v4/tools.rs`. |
| `crates/atrium-core/src/policy/deny.rs` (live evaluator) | **Deleted** (~1,200 lines removed) | Replaced by P2 `v4/paths/sensitive.rs` and P5 `v4/evaluate/best_effort.rs`. |
| `crates/atrium-core/src/policy/engine.rs` (live evaluator) | **Deleted** (~1,500 lines removed) | Replaced by P3 `v4/tools.rs`/`v4/rules.rs` and P5 `v4/evaluate.rs`. Only `DefaultPolicy` + `VirtualResolver` remain at runtime. |
| `crates/atrium-core/src/policy/gate.rs` (`dev` fs helpers: `reaches_linked_ssh_dir`, `reaches_tree_through_links`, `reaches_multiply_linked`, `expand_glob`, `alias_fix`, `tree_has_multiply_linked`) | **Deleted** | Replaced by P2 `FsResolver`, `build_protected_set`, `resolve_command_targets`, and `resolve_subject_paths`. |
| Best-effort `dev` hard-deny matching (`raw_hit`, `raw_pipe_to_shell`, `glob_hit`, `search_glob_hit`, privilege escalation, disk wipe, force push, recursive delete, compound-frame `cd` / `env -C`) | **Kept on purpose** (ported budgeted to `v4/evaluate/best_effort.rs`) | Required by contract §2 for best-effort hard-deny detection on `FullAccess`, `Any`, and `Unread` calls. |
| `apply_rules` precedence (`Thread > Home > Global`, Deny before Allow at same rank, call-level suppression) | **Kept on purpose** (in P3 `v4/rules.rs`) | Required by contract §7 and §10 row 26. |
| `Sensitive` (`SharedSensitive` value-keyed thread-local cache, listed folds, CR-12/CR-17 stores, probes) | **Kept on purpose** (in P2 `v4/paths/sensitive.rs`) | Required by contract §5 and §12 (M1-31 N11). |
| `app/src/ui/security.test.ts` | **Updated** | Source path changed from retired `crates/atrium-core/src/policy/deny.rs` to live `crates/atrium-core/src/policy/v4/paths/sensitive.rs` (`ASK_FILES`, `ASK_TREES`, `ASK_NAME_PREFIXES`, already containing the 4 CR-17 stores). |
| `atrium_protocol::policy::HOME_READONLY_COMMANDS` | **Kept** (see CR-P5-2) | Still referenced by frozen P0 `policy/tools.rs` tests, P3 `v4/tools/tests.rs`, and `crates/atrium-protocol/src/policy.rs` parity tests. Not used by live evaluation. |
| `#[cfg(test)]` parity constants in `policy/deny.rs` (`MAX_BYTES`, `DOWNLOADERS`, `PRIV`), `policy/mod.rs` (`mod shell`, `mod words`), and `policy/engine.rs` (`RG_*`, `GREP_*`, `GIT_CRED_*`) | **Kept under `#[cfg(test)]` only** (zero runtime code; see CR-P5-2) | Required strictly so frozen P0 unit tests in `policy/limits.rs` and `policy/tools.rs` compile and pass without modifying frozen P0 files. |
| `pub fn fs_resolve` in `crates/atrium-core/src/policy/gate.rs` | **Kept** | Public `atrium_core::policy::fs_resolve` API referenced by tests. |

---

## 3. P1–P3 wiring fixes & Change Requests against frozen interfaces

### P1–P3 wiring fix (1 line total)
- **`crates/atrium-core/src/policy/v4/tools.rs:567` (`classify_cd`):**
  - Added `effects.tool_name = Some("cd");`.
  - **Why:** `cd` is intentionally omitted from `TOOLS` (`UNKNOWN_BY_DESIGN`), so `classify_cd` left `effects.tool_name = None`. P2's `unit_is_possible_writer` treats any unit with `e.tool_name.is_none()` as a possible writer, which caused every unit following `cd <dir> && ...` to be marked `Unread(AfterPossibleWriter)`. Setting `tool_name = Some("cd")` while keeping `writes = redirect_writes` lets P2 track `cd` cleanly without misclassifying it as a writer.

### Change Requests against frozen interfaces (no frozen file was edited)

1. **CR-P5-1 (`crates/atrium-core/src/policy/v4/tools.rs` — `HomeReadOnly` `operand_access`):**
   - **Problem:** `classify_home_readonly` leaves `operand_access = AccessMode::ReadWrite` on `HomeReadOnly` tools (`cat`, `ls`, `head`, `git diff`, etc.). In P2, any non-`Read` access to a write-only protected path (`~/.ssh/config`, `~/.ssh/authorized_keys`, `~/.config/atrium/config.toml`, `/Applications/Atrium.app`) produces a `HardDenyHit`, whereas `cat ~/.ssh/config`, `ls ~/.config/atrium/config.toml`, and `git diff --no-index ~/.ssh/config x` are read-only inspections that must not hard-deny.
   - **Workaround in P5:** `v4/evaluate.rs` (`home_ro_tool_reads_only_without_redirects`) narrows non-redirect operand access to `AccessMode::Read` on pure read-only `HomeReadOnly` units before calling `resolve_command_targets`.
   - **Follow-up:** Set `operand_access = AccessMode::Read` directly inside `classify_home_readonly` when eligible (keeping redirect writes `Write`) and remove the P5 workaround.

2. **CR-P5-2 (Retire `dev` parity tests, `#[cfg(test)]` shims, and `HOME_READONLY_COMMANDS`):**
   - **Problem:** Frozen P0 files (`policy/limits.rs`, `policy/tools.rs`), P3 tests (`v4/tools/tests.rs`), and `crates/atrium-protocol/src/policy.rs` still contain parity unit tests asserting equality against `dev`'s `deny::{MAX_BYTES, DOWNLOADERS, PRIV}`, `shell::{MAX_UNITS, MAX_DEPTH, RESERVED, SHELLS, COMMAND_RUNNERS}`, `words::{MAX_USES, MAX_PATHS, WRITERS, READERS}`, `engine::{RG_*, GREP_*, GIT_CRED_*}`, and `atrium_protocol::policy::HOME_READONLY_COMMANDS`.
   - **Workaround in P5:** Kept `HOME_READONLY_COMMANDS` untouched in `atrium-protocol` and kept minimal `#[cfg(test)]` parity constants in `policy/deny.rs` (9 lines), `policy/mod.rs`, and `policy/engine.rs` (zero runtime code).
   - **Follow-up:** Once P0's freeze lifts after P5 merges, delete those `dev`-parity assertions and remove `policy/deny.rs`, the `#[cfg(test)]` constant shims in `mod.rs`/`engine.rs`, and `HOME_READONLY_COMMANDS` (regenerating `bindings/protocol.ts` if affected).

---

## 4. Goal 4 — Test suite migration & verdict changes

### New rev 4 test suites (contract §10 & §13)
- **`crates/atrium-core/tests/policy_rev4_section10.rs`:** tests all 26 normative rows of contract §10 plus `ToolSubject::Unknown` end-to-end through `SessionGate` (verifying `GateDecision`, `UnreadNotice`, `rememberable`, and fixtures on real temp disks).
- **`crates/atrium-core/tests/policy_rev4_properties.rs`** (+ unit property sweeps in `v4/evaluate/tests.rs`): independent generators per construct outside §3/§4 verifying:
  1. in Home and under prefix rules, a line containing an outside-model construct is never `Allow`;
  2. a fixture-planted hard-deny target touched by a modelled part is `HardDeny` in every setting (`Careful`, `Home`, `Normal`, `Trusted`, `FullAccess`, and under `Any` rules);
  3. compute/allocation exhaustion with no earlier decision is `Ask` in every setting;
  4. exhaustion never creates a hit.

### Formerly `#[ignore]`d tests (0 `#[ignore]` left anywhere in `crates/`)
`git grep '#[ignore' 5c4d154f -- crates/` found 6 `#[ignore]`d tests at P5's base `5c4d154f` (the 7th from M1-13 had already been un-ignored before `5c4d154f`). Every one of the 6 is now un-ignored and active:

| Test | Previous status | Rev 4 disposition & citation |
|---|---|---|
| `adversarial_policy.rs::finding_4_ansi_c_quoted_words_are_decoded` | `#[ignore = "M1-13 finding 4"]` | **Un-ignored; updated expectation** (`Ask / Ask / Allow / Allow` for `$'...'` words): §3, §10 row 6, §11 items 1–2, §12 (M1-28 `$'...'` reader dropped). |
| `adversarial_policy.rs::finding_5_a_recursive_search_of_a_folder_with_a_credential_file_is_blocked` | `#[ignore = "M1-13 finding 5"]` | **Un-ignored; updated fixture:** asserts S8 `HardDeny` on disk with planted credential files through `Tmp` + `SessionGate` (§5 S8, §10 row 16, §11 item 12), and `Ask` (`StoreHit`) in virtual `Env::home()`. |
| `adversarial_policy.rs::finding_6_stores_outside_the_cr_12_list_ask_in_home` | `#[ignore = "M1-13 finding 6"]` | **Un-ignored verbatim** (CR-17 4 stores ask in Home; D-111, §11). |
| `adversarial_gate.rs::finding_10_a_glob_that_reaches_a_credential_through_a_link_is_blocked` | `#[ignore = "M1-13 finding 10"]` | **Un-ignored verbatim** (§5, §10 row 8). |
| `adversarial_gate.rs::finding_11_a_glob_inside_atriums_folder_names_is_blocked` | `#[ignore = "M1-13 finding 11"]` | **Un-ignored verbatim** (§5 + best-effort protected-directory prefix probes). |
| `adversarial_fuzz.rs::random_spellings_of_sensitive_paths_are_always_hard_denied` | `#[ignore = "M1-13 finding 11"]` | **Un-ignored; updated `cd` shape:** keeps wildcards on all 9 direct path shapes while using a non-glob directory spelling for `cd {d}; {reader} {f}` (§12 M1-28 dropped globbed `cd` candidates). |

### Migrated expectations in existing suites (every changed assertion cites §10, §11, §12, or a decision)

| Suite & test | Change | Citation |
|---|---|---|
| `policy.rs::hard_deny_table` | `rg -L token sub`: `Ask` → `Allow` | §10 row 15, §11 item 15, D-139 |
| `policy.rs::compound_command_allowed_only_if_every_segment_is` | `cargo fmt && cargo test`, `cargo fmt; cargo test \| cat`, `cargo fmt && git status`: `Allow` → `Ask` (possible writer taints following/piped units); added non-writing compound `Allow` checks (`cat a && git status`, `ls; cat a \| head -5`) | §5, §11 items 6 & 10 |
| `policy.rs::parse_fills_assignments_and_redirects` | Leading `A=1 B='x y'`, `\ `, `&>`/`>&` now `ParsedCommand::Unparseable` (outside §3); tested modeled `cmd arg ...` and `e\*f` | §3, §11 item 2 |
| `policy.rs::path_rules_resolve_symlinks_and_relative_paths` | `VAR=link cat $VAR`: `Deny` → `Ask` with `UnreadNotice::Unread` and `!rememberable` | §2, §3, §11 items 1–2 |
| `policy.rs::engine_never_panics_on_garbage` & `multibyte_flags_and_oversized_calls_through_the_gate` | `{a,b}` ×2000 / ×5000 (under `MAX_BYTES`/`MAX_UNITS`): `Deny` → `Ask` (N11 bounded brace expansion stops at 256 without `TooLarge`); `MAX_UNITS` still `Deny`; unquoted non-ASCII `cat -é`, `ls -é`, `cat -日x`: `Allow` → `Ask` | §3, §8, §10 row 10, §11 item 2, §12 (N11) |
| `policy.rs::review_round1_table` | `ls \r`, `cat -é`, `ls -é日本`: `Allow` → `Ask` (§3); `grep -R`, `grep -rR`, `grep --dereference-recursive`: `Ask` → `Allow` (§10 row 15, §11 item 15, D-139; `--deref` stays `Ask`) | §3, §10 rows 10 & 15, §11 items 2 & 15, D-139 |
| `policy.rs::macos_path_aliases_and_hard_links_reach_nothing` | `cat /.vol/...`, `Read /.vol/...`, `Read /.vol`: `Deny` → `Ask` with `UnreadNotice::Unread` and `!rememberable` | §3, §5, §11 item 14 |
| `policy.rs::review_round2_table`, `review_round3_table`, `review_round4_table` | `ssh -F ~/.ssh/config`, `ssh-keygen -lf ~/.ssh/authorized_keys`, `open /Applications/Atrium.app`, `codesign -dv /Applications/Atrium.app`, `sort ~/.config/atrium/config.toml`, `find ~/.config/atrium -name x`, `awk '{print}' ~/.config/atrium/config.toml`: `Ask` → `HardDeny` (`UNKNOWN_BY_DESIGN` / `CodeRunner` keep conservative `ReadWrite` operand access; `git diff --no-index ~/.ssh/config x` and executing `/Applications/Atrium.app/...` unchanged) | D-141, D-156, D-157 |
| `adversarial_policy.rs::leftover_hard_linked_files_ask` | `proj/innocent.txt` & `proj/data.db` (hard-linked to protected files): `Ask` → `Deny`; ordinary hard link stays `Ask` | D-145 item 2, §1 step 4, §5 |
| `adversarial_policy.rs::ordinary_commands_are_not_hard_denied` | `cat ~/.ѕsh/id_rsa` (Cyrillic `ѕ`): `Allow` → `Ask` (unquoted non-ASCII is `Unread`) | §3, §10 row 10, §11 item 2 |
| `adversarial_ordinary.rs::ordinary_lines_after_a_working_cd` & `ordinary_nested_cds` | Virtual `Env::home()` prefix column → `"Ask"` (`cd` fails closed when `resolver.stat` returns `NotFound`), plus added real-disk `Tmp` + `SessionGate` assertions verifying `GateDecision::Allow` | §5 (`cd` resolver check) |
| `adversarial_ordinary.rs::round_8_major_6` & `round_8_fix_rg_real_follow_flag_keeps_its_verdict` | `rg -L` / `--follow` / `-iL` on clean `safe_sub`: `Ask` → `Allow` in Home | §10 row 15, §11 item 15, D-139 |
| `adversarial_ledger.rs` | `"rm -rf build; ls"`, `"rm -rf build \| cat"`, `"make; make"`, `"make && make clean"` moved to `Ask` (`not_same`/`no`) because possible writers taint following/piped units | §5, §10 row 21, §11 items 6 & 10 |
| `adversarial_regression.rs::finding_2_and_chains_follow_only_success` | Virtual `Env::home()` `cd` prefix rows → `Ask` | §5 (`cd` resolver check) |
| `adversarial_regression.rs::round_5_blocker_1`, `round_6_items_1_and_2`, `round_7_item_3`, `round_8_blocker_2` | Split modeled `Search` tools (`rg`/`grep`) + direct `vault/work` operands (stay `Deny`) from D-141 non-`Search` tools (`ag`, `ack`, `find`, `tar`, `zip`, `cp -r`, `rsync`) on ancestor dirs (`Ask` in Home); `ls proj/*` moved to `Deny` in `round_8_blocker_2` because §5 glob expansion resolves `proj/deploy_key -> ~/.ssh/id_rsa` (while `ls proj/vault_link` stays non-`Deny`) | D-141, §5, §6, §11 items 1 & 10 |
| `adversarial_regression.rs::round_7_item_1` | `${DIR}`/`$DIR`/`$(...)` ask in Home and allow under `Any`/`FullAccess`; all 26 brace-range cases stay `Deny` | §3, §11 item 1 |
| `adversarial_m1_18.rs` | `cat ~/.zſh_history` → `DEV_ASK` (§3, §10 row 10); `git` pathspec magic `:(top)`/`:/`/`:!` → `ASKS_HOME_NO_PREFIX` (§6); `gate_rows()` updated for D-141 non-`Search` tools (`find`/`ag`/`ack`/`cp`/`tar`/`rsync`/`zip`/`scp`/`7z`) & unmodeled `--deref`, `big_dir/*/*`, `{1..65}/*`, `${DIR}/*` → `dev_glob`, `rg -L`/`--follow x clean` → `Allow` in all settings (§10 row 15, D-139), `find -L clean -name '*.rs'` → `["Ask", "Allow", "Allow"]` (§10 row 9, D-141); `fix2_k1` unbounded `env`, `fix3_finding2` (keeping `-S` cred/sudo & `-C '../.ssh'` hits as `H_CR`/`H_PE`), `fix4_finding2`, `fix5_finding2` → `DEV_ASK` (§4, §11 items 1–2, §12 K9 dropped) | §2, §3, §4, §6, §10 rows 9, 10, 15, §11 items 1, 2, 10, 11, 15, §12 (K9), D-139, D-141 |
| `adversarial_gate.rs` | Un-ignored findings 10 & 11; **zero** other verdict changes | §5, §10 row 8 |
| `adversarial_fuzz.rs` | Un-ignored finding 11 with non-glob `cd` spelling; **zero** other changes | §12 (M1-28) |
| `crates/atrium-agents/tests/{claude_policy_e2e,codex_policy_e2e,consistency,gate_withdraw,M1-28-followups}.rs` and `crates/atrium-core/tests/ledger.rs` | Audited line-by-line; **zero** verdict changes required | — |

---

## 5. Cross-platform & verification notes

- **macOS & Linux portability:** All P5 `#[cfg(unix)]` / `#[cfg(not(unix))]` helpers compile cleanly on both macOS and Linux; no platform-specific syscalls were added in P5 (`FsResolver` already charges identically on macOS and Linux from P2).
- **Worker verification run (D-123 allowlist only):**
  - `cargo fmt --all` — clean
  - `cargo check --workspace --tests` — clean (0 errors, 0 warnings)
  - Per D-123, `cargo test`, `cargo clippy`, and `pnpm` were **not** run by the worker.

### Verification commands for the orchestrator (fish-compatible)

```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/check-protocol-ts
cargo test --workspace --no-fail-fast
# Record policy and fuzz timings against dev's baseline (Definition of Done):
cargo test -p atrium-core --test policy -- --nocapture
cargo test -p atrium-core --test policy_rev4_section10 -- --nocapture
cargo test -p atrium-core --test policy_rev4_properties -- --nocapture
cargo test -p atrium-core --test adversarial_fuzz -- --nocapture
cargo test -p atrium-core --test adversarial_policy --test adversarial_ordinary --test adversarial_ledger --test adversarial_regression --test adversarial_m1_18 --test adversarial_gate -- --nocapture
pnpm --dir app typecheck
pnpm --dir app test
pnpm --dir app build
python3 app/scripts/m1_demo.py
```

- **Linux verification:** owed for the gate on rodrigo's machine (`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast`).
