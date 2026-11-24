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
  - Globs: `*`, `?`, and non-empty closed `[...]` without `!`, `^`, nested `[`, `/`, or non-ASCII (`[!…]` / `[^…]` → `NegatedGlobClass`; empty/unclosed/stray `]`/nested `[`/slash → `UnquotedByteOutsideModel`). Permitted §3 quotes and escapes inside `[...]` are processed while retaining class state and per-byte provenance.
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
- **Command-word folding (`fold_command_word`):** takes the basename after the last `/`, applies string-level Unicode `str::to_lowercase()` (preserving contextual rules such as Greek final sigma `'ΟΣ'` → `"ος"`) plus `FoldRules::{fold_long_s, fold_kelvin}` (`ſ` U+017F → `s`, `K` U+212A → `k`), with budgeted pre-allocation and **no** Unicode normalization (NFC/NFD preserved verbatim).
- **Command-word globs:** an unquoted glob in effective `argv[0]` emits `UnreadReason::CommandWordGlob` (not expanded as a wrapper or tool).
- **Closed wrapper list (`unwrap_unit`):** reads wrapper specs and flags from P0's `policy::tools::{lookup, lookup_outside_model, flag}` without editing `tools.rs`. Strips only:
  - `command [--]`
  - `exec` (no options)
  - `nice`, `nice -n N`
  - `timeout [-s SIG] [-k D] D`
  - `time` (no options)
  - bare `env` (no options, no `=` assignments — even quoted)
  - Bounded by `limits::MAX_WRAP` (8); reaching 8 wrapper removals (7 remain Clear, 8th triggers the limit) immediately emits `UnreadReason::MaxWrapReached` and stops (§4). Any other wrapper option/near miss or any of the 16 outside-model wrappers in `tools::OUTSIDE_MODEL` emits `UnreadReason::UnsupportedWrapperForm`.
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
| `rule_matching_keeps_original_case` | **Deferred to P3/P5** | Exact `Word` retains original bytes/case (`CommandUnit::argv` is never mutated by folding); rule matching is P3/P5 (§6, §7, §10 row 1, §11 "Unchanged on purpose: Home eligibility by the command word as written"). |
| `ordinary_lines_with_capitals_keep_dev_verdicts` | **Kept (ported to P1; P5 integrates)** | Uppercase ordinary words lex cleanly with original bytes preserved (`v4/lex/tests.rs`; §10 row 1, §11 "Unchanged on purpose: Home eligibility by the command word as written"). |
| `escalated_sudo_case_folding_needs_deny_flags_outside_owned_range` | **Kept (now un-ignored under rev 4)** | `SUDO id` and `/usr/bin/sudo id` fold to `"sudo"` in `fold_command_word` (`v4/lex/tests.rs`, `v4/units/tests.rs`, §10 row 3). |
| `folded_deny_hard_denies_and_mixed_paths_match_lowercase` | **Kept (command folding in P1; path/deny integration in P2/P5)** | CR-19 command folding covered in `v4/lex/tests.rs`; path folding & hard-deny matching belong to P2/P5 (§5, §10 row 3). |
| `k7_exact_shape_resolves_to_payload_verdicts` | **Kept with new expectation** | All `sh -c SCRIPT _ args…` forms become `Unread(UnsupportedNestedShellForm)` in exact mode (§4, §11 item 2), while best-effort still extracts `SCRIPT` (§10 row 4; `v4/units/tests.rs`). |
| `k7_unquoted_expansion_asks_or_hard_denies_on_default_reading` | **Dropped (replaced by §10 row 4)** | K7 positional resolution removed (§4, §11 item 2, §12); `sh -c 'X="$1" sudo id' _ foo` covered in `v4/units/tests.rs` & `v4/lex/tests.rs`. |
| `k7_unresolvable_positional_sh_c_fails_closed` | **Kept with new expectation** | Trailing operands after `-c SCRIPT` are `Unread(UnsupportedNestedShellForm)` (Ask in Home/rules, Allow\* under unscoped Any/FullAccess after best-effort; §2, §4, §11 items 1–2). |
| `k7_here_document_body_is_still_checked` | **Kept (best-effort Scan 7)** | Here-doc bodies fed to shells are still extracted by `lex_best_effort` (`v4/lex/tests.rs`) and marked `ReservedWordOrConstruct { construct: "<<" }` in `lex_exact` (§2, §3, §11 items 1–2). |
| `k7_reading_size_is_bounded_before_allocation` | **Dropped (replaced by §8 budget tests)** | K7 builder removed (§12); pre-allocation budget charging verified in `v4/lex/tests.rs` & `v4/units/tests.rs` (§8, §11 "Unchanged on purpose: Unbounded work asks everywhere"). |
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
| `review1_f7_nested_braces_are_linear` | **Kept (ported to `v4/lex/tests.rs`)** | `brace_expand_budgeted` on `{`×10,000 `x` `}`×10,000 charges linearly (`< 8 * len` compute units, auxiliary stack/pair vectors budgeted before growth; §8, §11 item 1 & "Unchanged on purpose: Unbounded work asks everywhere"). |
| `review1_f10_brace_shortcut_does_not_refuse_dollar_words` | **Kept (ported to `v4/lex/tests.rs`)** | `$`-bearing words bypass the naive lower-bound shortcut: `"{,$}".repeat(9)` succeeds with 89 bounded expansions (`Ok(Some(_))`, because `$` suppresses the immediately following `{`), preserving M1-31's FullAccess Allow (§2, §11 item 1); genuinely overflowing cases (`"{a,b}".repeat(9)` and `"{,$}".repeat(12)`) return `Ok(None)` (§11 "Unchanged on purpose: Unbounded work asks everywhere"). In exact lexing, unquoted `{` is `Unread` (§3, §11 items 1–2). |
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

