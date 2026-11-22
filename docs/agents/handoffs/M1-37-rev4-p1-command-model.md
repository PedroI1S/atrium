# Handoff: M1-37-rev4-p1-command-model (P1)

- **Branch:** `agent/neon/M1-37-rev4-p1-command-model` (branched from P0 tip `1a09705`)
- **Model:** Gemini 4 Neon (High)
- **Contract:** `docs/contracts/m1-policy-rev4.md` (§2 best-effort lexing, §3, §4, §8, §10, §12)
- **Behaviour change:** None. Nothing is wired into `SessionGate` (that is P5); `dev`'s evaluator remains live and untouched.

---

## 1. What was implemented, by contract section

### Owned files written
- `crates/atrium-core/src/policy/v4/lex.rs` — `lex_exact`, `lex_best_effort`, `fold_command_word`, and shared character/assignment/CR-19 helpers.
- `crates/atrium-core/src/policy/v4/lex/best_effort.rs` — budgeted linear tolerant best-effort lexer (`lex_best_effort_into`), N11 budgeted thread-local memo (`BE_LEX_MEMO`), bounded code scans, and N11 budgeted brace expansion (`brace_expand_budgeted`).
- `crates/atrium-core/src/policy/v4/units.rs` — `build_units`, `build_units_with_hook` (optional P3 code-runner hook), `unwrap_unit`, nested-shell classification/recursion, and `HOME` modification tracking.
- `crates/atrium-core/src/policy/v4/lex/tests.rs` — §3 boundaries, §10 lex rows, provenance, CR-19 folding, best-effort scans, N11 brace expansion & memo determinism, budget exhaustion, and 10 KB → `MAX_BYTES` linearity.
- `crates/atrium-core/src/policy/v4/units/tests.rs` — §10 P1 rows, §4 wrapper forms & near misses, `MAX_WRAP`, command-word globs, §4 nested-shell forms & near misses, `MAX_DEPTH`, `HOME` tracking, P3 code-runner hook, and `build_units` / `unwrap_unit` budget exhaustion & linearity.

