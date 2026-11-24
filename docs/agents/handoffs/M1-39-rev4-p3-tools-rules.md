# Handoff: M1-39-rev4-p3-tools-rules (P3)

- **Branch:** `agent/neon/M1-39-rev4-p3-tools-rules` (branched from P0 approved tip `a5b9b3b`)
- **Model:** Gemini 4 Neon (D-134)
- **Contract:** `docs/contracts/m1-policy-rev4.md` (§6, §7, §8, §10, §11 items 4–5 & 8–10 & 15, §12), annex `docs/contracts/m1-policy-rev4-tools.md`, and decisions D-133..D-151 (notably D-141, D-142, D-144..D-149)
- **Behaviour change:**
  - `atrium-core`: `v4::tools` and `v4::rules` live alongside `dev`'s evaluator under `policy/v4/` and are not wired into `SessionGate` until P5.
  - `atrium-daemon` & `app`: `CreateRule` now validates drafts under rev 4 §7 (returning `ErrorCode::Invalid` with an explanation), `ListRules` keeps stored invalid rules listed with an optional `invalid_reason` field (omitted on valid rules for 100% wire compatibility), and `app/src/chat/rule-form.ts` mirrors rev 4 §7 validation and marks stored invalid rules (§11 items 4–5).

---

## 1. What was implemented, by contract section

### Owned files written / modified
- `crates/atrium-core/src/policy/v4/tools.rs` — `classify_unit`, `check_git_selection`, shared §8 pre-charge helpers, CR-19 folding helpers, and `cd` cwd-tracker handoff (`cd_target_operand`).
- `crates/atrium-core/src/policy/v4/tools/sed.rs` — linear, budgeted `sed` script parser (`inspect_sed_script`).
- `crates/atrium-core/src/policy/v4/tools/git.rs` — CR-12 & CR-17 Git name-selection, credential-ancestor, pathspec magic/exclusion, `-L` line-range, and wildcard-pathspec checks (`check_git_selection_impl`).
- `crates/atrium-core/src/policy/v4/tools/tests.rs` — comprehensive unit tests for §6, all 6 D-141 row kinds, `HOME_READONLY_COMMANDS` parity, `OUTSIDE_MODEL`, `UNKNOWN_BY_DESIGN`, `cd`, CR-12/CR-17 Git selection, and §8 budgets.
- `crates/atrium-core/src/policy/v4/rules.rs` — `validate_rule_matcher`, `format_validation_error`, `evaluate_rules`, and `remember_matcher`.
- `crates/atrium-core/src/policy/v4/rules/tests.rs` — comprehensive unit tests for §7 rule validation, precedence, Deny on best-effort, Allow per outcome, pinned programs, canonical path scopes, stored invalid rules, `remember_matcher`, and §8 budgets.
- `crates/atrium-protocol/src/policy.rs` — added `#[cfg(test)] mod tests` verifying `HOME_READONLY_COMMANDS` invariants and 1-to-1 rev 4 §6 parity (keeping `HOME_READONLY_COMMANDS` intact for `dev` until P5).
- `crates/atrium-daemon/src/commands.rs` — rev 4 §7 validation in `create_rule` (`ErrorCode::Invalid` with plain reason) and stored-rule validity annotation in `ListRules` via `ListedRule` (`invalid_reason` omitted when valid).
- `crates/atrium-daemon/tests/create_rule.rs` — end-to-end daemon socket tests for `CreateRule`, `ListRules`, and `DeleteRule` under §7 / §11 items 4–5.
- `app/src/chat/rule-form.ts` & `app/src/chat/rule-form.test.ts` — aligned `commandRisk` / `validateRule` with rev 4 §7, added `ListedRule` / `StoredRuleValidity` / `checkStoredRule` / `markStoredRules` for stored invalid rules (§11 item 5), and updated/added unit tests.