---

## Round 3 (Review round 1 rework — all 9 findings)

Addressed all 9 findings from `.orchestration/review-input/review-round1.md` plus the `sed -f -` heredoc documentation test gap and handoff disposition corrections. P0's frozen interface (`model.rs`, `limits.rs`, `tools.rs`) remains untouched.

### Finding-by-finding fixes and regression tests
1. **Finding 1 (Major — Exhaustion drops a fully scanned unit):**
   - **Fix:** Charged unit metadata, output vector growth, and redirect-only reason allocation **before** `std::mem::replace(cur, ...)` in both `crates/atrium-core/src/policy/v4/lex.rs` (`flush_unit`) and `crates/atrium-core/src/policy/v4/lex/best_effort.rs` (`flush_builder`), so exhaustion leaves the pending unit intact for the recovery handler to preserve both its words and its `UnreadReason`s (§8).
   - **Test:** `exhaustion_at_flush_preserves_pending_unit_words_and_reasons` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (covers `lex_exact("sudo id", 0, None, &mut ComputeBudget::new(16, 10_000))`, `lex_exact("sudo $id", ...)` preserving `UnquotedByteOutsideModel`, allocation exhaustion at `flush_unit`, and `lex_best_effort_into(&["sudo id"], ...)` at `flush_builder`).
2. **Finding 2 (Major — Significant allocations bypass the shared budget):**
   - **Fix:** Added shared budgeted collection helpers (`charge_vec_growth`, `vec_push_budgeted`, `vec_extend_budgeted`, `vec_with_capacity_budgeted`, `charge_string_growth`) in `crates/atrium-core/src/policy/v4/lex.rs` and audited all three owned runtime files:
     - `crates/atrium-core/src/policy/v4/lex/best_effort.rs`: charged all auxiliary vectors/stacks in `brace_expand_budgeted`, `brace_sequence`, and `brace_results_lower_bound` (rewritten in O(n) open-order without sorting), charged cold `BE_LEX_MEMO` insertion copies (skipping optional cache insertion when unaffordable), accounted for `Vec<String>` / redirect headers in `estimate_unit_bytes`, and budgeted `raw_to_record_idx`, `raw_units`, `heredocs`, `heredoc_owners`, `bodies`, `cur.*`, `deferred_payloads`, `pipeline_units`, `checked_pipes`, `all_alt`, `find_execs`, and `record.units`/`raw_texts` growth.
     - `crates/atrium-core/src/policy/v4/lex.rs`: charged `cur.argv`, `cur.redirects`, `cur.unreads`, `heredocs`, `delim`, `out.units`, `out.outcomes`, and unread reason strings before allocating them.
     - `crates/atrium-core/src/policy/v4/units.rs`: eliminated the uncharged `local_to_global` vector via arithmetic offset (`UnitIndex(base_idx + i)`), charged `parsed.units` / `parsed.outcomes` / `nested_scripts` / `removed_wrappers` growth, and charged `UnsupportedNestedShellForm` string copies before allocating.
   - **Test:** `auxiliary_allocations_and_cold_memo_copies_are_budgeted` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (verifies `"{" × 10000 + "x" + "}" × 10000` with `ComputeBudget::new(1_000_000, 200_100)` returns `Err(Exhausted { reason: AllocationBudget })`, verifies full auxiliary charge under default limits, and verifies cold memo copy budgeting and unaffordable-skip without exhaustion).
3. **Finding 3 (Major — `MAX_WRAP` enforced one wrapper too late):**
   - **Fix:** In `crates/atrium-core/src/policy/v4/units.rs` (`unwrap_unit_preserving`), check `removed_wrappers.len() >= limits::MAX_WRAP` immediately after pushing each stripped wrapper so reaching 8 removals emits `UnreadReason::MaxWrapReached` and stops (§4).
   - **Test:** Updated `unwrap_unit_max_wrap_boundary` in `crates/atrium-core/src/policy/v4/units/tests.rs` to assert 7 wrappers (`MAX_WRAP - 1`) stay Clear while reaching 8 wrappers (`"command " × 8 + "cat a"`) and 9 wrappers emit `MaxWrapReached` (tested via both `unwrap_unit` and `build_units`).