### Contract §3: Words with provenance, separators, redirects, assignments, `HOME`
- **Per-byte provenance (`v4/lex.rs`):** every decoded byte in `Word` records `ByteProvenance::{Unquoted, SingleQuoted, DoubleQuoted, Escaped}`. Bytes produced from quotes or escapes are never re-parsed as shell syntax.
- **Unquoted bytes:**
  - Allowed: `A-Z a-z 0-9 _ . / , : @ % + -`, plus `=` only when not the word's first byte (leading `=` → `UnreadReason::UnquotedByteOutsideModel { byte: b'=' }` for zsh `=cmd`).
  - Globs: `*`, `?`, and non-empty closed `[...]` without `!`, `^`, nested `[`, `/`, quotes, or non-ASCII (`[!…]` / `[^…]` → `NegatedGlobClass`; empty/unclosed/stray `]`/nested `[`/slash → `UnquotedByteOutsideModel`).
  - `~`: allowed unquoted only as a word's first byte followed by `/` or end of word (`~user`, `~+`, `~-`, `~''` → `UnsupportedTildeForm`; non-leading unquoted `~` → `UnquotedByteOutsideModel { byte: b'~' }`).
  - Any other unquoted byte (`$`, `` ` ``, `{`, `}`, `#`, `!`, `^`, `(`, `)`, `&`, non-ASCII, control bytes, etc.) emits the precise `UnreadReason`.
- **Quotes and escapes:**
  - Single quotes accept any bytes verbatim (`SingleQuoted`); unterminated → `UnsupportedQuoteOrEscape`.
  - Double quotes accept any bytes except `$`, backtick, `!`, and `\` outside `\"` / `\\` (so `"\q"`, `"\$"`, `"\\\n"`, `"$X"`, `` "`id`" ``, `"hi!"`, and unterminated `"` all emit `UnsupportedQuoteOrEscape`).
  - Outside quotes: `\` is accepted only before an allowed character (including `=`) or glob character (`*`, `?`, `[`, `]`), producing `ByteProvenance::Escaped`. Backslash-newline and all other escapes (`\ `, `\$`, `\;`, `\~`, `\\`, trailing `\`) emit `UnsupportedQuoteOrEscape`.
- **Separators and outside constructs:**
  - Modeled separators: `;`, `&&`, `||`, `|` (assigning shared `PipelineId`s), and `\n`.
  - Comments (`#`), here-documents (`<<`, `<<-`, skipping bodies linearly so body text is never mis-parsed as commands), here-strings (`<<<`), process substitutions (`<(`, `>(`), subshells/groups/arithmetic/functions (`(`, `)`, `((`, `))`, `()`, `{`, `}`), `&`, `|&`, `;;`, empty/dangling operators, redirect-only commands, and all 19 reserved words from `tools::OUTSIDE_MODEL` (`§3`) emit `ReservedWordOrConstruct`.
- **Closed redirect list:**
  - Modeled: `<`, `>`, `>>`, `2>`, `2>>` (requiring a literal non-glob target), `n>&m` with `n, m ∈ 0..=2`, and `n>&-` with `n ∈ 0..=2`.
  - Outside the model: `&>` / `&>>` (`CombinedRedirect`), descriptors `> 2` (`DescriptorOutsideStd { fd }`), `<&`, `<>`, `>|`, `>&2`, `>&-`, `1>`, `0<`, `2<`, leading-zero fds (`02>`), glob redirect targets, and missing targets (`UnsupportedRedirect`).
- **Assignments & `HOME` (`v4/lex.rs`, `v4/units.rs`):**
  - Leading `NAME=value` / `NAME+=value` emits `UnreadReason::Assignment`.
  - Earlier units setting/clearing `HOME` (leading `HOME=` assignment, `export`/`declare`/`typeset`/`local`/`readonly`/`read` targeting `HOME`, `unset HOME`, `env` with `-i`/`-`/`-u HOME`/`-S`/`HOME=` including after `--`, `printf -v HOME`, including after leading assignments like `FOO=1 export HOME=/tmp`) mark later units (and child `-c` scripts) using an unquoted `~` in argv or redirects with `UnreadReason::HomeSetEarlier`.

### Contract §4: Command words, wrappers, nested shells, code-runner hook
- **Command-word folding (`fold_command_word`):** takes the basename after the last `/`, applies Unicode `char::to_lowercase()` plus `FoldRules::{fold_long_s, fold_kelvin}` (`ſ` U+017F → `s`, `K` U+212A → `k`), with **no** Unicode normalization (NFC/NFD preserved verbatim).
- **Command-word globs:** an unquoted glob in effective `argv[0]` emits `UnreadReason::CommandWordGlob` (not expanded as a wrapper or tool).
- **Closed wrapper list (`unwrap_unit`):** reads wrapper specs and flags from P0's `policy::tools::{lookup, lookup_outside_model, flag}` without editing `tools.rs`. Strips only:
  - `command [--]`
  - `exec` (no options)
  - `nice`, `nice -n N`
  - `timeout [-s SIG] [-k D] D`
  - `time` (no options)
  - bare `env` (no options, no `=` assignments — even quoted)
  - Bounded by `limits::MAX_WRAP` (8); a 9th wrapper (modeled or outside-model) emits `UnreadReason::MaxWrapReached` and stops immediately. Any other wrapper option/near miss or any of the 16 outside-model wrappers in `tools::OUTSIDE_MODEL` emits `UnreadReason::UnsupportedWrapperForm`.
- **Nested shells (`v4/units.rs`):**
  - Modeled: only `sh`, `bash`, `dash` (`spec.nested_shell == true` in `tools.rs`) as `NAME [-e|-u|-x]... -c SCRIPT` (using `tools::flag_short(spec.flags, ...)`), with nothing after `SCRIPT` and no unquoted glob/tilde on `SCRIPT`. `SCRIPT` is lexed and built recursively up to `limits::MAX_DEPTH` (6), preserving `parent_unit`, `depth`, distinct `PipelineId`s, and inherited `HOME` state. Exceeding `MAX_DEPTH` emits `UnreadReason::MaxNestingDepthReached`.
  - Script files (`sh x.sh`, `bash ./build.sh arg1`, `dash run.sh`) emit `UnreadReason::Program { program_words: [argv[0], argv[1]] }` (Unread (program)).
  - Stdin shells, trailing operands (`$0`/`$1`, K7 removed), `-i`/`-l`/`--rcfile`/`--login`/`-o`/`+e`/`--`, `zsh -c`, and all 12 other outside-model shells emit `UnreadReason::UnsupportedNestedShellForm`.
- **P3 code-runner hook:** `build_units_with_hook` exposes `CodeRunnerHook<'a>` called post-unwrap for every unit with `(&CommandUnit, folded_name, budget)` so P3/P5 can attach code-runner `UnreadReason`s without editing P1. `build_units` delegates to `build_units_with_hook(..., None, budget)`.

### Contract §2: Best-effort pass (`lex_best_effort`, `v4/lex/best_effort.rs`)
- Produces a completely separate `BestEffortRecord { raw_texts, units }` so the exact model never mistakes a best-effort unit for `Clear`.
- Strictly linear byte-slice scanning (`scan_paren_bytes`, `skip_dq_bytes`, `scan_backtick_bytes`, `scan_dollar_brace_bytes`, bounded by `MAX_SCAN_NEST = 64`), tolerant wrapper unwrapping up to `MAX_BEST_EFFORT_WRAP = 32` (covering §10 row 5 `command` ×13 `sudo id`), CR-19 folded shell/wrapper/runner recognition, and recursive derivation up to `limits::MAX_DEPTH`.
- **Bounded code scans kept in the best-effort pass (listed one by one):**
  1. **Shell `-c` / `--command` / fish `-C` / `--init-command` scripts:** extracts inline scripts for all CR-19-folded shells in `TOOLS`/`OUTSIDE_MODEL` (plus `$SHELL`/`$BASH`/`$0`), without K7 positional resolution. Covers §10 row 4 (`sh -c 'X="$1" sudo id' _ foo`).
  2. **`eval` arguments:** joins operands after `eval [--]` and recurses at `depth + 1`.
  3. **`env -S` / `--split-string`:** extracts split-string payloads (combining trailing operands quoted) at **every** wrapper-unwrapping step (catching wrapped `command env -S 'sh -c' 'sudo id'`).
  4. **`script -c` / `--command` and `flock -c` / `--command`:** extracts inline command payloads at every wrapper-unwrapping step (catching wrapped `nice script -c 'sudo id' /dev/null`).
  5. **`find -exec` / `-execdir` / `-ok` / `-okdir`:** extracts sub-command argv slices up to `;`/`+` as derived `BestEffortUnit`s (and unwraps their wrappers/payloads).
  6. **Command & process substitutions:** extracts `$(...)`, `` `...` ``, `${...}` nested substitutions, `$((...))` nested substitutions, `<(...)` / `>(...)` process substitutions (and `<(...)` passed to shells/`source`/`.`), plus simple command-word substitutions `$(which CMD)` / `$(command -v CMD)` / `$(type -p CMD)` / `$(echo CMD)` (CR-19 folded) into `alt_argv`.
  7. **Here-doc & here-string bodies:** extracts bodies (and crontab command columns for `crontab`) fed to shells, interpreters, `eval`/`source`/`.`, code runners, reserved words, or pipelines.
  8. **Piped `echo` / `printf` into shell / `eval` / `source` / `.` / `crontab`:** extracts upstream `echo`/`printf` text operands in the same pipeline and recurses at `depth + 1`.
  9. **N11 bounded brace expansion (`brace_expand_budgeted`):** exposed as `pub(crate)` in `v4/lex.rs` for P5's best-effort path matching (`MAX_BRACE_RESULTS = 256`, `MAX_SEQUENCE = 64`, `MAX_BRACE_WORK = 2_000_000`, safe `i128` range arithmetic, linear on nested braces, `$`-guarded shortcut).

### Contract §8: Budgets, linearity, and caps
- Every stage (`lex_exact`, `fold_command_word`, `unwrap_unit`, `build_units`, `lex_best_effort`, `brace_expand_budgeted`) charges `ComputeBudget::charge` and `ComputeBudget::charge_alloc` **before** scanning, copying, or allocating.
- `Err(Exhausted)` is never swallowed; all units and outcomes discovered before exhaustion are preserved in `LexExactOutput`, `ParsedCommandUnits`, and `BestEffortRecord` (keeping all `PartId::Unit(idx)` indices in bounds).
- `BE_LEX_MEMO` (4-entry thread-local memo carried from M1-31 N11) records the exact `(compute_units, alloc_bytes)` spent and only hits when the remaining budget can afford both, charging identical totals on hits and falling through to incremental execution on tight budgets so exhaustion and partial-unit preservation remain 100% deterministic.
- `limits::MAX_BYTES` and `limits::MAX_UNITS` return `Err(PartOutcome::TooLarge(Cap::Bytes | Cap::Units))`.
- Linear charge scaling from 10 KB to `MAX_BYTES` is verified in `v4/lex/tests.rs` and `v4/units/tests.rs` on both single-unit, multi-unit, and adversarial unclosed `$(` inputs.

---

## 2. Disposition list for carried hunks and tests (contract §12)

### Source branch 1: `agent/hydrogen/M1-27-engine-name-fold`

#### Hunks
| Source file & hunk | Disposition | Where / why |
|---|---|---|
| `policy/shell.rs` `cmd_name` (CR-19 basename + lowercase + `ſ` → `s`, `K` → `k`) | **Rewritten** | Ported to `v4::lex::fold_command_word` & `fold_command_bytes` with `FoldRules` toggles, `ComputeBudget` pre-charging, and no Unicode normalization. |
| `policy/shell.rs` CR-19 comparisons in `lex_depth_uncached`, `runs_here`, `pipe_code_eff`, `code_texts_bounded`, `unwrap_reading_steps`, `is_shell`, `substituted_program_at`, `reads_stdin_as_script`, `echo_printf_texts`, `payloads_of`, `env_payloads_bounded`, `find_exec_units` | **Rewritten (best-effort only)** | Ported into `v4/lex/best_effort.rs` (Scans 1–8 above, all CR-19 folded and budgeted). Exact model uses `fold_command_word` for §4 wrapper/nested-shell lookup only; Home eligibility keeps as-written matching (P3). |
| `policy/deny.rs` CR-19 comparisons (`unknown_tok_dangerous`, `unknown_words_could_be_dangerous`, `is_interpreter`, `reads_stdin_as_code_at`, `is_executor`, `contains_downloader`, `force_push`, `PRIV`) | **Rewritten (best-effort / P5)** | `v4/lex/best_effort.rs` folds interpreter/shell/wrapper/runner names in Scans 1–8 using `fold_str_cr19`; exact `fold_command_word` folds `SUDO` / `/usr/bin/sudo` → `"sudo"` for P3/P5 tool-table lookup. Raw-text & `deny.rs` flag-level matching integration is P5. |
| `policy/shell.rs` K7 positional resolution (`PosResolve`, `uses_positional`, `parse_pos_script`, `resolve_positional_script`, `shell_c_codes_checked`, `payloads_of_checked`, `may_append_operands`) | **Dropped** | Explicitly removed by contract §4, §11 item 2, §12. Trailing operands after `sh -c SCRIPT` produce `UnreadReason::UnsupportedNestedShellForm` in exact mode; best-effort still extracts the raw `-c` script (§10 row 4). |
| `policy/shell.rs` K9 long-`env` parity (`expand_unknown_env_cmd` / `EnvReach` / `env_reach`) | **Dropped** | Explicitly removed by contract §4, §11 item 2, §12. Any `env` with options or assignments is `UnreadReason::UnsupportedWrapperForm` in exact mode; best-effort tolerantly unwraps `env`. |

#### Tests (`crates/atrium-core/tests/M1-27-engine-name-fold.rs`)
| Test | Disposition | Rev 4 expectation & contract citation |
|---|---|---|
| `folded_shells_match_lowercase_verdicts` | **Kept (ported to P1 unit tests; P5 integrates full verdicts)** | `v4/lex/tests.rs` & `v4/units/tests.rs`: `/bin/BASH -c …`, `/bin/baſ​h -c …`, `/bin/Ksh -c …` fold to `bash`/`ksh`; `sh`/`bash`/`dash -c` recursively modeled, `ksh` → `UnsupportedNestedShellForm`, all derived in best-effort (§4, §10 row 3). |
| `folded_wrappers_match_lowercase_verdicts` | **Kept with new expectation** | Modeled forms (`COMMAND`, `NICE`, `TIMEOUT 5`, `/usr/bin/ENV`) unwrap in exact model (§4); outside-model forms (`ENV -i`, `ENV -S`, `SCRIPT -c`, `ARCH`, `CHROOT`) → `UnsupportedWrapperForm` in exact model (§11 item 10), derived in best-effort (`v4/lex/tests.rs`, `v4/units/tests.rs`). |
| `folded_interpreters_and_substitutions_match_lowercase` | **Kept (ported to best-effort tests; P3/P5 integrate)** | `$(/usr/bin/WHICH sudo) id`, `FIND -exec`, `EVAL`, `CRONTAB`, `echo 'sudo id' \| BASH` produce derived units in `lex_best_effort` (`v4/lex/tests.rs`); exact model marks substitutions/runners `Unread` (§3, §4, §11 item 1). |
| `rule_matching_keeps_original_case` | **Deferred to P3/P5** | Exact `Word` retains original bytes/case (`CommandUnit::argv` is never mutated by folding); rule matching is P3/P5 (§6, §7). |
| `ordinary_lines_with_capitals_keep_dev_verdicts` | **Kept (ported to P1; P5 integrates)** | Uppercase ordinary words lex cleanly with original bytes preserved (`v4/lex/tests.rs`). |
| `escalated_sudo_case_folding_needs_deny_flags_outside_owned_range` | **Kept (now un-ignored under rev 4)** | `SUDO id` and `/usr/bin/sudo id` fold to `"sudo"` in `fold_command_word` (`v4/lex/tests.rs`, `v4/units/tests.rs`, §10 row 3). |
| `folded_deny_hard_denies_and_mixed_paths_match_lowercase` | **Kept (command folding in P1; path/deny integration in P2/P5)** | CR-19 command folding covered in `v4/lex/tests.rs`; path folding & hard-deny matching belong to P2/P5 (§5, §10 row 3). |
| `k7_exact_shape_resolves_to_payload_verdicts` | **Kept with new expectation** | All `sh -c SCRIPT _ args…` forms become `Unread(UnsupportedNestedShellForm)` in exact mode (§4, §11 item 2), while best-effort still extracts `SCRIPT` (§10 row 4; `v4/units/tests.rs`). |
| `k7_unquoted_expansion_asks_or_hard_denies_on_default_reading` | **Dropped (replaced by §10 row 4)** | K7 positional resolution removed (§4, §11 item 2, §12); `sh -c 'X="$1" sudo id' _ foo` covered in `v4/units/tests.rs` & `v4/lex/tests.rs`. |
| `k7_unresolvable_positional_sh_c_fails_closed` | **Kept with new expectation** | Trailing operands after `-c SCRIPT` are `Unread(UnsupportedNestedShellForm)` (Ask in Home/rules, Allow\* under unscoped Any/FullAccess after best-effort; §2, §4, §11 items 1–2). |
| `k7_here_document_body_is_still_checked` | **Kept (best-effort Scan 7)** | Here-doc bodies fed to shells are still extracted by `lex_best_effort` (`v4/lex/tests.rs`) and marked `ReservedWordOrConstruct { construct: "<<" }` in `lex_exact` (§2, §3). |
| `k7_reading_size_is_bounded_before_allocation` | **Dropped (replaced by §8 budget tests)** | K7 builder removed (§12); pre-allocation budget charging verified in `v4/lex/tests.rs` & `v4/units/tests.rs` (§8). |
| `k9_long_readable_env_matches_short_form` | **Dropped** | K9 parity explicitly removed (§4, §11 items 2 & 10, §12); any `env` with options/assignments is `Unread(UnsupportedWrapperForm)` (`v4/units/tests.rs`). |
| `k9_genuinely_unbounded_env_still_fails_closed` | **Dropped (replaced by §4 & §8 tests)** | `env` with options is `Unread(UnsupportedWrapperForm)` (§4, §11 item 10), and padded lines hitting budget are `Exhausted` (`v4/units/tests.rs`, §8). |

### Source branch 2: `agent/hydrogen/M1-31-credential-folder-search`

#### Hunks (P1-owned items only; S8/K10/`Sensitive` belong to P2)
| Source file & hunk | Disposition | Where / why |
|---|---|---|
| `policy/words.rs` N11 bounded brace expansion (`brace_expand`, `brace_sequence`, `brace_results_lower_bound`, `MAX_BRACE_RESULTS`, `MAX_SEQUENCE`, `MAX_BRACE_WORK`, `BraceWords`, `$`-guard, safe range math, linear nested-brace stack) | **Rewritten (best-effort only)** | Ported to `v4::lex::best_effort::brace_expand_budgeted` (re-exported `pub(crate)` from `v4::lex`) with `ComputeBudget` pre-charging and `i128` range arithmetic. Never used by exact lexing (where `{` is `Unread`). |
| `policy/shell.rs` N11 `LEX_MEMO` (4-entry `thread_local!` memo in `shell::lex`) | **Rewritten (budget-deterministic)** | Ported to `BE_LEX_MEMO` in `v4/lex/best_effort.rs`, storing `(line, units, compute_units, alloc_bytes)` so hits charge identical compute & alloc units and tight budgets bypass the memo to run incrementally and preserve partial units (§8). |
| `policy/deny.rs` / `policy/engine.rs` (S8, K10 glob normalizer, shared `Sensitive`) | **Not P1 (owned by P2 / dropped per §12)** | Left untouched by P1. |

#### Tests (`crates/atrium-core/tests/M1-31-credential-folder-search.rs`, N11 P1 items)
| Test | Disposition | Rev 4 expectation & contract citation |
|---|---|---|
| `review1_f6_full_i64_brace_range_does_not_panic` | **Kept (ported to `v4/lex/tests.rs`)** | `brace_expand_budgeted("{9223372036854775807..-9223372036854775808}")` returns `Ok(Some([literal]))` without overflow/panic; exact lexer marks `{` Unread (§3, §11 item 1). |
| `review1_f7_nested_braces_are_linear` | **Kept (ported to `v4/lex/tests.rs`)** | `brace_expand_budgeted` on `{`×10,000 `x` `}`×10,000 charges linearly (`< 8 * len` units; §8). |
| `review1_f10_brace_shortcut_does_not_refuse_dollar_words` | **Kept (ported to `v4/lex/tests.rs`)** | `$`-bearing words bypass the lower-bound shortcut: `{,$}`×9 hits `MAX_BRACE_RESULTS` → `Ok(None)`, `{,$}{a,b}` succeeds (`Ok(Some(...))`). |
| Remaining tests in `M1-31-credential-folder-search.rs` (S8, K10, `Sensitive` cache, `grep -d`, `git -C`, `GlobBudget`) | **Not P1 (owned by P2 / dropped per §12)** | Handled by P2 (`M1-38-rev4-p2-paths`). |

---

## 3. What was reused from `dev`'s evaluator

- **Runtime code (`lex.rs`, `lex/best_effort.rs`, `units.rs`): zero items from `dev`'s evaluator (`shell.rs`, `words.rs`, `deny.rs`, `engine.rs`, `gate.rs`).**
  - All reserved words, shells, wrappers, wrapper flags, nested-shell flags, and code runners are read exclusively from P0's frozen `policy::tools` (`lookup`, `lookup_outside_model`, `outside_model`, `flag`, `flag_short`) and `policy::limits`.
  - Best-effort lexing and N11 bounded brace expansion were implemented cleanly under `ComputeBudget` in `v4/lex/best_effort.rs` rather than calling unbudgeted `shell::lex` or `words::brace_expand`.
- **Consequence for P5:** P5 can delete `policy/shell.rs`, `policy/words.rs`, and `policy/deny.rs` without touching a single line of P1.

---

## 4. Change requests against P0's interface

**None.** All five P0 stubs (`lex_exact`, `lex_best_effort`, `fold_command_word`, `build_units`, `unwrap_unit`) match P0's frozen signatures and `model.rs` / `limits.rs` / `tools.rs` types verbatim.

Additional P1-owned `pub(crate)` helpers provided for P3/P5 convenience (without altering any P0 stub):
- `v4::units::build_units_with_hook` (and `CodeRunnerHook`) — allows P3/P5 to attach code-runner `UnreadReason`s during unit building (§4).
- `v4::lex::brace_expand_budgeted` — exposes N11 budgeted brace expansion for P5's best-effort path matching (§2, §12).

---

## 5. Verification commands

### Run by worker (allowed under D-123)
- `cargo fmt --all` — clean
- `cargo check -p atrium-core --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core --tests -- -D warnings` — clean (0 errors, 0 warnings)

### Commands for the orchestrator's verification task (fish-compatible)
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core policy::v4:: -- --nocapture
cargo test --workspace --no-fail-fast
```

---

## Round 2 (P0 final, D-141)

Adapted P1 to P0's final D-141 interface (`a1f0174`), where `ToolSpec` carries `{ name, platforms, kind: ToolRowKind, changes_default }` across the six row kinds and 70 tools moved out of `TOOLS` into `UNKNOWN_BY_DESIGN`.

### Changes by call site
- **`crates/atrium-core/src/policy/v4/units.rs`:**
  - `process_text_recursive` (nested-shell check): `spec.nested_shell` → `spec.is_nested_shell()` (matches `ToolRowKind::NestedShell(_)`, i.e., `sh`, `bash`, `dash`).
  - `unwrap_unit_preserving`: `spec.wrapper` → `spec.wrapper()` (matches `ToolRowKind::Wrapper(_)`, i.e., §4's 6 closed wrappers).
  - `parse_wrapper_prefix` (`WrapperKind::Nice`, `WrapperKind::Timeout`) and `classify_nested_shell`: `spec.flags` → `spec.flags()` (3 call sites + doc comment).
  - `lookup_outside_model` checks for the 12 outside-model shells and 16 outside-model wrappers remain exact against D-141's `OUTSIDE_MODEL` entries.
- **`crates/atrium-core/src/policy/v4/lex/best_effort.rs`:**
  - Replaced `use crate::policy::tools::{self, CodeRunnerBehavior}` with `use crate::policy::tools::{self, ToolRowKind, ToolSpec}`.
  - `is_be_shell`: `tools::lookup(&folded).is_some_and(|t| t.nested_shell)` → `tools::lookup(&folded).is_some_and(ToolSpec::is_nested_shell)`.
  - `is_be_code_runner`: `tools::lookup(folded).is_some_and(|t| !matches!(t.code_runner, CodeRunnerBehavior::Never))` → `tools::lookup(folded).is_some_and(|t| matches!(t.kind, ToolRowKind::CodeRunner(_)))`.

### Decision: "is a code runner" in the best-effort pass (`is_be_code_runner`)
- `is_be_code_runner` is called only in **Scan 7** (`lex_one_text`, deciding whether a here-doc or here-string body fed to `owner_unit` on stdin should be recursively lexed as shell commands).
- Under D-141, **`is_be_code_runner` matches `ToolRowKind::CodeRunner(_)` only, excluding `ToolRowKind::ConditionalRunner(_)` (`find`, `git`, `sed`)**:
  - Unconditional `CodeRunner` tools (`awk`/`gawk`/`mawk`, `perl`/`python`/`python3`/`node`/`ruby`, `eval`/`xargs`, `source`/`.`/`npx`, `cargo`/`pipx`, and all 29 `shell::COMMAND_RUNNERS` such as `ssh`, `docker`, `kubectl`, `parallel`, `su`) can execute code or commands supplied on stdin, so Scan 7 keeps extracting their heredoc/herestring bodies.
  - `ConditionalRunner` (`find`, `git`, `sed`) is excluded from Scan 7: `git` reads data on stdin (e.g. `git commit -F - <<EOF`, `git apply <<EOF`), never shell code (even its code-running triggers come from argv/config); `find` does not execute stdin (`-exec`/`-execdir`/`-ok`/`-okdir` are already extracted from argv by Scan 5); and plain `sed 's/a/b/' <<EOF` processes text data (its `e`/`w`/`r`/`-f` triggers are classified as `Unread` by P3 in the exact model). Excluding `ConditionalRunner` avoids false hard-deny hits on commit messages or plain text data in heredocs while matching `dev`'s `reads_stdin_as_script` behaviour.

### Test audit against D-141's table
- Audited all existing tests in `v4/lex/tests.rs` and `v4/units/tests.rs`: none depended on per-tool rows removed under D-141 (`cat` remains `HomeReadOnly`; `sh`/`bash`/`dash`, all 6 wrappers, all 12 outside shells, all 16 outside wrappers, and all 19 reserved words remain in `TOOLS` / `OUTSIDE_MODEL`; `SUDO id` / `echo` / `printf` / `true` receive §4 unknown-tool treatment in P1 without emitting spurious `Unread`, while `printf -v HOME` still triggers `HomeSetEarlier` by folded name).
- Added `lex_best_effort_heredocs_follow_d141_code_runner_vs_conditional_runner` in `crates/atrium-core/src/policy/v4/lex/tests.rs` verifying `CodeRunner` vs `ConditionalRunner` heredoc handling in Scan 7.
- Added `build_units_treats_d141_unknown_by_design_tools_as_unknown` in `crates/atrium-core/src/policy/v4/units/tests.rs` verifying `rm`, `curl`, `patch`, `diff`, `sudo` are in `UNKNOWN_BY_DESIGN` (not `TOOLS` or `OUTSIDE_MODEL`), unwrap cleanly under §4 wrappers, and emit no P1 `Unread`.

### Change requests against P0's interface
- **None.** `ToolSpec::{flags, wrapper, is_nested_shell}`, `ToolRowKind::CodeRunner`, `lookup`, `lookup_outside_model`, `lookup_unknown_by_design`, `flag`, and `flag_short` cover every P1 call site cleanly with no edits to `tools.rs`, `model.rs`, or `limits.rs`.

### Round 2 verification
Run by worker (allowed under D-123):
- `cargo fmt --all` — clean
- `cargo check -p atrium-core --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core --tests -- -D warnings` — clean (0 errors, 0 warnings)

Commands for the orchestrator's verification task (fish-compatible):
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core policy::v4:: -- --nocapture
cargo test --workspace --no-fail-fast
```
