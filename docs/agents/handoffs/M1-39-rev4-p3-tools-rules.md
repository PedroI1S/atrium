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