### Contract §6: Known tools (D-141 six row kinds) & CR-12/CR-17 Git checks
- **CR-19 command-word folding (`v4/tools.rs`):** basename after last `/`, Unicode `str::to_lowercase()`, optional `ſ` (U+017F) → `s` and `K` (U+212A) → `k`, with **no** Unicode normalization. (Implemented locally because P1's `v4::lex::fold_command_word` is a `todo!()` stub on this branch.)
- **Kind 1 — `HomeReadOnly`:** matches Home eligibility on the command word **as written** (`cat` qualifies; `/bin/cat` and `CAT` do not), enforces `m1.md` §9 simple-command guards (no wrappers, no unquoted globs, only harmless redirects), keeps every non-command word as `OperandRole::Path` / `AccessMode::ReadWrite`, and checks `forbidden` (via `tools::is_forbidden_flag`) and `only` solely to remove Home eligibility (setting `writes = true`, never emitting `Unread`). Verified 1-to-1 against all 24 entries in `atrium_protocol::policy::HOME_READONLY_COMMANDS`.
- **Kind 2 — `Search` (`rg` and `grep` family):** applies `tools.rs` flag tables, assigns `OperandRole::Pattern` to positional `[0]` unless `-e`/`-f`/`--files`/`--type-list` supplies the pattern (§11 item 9), applies ordered left-to-right toggles (`recursive`, `links.at_root`, `links.below_root`, `filter_case_insensitive`), enforces `FilterCase::{AlwaysSensitive, AlwaysInsensitive, FollowGlobalToggle}` (final global toggle governs every `FollowGlobalToggle` filter regardless of position), emits `UnreadReason::UnsupportedFilterForm` for multiple/negated/unmodeled filters, emits `UnreadReason::UnsupportedFlagEffect` for listed `FlagUnread::FlagEffect` flags (`grep -d`) and attached path-valued flags (`rg --file=X`, `grep -fX`, since P2 resolves whole words), and emits `UnreadReason::UnknownFlag` on unlisted flags while retaining conservative `Path`/`ReadWrite` on all operands. Respects `ALLOW_HOME_FOLLOWED_SEARCH` (`rg -L`, `grep -R`, §10 row 15 / §11 item 15).
- **Kind 3 — `Wrapper` & `NestedShell`:** units still starting with a §4 wrapper emit `UnsupportedWrapperForm`; modeled `NestedShell` (`sh`, `bash`, `dash`) with a named script (`sh x.sh`) emit pinned `UnreadReason::Program` (`CodeExecution::Program`, `OperandRole::ProgramFile`), while all other forms emit `UnsupportedNestedShellForm`.
- **Kind 4 — `CodeRunner`:** always `Unread`. Emits pinned `UnreadReason::Program` when `program_pin` matches (`python build.py`, `cargo test`, `pipx run ruff`, `awk -f prog.awk` without competing `-f`/`-e`/`--source`/`--eval`), and `UnreadReason::CodeRunnerInlineCode` (recording `code_operands`) otherwise.
- **Kind 5 — `ConditionalRunner` (`find`, `git`, `sed`):**
  - `find`: unknown-tool treatment unless `-exec`/`-execdir`/`-ok`/`-okdir` appears → `CodeRunnerInlineCode`.
  - `git`: walks global options to locate subcommand, checks `GIT_TRIGGERS` (`-c`, `--config-env`, `--exec-path`, `merge-index`, `filter-branch`, `bisect run`, `submodule foreach`, `rebase -x/--exec`, `difftool`/`mergetool -x/--tool-cmd`, `diff --ext-diff`, `push --receive-pack/--exec`), applies modeled `HomeReadOnly` and `DerivedDestinationWriter` subcommands, and marks `git cat-file` with `git_selects_by_name = true` under unknown-tool treatment.
  - `sed`: linear budgeted script parser (`v4/tools/sed.rs`) handles addresses, blocks, `s`/`y` delimiters and flags without false triggers on `e`/`w`/`r` in pattern/replacement text, triggers `CodeRunnerInlineCode` on `e`/`w`/`r`/`W`/`R` commands, `s///e`, `s///w`, `s///W`, or any unrecognized/unclosed syntax; pins single separate `sed -f`/`--file <file>` as `UnreadReason::Program`.
- **Kind 6 — `DerivedDestinationWriter`:** `cp` (`-R`/`-r`/`-a`/`--recursive`/`--archive`), `tar` (`-x`/`--extract`/`--get` and old-style bundled `tar xf`/`xzf`), `rsync`, `unzip`, `git clone`, `git checkout` emit `UnreadReason::DerivedDestinations` while leaving every word `Path`/`ReadWrite`; `mv` sets `DerivedDestinations::WhenSourceIsDir` with no P3 `Unread` (P2 resolves source types); non-triggered forms (`cp a b`, `tar -cf`, `tar -tf`) receive unknown-tool treatment.
- **`OUTSIDE_MODEL`, `UNKNOWN_BY_DESIGN`, and `cd`:** `OUTSIDE_MODEL` emits `OutsideToolModel`; `UNKNOWN_BY_DESIGN` (`rm`, `patch`, `curl`, `diff`, `ln -s`, etc.) gets unknown-tool treatment (`Path`/`ReadWrite`, `writes = true`, never Home-eligible, no `Unread`); `cd` clears `writes` (unless output redirect outside `/dev/null`) and populates `cd_target_operand` for modeled `cd [-L|-P|-e|-@] [--] DIR` (emitting `UnknownFlag` on unknown flags).
- **`check_git_selection` (`v4/tools/git.rs`):** checks `git show`/`cat-file`/`log`/`diff` for `GIT_CRED_NAMES`/`GIT_CRED_SUFFIXES`/`GIT_CRED_DIRS` plus CR-17 stores (`.gem/credentials`, `.terraform.d/credentials.tfrc.json`, `.config/op/config`, `.yarnrc.yml`, `credentials.tfrc.json`), proper directory ancestors (`.gem`, `.terraform.d`, `.config/op`, `.ssh`, `.aws`, etc., M1-29 round 4 finding 1), `-L`/`--line-range`, `REV:PATH`, `:N:PATH`, pathspec magic/exclusion (`StoreHit`), and active wildcard pathspecs (`Unread(GitWildcardPathspec)`), plus wildcard pathspecs on `git status` and `git ls-files`. Component-slice matching avoids quadratic string joins.

### Contract §7: Rules (`v4/rules.rs`, daemon `commands.rs`, `app/src/chat/rule-form.ts`)
- **`validate_rule_matcher`:** rejects empty argv, invalid `globset` syntax, and Allow `CommandPrefix` rules ending at a code runner without its required program pin (`awk`, `gawk`, `mawk`, `python`/`python3`, `perl`, `node`, `ruby`, `pipx`, `pipx run`, `cargo`, `git` bare or triggered, `find`, `sed` bare/unsafe, `sh`/`bash`/`dash` bare or flagged, `source`/`.` bare, `npx` bare, `eval`, `xargs`, `COMMAND_RUNNERS`, and `OUTSIDE_MODEL` shells/wrappers), with CR-19 folding. Allows `Deny` rules on all code runners, pinned Allow prefixes, and `CommandExact`.
- **`evaluate_rules`:** implements `dev`'s precedence (`Thread` > `Home` > `Global`, `Deny` before `Allow` within the same rank, higher-ranked `Allow` suppressing lower-ranked `Deny` on covered exact units and their matching best-effort twins without suppressing unrelated best-effort units). `Deny` matches exact and best-effort units (`effective_argv`, `raw_argv`, `alt_argv`). `Allow` rejects stored rules failing rev 4 validation (§11 item 5), enforces simple-command guards (no unquoted globs, only harmless redirects), sets `covers_pinned_program = true` when a prefix/exact rule pins `UnreadReason::Program`, never covers plain `Unread` with prefix/exact/scoped rules, covers plain `Unread` only via unscoped `Any` (never for `ToolSubject::Unknown`), and requires every target under a `path_scope`/`PathGlob` to be resolved canonical (never `Unread`, never `variant_only`).
- **`remember_matcher`:** emits `CommandExact` for single simple commands or escaped `PathGlob` for single-path file tools on `Clear`/`StoreHit`, returning `None` whenever any part is `Unread` (including `Program`), `Exhausted`, `TooLarge`, or `HardDenyHit`.
- **Daemon & editor:** `CreateRule` validates under §7 returning `ErrorCode::Invalid` with explanation; `ListRules` keeps stored invalid rules listed with `invalid_reason` (valid rules omit it); `rule-form.ts` aligns `commandRisk`/`validateRule` with §7 and exports `checkStoredRule` / `markStoredRules` for stored invalid rules.

### Contract §8: Budgets
- Every loop, scan, copy, string clone/join, `Vec` growth, `Word` slice, glob compilation, sed parse, and git component fold in `v4/tools.rs`, `v4/tools/sed.rs`, `v4/tools/git.rs`, and `v4/rules.rs` charges `ComputeBudget::charge` and `ComputeBudget::charge_alloc` **before** doing work or allocating. `Err(Exhausted)` propagates immediately and is never swallowed.

### Contract §10 rows owned / exercised by P3
- **Row 1** (`cat README.md`): `HomeReadOnly` eligible, `Clear`, as-written check (`v4/tools/tests.rs`).
- **Row 2** (`cat ~/.ssh/id_rsa`): `cat` keeps operand as `Path`/`ReadWrite` for P2 hard-deny (`v4/tools/tests.rs`).
- **Row 3** (`SUDO id`, `/usr/bin/sudo id`, folded runners): CR-19 folding in tool lookup and rule validation (`v4/tools/tests.rs`, `v4/rules/tests.rs`, `tests/create_rule.rs`).
- **Row 13** (`awk 'BEGIN{system("touch v")}'`, `["awk"]` Invalid): `CodeRunnerInlineCode`, Allow prefix `["awk"]` rejected at creation and ignored if stored (`v4/tools/tests.rs`, `v4/rules/tests.rs`, `tests/create_rule.rs`, `rule-form.test.ts`).
- **Row 14** (`python build.py` with `CommandExact ["python","build.py"]`): pinned `Unread(Program)` covered with `covers_pinned_program = true` (`v4/tools/tests.rs`, `v4/rules/tests.rs`).
- **Row 15** (`rg -L x proj`): `reads_recursively = true`, `links.below_root = true`, `home_readonly_eligible = ALLOW_HOME_FOLLOWED_SEARCH` (`v4/tools/tests.rs`).
- **Row 17** (`ln -s ~/.ssh/id_rsa x`): `UNKNOWN_BY_DESIGN`, target operand stays `Path`/`ReadWrite` (`v4/tools/tests.rs`).
- **Row 18** (`cp -R src/. proj`): `DerivedDestinationWriter` emits `Unread(DerivedDestinations)` (`v4/tools/tests.rs`).
- **Row 19** (`cat "$X"` with `Deny ["cat"]`): `Deny` matches best-effort unit (`v4/rules/tests.rs`).
- **Row 21** (`cat a | python mutate.py`): `python mutate.py` has `writes = true` so P2 marks every pipeline stage Unread (`v4/tools/tests.rs`).
- **Row 22** (`cat README.md >/dev/null`): harmless redirect keeps `writes = false`, `home_readonly_eligible = true`, and Allow rule eligibility (`v4/tools/tests.rs`, `v4/rules/tests.rs`).
- **Row 26** (global Deny `["cat"]` + thread Allow `["cat"]` on `cat README.md`): Thread Allow wins over Global Deny even with matching best-effort unit (`v4/rules/tests.rs`).

---

## 2. Disposition list for carried hunks and tests (contract §12)

### Source branch 1: `agent/hydrogen/M1-29-cr17-stores`

#### Hunks
| Source file & hunk | Disposition | Where / why |
|---|---|---|
| `crates/atrium-core/src/policy/rule_check.rs` (command-prefix validation) | **Rewritten** | Replaced by `v4::rules::validate_rule_matcher` driven directly from P0's normative `policy::tools` table (§6–§7, §11 item 4). Per-tool ad-hoc prefix guards dropped per §12. |
| `crates/atrium-core/src/policy/engine.rs` (canonical path-scope semantics & CR-12/CR-17 Git credential-ancestor checks from M1-29 round 4 findings 1–3) | **Rewritten** | Canonical path-scope semantics (`allow_targets_match_glob`: requires resolved canonical targets, rejects `Unread` and `variant_only`) implemented in `v4/rules.rs`; CR-12/CR-17 Git credential-ancestor & wildcard pathspec checks implemented in `v4/tools/git.rs`. |
| `crates/atrium-daemon/src/commands.rs` (`create_rule` validation & `ListRules` invalid marking) | **Rewritten** | Implemented in `crates/atrium-daemon/src/commands.rs` under rev 4 §7 (`ErrorCode::Invalid` with reason on `CreateRule`; `ListedRule` with `invalid_reason` on `ListRules`, §11 items 4–5). |
| `app/src/chat/rule-form.ts` | **Rewritten (in place)** | Aligned with rev 4 §7 / §11 items 4–5 (`awk`, `sed`, `git`, `cargo`, `pipx`, `COMMAND_RUNNERS`, pinned `sh`/`source`/`.`, plus `checkStoredRule` / `markStoredRules`). |
| `app/src/ui/Security.tsx` & `app/src/ui/security.test.ts` | **Not P3 (owned by P4)** | Brief assigns UI text and `Security.tsx` exclusively to P4 to avoid merge conflicts. |
| CR-17 store definitions in `policy/deny.rs` / `policy/engine.rs` | **Not P3 (owned by P2)** | Filesystem store matching belongs to P2; P3 covers CR-17 stores inside `v4/tools/git.rs` for Git selection (`git show HEAD -- .gem`, etc.). |

#### Tests
| Source test | Disposition | Rev 4 expectation & contract citation |
|---|---|---|
| `crates/atrium-daemon/tests/create_rule.rs` (daemon rule validation) | **Rewritten** | `crates/atrium-daemon/tests/create_rule.rs`: tests rev 4 §7 rejection of all unpinned Allow code-runner prefixes (including `awk`, `python`, `pipx run`, `git`, `sed`, `cargo`, `find`, `sh`, `ssh`, `docker`, CR-19 folds) and invalid globs, acceptance of pinned Allow prefixes & Deny rules, and `ListRules` keeping stored invalid rules listed with `invalid_reason` (§7, §10 row 13, §11 items 4–5). |
| `app/src/chat/rule-form.test.ts`: `git -c x=y status` (`commandRisk` was `null`) | **Kept with new expectation** | Now refused as an Allow prefix (`git` prefix cannot start with `-c` / must pin a non-code-running subcommand; §7, §11 item 4). |
| `app/src/chat/rule-form.test.ts`: `ssh host`, `docker`, `docker run img` (was warn on prefix) | **Kept with new expectation** | Now refused as Allow `command_prefix` (`COMMAND_RUNNERS` have `code_runner_no_pin` and no program pin; §6, §7, §11 item 4); `command_exact` remains accepted with a warning. |
| `app/src/chat/rule-form.test.ts`: `. ./env.sh` and `Source x` (was refused) | **Kept with new expectation** | Now accepted as pinned `FirstOperand` with a warning (`sh x.sh`, `source env.sh`, `. ./env.sh` are pinnable under §4/§6/§7, §11 item 8); bare `source`, `.`, `sh`, `bash`, and `sh -c` remain refused. |
| `app/src/chat/rule-form.test.ts`: new tests for `awk`, `sed`, `git`, `cargo`, `pipx run`, and stored invalid rules | **Added** | Covers §7, §10 row 13, §11 items 4–5 (`checkStoredRule`, `markStoredRules`). |
| Per-tool prefix guard tests from M1-29 (`rule_check.rs` ad-hoc allowlists) | **Dropped** | Replaced by `v4/rules/tests.rs` driven by `policy::tools` (§6–§7, §12). |

### Source branch 2: `agent/hydrogen/M1-32-link-globs`

#### Hunks & Tests (P3-owned items only; resolution/link-walk belong to P2)
| Source file & hunk | Disposition | Where / why |
|---|---|---|
| Archive operand roles for `tar`/`unzip` | **Rewritten** | Under D-141, `tar` (with `-x`/`--extract`/`--get` or bundled `xf`/`xzf`) and `unzip` are `DerivedDestinationWriter` rows in `v4/tools.rs`: when triggered they emit `UnreadReason::DerivedDestinations` while keeping every word a candidate `Path` (`ReadWrite`), and `tar -cf`/`-tf` receive unknown-tool treatment (§5, §6, §11 item 10, `v4/tools/tests.rs`). |
| `rg -L` link-following policy flag | **Rewritten** | `v4/tools.rs` sets `links.below_root = true`, `reads_recursively = true`, and `home_readonly_eligible = ALLOW_HOME_FOLLOWED_SEARCH` (`v4/tools/tests.rs`; §10 row 15, §11 item 15, D-139). |

---

## 3. Change requests against P0's frozen interface

P0's frozen files (`model.rs`, `limits.rs`, `tools.rs`, `policy/mod.rs`, and other `v4/` stubs) were **not edited**. The following change requests are raised for P0/P5 integration:

1. **CR-P3-1 (`crates/atrium-core/src/policy/mod.rs` — re-export `validate_rule_matcher`):**
   - `atrium_core::policy::v4` is private (`mod v4;`) and `validate_rule_matcher` is `pub(crate)`, so the separate `atrium-daemon` crate cannot call `atrium_core::policy::v4::rules::validate_rule_matcher`.
   - **Workaround in P3:** `crates/atrium-daemon/src/commands.rs` currently contains a private rev 4 §7 validator mirror (`validate_rule_v4`) matching `v4::rules::validate_rule_matcher` and `globset 0.4.21`.
   - **Requested change:** expose `pub fn validate_rule_matcher(...) -> Result<(), String>` (or `pub fn validate_rule_draft`) from `atrium_core::policy` so `atrium-daemon` calls the single source of truth and removes the mirror.

2. **CR-P3-2 (`crates/atrium-protocol/src/policy.rs` — optional `invalid_reason` on `Rule`):**
   - Adding a field directly to `pub struct Rule` in `atrium-protocol` requires updating ~8 unowned struct-literal call sites (`ledger.rs`, `tests/policy.rs`, `tests/store.rs`, `tests/ledger.rs`, `crates/atrium-protocol/tests/serde_shape.rs`, etc.) and regenerating `bindings/protocol.ts`.
   - **Workaround in P3:** `pub struct Rule` is unchanged; `DaemonCommand::ListRules` serializes `ListedRule { #[serde(flatten)] rule, #[serde(skip_serializing_if = "Option::is_none")] invalid_reason }` (100% wire-compatible with `Vec<Rule>`), and `app/src/chat/rule-form.ts` exports `ListedRule = Rule & { invalid_reason?: string | null }`.
   - **Requested change (optional for P4/P5):** if `Rule` itself should carry `#[serde(default, skip_serializing_if = "Option::is_none")] pub invalid_reason: Option<String>` in the generated TS bindings, add it during P4/P5 when unowned test sites and `bindings/protocol.ts` can be updated together.

3. **CR-P3-3 (`crates/atrium-core/src/policy/model.rs` — `RuleEvalContext::outcomes` for P1/P2 unit-level `Unread`):**
   - `RuleEvalContext` currently carries `units`, `effects`, `targets`, `best_effort`, and `folds`, but no `outcomes: &'a [PartResult]` from P1/P2.
   - **Edge case affected:** if a unit has a P1/P2 unit-level `Unread` (e.g., `FOO=1 cat a` with `UnreadReason::Assignment`, or `cat "$X"` if emitted as a partial unit) AND the store has BOTH a higher-ranked `Thread Allow CommandPrefix ["cat"]` AND a lower-ranked `Global Deny CommandPrefix ["cat"]`, `evaluate_rules` cannot see the P1/P2 `Unread` and would mark the unit covered by Thread Allow, suppressing Global Deny — whereas §1 step 3 + §7 require Global Deny to win because a prefix Allow never covers plain `Unread`.
   - **Requested change:** add `pub(crate) outcomes: &'a [PartResult]` (or per-unit `has_unread: bool`) to `RuleEvalContext` in P0/P5 so `evaluate_rules` treats units with P1/P2 `PartOutcome::Unread` identically to P3 `ToolEffects::unread`.

4. **CR-P3-4 (`crates/atrium-core/src/policy/tools.rs` — CR-17 stores in `GIT_CRED_*`):**
   - Frozen `tools.rs` carries `dev`'s pre-CR-17 `GIT_CRED_SUFFIXES` / `GIT_CRED_NAMES` (omitting `.gem/credentials`, `.terraform.d/credentials.tfrc.json`, `.config/op/config`, `.yarnrc.yml`, `credentials.tfrc.json`).
   - **Workaround in P3:** `v4/tools/git.rs` augments `tools::GIT_CRED_*` with `CR17_GIT_CRED_SUFFIXES` and `CR17_GIT_CRED_NAMES` so `git show HEAD -- .gem` and all CR-17 stores return `StoreHit`.
   - **Requested change (optional):** fold those five CR-17 entries directly into `tools::GIT_CRED_SUFFIXES` / `tools::GIT_CRED_NAMES` in P5.

---

## 4. Verification commands

### Run by worker (allowed under D-123)
- `cargo fmt --all` — clean
- `cargo check -p atrium-core -p atrium-daemon -p atrium-protocol --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core -p atrium-daemon -p atrium-protocol --tests -- -D warnings` — clean (0 errors, 0 warnings)

### Commands for the orchestrator's verification task (fish-compatible)
```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core policy::v4::tools:: -- --nocapture
cargo test -p atrium-core policy::v4::rules:: -- --nocapture
cargo test -p atrium-protocol policy::tests:: -- --nocapture
cargo test -p atrium-daemon --test create_rule -- --nocapture
cargo test --workspace --no-fail-fast
pnpm --dir app run typecheck
pnpm --dir app test -- src/chat/rule-form.test.ts
./scripts/check-protocol-ts
```


---

## Round 2

Rework addressing all 14 findings from `.orchestration/review-input/review-round1.md` and implementing CR-P3-1, CR-P3-2, and CR-P3-3 under D-153 (leaving CR-P3-4 as-is).

### 1. Findings fixed and regression tests

| # | Finding | Fix | Regression test(s) |
|---|---|---|---|
| 1 | `RuleCoverage` ignored P1/P2 `Unread` & `Exhausted` outcomes (CR-P3-3) | Added `outcomes: &'a [PartResult]` to `RuleEvalContext` (`policy/model.rs`). `evaluate_rules` (`v4/rules.rs`) now scans `call.effects`, `call.outcomes`, and `call.targets` (including `variant_only`), never granting Allow coverage or suppressing Deny for plain `Unread` (outside a single pinned `Unread(Program)` with no competing plain `Unread`/`Exhausted`) or `Exhausted` (which blocks even unscoped `Any`). | `v4/rules/tests.rs::finding_1_p1_p2_unread_and_exhausted_block_allow_and_do_not_suppress_deny` (Unread target, P1 `Assignment`, P2 `AfterPossibleWriter`, `WholeCall` `UnknownCwd`, `Exhausted` blocking unscoped `Any`, and pinned `python build.py` combined with an Unread target). |
| 2 | Partial coverage suppressed lower-ranked Deny rules | Retained `dev`'s call-level suppression in `evaluate_rules`: `all_units_and_be_covered` breaks out of the rank loop only when the whole call is covered; while uncovered, `deny_command_matches` checks every exact and best-effort unit without skipping covered units. | `v4/rules/tests.rs::precedence_and_best_effort_deny_semantics` (fixed `cat a && ls b` assertion to expect Global Deny `r_g_deny_cat`; added full-coverage suppression case `cat a && ls b` with both Thread Allows). |
| 3 | Matched Deny could be lost to post-match exhaustion | `evaluate_rules` pre-charges compute and allocation for the Deny reason and `RuleId` **before** calling `deny_rule_matches`, so committing an established Deny is infallible (`String::with_capacity` + `push_str`). | `v4/rules/tests.rs::finding_3_matched_deny_never_lost_to_post_match_exhaustion` (sweeps compute units `0..=80` — covering `(12, 1_048_576)` — and allocation `0..=256`, asserting monotonic Deny preservation). |
| 4 | `check_git_selection` discarded earlier `StoreHit`s on later exhaustion | Amended `check_git_selection` to return `GitSelectionOutcome { results, exhausted }` and added `check_git_selection_into` (`v4/tools.rs`, `v4/tools/git.rs`), pre-reserving result slots before each operand so established `StoreHit`s survive later exhaustion. | `v4/tools/tests.rs::classify_unit_and_git_selection_respect_compute_and_alloc_budgets` (`git show -- ':!x' y` sweep across compute budgets `0..=40`, verifying retained `StoreHit` alongside `ExhaustReason::ComputeBudget`). |
| 5 | BSD/macOS bare `sed -i` could hide a trigger (`sed -i p 'w out' input`) | `classify_sed` (`v4/tools.rs`) now consumes only an empty suffix (`sed -i '' ...`) cleanly; any other bare `-i` form sets `uncertain = true` and emits `UnreadReason::CodeRunnerInlineCode`. | `v4/tools/tests.rs::conditional_runners_find_git_sed` (`sed -i p 'w out' input`, `sed -ni p 'w out' input`, `sed -i 's/a/b/' input` → `CodeRunnerInlineCode`; `sed -i '' 's/a/b/' input` and `-i.bak` remain clear). |
| 6 | Deny `path_scope` used existential matching and ignored `Unread` targets | `deny_path_scope_ok` (`v4/rules.rs`) now calls `all_targets_match_glob`, requiring every canonical target to be resolved and inside `path_scope` (rejecting `Unread` and `variant_only` targets); existential `deny_targets_match_glob` is kept solely for the Deny `PathGlob` matcher itself. | `v4/rules/tests.rs::finding_6_deny_path_scope_requires_every_target_and_rejects_unread` (mixed `/proj/a` + `/elsewhere/b`, `/proj/a` + Unread target, all in-scope, and Deny `PathGlob` existential check). |
| 7 | Daemon validator mirror diverged from core (CR-P3-1; `["sed", "z"]`) | Exposed `pub fn validate_rule_matcher` in `crates/atrium-core/src/policy/mod.rs` calling `v4::rules::validate_rule_draft_reason`. `crates/atrium-daemon/src/commands.rs` calls it for both `CreateRule` and `ListRules` and removes the entire 731-line `validate_rule_v4` mirror. | `v4/rules/tests.rs::finding_7_public_core_validator_matches_v4_rules`; `crates/atrium-daemon/tests/create_rule.rs` (`["sed", "z"]` and D-141 unknown tools accepted by `CreateRule` and listed valid). |
| 8 | App retained pre-D-141 per-tool guards (`deno`, `bun`, `php`, `lua`, `pwsh`, etc.) | Rewrote `commandRisk` and `checkStoredRule` in `app/src/chat/rule-form.ts` to match core's frozen D-141 six row kinds 1-to-1 (accepting unknown tools like `deno`/`bun`/`php`/`lua`/`pwsh`, `sed z`, and `command_exact` rules). Rewrote/dropped conflicting carried tests in `app/src/chat/rule-form.test.ts` (dispositions below). | `app/src/chat/rule-form.test.ts` (`rev 4 §7 & D-141 parity with core`, `stored invalid rules (§11 item 5)`). |
| 9 | Stored invalid rules were not marked in the rendered Settings rules list (CR-P3-2) | Added `pub struct ListedRule` (`invalid_reason: Option<String>` with `#[serde(default)]`) in `atrium-protocol`, registered in `examples/export-ts.rs`, mirrored in `bindings/protocol.ts` and `app/src/daemon/types.ts`, returned by daemon `ListRules`, consumed by `Settings.tsx` and `app/src/chat/rules.ts`, and rendered via `app/src/ui/RulesList.tsx` (`data-invalid="true"`, `"needs-you"` dot, reason + `"covers nothing"`, active Delete button). | `crates/atrium-protocol/src/policy.rs::listed_rule_wire_compatibility_and_invalid_reason_default`; `crates/atrium-daemon/tests/create_rule.rs`; `app/src/chat/rules.test.ts`; `app/src/ui/rules-list.test.ts` (rendered `happy-dom` test). |
| 10 | Root pathspecs (`git show HEAD -- .`, lone `:`) escaped CR-12 checks | `v4/tools/git.rs` now recognizes root ancestor selections (`.`, `./`, `..`, `src/..`, `/`, `""`) in pathspec context (`allow_root_ancestor = true`) and lone `:` pathspec as `StoreHit`, while keeping revision-object parsing (`HEAD:.`, `:0:path`, `-L`) separate (`allow_root_ancestor = false`). | `v4/tools/tests.rs::git_selection_cr12_and_cr17_stores` (`git show HEAD -- .`, `./`, `..`, `src/..`, `:`, `git show -- .`, while `git show HEAD:.` stays clear). |
| 11 | `--noglob-pathspecs` left wildcards active | `v4/tools/git.rs` tracks `pathspec_magic_enabled` separately from `default_glob_wildcards` and parses `:(literal)` / `:(glob)` magic flags: `--noglob-pathspecs` makes `*`/`?`/`[` literal by default while keeping pathspec magic (`:!`, `:(glob)`) active. | `v4/tools/tests.rs::git_selection_cr12_and_cr17_stores` (`--noglob-pathspecs status -- '*.rs'` clear, `--noglob-pathspecs status -- ':(glob)*.rs'` → `GitWildcardPathspec`, `--noglob-pathspecs show HEAD -- ':!*.rs'` → `StoreHit` only). |
| 12 | Program pinning rescanned a word before charging | `ProgramPin::FirstOperand` in `v4/tools.rs` retains the validated `&str` views from `word_as_str_budgeted`, removing the uncharged `w.as_str()` UTF-8 rescan. Also swept `v4/tools.rs`, `v4/tools/git.rs`, and `v4/rules.rs` so every loop/scan charges before running. | `v4/tools/tests.rs::classify_unit_and_git_selection_respect_compute_and_alloc_budgets` (`python <2048-byte script>` boundary sweep proving exhaustion before cloning `program_words`). |
| 13 | Search test invented unsupported `rg --no-follow` flag | Replaced the unsupported toggle expectation in `v4/tools/tests.rs` with `UnknownFlag` + `!home_readonly_eligible` (matching the frozen table), and added a table-supported `grep -R -r` toggle check. | `v4/tools/tests.rs::search_tools_rg_and_grep`. |
| 14 | Daemon fixture created `data_dir` with 0755 permissions | `crates/atrium-daemon/tests/create_rule.rs` now creates `&rig.paths.data_dir` with `atrium_host::lifecycle::secure_dir` (0700) before opening the store. | `crates/atrium-daemon/tests/create_rule.rs::list_rules_keeps_stored_invalid_rules_visible_with_invalid_reason`. |

### 2. Interface changes made under D-153

1. **`crates/atrium-core/src/policy/mod.rs` (CR-P3-1):**
   - Added `pub fn validate_rule_matcher(matcher: &RuleMatcher, decision: RuleDecision, path_scope: Option<&str>) -> Result<(), String>` delegating to `v4::rules::validate_rule_draft_reason(..., &model::FoldRules::REV4)`.
2. **`crates/atrium-core/src/policy/model.rs` (CR-P3-3):**
   - Additively added `pub(crate) outcomes: &'a [PartResult]` to `RuleEvalContext<'a>` with a doc comment citing §7 and D-153.
3. **`crates/atrium-protocol/src/policy.rs`, `examples/export-ts.rs`, `bindings/protocol.ts` (CR-P3-2):**
   - Added `pub struct ListedRule` (all `Rule` fields plus `#[serde(default)] pub invalid_reason: Option<String>` and `ListedRule::from_rule`), registered it in `export-ts.rs`, and added the matching `export type ListedRule` in `bindings/protocol.ts` and `app/src/daemon/types.ts`. Wire-compatible with both `Vec<ListedRule>` and `Vec<Rule>`.
4. **`crates/atrium-core/src/policy/v4/tools.rs` (Finding 4 / D-153 widening):**
   - Added `pub(crate) struct GitSelectionOutcome { pub results: Vec<PartResult>, pub exhausted: Option<Exhausted> }`, updated `check_git_selection` to return `GitSelectionOutcome` (instead of `Result<Vec<PartResult>, Exhausted>` which dropped prior `StoreHit`s on later exhaustion), and added `check_git_selection_into` writing into a caller-provided `Vec<PartResult>`.

### 3. Corrected §12 dispositions for `app/src/chat/rule-form.test.ts`

To align `app/src/chat/rule-form.ts` 1-to-1 with core's frozen D-141 six-row table (Finding 8), the following carried pre-D-141 tests from `dev` / `M1-29` were rewritten or dropped:

| Carried test in `app/src/chat/rule-form.test.ts` | Disposition | Reason (D-141 / §7 / §12) |
|---|---|---|
| `refuses unpinned shells, eval, bare source/., and every wrapper` (asserted refusal on `command_exact` and on §4 wrappers `env`/`exec`/`command`/`time`/`nice`/`timeout` and `sudo`/`doas`) | **Rewritten** | Core `validate_rule_matcher` only restricts Allow `CommandPrefix` on `OUTSIDE_MODEL`, `NestedShell`, `CodeRunner`, and `ConditionalRunner`; §4 `Wrapper` rows and `CommandExact` are accepted. Replaced by `refuses unpinned Allow prefixes for OUTSIDE_MODEL shells/wrappers, NestedShell, CodeRunner, and ConditionalRunner` and `accepts CommandExact rules and Deny rules matching core validate_rule_matcher`. |
| `refuses inline-code flags on interpreters` (`python3 -c`, `node -e`, `php -r`, `lua -e`, `osascript -e`, `deno eval`, `bun -e` on `command_exact` and prefix) | **Rewritten** | `python`/`python3`/`node`/`perl`/`ruby` Allow prefixes without a script pin remain refused; `deno`, `bun`, `php`, `lua`, `osascript` were removed from the frozen table by D-141 and are now asserted accepted as unknown tools. |
| `refuses PowerShell given code, by any prefix PowerShell accepts` (`pwsh`, `powershell`) | **Dropped** | `pwsh`/`powershell` are absent from the frozen D-141 table (unknown tools); replaced by unknown-tool parity assertion. |
| `refuses privilege wrappers` (`run0`, `sudoedit`, `pkexec`, `doas`, `su -c`) | **Rewritten** | `su` remains refused as a `COMMAND_RUNNERS` Allow prefix; `run0`/`sudoedit`/`pkexec`/`doas` are outside the frozen table (privilege escalation is enforced by hard-deny, not prefix validation). |
| `refuses a prefix rule on an interpreter, runner or find unless the script, package or file comes right after it` (`deno`, `bun`, `uvx`, `pnpx`, `command_exact` `-c` checks) | **Rewritten** | Retained for frozen-table `FirstOperand` runners (`python`, `python3`, `node`, `perl`, `ruby`, `source`, `.`, `npx`) and `find`; dropped `deno`/`bun`/`uvx`/`pnpx` and `command_exact` refusals to match core. |
| `bun exec, package runner -c, stdin scripts, and bare exact rules` (`bun exec`, `pnpx`/`bunx`/`uvx -c`, `/dev/stdin`, bare `command_exact` interpreters) | **Dropped** | Pre-D-141 per-tool guards removed by D-141; core `validate_rule_matcher` accepts `CommandExact` and treats `bun`/`pnpx`/`bunx`/`uvx` as unknown tools. |

### 4. Updated verification commands

#### Run by worker (D-123)
- `cargo fmt --all` — clean
- `cargo check -p atrium-core -p atrium-daemon -p atrium-protocol --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core -p atrium-daemon -p atrium-protocol --tests -- -D warnings` — clean (0 errors, 0 warnings)

#### Commands for the orchestrator's verification task (fish-compatible)
```fish
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core policy::v4::tools:: -- --nocapture
cargo test -p atrium-core policy::v4::rules:: -- --nocapture
cargo test -p atrium-protocol policy::tests:: -- --nocapture
cargo test -p atrium-daemon --test create_rule -- --nocapture
cargo test --workspace --no-fail-fast
./scripts/check-protocol-ts
pnpm --dir app run typecheck
pnpm --dir app test -- src/chat/rule-form.test.ts src/chat/rules.test.ts src/ui/rules-list.test.ts
```

---

## Round 3

Rework addressing both medium findings from `.orchestration/review-input/review-round2.md` plus a full sweep of the option-value scanning class across all P3 rows and `app/src/chat/rule-form.ts`.

### 1. Findings fixed and regression tests

| # | Finding / Class | Fix | Regression test(s) |
|---|---|---|---|
| 1 | Wrapped pinned programs received no rule coverage (`v4/rules.rs`) | Pre-computed `wrapper_words` from `CommandUnit::removed_wrappers` in `UnitEvalInfo` (budgeted via `wrapper_word_count`) and added `rule_argv_names_pinned_program`, checking `rule_argv[wrapper_offset..]` against `prog_words` in both `CommandPrefix` and `CommandExact` while keeping `words_match_argv(u.original_argv(), ...)` intact for rule matching itself. | `v4/rules/tests.rs::round3_finding_1_wrapped_pinned_programs_receive_coverage_and_suppress_lower_deny` (`command python build.py` with `CommandExact` and `CommandPrefix`, higher-ranked Thread Allow suppressing lower-ranked Global Deny, unpinned `["command"]`/`["command","python"]` rejected and Deny preserved, unwrapped rule rejected on wrapped call, stacked `nice -n 5 command -- python build.py --flag`, plus `timeout 30 cargo test` and `env awk -f prog.awk`). |
| 2 | Git short-option values read as option letters (`v4/tools.rs`, `v4/tools/git.rs`, `v4/rules.rs`, `app/src/chat/rule-form.ts`) | Replaced substring short-option matching with `ShortOptArity`-based parsers (`parse_git_global_opt`, `git_subcommand_opt_step`, `git_subcommand_has_flag_trigger_budgeted` / `_pure`, mirrored in `rule-form.ts::findGitSubcommandTriggerFlag`). Value-taking options (`-C`, `-c`, `--git-dir`, `--attr-source`, `rebase -s`/`-X`/`-S`, `difftool`/`mergetool -t`, `diff -S`, `push -o`, etc.) consume the rest of the token or the next word so value bytes are never scanned as option letters. `check_git_selection` (`v4/tools/git.rs`) now uses `parse_git_global_opt` so `git -C --literal-pathspecs show ...` never mistakes `-C`'s directory value for `--literal-pathspecs`. | `v4/tools/tests.rs::round3_finding_2_git_short_option_values_and_subcommand_values` (`git -Csrc status`, `git -C src status`, `git -C -c status`, `git -pCsrc status` → no trigger; `git -c x=y status`, `git -cx=y status`, `git -pcx=y status` → trigger; subcommand attached/separate values on `rebase`, `difftool`, `mergetool`, `diff`, `push`; `git -C --literal-pathspecs show HEAD -- ':!.env'` → `StoreHit`); `v4/rules/tests.rs::round3_finding_2_git_option_values_preserve_exact_rule_coverage_and_validation`; `app/src/chat/rule-form.test.ts`. |
| Class | Same option-value scanning class across `rg`/`grep`, `cp`, `tar`, `awk`, `sed`, `find`, and interpreters | **Search (`rg`/`grep`):** removed blanket `is_forbidden_flag` over whole short clusters in `classify_search`; now checks `home.forbidden` per long option name and per actual short option letter before breaking on `FlagKind::Value` (`rg -efoo src`, `rg '-g*.pdf' needle src`, `grep -efoo README.md` keep `home_readonly_eligible = true`). **DerivedDestinationWriter (`cp`/`tar`):** replaced `matches_exact_or_short_cluster` with `derived_writer_opt_step` plus old-style `tar` key-arg skipping (`cp -tarchive`, `cp -S.bak`, `tar -cfextra.tar`, `tar -Cextra`, `tar cf -x` no longer falsely trigger `-a`/`-r`/`-x`). **CodeRunner (`awk`/`gawk`/`mawk` & interpreters):** `awk_opt_step` respects `-F`/`-v`/`-W`/`--field-separator`/`--assign` in both `v4/tools.rs`, `v4/rules.rs`, and `rule-form.ts`; `parse_interpreter_opt` respects value-taking short options on `python`/`node`/`perl`/`ruby`. **ConditionalRunner (`sed` & `find`):** `has_competing_sed_flags_pure` / `hasCompetingSedFlags` respect attached `-i.ref`/`-l80` while rejecting bare `-i`/`-l`; bare `-l` in `classify_sed` now fails closed (`uncertain = true`, avoiding macOS/BSD `-l` zero-arg ambiguity); `find` skips values of unambiguous value-taking primaries (`find . -name -exec`). | `v4/tools/tests.rs::round3_option_value_scanning_class_rg_grep_cp_tar_awk_sed_find_interpreters`; `v4/rules/tests.rs::round3_finding_2_git_option_values_preserve_exact_rule_coverage_and_validation`; `app/src/chat/rule-form.test.ts`. |

### 2. Verification run by worker (D-123)

- `cargo fmt --all` — clean
- `cargo check -p atrium-core -p atrium-daemon -p atrium-protocol --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core -p atrium-daemon -p atrium-protocol --tests -- -D warnings` — clean (0 errors, 0 warnings)
- No `cargo test` or `pnpm` run by worker; orchestrator verification runs the fish commands listed in Round 2 §4 above.