4. **Finding 4 (Major — Cached units reuse pipeline IDs from separate pipelines):**
   - **Fix:** In `crates/atrium-core/src/policy/v4/lex/best_effort.rs`, both `BE_LEX_MEMO` and the per-call recursion cache (`state.cache`) normalize stored `PipelineId`s to 0-based relative offsets alongside `pipe_count`, and both cache-hit paths assign fresh `PipelineId`s offset from `state.next_pipe_id` and advance `state.next_pipe_id` by `pipe_count` (§2).
   - **Test:** `cached_best_effort_hits_assign_fresh_pipeline_ids` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (covers warming `BE_LEX_MEMO` with `curl x | cat` then lexing `["curl x | cat", "echo ok | sh"]`, duplicate cached texts, repeated cached nested scripts containing pipelines via `state.cache`, and cold/warm multi-text equivalence).
5. **Finding 5 (Minor — Command folding differs from P0's Unicode lowercase rule):**
   - **Fix:** Updated `fold_command_bytes` in `crates/atrium-core/src/policy/v4/lex.rs` to use string-level `str::to_lowercase()` (preserving contextual Greek final sigma `'ΟΣ'` → `"ος"`) with budgeted pre-allocation, `FoldRules::{fold_long_s, fold_kelvin}` support, and zero Unicode normalization (§4).
   - **Test:** Extended `fold_command_word_cr19_rules_and_no_unicode_normalization` in `crates/atrium-core/src/policy/v4/lex/tests.rs` with `'ΟΣ'` → `"ος"`, medial vs final sigma, `FoldRules` interactions, and Unicode allocation-budget exhaustion.
6. **Finding 6 (Minor — Valid quoting inside a glob class is rejected):**
   - **Fix:** Factored single-quote, double-quote, and unquoted-escape scanning helpers in `crates/atrium-core/src/policy/v4/lex.rs` and used them inside the `[...]` class loop so permitted §3 quotes and escapes inside a glob class preserve class state (`class_len`, closing unquoted `]`, `!`/`^` negation check) and per-byte provenance (§3).
   - **Test:** `lex_exact_allows_permitted_quotes_and_escapes_inside_glob_class` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (covers `cat /usr/bi[n'n']`, double quotes, permitted escapes, escaped `\]`, and disallowed/negated/empty/unclosed class cases).
7. **Finding 7 (Minor — Empty quotes lose their effect on keyword recognition):**
   - **Fix:** Retained token-level `contained_quoting` and `is_unquoted_assignment` flags on `ReadWordOutcome` in `crates/atrium-core/src/policy/v4/lex.rs`, using them for reserved-word recognition, leading assignment recognition (suppressing `''FOO=1` / `FOO''=1` while keeping `FOO=''`), and plain-unquoted `n>&-` close targets (§3).
   - **Test:** `lex_exact_empty_quotes_suppress_keywords_and_assignments` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (covers `''if foo`, `""if foo`, `if'' foo`, empty-quoted assignment names vs value-quoted assignments, and `2>&''-`).
8. **Finding 8 (Minor — Brace failure is a test defect):**
   - **Fix:** Updated `brace_expand_budgeted_carries_m1_31_n11_guarantees` in `crates/atrium-core/src/policy/v4/lex/tests.rs` to expect `Ok(Some(expanded))` with 89 bounded results for `"{,$}".repeat(9)` (preserving M1-31 review-1 finding 10), while adding separate genuinely overflowing cases (`"{a,b}".repeat(9)` and `"{,$}".repeat(12)` → `Ok(None)`). Corrected the N11 disposition in §2 above and added §10/§11 citations across all retained entries.
9. **Finding 9 (Minor — Linearity failure is a test defect):**
   - **Fix:** Updated `linear_charge_growth_from_10kb_to_max_bytes` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (and `build_units_and_unwrap_unit_budget_exhaustion_and_linearity` in `crates/atrium-core/src/policy/v4/units/tests.rs`) to measure both compute **and allocation** scaling under explicit generous budgets, and separately assert that `adv_large` returns `Exhausted(AllocationBudget)` under P0's frozen default limits while preserving earlier units found before exhaustion.
10. **Test gap (`sed -f -` / `sed --file=-` heredoc documentation):**
    - **Test:** Added `lex_best_effort_sed_stdin_script_heredoc_limit_matches_dev` in `crates/atrium-core/src/policy/v4/lex/tests.rs` documenting that Scan 7 intentionally omits `sed` (`ConditionalRunner`) heredoc bodies (matching `dev`, while `lex_exact` still marks `<<` as `Unread`).

### Corrected dispositions
- `review1_f10_brace_shortcut_does_not_refuse_dollar_words` (§2 table above): corrected to reflect `"{,$}".repeat(9)` returning `Ok(Some(_))` (89 results, FullAccess Allow; §2, §11 item 1) and genuinely overflowing inputs returning `Ok(None)` (§11 "Unchanged on purpose: Unbounded work asks everywhere").
- Added missing §10/§11 citations to `rule_matching_keeps_original_case`, `ordinary_lines_with_capitals_keep_dev_verdicts`, `k7_here_document_body_is_still_checked`, `k7_reading_size_is_bounded_before_allocation`, and `review1_f7_nested_braces_are_linear`.
- Corrected §1 `MAX_WRAP`, glob-class quoting, and Unicode `str::to_lowercase()` descriptions above.

### Change requests against P0's interface
- **None.**

### Round 3 verification
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

---

## Round 4 (Review round 2 rework — all 6 findings)

Addressed all 6 findings from `.orchestration/review-input/review-round2.md` (3 major, 3 minor, including the 2 round-3 test defects). P0's frozen interface (`model.rs`, `limits.rs`, `tools.rs`) and `SessionGate` remain untouched.

### Finding-by-finding fixes and regression tests
1. **Finding 1 (Major — Best-effort assignment detection performs uncharged quadratic work):**
   - **Fix:** Added an O(1)-per-byte incremental `AssignPrefix` state machine (`Empty` / `ValidName` / `ValidNamePlus` / `Invalid`) shared by `crates/atrium-core/src/policy/v4/lex.rs` and `crates/atrium-core/src/policy/v4/lex/best_effort.rs`, and tracked `seen_unquoted_eq` and `all_unquoted_digits` incrementally in `best_effort.rs` so only the first unquoted `=` is considered once with zero prefix rescanning. Charged remaining per-word scans before performing them (`Dup` redirect digit check, `has_cmd_sub`, and `env` wrapper `split_once('=')` / `is_posix_name`).
   - **Test:** `best_effort_repeated_equals_after_invalid_assignment_prefix_is_linear` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (covers the exact 60,000-byte failing case `"echo " + "a".repeat(60_000) + "-" + "=".repeat(60_000)` across both `lex_best_effort` and `lex_exact`, asserts an affine linear compute bound and linear scaling against a 6,000-byte fixture, and checks incremental assignment edge cases).
2. **Finding 2 (Major — Exact-word buffer allocations still bypass size accounting):**
   - **Fix:** Added `WordBuilder` in `crates/atrium-core/src/policy/v4/lex.rs` whose `reserve_growth` builds both the `Vec<u8>` byte buffer and `Vec<ByteProvenance>` provenance buffer using budgeted geometric capacity growth (minimum capacity 8 each), charging reallocation copy work (`old_cap`) and allocated capacity deltas (`target_cap - old_cap`) across both vectors before calling `reserve_exact`, and then constructs each `Word` via P0's frozen `Word::from_parts` without calling `Word::push`.
   - **Test:** `lex_exact_word_buffer_capacity_and_realloc_copy_are_budgeted` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (covers the review's exact-budget failing case `lex_exact("x", ...)` with `2 + 4 * size_of::<Word>() + 4 * size_of::<RawCommandUnit>()`, verifies the exact 15-byte-exhausts / 16-byte-succeeds boundary, verifies 8 → 16 reallocation capacity and 8 + 8 = 16 copy compute units on `"abcdefghi"`, and updates `exhaustion_at_flush_preserves_pending_unit_words_and_reasons`'s pre-flush allocation calculation for `"sudo id"` to reflect the 16 + 16 = 32-byte word capacities).
3. **Finding 3 (Major — The carried `find -exec` scan omits nested `-c` scripts):**
   - **Fix:** Factored `resolve_cmd_sub_readings`, `collect_shell_proc_sub_payloads`, and `process_find_exec_cmds` in `crates/atrium-core/src/policy/v4/lex/best_effort.rs` and tagged `deferred_payloads` with their target recursion depth so commands extracted from `find -exec` / `-execdir` / `-ok` / `-okdir` run through the same bounded wrapper unwrapping, command-word substitution resolution, `extract_code_payloads` (across both effective and alternative readings), shell `<(...)` process-substitution extraction, nested `find -exec` extraction, and recursive lexing as ordinary units, strictly respecting `limits::MAX_DEPTH` and `limits::MAX_UNITS`.
   - **Test:** `lex_best_effort_find_exec_recurses_into_nested_scripts_within_caps` in `crates/atrium-core/src/policy/v4/lex/tests.rs` (covers `find . -exec sh -c 'sudo id' \;` asserting derived `["sh", "-c", "sudo id"]` and `["sudo", "id"]` units, plus `-execdir bash -c 'eval ...' +`, alternative wrapper reading `command -Z arg sh -c ...`, `$(which sh) -c ...`, nested `find -exec find ... -exec ...`, `MAX_DEPTH` cutoff, and `MAX_UNITS` `TooLarge(Cap::Units)` enforcement).
4. **Finding 4 (Minor — Empty quotes still disappear during HOME assignment detection):**
   - **Fix:** Added private (`pub(super)`) `RawUnitLexMeta` / `LexExactWithMeta` and `lex_exact_with_meta` in `crates/atrium-core/src/policy/v4/lex.rs` (budgeted via `charge_vec_growth` before moving `cur` in `flush_unit`, while leaving P0's `lex_exact` signature and zero-overhead path unchanged) so `crates/atrium-core/src/policy/v4/units.rs` consumes the lexer's token-level leading-assignment count and `HOME`-assignment flag directly instead of reconstructing assignment syntax from decoded bytes. Removed `leading_assignment_name`, `has_leading_home_assignment`, and `leading_assignment_count`.
   - **Test:** `build_units_empty_quoted_home_does_not_trigger_home_set_earlier` in `crates/atrium-core/src/policy/v4/units/tests.rs` (covers `''HOME=/tmp; cat ~/x` plus `""HOME=/tmp`, `HOME''=/tmp`, `H""OME=/tmp`, `HOME+''=/tmp`, `''FOO=1 export HOME=/tmp`, `FOO=1 ''HOME=/tmp`, and positive value-quoted contrasts `HOME=''/tmp`, `HOME="/tmp"`, `HOME+=/tmp`, `FOO=1 HOME=/tmp`).
5. **Finding 5 (Minor — Reported glob-class failure is a test defect):**
   - **Fix (test only, no implementation change):** Updated `lex_exact_allows_permitted_quotes_and_escapes_inside_glob_class` in `crates/atrium-core/src/policy/v4/lex/tests.rs` to expect `r"cat /usr/bi[n\]]"` to be accepted (no `UnreadReason`, literal bytes `b"/usr/bi[n]]"`, `provenance()[9] == ByteProvenance::Escaped`, `provenance()[10] == ByteProvenance::Unquoted`), and added `r"cat /usr/bi[n\]"` as the genuinely unclosed case emitting `UnreadReason::UnquotedByteOutsideModel { byte: b'[' }` with `provenance()[9] == ByteProvenance::Escaped`.
6. **Finding 6 (Minor — Reported allocation-linearity failure is a test defect):**
   - **Fix (test only, no implementation change):** Updated `linear_charge_growth_from_10kb_to_max_bytes` in `crates/atrium-core/src/policy/v4/lex/tests.rs` and `build_units_and_unwrap_unit_budget_exhaustion_and_linearity` in `crates/atrium-core/src/policy/v4/units/tests.rs` to assert §8 affine upper bounds `C(n) <= a*n + b` and `A(n) <= a*n + b` (with explicit fixed metadata overhead `b`) plus upper growth-rate bounds and monotonic positive growth, removing the spurious proportional lower bounds while keeping default-budget exhaustion checks separate.

### Change requests against P0's interface
- **None.**

### Round 4 verification
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

---

## Round 5 (Opus 5.5, D-143)

Escalated rework for the three open majors of `.orchestration/review-input/review-round3.md`. All three are one class: work or allocation done before, or without, a charge. The fix is a budget discipline applied to every function in P1's files, written down in `v4/lex.rs`'s module docs:

1. **Charge, then work.** Every scan or copy of input-derived bytes charges one unit per byte before it runs. Every lookup in P0's tables charges its row scan before it runs (`lookup_tool`, `lookup_outside`, `flag_charged`, `flag_short_charged`; costs come from per-length row counts computed at compile time from P0's tables). Every comparison of two input-derived strings charges the bytes it may compare (`bytes_eq`, `readings_eq`). Comparisons against compile-time constants are bounded by the constant and ride on the step's charge.
2. **Charge, then allocate.** Every `Vec`/`String` growth charges its new capacity and the copy of the old elements before reserving.
3. **Reserve, then discover.** A value's storage (argv slot, redirect slot, reason slot, the pending unit's output slot, the slot that records the final `Exhausted`) is reserved and charged before the work that produces the value. Storing a finished value is then infallible (`push_reserved`, debug-asserted), so exhaustion never drops anything found earlier, and recovery after exhaustion allocates nothing. A word cut off by exhaustion is dropped whole, never kept truncated: a truncated `~/.ssh/id_rsa.pub` would read `~/.ssh/id_rsa`, and exhaustion must never create a hit (§8).

### The three findings

1. **Finding 1 (completed word lost on allocation exhaustion).**
   - **Fix:** the exact lexer writes through an `ExactSink`. Before a word is scanned, `reserve_word_storage` reserves the pending unit's output slot, the word's argv slot and, in command position, a *held* reason slot plus its unit of work for the `Assignment` / reserved-word reason. Before a redirect target is read, `lex_exact_redirect` reserves the redirect slot, two held reason slots, their work and the longest reason string (16 bytes). Reasons recorded while the word is scanned reserve beyond the held slots (`ReasonSink::hold` / `push_held`), so `FOO=$x` cannot use up the slot its `Assignment` needs. The keyword check after a command word charges its reason string first. `lex_exact` reserves the `Exhausted` slot on entry. If even that fails it returns `Err(PartOutcome::Exhausted)` before lexing anything. The best-effort builder reserves its word's slot (argv, redirect or here-string) before scanning it, reserves the assignment slot at the `=` while still scanning, and pays in advance (byte by byte) for the copy that the effective reading or recovery makes later. The assignment name/value split uses that advance payment.
   - **Tests:** `r5_f1_completed_word_or_redirect_is_never_dropped` (the review's case `lex_exact("sudo", …, ComputeBudget::new(1000, 16))`; the same shape one step later, where `sudo` is scanned and a later 4-byte allocation fails and the word is kept; redirect targets, assignments and best-effort redirects/assignments one unit short). The class sweeps below.
2. **Finding 2 (uncharged scans in exact wrapper validation).**
   - **Fix:** `is_signed_integer`, `is_timeout_signal`, `is_timeout_duration` are single passes charged by length (`bytes_with_glob` fuses the glob check). `parse_wrapper_prefix` charges the `env` operand's `=` scan, the UTF-8 checks (`word_as_str`) and flag lookups. The unwrap loop charges the command-word glob scan and the table lookups. `classify_nested_shell` charges glob and UTF-8 scans, each flag letter's lookup and the script copy. `command_modifies_home` charges each argument (one fused pass for `env`), and `unit_uses_unquoted_tilde` charges its walk. Wrapper words are moved out with one charged argv shift, not one per wrapper.
   - **Tests:** `r5_f2_wrapper_operand_scans_are_charged` (the review's case: `nice -n` + 60,000 zeros, `unwrap_unit` with `ComputeBudget::new(100, 10_000)` is `Exhausted`, and a large budget is charged ≥ 60,000. Then a table of every variable-length operand form, each charged at least its operand's length and exhausting at half of it. The argv shift is charged and linear.) `r5_f2_nested_shell_and_home_scans_are_charged`.
3. **Finding 3 (uncharged quadratic payload deduplication).**
   - **Fix:** `PayloadSet`, an open-addressing index of FNV-1a hashes kept at most half full. Hashing charges one unit per byte, each probe one unit, a comparison on equal hashes the bytes it compares, and the table its capacity, all before the work. Expected cost is O(n + L); colliding inputs only cost more charged probes. It replaces every payload `.contains` (unit payloads, `find -exec` payloads, process-substitution payloads). Alternative readings (at most 8) use charged comparisons (`readings_eq`, `has_reading`).
   - **Tests:** `r5_f3_payload_dedup_is_linear_and_charged` (255 and 510 distinct payloads with a 500-byte common prefix: linear bound and doubling ratio; 255 duplicates charge a hash and a full comparison each; a half budget exhausts; the review's exact 130,817-byte line runs through `lex_best_effort` and derives all 255 `echo` units).

### Class tests (catch work done without a charge, not only charge growth)
- `r5_class_exact_lexer_below_cost_keeps_only_finished_values`: 14 lines (assignments, quoted/non-ASCII values, every redirect form, reserved words, groups, pipelines, redirect-only, here-docs). For **every** compute and allocation budget below the measured cost: `Exhausted`, only whole words and redirects of the full run kept, in order, and every unit outcome points at a kept unit. One unit below the compute cost keeps everything.
- `r5_class_best_effort_below_cost_keeps_only_finished_values`: the same over every budget below the smallest clean one, for here-strings, here-docs into `sh`, `echo | sh`, `-c`, wrappers and `find -exec`.
- `r5_class_build_units_below_cost_keeps_only_finished_values`: the same for `build_units` (wrappers, `MAX_WRAP`, HOME, nested `-c`, outside shells).
- Every store into reserved capacity is `debug_assert`ed (`push_reserved`, `push_held`), so these sweeps fail if any path stores without a reservation.
- `r5_neighbours_lookups_comparisons_folds_and_memo_are_charged`: lower bounds for table lookups, the three fold passes, here-doc delimiter comparisons, the memo lookup and linear Scan 8.

### The audit, by function

**`v4/lex.rs`**
- New charged primitives: `lookup_tool`, `lookup_outside`, `flag_charged`, `flag_short_charged`, `bytes_eq`, `word_has_unquoted_glob`, `word_as_str`, `push_reserved`. `is_reserved_word` is charged.
- `ExactSink` (`OutputSink` for `lex_exact`, `UnitsSink` in `units.rs`), `ReasonSink` (pending reasons, held slots, outcome-slot invariant `pending + held + 1`), `CoreStop`.
- `WordBuilder` tracks unquoted-glob and all-unquoted flags while building, and `read_word` tracks the digit value. So `lex_exact_redirect` no longer rescans a target (`has_unquoted_glob`, `is_all_unquoted`, `parse_u32_digits` were uncharged second passes; `parse_u32_digits` is removed).
- `read_standalone` charges the reason string before consuming `{ } ! [[`. The heredoc delimiter's slot is reserved before its scan.
- `skip_heredocs`: tab stripping and delimiter comparisons are charged.
- `flush_unit` (now a function) charges everything before moving the pending unit. Recovery in `lex_exact_core` allocates nothing. It no longer guesses the redirect-only reason for a unit cut off mid-way.
- `fold_command_bytes`: each pass is charged: basename search, ASCII check, copy; on the Unicode path, sizing at 4 units/byte, `to_lowercase` and the rewrite. Before, the ASCII check and the Unicode sizing pass were uncharged.
- `vec_extend_budgeted` charges the moves.

**`v4/units.rs`**
- `process_text_recursive` lexes straight into `ParsedCommandUnits` through `UnitsSink`. The old copy of every raw unit into `parsed.units` (and its uncharged recovery pushes `flush_remaining_raw`) is gone. Units are classified in place by `process_unit`, which reserves its fixed reason slots plus the `Exhausted` spare before any work.
- `build_units_with_hook` reserves the `Exhausted` slot on entry, and returns `Err(PartOutcome::Exhausted)` only if that fails.
- `unwrap_in_place` / `unwrap_unit`: charged glob scan, fold, lookups, and one charged shift. On exhaustion the unit keeps its full argv, and any wrapper reason already found is still recorded.
- Validators, `classify_nested_shell`, `command_modifies_home`, `unit_uses_unquoted_tilde`: charged as listed under finding 2.
- Neighbouring bug fixed: a nested script's whole-text outcomes now go to the parent `-c` unit. Before, the lexer emitted `Unit(parent)` and the remapping read it as a local index, which could point at the wrong unit.

**`v4/lex/best_effort.rs`**
- `lex_best_effort_into`: the texts walk is charged. The memo lookup charges a fixed worst case (`LEX_MEMO × (1 + len)`) so cold and warm runs charge the same. Memo entries store their copy size, so hits no longer walk units uncharged. Memo insertion sizes its copy only when affordable (`estimate_units_bytes_if_affordable`), and charges the memo's own growth.
- The per-call `HashMap` recursion cache is **removed**: its hashing, probing and table growth were uncharged. Re-lexing a repeated payload is charged and gives the same units with fresh `PipelineId`s.
- `lex_one_text`: recovery slots are kept (`reserve_recovery_slots`: one `raw_units` slot for the pending builder, one `record.units` slot per unconverted builder). Words are reserved and paid in advance (`WordBuf`). The `>&N` digit check is incremental. Here-doc delimiter comparisons and tab stripping are charged. Conversion recovery stores the readings already computed (`push_converted`). `find -exec` derivation keeps the record slots of builders not yet converted (`keep`). Scan 8 groups each pipeline's run once; it was a nested loop over all pipeline units, an uncharged quadratic walk.
- `unwrap_best_effort` takes an owned argv: one charged final shift (it used uncharged `remove(0)` / `drain` per step). The basename scan is charged, alternative readings are reserved and compared with charges, and on exhaustion it hands the argv back.
- `resolve_cmd_sub_readings`, `substituted_program_budgeted`, `wrapper_skip_be`, `extract_code_payloads`, `extract_env_payloads`, `extract_echo_printf_texts`, `crontab_commands_budgeted`: every `contains`, `bytes().all`, `filter().count`, `trim`, `split_once` and `split_whitespace` is charged by the bytes it scans.
- `is_be_shell`, `is_be_reserved`, `is_be_code_runner`: charged lookups.
- Brace expansion: one fused `{`/`$` scan, charged second walks over the char vectors, and `padded_number` (exactly sized and charged) instead of `format!`, whose capacity is not exact.
- Moves of elements that are already stored (draining pending reasons or payload lists) are one step per element, each element charged at least one unit when it was created.

### Test expectations changed (each against the contract, not the code)
- `exhaustion_at_flush_preserves_pending_unit_words_and_reasons`, `lex_exact_word_buffer_capacity_and_realloc_copy_are_budgeted`, the tight budgets in `budgets_exhaustion_preserves_early_units_and_never_allows_padded_lines` and in `build_units_and_unwrap_unit_budget_exhaustion_and_linearity`: budgets are now derived from measured runs, not hard-coded totals. Same assertions about what is kept (§8).
- `lex_exact("cat README.md", …, new(100_000, 16))` is now `Err(Exhausted(AllocationBudget))` on entry. A later allocation failure is checked separately.
- `linear_charge_growth_from_10kb_to_max_bytes`: under default limits the adversarial line must be `Exhausted`; which budget runs out first is not in the contract (§8), so either reason is accepted.
- `brace_expand_budgeted_carries_m1_31_n11_guarantees`: the linear bound for nested braces is 12 units/byte, not 8, because the second walks are now charged (about 9 units/byte, listed in the test).
- `build_units_code_runner_hook_attaches_p3_reasons`: uses the new hook signature.

### Notes for P3 and P5
- **`CodeRunnerHook` (P1's own API) changed:** the hook now receives `&mut UnitReasonSink` and records reasons through it (`reserve`, then `push`) instead of returning a `Vec`. That way a reason the hook builds is never lost when the move into the outcomes would fail.
- `lex_exact` and `build_units` return `Err(PartOutcome::Exhausted)` only when the budget is spent on entry (nothing found). Otherwise exhaustion is an outcome, as before.

### Change requests
- **CR (low, P0 interface): `unwrap_unit` cannot hand back the unit on exhaustion.** Its frozen signature `unwrap_unit(raw: RawCommandUnit, …) -> Result<UnwrapResult, Exhausted>` takes the unit by value, so on `Exhausted` the caller loses it. Failing case: `unwrap_unit(raw, …, ComputeBudget::new(2, 10_000))` returns `Err` and `raw` is gone. P1 works around it: `build_units` unwraps in place and loses nothing. Proposed change: return `Result<UnwrapResult, (CommandUnit, Exhausted)>`, or take `&mut CommandUnit`. Not made (frozen).
- None other. `ComputeBudget`, `model.rs`, `limits.rs` and `tools.rs` are untouched. D-144's new `ProtectedSet` field does not affect P1.

### Round 5 verification
Run by the worker (D-123):
- `cargo fmt --all`: clean
- `cargo check -p atrium-core --tests`: clean
- `cargo clippy -p atrium-core --tests -- -D warnings`: clean

Not run (D-123): any test. The new sweeps run lexing a few thousand times per line on short inputs. Run them in debug, because the reservation checks are `debug_assert`s.

Commands for the orchestrator's verification task (fish-compatible):
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core policy::v4:: -- --nocapture
cargo test --workspace --no-fail-fast
```

---

## Round 6 (Opus 5.5, review round 4)

Round 5 was green in the orchestrator's verification at `8b5a298` (rebased onto P0's final tip `4f18a59`). The Codex review (`.orchestration/review-input/review-round4.md`) asked for rework with two findings. Both are fixed, along with the neighbouring cases of the same class: a finished result dropped on an error path, or a std helper scanning before its charge.

### The two findings
1. **Major: best-effort unwrapping dropped completed alternative readings on `Exhausted`.**
   - **Fix:** `unwrap_best_effort` pays for its one final argv shift on entry. On exhaustion it returns `Err((BestEffortUnwrap, Exhausted))` holding the effective reading (shifted past every finished wrapper step) and every alternative found so far. An alternative whose duplicate check runs out is kept anyway. Every caller now keeps these readings:
     - the conversion loop (`push_converted` gets the partial readings);
     - `resolve_cmd_sub_readings`, which stores each reading into its reserved slot before anything else can fail. Once the budget is out, the duplicate checks can't run, so remaining readings are kept unchecked: a repeated reading is harmless, a dropped one would lose a result;
     - `process_find_exec_cmds`.
   - **Tests:**
     - `r6_f1_best_effort_unwrap_keeps_alternative_readings_on_exhaustion` runs the review's case (`"command -Z arg sudo id"`, `ComputeBudget::new(402, 100_000)`). It then sweeps the helper at every budget below its cost: whole-word effective reading, only the full run's alternatives, a stripped reading always with its alternative, and some exhausted run that keeps it. The round-5 code fails that last check. It also covers the `find -exec` and `$(which …)` callers one unit short.
     - The best-effort class sweep's `assert_best_effort_subset` now also checks that every kept alternative reading is one the full run has. The sweep gained `command -Z arg sudo id`, `$(which sh) -c 'id'`, a nested `find -exec … -ok` line and a `crontab` here-doc.
2. **Minor: crontab line scanning ran before its charge.**
   - **Fix:** `crontab_commands_budgeted` finds each line end with a byte scan charged one unit before each byte is examined. It splits like `str::lines` (`\n`, optional `\r`, no empty trailing line). Each line's field parse is charged (2 units/byte) before it runs.
   - **Test:** `r6_f2_crontab_lines_are_charged_before_they_are_scanned`: the review's case (`"x"×60,000` with `ComputeBudget::new(0, 0)`), a small budget, the lower bound, and the line and field results of the old parser on CRLF, comments, blank lines, `@` lines and short lines.

### Same class, fixed in the audit
- **`find -exec` derived units were dropped on error paths.** Before, a command extracted from a unit's argv was lost when derivation, the parent's remaining work or the pipeline bookkeeping ran out. Now `extract_find_exec_cmds` reserves each command's `out` slot and its record slot (after the parent and the builders still to convert) when it sees `-exec`. It pays both argv copies (the command's argv, and the effective reading made later by derivation or recovery) word by word as its scan passes them. On any error path, a command being derived is recorded with its readings so far. Commands not yet derived (nested ones too) are recorded with their argv as their reading (`flush_find_execs_raw`), and nothing allocates.
- **Exact unwrapping discarded recognised wrappers on exhaustion** (round 5 left the unit raw). `unwrap_in_place` now pays the argv shift before the first wrapper is parsed, and each wrapper's storage (`removed_wrappers` slot, up to `MAX_WRAPPER_WORDS` = 6 words) before that wrapper is parsed. So every wrapper recognised before exhaustion is moved out and kept. Test: `r6_exact_unwrap_keeps_recognised_wrappers_on_exhaustion` (every budget below the cost: words unchanged, removed wrappers a prefix of the full run's, all 4 kept by some exhausted run; also through `build_units`).
- **Helpers that walked before charging:**
  - `clone_vec_string_budgeted` summed its strings before charging; it now charges the walk first (`argv_copy_cost`).
  - The `eval` and `echo` join sizing walked the arguments before charging.
  - Brace expansion summed `char` slices (inner, alternatives, prefix/suffix) before charging.
  - All of these now charge the walk first.
- **Re-checked and already charged before they run:**
  - every remaining `split`, `split_once`, `split_whitespace`, `trim`, `rsplit`, `contains`, `find`, `chars` and `bytes().all` in P1's files;
  - the memo lookup, Scan 7 and 8, `PayloadSet`, and the here-doc and delimiter scans;
  - `fold_command_bytes`, the exact lexer and `units.rs`.
  - `text[i..].chars().next()` decodes one char under the step's charge.

### Change requests
- **Withdrawn: `unwrap_unit` by-value signature.** The review judged it not a defect, and I agree. No contract-defined caller loses information: `build_units` unwraps in place and keeps every unit, and nothing else in P1 or the contract needs the unit back after `Exhausted`. No open change requests.

### Round 6 verification
Run by the worker (D-123):
- `cargo fmt --all`: clean
- `cargo check -p atrium-core --tests`: clean
- `cargo clippy -p atrium-core --tests -- -D warnings`: clean

Not run (D-123): any test. Run in debug: the reservation checks are `debug_assert`s.

Commands for the orchestrator's verification task (fish-compatible):
```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p atrium-core policy::v4:: -- --nocapture
cargo test --workspace --no-fail-fast
```
