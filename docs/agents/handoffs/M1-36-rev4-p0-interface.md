# Handoff: M1-36-rev4-p0-interface (P0)

- **Branch:** `agent/neon/M1-36-rev4-p0-interface`
- **Model:** Gemini 4 Neon (High)
- **Contract:** `docs/contracts/m1-policy-rev4.md` (draft 5)
- **Behaviour change:** None. `dev`'s evaluator remains live and untouched; nothing in `dev` calls the new modules.

## Summary of changes

1. **Visibility-only edits in `dev` (`const` → `pub(super) const`):**
   - `crates/atrium-core/src/policy/deny.rs`: `MAX_BYTES`, `DOWNLOADERS`, `PRIV`.
   - `crates/atrium-core/src/policy/shell.rs`: `MAX_DEPTH`, `RESERVED`, `SHELLS`, `COMMAND_RUNNERS`.
   - `crates/atrium-core/src/policy/words.rs`: `WRITERS`, `READERS`.
   - `crates/atrium-core/src/policy/gate.rs`: `MAX_HOPS`, `ReaderOpts` (and fields), `AG_OPTS`, `ACK_OPTS`.
   - `crates/atrium-core/src/policy/engine.rs`: `GIT_CRED_SUFFIXES`, `GIT_CRED_NAMES`, `GIT_CRED_DIRS`.
   - `crates/atrium-core/src/policy/mod.rs`: added `mod limits; mod model; mod tools; mod v4;` only.
2. **`crates/atrium-core/src/policy/limits.rs`:** frozen rev 4 caps and budgets (§8, Q2) with tests asserting equality with `dev`'s shared constants.
3. **`crates/atrium-core/src/policy/model.rs`:** `pub(crate)` data model for P1–P5 (words with 1-byte provenance, modeled units, separate best-effort unit record, outcomes + closed `UnreadReason` enum, `ToolEffects`, canonical `Target`s, shared `FoldRules` & `ProtectedSet`, rule/evaluation DTOs, sticky `ComputeBudget`, and `FsCaps` → `CapReached`) plus unit tests.
4. **`crates/atrium-core/src/policy/tools.rs` & `docs/contracts/m1-policy-rev4-tools.md`:** normative static tool table (`TOOLS`), explicit `OUTSIDE_MODEL` list, mirrored `GIT_CRED_*` constants, plain table-read accessors (`lookup`, `lookup_outside_model`, `subcommand`, `flag`, `flag_short`, `flag_long`), deterministic Markdown renderer, and all required integrity + Markdown-snapshot tests.
5. **`crates/atrium-core/src/policy/v4/`:** frozen function-level stubs (`todo!()`) with `#![allow(dead_code, clippy::todo)]` in `mod.rs`:
   - `lex.rs`, `units.rs` (P1)
   - `paths.rs` (P2, including `pub(crate) trait PathResolver`)
   - `tools.rs`, `rules.rs` (P3)
   - `evaluate.rs` (P5)

---

## Open choices made

### Type shapes (`model.rs`, `tools.rs`, `v4/`)
- **Budget location:** kept `ComputeBudget` and `FsCaps` inside `model.rs` so `policy/mod.rs` adds only `mod limits; mod model; mod tools; mod v4;` and all `v4/` signatures reference `model.rs` types only.
- **Sticky exhaustion:** `ComputeBudget` tracks `remaining_units: u64`, `remaining_alloc_bytes: usize`, and a shared `exhausted: Option<ExhaustReason>`. Once either counter runs out, every subsequent `charge` or `charge_alloc` (including `0`) returns `Err(Exhausted)`.
- **`ByteProvenance`:** 1-byte `#[repr(u8)]` enum (`Unquoted`, `SingleQuoted`, `DoubleQuoted`, `Escaped`, `Derived`) stored in a parallel `Vec<ByteProvenance>` alongside `Vec<u8>` on `Word`.
- **`OperandRole` vs `ValueRole`:** split non-flag roles into `Copy` `ValueRole` so `OperandRole::FlagValue(ValueRole)` is `Copy`, zero-allocation, and usable in `const` tables; added `OperandRole::Command` for argv[0] / subcommand positions.
- **Per-operand read/write mode:** added `ToolEffects::operand_access: Vec<AccessMode>` and `tools::OperandAccessMode` (`AllRead`, `AllWrite`, `SourcesReadLastWrite`, `FlagOnlyWrite`) so P2 can treat e.g. `ln` target and `cp`/`rsync` sources as `Read` while destinations are `Write` without hardcoding tool names in P2.
- **`Cap` variant names:** named `Bytes`, `Units`, `Uses`, `Paths` (avoiding a shared `Max` prefix that would trigger `clippy::enum_variant_names`).
- **Additional closed `UnreadReason` variants beyond the brief's summary list:** added `NegatedGlobClass`, `UnsupportedTildeForm`, `UnsupportedQuoteOrEscape`, `UnsupportedRedirect` (for `<&`, `<>`, `>|`, missing target), `MaxNestingDepthReached` (§4 `MAX_DEPTH`), and `OutsideToolModel` (§6 `OUTSIDE_MODEL` / unsupported subcommand) so P1–P3 need not invent any variants.
- **`find` single-dash primaries:** `find` predicates (`-follow`, `-delete`, `-depth`, `-maxdepth`, `-mindepth`, `-exec`, `-execdir`, `-ok`, `-okdir`, `-fprint`, `-fprint0`, `-fprintf`, `-fls`) are stored in `FlagSpec::long` (without leading `-`), documented for P3 to match as single-dash primaries.
- **Shared context parameters in `v4/`:** `FoldRules` (CR-19 + `deny.rs` path folds) and `ProtectedSet` (canonical home/cwd, Atrium data/protected paths, hard-deny & store identities, `~/.ssh` linked dirs, and `FoldRules`) are passed explicitly to P1–P3 functions.
- **Bare `find` and `sed` prefixes:** marked `code_runner_no_pin: true` (consistent with `git` in §7, since `find -exec` and `sed 'e'` run code). Confirm during table review whether `["find"]` should remain invalid at creation or be relaxed.

### Cap values & compute budget (`limits.rs`, Q2)
- **Shared with `dev` (exact match):** `MAX_BYTES = 131_072`, `MAX_UNITS = 512`, `MAX_USES = 4_096`, `MAX_PATHS = 16_384`, `MAX_DEPTH = 6`, `MAX_HOPS = 40`.
- **Walk & model caps:** `MAX_WRAP = 8`, `WALK_MAX_ENTRIES = 4_096`, `WALK_MAX_DEPTH = 16` (from `gate.rs` `links_below`), `GLOB_MAX_DEPTH = 12`, `HARD_LINK_WALK_ENTRIES = 4_096`, `HARD_LINK_WALK_DEPTH = 8`, `HARD_LINK_WALK_SKIP_DIRS = &["node_modules", ".git"]`.
- **Per-check filesystem caps (`FsCaps`, cap hit → `Unread`):**
  - `FS_MAX_ENTRIES_READ = 16_384` (4× `WALK_MAX_ENTRIES`, covering multi-root/multi-unit walks)
  - `FS_MAX_DIR_OPENS = 1_024`
  - `FS_MAX_STATS = 16_384`
  - `FS_MAX_READLINKS = 1_024`
  - `FS_MAX_CANONICALIZATIONS = 4_096` (matches `MAX_USES`)
- **Compute & allocation budgets (`ComputeBudget`, exhaustion → `Exhausted` → `Ask` everywhere):**
  - `COMPUTE_BUDGET_UNITS = (MAX_BYTES as u64) * 128` (= `16_777_216` units)
  - `COMPUTE_BUDGET_ALLOC_BYTES = MAX_BYTES * 64` (= `8_388_608` bytes = 8 MiB)
  - Sized to cover linear exact lexing + provenance, recursive `-c` up to `MAX_DEPTH = 6`, best-effort raw-text matching (~60 forms) + tolerant lexing, and bounded expansion/matching for every §10 row and the longest adversarial test lines (e.g. `adversarial_gate.rs` 4,000-byte nested path and `MAX_BYTES`-scale lines), while cutting off quadratic blowup.

---

## Rows marked `changes_default` (for Pedro, §11)

| Entry | §11 item(s) | Reason |
|---|---|---|
| `command`, `exec`, `nice`, `timeout`, `time`, `env` | §11 item 10 | Only §4's closed wrapper forms stay in the model; other wrapper flags/forms become `Unread`. |
| `sh`, `bash`, `dash` | §11 item 2; §11 item 4; §11 item 8 | K7 positional resolution removed (item 2); bare prefix invalid at creation (item 4); script file (`sh x.sh`) is pinnable `Unread (program)` (item 8). |
| `which`, `date`, `echo`, `printf`, `whereis`, `type`, `tr` | §11 item 9 | Non-path operands (`Text`) stop counting as candidate paths. |
| `rg`, `grep`, `egrep`, `fgrep`, `zgrep`, `ggrep`, `rgrep`, `ag`, `ack`, `jq` | §11 item 9 | Pattern operand (`[0] Pattern` / `-e`) stops counting as a candidate path. |
| `chmod`, `chown`, `chgrp` | §11 item 9 | Mode / owner operand (`[0] Text`) stops counting as a candidate path. |
| `git` (bare prefix) | §11 item 4 | Bare `["git"]` Allow prefix is invalid at creation. |
| `git status`, `git ls-files` | §11 item 10 | Active wildcard pathspecs become `Unread`. |
| `git log`, `git diff`, `git show` | §11 item 9; §11 item 10 | Ref operands stop counting as filesystem paths (still checked by CR-12 git selection); active wildcard pathspecs become `Unread`. |
| `git branch`, `git rev-parse`, `git cat-file`, `git push` | §11 item 9 | Ref/remote/branch operands stop counting as filesystem paths. |
| `git clone`, `git checkout` | §11 item 10 | Derived destinations make the writer itself `Unread`. |
| `git merge-index`, `git filter-branch`, `git bisect`, `git rebase`, `git submodule`, `git difftool`, `git mergetool` | §11 item 4 | Program-running git subcommands; unpinned Allow prefixes invalid at creation. |
| `cargo` (bare) / `cargo test`, `build`, `run`, `check`, `bench`, `doc`, `clippy` | §11 item 4; §11 item 8 | Bare `["cargo"]` invalid at creation (item 4); `["cargo", "<subcmd>"]` pins `Unread (program)` (item 8). |
| `pipx`, `pipx run` | §11 item 4; §11 item 8 | Bare `["pipx"]` and `["pipx", "run"]` invalid at creation (item 4); `["pipx", "run", "<pkg>"]` pins `Unread (program)` (item 8). |
| `find` | §11 item 4 | Bare `["find"]` marked invalid at creation (`-exec`/`-execdir`/`-ok` run code). |
| `cp`, `mv`, `rsync` | §11 item 10 | Derived destinations (`cp -r/-R/-a`, `mv` of directory, `rsync`) become `Unread`. |
| `tar`, `unzip` | §11 item 9; §11 item 10 | Archive member operands stop counting as host paths (item 9); extraction (`tar -x`, `unzip`) has derived destinations → `Unread` (item 10). |
| `awk`, `gawk`, `mawk`, `sed`, `eval`, `xargs`, `COMMAND_RUNNERS` (`ssh`..`parallel`) | §11 item 4 | Code runners; unpinned Allow prefix invalid at creation. |
| `perl`, `python`, `python3`, `node`, `ruby`, `source`, `.`, `npx` | §11 item 4; §11 item 8 | Unpinned Allow prefix invalid at creation (item 4); `[tool, script]` pins `Unread (program)` (item 8). |
| `OUTSIDE_MODEL` shells (`zsh`, `ksh`, `fish`, `ash`, `csh`, `tcsh`, `mksh`, `yash`, `rbash`, `posh`, `elvish`, `xonsh`) | §11 item 4 | Outside §4's nested-shell model; unpinned Allow prefix invalid. |
| `OUTSIDE_MODEL` wrappers (`nohup`, `builtin`, `busybox`, `toybox`, `setsid`, `unbuffer`, `caffeinate`, `sandbox-exec`, `chroot`, `ionice`, `stdbuf`, `chrt`, `taskset`, `flock`, `script`, `arch`) | §11 item 10 | Outside §4's closed wrapper list → `Unread`. |
| `OUTSIDE_MODEL` `ed`, `ex`, `open` | §11 item 4 | Interactive editors (`!cmd`) / macOS app launcher → `Unread`, unpinned Allow prefix invalid. |

---

## Rows and tools left out, and why

1. **Explicit `OUTSIDE_MODEL` entries (unit becomes `Unread`):**
   - **12 shells** from `shell::SHELLS` other than `sh`/`bash`/`dash` (`zsh`, `ksh`, `fish`, `ash`, `csh`, `tcsh`, `mksh`, `yash`, `rbash`, `posh`, `elvish`, `xonsh`): contract §4 explicitly restricts nested shells to `sh`/`bash`/`dash`.
   - **16 wrappers** from `shell.rs` outside §4's closed list of 6 (`nohup`, `builtin`, `busybox`, `toybox`, `setsid`, `unbuffer`, `caffeinate`, `sandbox-exec`, `chroot`, `ionice`, `stdbuf`, `chrt`, `taskset`, `flock`, `script`, `arch`): contract §4 explicitly leaves them outside the model (`xargs` is in `TOOLS` as a code runner per §4).
   - **19 reserved words** from `shell::RESERVED` (`if`, `then`, `else`, `elif`, `fi`, `for`, `while`, `until`, `do`, `done`, `case`, `esac`, `function`, `select`, `{`, `}`, `!`, `[[`, `coproc`): contract §3 puts compound commands outside the model.
   - **`ed`, `ex`** (`words::WRITERS`): interactive line editors with `!cmd` shell escapes.
   - **`pushd`, `popd`** (`words::READERS`): mutate the shell directory stack (`dev` already treats them as `Cd::Lost` / unknown cwd; §5).
   - **`test`, `[`** (`words::READERS`): unary/binary primaries have no flag table in `dev`.
   - **`open`, `spctl`, `mdfind`** (`words::READERS`, macOS): app launcher / Gatekeeper policy tool / whole-volume Spotlight search with no flag table in `dev`.
2. **Flags omitted where `dev` has no flag table (faithfulness rule):**
   - No flags were invented. Tools where `dev` has no flag table (`ls`, `cat`, `head`, `tail`, `wc`, `file`, `stat`, `du`, `df`, `which`, `uname`, `date`, `whoami`, `mv`, `unzip`, `git status`/`show`/`rev-parse`/`ls-files`/`cat-file`/`clone`/`checkout`, `cargo *`, `pipx run`, `tee`, `touch`, `chmod`, `chown`, `chgrp`, `mkdir`, `diff`, `jq`, etc.) have `flags: &[]` (so any flag on them is `Unread` until/unless approved into the contract table).
   - Similarly, `tar` has only the flags named in `gate.rs` and §5 (`-x`, `-c`, `-r`, `-u`, `-h`/`--dereference`; notably `-f`, `-z`, `-C`, `-v` are omitted because `dev` has no flag table for `tar`), and `find` has only the flags parsed by `shell.rs`/`gate.rs`/`deny.rs`/`words.rs` (e.g. `-name` is omitted since `dev` only mentions it in a comment).
   - `rg`'s `HOME_READONLY_COMMANDS` forbidden flags not in `RG_BOOL_*`/`RG_VAL_*` (`--pre`, `--pre-glob`, `-z`, `--search-zip`, `--hostname-bin`) are preserved verbatim in `home_readonly.forbidden` and omitted from `RG_FLAGS` (so they also remain `Unread` unknown flags).

---

## Verification commands

Run by worker (allowed under D-123):
- `cargo fmt --all` — clean
- `cargo check -p atrium-core --tests` — clean (0 errors, 0 warnings)

Commands for the orchestrator's verification task (fish-compatible):

```sh
env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

Note: `docs/contracts/m1-policy-rev4-tools.md` was written by hand without the expanded detailed flag tables for `rg`/`grep`/`ag`/`ack`/`find`; running the first command above regenerates the exact file for commit and review by Pedro and rodrigo.

---

## Round 2 (rework against Codex review round 1)

All 18 findings from `.orchestration/review-input/review-round1.md` and `.orchestration/review-input/clippy-round1.txt` are fixed. `docs/contracts/m1-policy-rev4-tools.md` is intentionally left untouched for the orchestrator to regenerate via `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.

### Orchestrator rulings recorded

1. **Finding 1 (Home parity & covering-rule restrictions):** The original brief's "never invent flags" was wrong for tools in `HOME_READONLY_COMMANDS` (and tools without `dev` flag tables under covering rules). Added `FlagMode` (`ListedOnly` vs `AnyExceptForbidden`) to `tools.rs`. Under `AnyExceptForbidden`, listed flags still apply their exact `FlagKind` and `FlagEffects`, any unlisted non-forbidden flag is accepted (not `Unread`), every non-`Pattern`/`Text`/`Ref`/`ArchiveMember` word (flag values included) is a candidate `Path` (matching `dev`'s all-arguments extraction), and operands marked `Pattern`/`Text`/`Ref`/`ArchiveMember` keep that role (§11 item 9). Removed unapproved restrictions (`test`, `[`, `ed`, `ex`, `open`, `spctl`, `mdfind` moved from `OUTSIDE_MODEL` to `TOOLS` with `AnyExceptForbidden`; `pwd`/`uname`/`whoami`/`date` operands kept as `Path` under `AnyExceptForbidden`; empty/partial flag tables switched to `AnyExceptForbidden`). Replaced metadata-only parity tests with effective accepted-form tests (`ls -la`, `head -n 5 README.md`, `wc -l README.md`, `git status --short`, etc.) and bidirectional flag-parity checks against `dev`'s tables.
2. **Finding 2 (followed searches `rg -L`, `grep -R`):** Contract §10 row 15 makes `rg -L x proj` (with no link out of `proj`, within budget) Allow in Home, while §11 did not list that change. Marked `rg` and `grep` with `"§11 item 15 (approved, D-139)"` pending driver approval, gated Home followed-search eligibility behind a single constant `tools::ALLOW_HOME_FOLLOWED_SEARCH` (`HomeReadOnlySpec::allow_followed_search`) so rejecting item 15 is a one-line revert to `false`, and updated the `changes_default` citation integrity test to accept item 15.

### Findings, fixes, and commits

| # | Finding | Fix | Commit |
|---|---|---|---|
| 1 | Unrecorded restrictions change Home & covering-rule verdicts | Added `FlagMode::AnyExceptForbidden` across all non-search `HOME_READONLY_COMMANDS` tools/git-subcommands and tools without `dev` flag tables; moved `test`, `[`, `ed`, `ex`, `open`, `spctl`, `mdfind` out of `OUTSIDE_MODEL` into `TOOLS`; kept `pwd`/`uname`/`whoami`/`date` operands as `Path`; added effective Home parity tests and bidirectional `dev` flag-table parity tests. | `f493858`, `8dcf720` |
| 2 | Followed-search relaxations (`rg -L`, `grep -R`) lacked §11 record | Annotated `rg` and `grep` with `"§11 item 15 (approved, D-139)"`, added single-line switch `ALLOW_HOME_FOLLOWED_SEARCH` + `HomeReadOnlySpec::allow_followed_search`, updated citation test to accept item 15. | `f493858`, `8dcf720` |
| 3 | `true`, `false`, `id` operand relaxations unmarked | Annotated `true`, `false`, `id` with `§11 item 9` and added `non_path_operand_relaxations_cite_item_9` integrity test. | `f493858`, `8dcf720` |
| 4 | Annex contradicted §4 unknown-tool fallback | Updated `tools.rs` module doc and `render_markdown()` intro to explicitly distinguish unknown tools (stay in model for hard-deny/store checks, every word a candidate `Path`, possible writer, never Home read-only) from excluded forms (`OUTSIDE_MODEL`, unlisted flags under `ListedOnly`, forbidden flags under `AnyExceptForbidden`). | `f493858` |
| 5 | `tar` creation operands misclassified as `ArchiveMember` | Changed `tar` default layout to `OperandLayout::all(ValueRole::Path)` and `FlagOnlyWrite`; added `FlagEffects::positional_role` so `-c`/`-r`/`-u` set `Path`, `-x`/`-t` set `ArchiveMember`; added `-f`/`--file` (`Path`) and `-C`/`--directory` (`DestinationRoot`). | `8dcf720` |
| 6 | Writer targets inherited `AllRead` (`git clone`, `codesign`, `git checkout`, etc.) | Added `OperandAccessMode::AllWriteWhenWriting` and `OperandLayout::after_double_dash`; set `git clone` to `SourcesReadLastWrite`, `git checkout` to `[0] Ref` + `rest`/`after_double_dash: DestinationRoot` + `AllWrite` + `-b`/`-B`/`--orphan`, `codesign`/`sed`/`perl`/`awk`/`gawk`/`mawk` to `AllWriteWhenWriting`, `unzip` `-d` (`DestinationRoot`, `FlagOnlyWrite`), and `git mergetool`/`sudoedit`/`curl`/`wget`/`fetch` to `AllWrite`. | `8dcf720` |
| 7 | `git branch NAME` modeled as non-writing | Added `WriteBehavior::WithFlagOrOperandCount(1)` on `git branch`, `-l`/`--list` setting `writes: FlagToggle::Disable`, and mutating flags (`-d`/`-D`/`-m`/`-M`/`-c`/`-C`/`-u`/`--unset-upstream`/`--edit-description`) setting `writes: FlagToggle::Enable`. | `8dcf720` |
| 8 | Flag effects could not express disabling or ordered overrides | Replaced boolean effect fields on `FlagEffects` with ordered `FlagToggle` (`Unchanged`/`Enable`/`Disable`, last wins via `FlagToggle::apply`); encoded disabling flags on `grep` (`-r`), `ag` (`--nofollow`, `-n`/`--norecurse`), `ack` (`--nofollow`, `-n`/`--no-recurse`), `find` (`-H`, `-P`), `cp` (`-a`, `-P`, `-d`), and `git branch` (`-l`/`--list`), with ordering tests. | `f493858`, `8dcf720` |
| 9 | Every `sed -e` marked as arbitrary code; positional script layout wrong after `-e` | Added `CodeRunnerBehavior::ScriptCommands(&['e', 'w', 'r'])` and `FlagEffects::provides_script`; changed `sed -e`/`--expression` to `provides_script: true, runs_code: false` (so P3 inspects script for `e`/`w`/`r` and omits positional `[0]` script), and added `sed -f`/`--file` (`ProgramFile`, `provides_script: true`, `runs_code: true`). | `8dcf720` |
| 10 | Conditional code runners lacked consistent prefix validation | Marked `ag`, `ack`, `git diff`, and `git push` with `code_runner_no_pin: true` and `§11 item 4`; enforced consistency across all code-runner rows in `code_running_rows_name_code_operand_or_pinning`. | `8dcf720` |
| 11 | Resolver errors indistinguishable from absence | Added `FsIoError` and `ResolveError` (`NotFound`, `Io(FsIoError)`, `CapReached`, `Exhausted`) in `model.rs` and updated every `PathResolver` method in `v4/paths.rs` to return `Result<..., ResolveError>`, with unit-test fixtures. | `7a598cb` |
| 12 | Resolver allocations could not charge shared budget | Passed `budget: &mut ComputeBudget` into every `PathResolver` method (`stat`, `lstat`, `read_link`, `canonicalize`, `read_dir`) with precharge contract and exhaustion fixtures. | `7a598cb` |
| 13 | `check_recursive_root` could not return multiply-linked flag | Added `RecursiveRootResult { outcomes, multiply_linked_file }` in `model.rs` and updated `v4::paths::check_recursive_root` to return `Result<RecursiveRootResult, Exhausted>`, with fixture test. | `7a598cb` |
| 14 | Filesystem-dependent derived destinations lost between P3 and P2 | Replaced `ToolEffects::writes_derived_destinations: bool` with `ToolEffects::derived_destinations: DerivedDestinations` (`None`, `Always`, `WhenSourceIsDir`) so P2 resolves `mv` of a directory against `PathResolver`. | `7a598cb` |
| 15 | Code-runner integrity test accepted missing code locations via `no_pin \|\|` | Encoded real `ValueRole::Code` operand locations on all 28 `COMMAND_RUNNERS` (`target_then_code_runner` / `direct_code_runner`), `git bisect`, and `git submodule`; removed `no_pin \|\|` bypass from `code_running_rows_name_code_operand_or_pinning`. | `8dcf720` |
| 16 | Generated Markdown omitted access modes, flag effects (<= 12 flags), subcommand flags, and single-dash `find` primaries | Added `ToolSpec::single_dash_long` (`true` on `find`) and `flag_for_tool`; updated `render_markdown()` to render `FlagMode`, `OperandAccessMode`, `after --`, inline + detailed flag tables for every tool and subcommand with flags, `allow_followed_search`, and single-dash `find` primaries (`-follow`, `-delete`, `-exec`), with rendering assertions. | `8dcf720` |
| 17 | `FoldRules` docs narrowed lowercase to ASCII | Updated `FoldRules` doc comments in `model.rs` to document Unicode `str::to_lowercase()` (matching `dev`'s `deny.rs`/`engine.rs`, e.g. `CAFÉ` → `café`, `K` → `k`) plus `ſ` → `s`, without Unicode normalization. | `7a598cb` |
| 18 | Clippy `assertions_on_constants` errors in `limits.rs:160–164` | Moved constant assertions in `limits.rs` into a `const { ... }` block; verified clean with `cargo clippy -p atrium-core --tests -- -D warnings`. | `7a598cb` |

### Full list of rows marked `changes_default` (Round 2, for Pedro)

| Entry | §11 item(s) | Reason |
|---|---|---|
| `command`, `exec`, `nice`, `timeout`, `time`, `env` | §11 item 10 | Only §4's closed wrapper forms stay in the model (`ListedOnly`); other wrapper flags/forms become `Unread`. |
| `sh`, `bash`, `dash` | §11 item 2; §11 item 4; §11 item 8 | K7 positional resolution and non-§4 flags become `Unread` (item 2); bare prefix invalid at creation (item 4); script file (`sh x.sh`) is pinnable `Unread (program)` (item 8). |
| `which`, `echo`, `printf`, `whereis`, `type`, `tr`, `true`, `false`, `id` | §11 item 9 | Non-path operands (`Text`) stop counting as candidate paths. |
| `rg`, `grep` | §11 item 9; §11 item 15 (approved, D-139) | Pattern operand (`[0] Pattern` / `-e`) stops counting as a candidate path (item 9); followed-link search (`rg -L`, `grep -R`) allowed in Home when link walk is within budget and reaches no protected path (proposed item 15, controlled by `ALLOW_HOME_FOLLOWED_SEARCH`). |
| `egrep`, `fgrep`, `zgrep`, `ggrep`, `rgrep`, `jq` | §11 item 9 | Pattern operand (`[0] Pattern` / `-e`) stops counting as a candidate path. |
| `ag`, `ack` | §11 item 4; §11 item 9 | Unpinned Allow prefix invalid at creation (`--pager`/`--output` run code; item 4); pattern operand stops counting as a candidate path (item 9). |
| `chmod`, `chown`, `chgrp` | §11 item 9 | Mode/owner operand (`[0] Text`) stops counting as a candidate path. |
| `git` (bare prefix) | §11 item 4 | Bare `["git"]` Allow prefix invalid at creation (`-c`/`--config-env`/`--exec-path` and program-running subcommands). |
| `git status`, `git ls-files` | §11 item 10 | Active wildcard pathspecs become `Unread`. |
| `git log`, `git show` | §11 item 9; §11 item 10 | Ref operands stop counting as filesystem paths (still checked by CR-12 git selection; item 9); active wildcard pathspecs become `Unread` (item 10). |
| `git diff` | §11 item 4; §11 item 9; §11 item 10 | Prefix ending at `["git", "diff"]` invalid at creation (`--ext-diff`; item 4); ref operands not filesystem paths (item 9); active wildcard pathspecs `Unread` (item 10). |
| `git push` | §11 item 4; §11 item 9 | Prefix ending at `["git", "push"]` invalid at creation (`--receive-pack`/`--exec`; item 4); ref/remote operands not filesystem paths (item 9). |
| `git branch`, `git rev-parse`, `git cat-file` | §11 item 9 | Ref/branch operands stop counting as filesystem paths. |
| `git clone`, `git checkout` | §11 item 9; §11 item 10 | Ref operand `[0]` not a filesystem path (item 9); derived destinations make the writer itself `Unread` (item 10). |
| `git merge-index`, `git filter-branch`, `git bisect`, `git rebase`, `git submodule`, `git difftool`, `git mergetool` | §11 item 4 | Program-running git subcommands; unpinned Allow prefixes invalid at creation. |
| `cargo` (bare) | §11 item 4; §11 item 8 | Bare `["cargo"]` invalid at creation (item 4); `["cargo", "<subcmd>"]` pins `Unread (program)` (item 8). |
| `cargo test`, `build`, `run`, `check`, `bench`, `doc`, `clippy` | §11 item 8 | Project build/test commands are `Unread (program)`, coverable by a rule naming `["cargo", "<subcmd>"]`. |
| `pipx`, `pipx run` | §11 item 4; §11 item 8 | Bare `["pipx"]` and `["pipx", "run"]` invalid at creation (item 4); `["pipx", "run", "<pkg>"]` pins `Unread (program)` (item 8). |
| `find` | §11 item 4 | Bare `["find"]` invalid at creation (`-exec`/`-execdir`/`-ok`/`-okdir` run code). |
| `cp`, `mv`, `rsync` | §11 item 10 | Derived destinations (`cp -r/-R/-a`, `mv` of a directory, `rsync`) make the writer `Unread`. |
| `tar`, `unzip` | §11 item 9; §11 item 10 | Extraction/listing archive member operands stop counting as host paths (item 9; `tar` creation/update sources stay `Path`); extraction (`tar -x`, `unzip`) has derived destinations → `Unread` (item 10). |
| `awk`, `gawk`, `mawk`, `sed`, `eval`, `xargs`, `COMMAND_RUNNERS` (28 tools: `ssh`..`parallel`) | §11 item 4 | Code runners; unpinned Allow prefix invalid at creation. |
| `perl`, `python`, `python3`, `node`, `ruby`, `source`, `.`, `npx` | §11 item 4; §11 item 8 | Unpinned Allow prefix invalid at creation (item 4); `[tool, script]` pins `Unread (program)` (item 8). |
| `OUTSIDE_MODEL` shells (12: `zsh`, `ksh`, `fish`, `ash`, `csh`, `tcsh`, `mksh`, `yash`, `rbash`, `posh`, `elvish`, `xonsh`) | §11 item 4 | Shells outside §4's modeled `sh`/`bash`/`dash` subset → `Unread`, unpinned Allow prefix invalid. |
| `OUTSIDE_MODEL` wrappers (16: `nohup`, `builtin`, `busybox`, `toybox`, `setsid`, `unbuffer`, `caffeinate`, `sandbox-exec`, `chroot`, `ionice`, `stdbuf`, `chrt`, `taskset`, `flock`, `script`, `arch`) | §11 item 10 | Wrappers outside §4's closed list → `Unread`. |

*(Removed from Round 1's list because they no longer change `dev` defaults: `date`, `ed`, `ex`, `open`.)*

### Restrictions kept under Finding 1 (forms `dev` allows in Home or under a covering rule that become `Unread`)

Every restriction below is approved by a specific §11 item; all unapproved restrictions (`test`, `[`, `ed`, `ex`, `open`, `spctl`, `mdfind`, and empty/partial flag tables) were removed:

| Restriction kept | Approving §11 item | Notes |
|---|---|---|
| Wrapper forms/flags outside §4's 6 closed forms (`command`, `exec`, `nice`, `timeout`, `time`, `env` with `FlagMode::ListedOnly`, plus the 16 wrappers in `OUTSIDE_MODEL`) | §11 item 10 (and item 2 for long `env`) | Explicitly listed in §4 and §11 item 10 ("the wrapper forms outside §4's list"). |
| Nested-shell forms outside §4 (`sh`/`bash`/`dash` `FlagMode::ListedOnly`, trailing operands after `-c SCRIPT`, and the 12 shells in `OUTSIDE_MODEL`) | §11 item 2; §11 item 4; §11 item 8 | K7 positional resolution removed (§4, §11 item 2); script files are pinnable `Unread (program)` (item 8); unpinned prefixes invalid (item 4). |
| Unknown flags on search tools with complete `dev` flag tables (`rg`, `grep`, `egrep`, `fgrep`, `zgrep`, `ggrep`, `rgrep`, `ag`, `ack` with `FlagMode::ListedOnly`) | §11 item 2 (contract §6) | `dev` already rejects unknown flags on `rg`/`grep` in Home (`engine::search_roots`); under covering rules, flags absent from `dev`'s complete `RG_*`/`GREP_*`/`AG_OPTS`/`ACK_OPTS` tables become `Unread` per §6 ("Unknown flags: a known tool with an unknown flag is Unread") and §11 item 2. |
| Forbidden Home flags not modeled in `row.flags` on `AnyExceptForbidden` Home rows (`file -C`/`--compile`/`-m`/`--magic-file`/`-f`/`--files-from`, `date -s`/`--set`/`-f`/`--file`, `tree -R`) | §11 item 2 (contract §6) | Already Ask in Home on `dev`; under covering rules, these flags (which write unmodeled files or read hidden path lists from files) become `Unread` under `AnyExceptForbidden`. Modeled flags (`tree -o`, `git diff --output`, `rg -f`, `grep -f`) remain modeled under covering rules while staying forbidden in Home. |
| Writers with derived destinations (`cp -r`/`-R`/`-a`, `mv` of directory, `rsync`, `tar -x`, `unzip`, `git clone`, `git checkout`) | §11 item 10 | Explicitly listed in §5 and §11 item 10. |
| Code runners (inline code) and unpinned code-runner Allow prefixes (`awk`, `sed` with `e`/`w`/`r` or `-f`, `find -exec*`/`-ok*`, `eval`, `xargs`, `ag --pager`, `ack --output`, `git -c`/`--config-env`/`--exec-path`/`--ext-diff`/`--receive-pack`/`--exec`/program-running subcommands, `COMMAND_RUNNERS`) | §11 item 4; §11 item 5 | Explicitly listed in §4, §6, §7, and §11 items 4–5. |
| `cd` flags outside `-L`/`-P`/`-e`/`-@` (`FlagMode::ListedOnly`) and `pushd`/`popd` (`OUTSIDE_MODEL`) | No change from `dev` | `dev` (`words.rs`) already marks cwd unknown (`Cd::Lost`), preventing Home and prefix/exact rule coverage. |
| Shell reserved words (`shell::RESERVED` in `OUTSIDE_MODEL`) | §11 item 2 | Compound constructs outside §3's simple-command model (already rejected in Home and prefix/exact rules on `dev`). |

### Round 2 verification

Run by worker (allowed under D-123 + Round 2 instruction):
- `cargo fmt --all` — clean
- `cargo check -p atrium-core --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core --tests -- -D warnings` — clean (0 errors, 0 warnings)

Commands for the orchestrator's verification task (fish-compatible):

```sh
env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```

---

## Round 3 (rework against Codex review round 2 & full-table audit under Rules A–F)

All 6 findings from `.orchestration/review-input/review-round2.md` are fixed, and the entire tool table (`TOOLS` + all subcommands) was audited under Rules A–F so no accepted flag or form can omit its real policy effect. `docs/contracts/m1-policy-rev4-tools.md` is intentionally left untouched for the orchestrator to regenerate via `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`. Commits `c89a8bd` (`dev` merge) and `f966066` (§11 item 15 approved) are preserved untouched.

### Findings, fixes, and commits

| # | Finding | Fix | Commit(s) |
|---|---|---|---|
| 1 | Unlisted options retained acceptance (`AnyExceptForbidden` outside Home) and lost write semantics (`cp -t vault README.md`, `sort --out=alias README.md`) | Added `AccessMode::ReadWrite` in `model.rs`; restricted `FlagMode::AnyExceptForbidden` strictly to the 22 non-`rg`/non-`grep` `HOME_READONLY_COMMANDS` rows (defaulting `base_tool`/`base_subcommand` to `FlagMode::ListedOnly`); documented conservative `ReadWrite` candidate-path treatment on unlisted flags and `UnknownFlag` fallbacks; added `is_forbidden_flag` (mirroring `engine::forbidden_flag`: short clusters + long abbreviations `>= 3` chars including `--`) and `flag_long_or_writing_abbrev` (so `sort --out=alias`, `tree --out=alias`, `git diff --out=alias` receive `--output`'s write effect); added `any_except_forbidden_restricted_to_home_readonly` integrity test and fixtures. | `ca6c07f`, `28da20f`, `3d9a9ae` |
| 2 | Ripgrep's global glob case setting (`--glob-case-insensitive` / `--no-glob-case-insensitive`) had no effect | Added `FilterCase` (`AlwaysSensitive`, `AlwaysInsensitive`, `FollowGlobalToggle`), `FilterSpec`, and `FlagEffects::filter_case_insensitive: FlagToggle`; set `--glob-case-insensitive` → `Enable`, `--no-glob-case-insensitive` → `Disable`, `-g`/`--glob` → `FollowGlobalToggle`, and `--iglob` → `AlwaysInsensitive`; added ordering/override fixtures. | `28da20f`, `3d9a9ae` |
| 3 | Unsupported search filters (`grep --exclude`, `--exclude-dir`, `--include-dir`, `ag` ignore/filter flags, etc.) lacked an `Unread` effect | Added `FlagUnread::FilterForm` → `UnreadReason::UnsupportedFilterForm`; marked `grep`'s `--exclude`, `--exclude-dir`, `--include-dir`, `ag`'s `-G`, `-g`, `-p` (preserving `Read` on its ignore file), `--ackmate-dir-filter`, `--depth`, `--ignore`, `--ignore-dir`, `--one-device`, `rg`'s `-t`, `-T`, `--type-add`, `--type-clear`, `-d`/`--max-depth`, `--maxdepth`, `--max-filesize`, `ack`'s `-g`, `-t`/`--type`, `--ignore-dir`, `--noignore-dir`, `-k`, and `find`'s `-x`, `-maxdepth`, `-mindepth` with `FlagUnread::FilterForm`; added fixtures. | `28da20f`, `3d9a9ae` |
| 4 | `tar -C` lacked per-operand base-directory resolution semantics | Added `UnreadReason::UnsupportedBaseDirChange { tool, flag }` in `model.rs` and `FlagUnread::BaseDirChange` in `tools.rs`; marked `tar -C`/`--directory` (preserving `WriteWhenWriting` on its dir arg) and `git`'s `-C`, `--git-dir`, `--work-tree` (preserving `Read` on their path args) and `--super-prefix` with `FlagUnread::BaseDirChange`; added fixtures. | `ca6c07f`, `28da20f`, `3d9a9ae` |
| 5 | Named `sed -f edit.sed` program could not receive program coverage | Added `ProgramPin::FlagValue` and `FlagEffects::runs_program: Option<ProgramPin>`; changed `sed -f`/`--file` to `runs_code: false, runs_program: Some(ProgramPin::FlagValue), value_access: Some(FlagValueAccess::Read)`, added `awk`/`gawk`/`mawk` `-f`/`--file` with the same pinning semantics (`CodeRunnerBehavior::ProgramOrInline { pin: ProgramPin::FlagValue }`), kept `sed -e` inline script inspection (`e`/`w`/`r`) unchanged, cited `§11 item 8` on `sed`/`awk`/`gawk`/`mawk`, and added `flag_program_files_have_pin_and_cite_item_8` integrity test + fixtures. | `28da20f`, `3d9a9ae` |
| 6 | Flag-value operand relaxations (`codesign -s`, `git` text-valued globals, `find` text-valued options) lacked `§11 item 9` citations | Annotated `codesign`, `git`, `find`, `nice`, `timeout`, `git bisect`, `git rebase`, `git submodule`, `git difftool`, `cargo` subcommands, and `target_then_code_runner` rows with `§11 item 9`; removed `wrapper`/`code_runner` bypasses from `non_path_operand_relaxations_cite_item_9` and extended it to check all `FlagKind::Value`/`OptionalValue` flag roles and `positional_role` overrides. | `28da20f`, `3d9a9ae` |

### Row-by-row audit changes (Rules A–F) and what became `Unread`

1. **Schema & integrity guarantees (Rules A–F):**
   - Replaced implicit `FlagEffects::NONE` with explicit `Effect::None { reason: &'static str }` vs. `Effect::Applied(FlagEffects)` on every `FlagSpec`; added `FlagValueAccess` (`Read`, `Write`, `WriteWhenWriting`), `FilterCase`, `FilterSpec`, `GitPushEffect`, and `FlagUnread` (`FilterForm`, `BaseDirChange`, `FlagEffect`).
   - Added `UnreadReason::UnsupportedBaseDirChange` and `UnreadReason::UnsupportedFlagEffect` plus `AccessMode::ReadWrite` in `model.rs`.
   - Added integrity tests `every_flag_declares_explicit_effect_or_none_reason` (Rule A), `any_except_forbidden_restricted_to_home_readonly` (Rules B & C), `flag_program_files_have_pin_and_cite_item_8` (Rule D), and extended `non_path_operand_relaxations_cite_item_9` to all rows/subcommands and flag values (Rule F).
2. **Row-by-row changes:**
   - **`ls`, `pwd`, `cat`, `head`, `tail`, `wc`, `file`, `stat`, `du`, `df`, `which`, `uname`, `date`, `whoami`, `tree`, `git status`, `git log`, `git diff`, `git show`, `git branch`, `git rev-parse`, `git ls-files` (22 Home read-only rows):** retained `FlagMode::AnyExceptForbidden` (explicitly set per row) with conservative `AccessMode::ReadWrite` treatment for unlisted flags and attached `--opt=value`; `tree -o`/`--output` and `git diff --output` gained explicit `value_access: Write` + long-abbreviation write matching (`--out=...`).
   - **All non-Home rows (`cp`, `mv`, `rsync`, `tar`, `unzip`, `ln`, `find`, `rm`, `tee`, `touch`, `chmod`, `chown`, `chgrp`, `truncate`, `install`, `mkdir`, `rmdir`, `unlink`, `shred`, `patch`, `wipefs`, `blkdiscard`, `ed`, `ex`, `diff`, `cmp`, `md5`, `md5sum`, `shasum`, `sha1sum`, `sha256sum`, `readlink`, `realpath`, `basename`, `dirname`, `nl`, `od`, `hexdump`, `strings`, `bat`, `column`, `fold`, `expand`, `less`, `more`, `cut`, `sort`, `uniq`, `xxd`, `echo`, `printf`, `whereis`, `type`, `tr`, `true`, `false`, `id`, `test`, `[`, `jq`, `mdls`, `open`, `spctl`, `mdfind`, `codesign`, `awk`, `gawk`, `mawk`, `sed`, `perl`, `python`, `python3`, `node`, `ruby`, `eval`, `source`, `.`, `npx`, `xargs`, `COMMAND_RUNNERS`, `PRIV`, `DOWNLOADERS`, `git` top-level + non-Home git subcommands, `cargo` + subcommands, `pipx` + `pipx run`):** switched to `FlagMode::ListedOnly` (Rule C), so any unlisted flag (e.g. `cp -t`, `rm -f`, `mkdir -p`, `test -f`, `cargo test --lib`) becomes `Unread(UnknownFlag)` with conservative `ReadWrite` fallback on unclassified words.
   - **`rg`:** `--glob-case-insensitive` / `--no-glob-case-insensitive` now toggle `filter_case_insensitive`; `-g`/`--glob` uses `FollowGlobalToggle`; `--iglob` uses `AlwaysInsensitive`; `-f`/`--file` sets `value_access: Read`; `-t`/`--type`, `-T`/`--type-not`, `--type-add`, `--type-clear`, `-d`/`--max-depth`, `--maxdepth`, `--max-filesize` now produce **`Unread(UnsupportedFilterForm)`**; all remaining flags carry explicit `Effect::None { reason }`. Added `§11 item 2`.
   - **`grep`, `egrep`, `fgrep`, `zgrep`, `ggrep`, `rgrep`:** `-f`/`--file` sets `value_access: Read`; `--include` uses `AlwaysSensitive`; `--exclude`, `--exclude-dir`, and `--include-dir` (added for §5/Rule A) now produce **`Unread(UnsupportedFilterForm)`**; `-d`/`--directories` keeps `recursive: Enable` and produces **`Unread(UnsupportedFlagEffect)`** (action depends on `read`/`skip`/`recurse` value); all remaining flags carry explicit `Effect::None { reason }`. Added `§11 item 2`.
   - **`ag`:** `-G`/`--file-search-regex`, `-g`/`--filename-pattern`, `-p`/`--path-to-ignore` (with `value_access: Read`), `--ackmate-dir-filter`, `--depth`, `--ignore`, `--ignore-dir`, and `--one-device` now produce **`Unread(UnsupportedFilterForm)`**; `-z`/`--search-zip` now produces **`Unread(UnsupportedFlagEffect)`**; all remaining flags carry explicit `Effect::None { reason }`. Added `§11 item 2`.
   - **`ack`:** `-g`, `-t`/`--type`, `--ignore-dir`, `--noignore-dir`, and `-k` now produce **`Unread(UnsupportedFilterForm)`**; `-x` (reads file list from stdin) now produces **`Unread(UnsupportedFlagEffect)`**; all remaining flags carry explicit `Effect::None { reason }`. Added `§11 item 2`.
   - **`tar`:** `-f`/`--file` sets `value_access: WriteWhenWriting`; `-C`/`--directory` sets `value_access: WriteWhenWriting` and now produces **`Unread(UnsupportedBaseDirChange)`**. Added `§11 item 2`.
   - **`unzip`:** `-d` sets `value_access: Write`.
   - **`find`:** `-f` sets `value_access: Read`; `-fprint`/`-fprint0`/`-fprintf`/`-fls` set `value_access: Write`; `-delete` also sets `derived_destinations: Enable` (`find.derived_destinations = WithFlag`); `-x`, `-maxdepth`, and `-mindepth` now produce **`Unread(UnsupportedFilterForm)`**; `-E`/`-X`/`-d`/`-s`/`-depth`/`-D` carry explicit `Effect::None { reason }`. Added `§11 item 2`, `§11 item 9`, `§11 item 10`.
   - **`sort` / `tree`:** `-o`/`--output` sets `value_access: Write`; long abbreviations (`--out=...`, `--o=...`) match `--output` via `flag_long_or_writing_abbrev`.
   - **`printf`:** `-v` (bash variable assignment, which can set `HOME`) now produces **`Unread(UnsupportedFlagEffect)`**. Added `§11 item 2`.
   - **`codesign`:** `-d`/`-v` carry explicit `Effect::None { reason }`; row now cites `§11 item 9` (for `-s`/`--sign` `Value(Text)`).
   - **`sed`:** `-f`/`--file` changed from `runs_code: true` to `runs_program: Some(ProgramPin::FlagValue), value_access: Some(FlagValueAccess::Read)` (pinnable `Unread(program)`). Added `§11 item 8`.
   - **`awk`, `gawk`, `mawk`:** added `-f`/`--file` (`Value(ProgramFile)`, `runs_program: Some(ProgramPin::FlagValue)`, `value_access: Read`, `provides_script: true`, `writes: Enable`) and switched `code_runner` to `ProgramOrInline { pin: ProgramPin::FlagValue }`. Added `§11 item 8`.
   - **`sh`, `bash`, `dash`, `perl`, `python`, `python3`, `node`, `ruby`, `source`, `.`, `npx`, `pipx run`:** changed trailing script-argument `rest` role from `ValueRole::Text` to `ValueRole::Path` so script arguments (e.g. `python build.py proj/key`) keep candidate-path hard-deny checks instead of being relaxed.
   - **`git` (top-level):** `-C`, `--git-dir`, `--work-tree` set `value_access: Read` and produce **`Unread(UnsupportedBaseDirChange)`**; `--super-prefix` produces **`Unread(UnsupportedBaseDirChange)`**; `--namespace` carries `Effect::None { reason }`. Added `§11 item 2; §11 item 9`.
   - **`git push`:** `-f`/`--force`/`--force-with-lease`/`--force-if-includes`/`--mirror` carry `git_push: Some(Force)`; `--all`/`--tags`/`--branches`/`--prune` carry `git_push: Some(ImplicitTargets)`; `-o`/`--push-option` carries `Effect::None { reason }`.
   - **`git checkout`:** `-b`/`-B`/`--orphan` set `writes: Enable`.
   - **`git bisect`, `git rebase`, `git submodule`, `git difftool`, `nice`, `timeout`, `cargo` subcommands, `target_then_code_runner` (19 tools):** added `§11 item 9` per Rule F.

### Updated full list of rows marked `changes_default` (Round 3, for Pedro)

| Entry | §11 item(s) | Reason |
|---|---|---|
| `command`, `exec`, `time`, `env` | §11 item 10 | Closed §4 wrapper forms (`ListedOnly`); other forms/options → `Unread`. |
| `nice`, `timeout` | §11 item 9; §11 item 10 | Non-path priority/signal/duration flag/operand values (`Text`; item 9); closed §4 wrapper forms (item 10). |
| `sh`, `bash`, `dash` | §11 item 2; §11 item 4; §11 item 8 | K7 positional resolution & non-§4 flags → `Unread` (item 2); bare prefix invalid at creation (item 4); script file (`sh x.sh`) pinnable `Unread (program)` (item 8). |
| `which`, `echo`, `whereis`, `type`, `tr`, `true`, `false`, `id`, `jq`, `chmod`, `chown`, `chgrp`, `codesign` | §11 item 9 | Non-path operands or flag values (`Text`/`Pattern`; including `codesign -s`) stop counting as candidate paths. |
| `printf` | §11 item 2; §11 item 9 | `-v` shell-variable assignment → `Unread` (item 2); format/value operands (`Text`) stop counting as candidate paths (item 9). |
| `rg`, `grep` | §11 item 2; §11 item 9; §11 item 15 | Unsupported filters/depth/size/dir-action flags → `Unread` (item 2); pattern & text values not paths (item 9); followed-link search allowed in Home when link walk is within budget (item 15, D-139). |
| `egrep`, `fgrep`, `zgrep`, `ggrep`, `rgrep` | §11 item 2; §11 item 9 | Unsupported filters/dir-action flags → `Unread` (item 2); pattern & text values not paths (item 9). |
| `ag`, `ack` | §11 item 2; §11 item 4; §11 item 9 | Unsupported ignore/filter/zip/stdin flags → `Unread` (item 2); unpinned Allow prefix invalid (item 4); pattern & text values not paths (item 9). |
| `git` (bare prefix) | §11 item 2; §11 item 4; §11 item 9 | `-C`/`--git-dir`/`--work-tree`/`--super-prefix` → `Unread(UnsupportedBaseDirChange)` (item 2); bare `["git"]` prefix invalid (item 4); `--namespace`/`--super-prefix` `Text` values not paths (item 9). |
| `git status`, `git ls-files` | §11 item 10 | Active wildcard pathspecs → `Unread`. |
| `git log`, `git show` | §11 item 9; §11 item 10 | Ref operands / `-L` text not filesystem paths (item 9); active wildcard pathspecs → `Unread` (item 10). |
| `git diff` | §11 item 4; §11 item 9; §11 item 10 | Prefix ending at `["git", "diff"]` invalid (`--ext-diff`; item 4); ref operands not paths (item 9); active wildcard pathspecs → `Unread` (item 10). |
| `git push` | §11 item 4; §11 item 9 | Prefix ending at `["git", "push"]` invalid (`--receive-pack`/`--exec`; item 4); ref operands & `-o` text not paths (item 9). |
| `git branch`, `git rev-parse`, `git cat-file` | §11 item 9 | Ref operands / `-u` ref not filesystem paths. |
| `git clone`, `git checkout` | §11 item 9; §11 item 10 | Ref operands / `-b`/`-B`/`--orphan` not filesystem paths (item 9); derived destinations → `Unread` (item 10). |
| `git merge-index`, `git filter-branch`, `git mergetool` | §11 item 4 | Program-running git subcommands; unpinned Allow prefixes invalid at creation. |
| `git bisect`, `git rebase`, `git submodule`, `git difftool` | §11 item 4; §11 item 9 | Program-running git subcommands (item 4); subarg/ref operands (`Text`/`Ref`) not filesystem paths (item 9). |
| `cargo` (bare) | §11 item 4; §11 item 8 | Bare `["cargo"]` invalid at creation (item 4); `["cargo", "<subcmd>"]` pins `Unread (program)` (item 8). |
| `cargo test`, `build`, `run`, `check`, `bench`, `doc`, `clippy` | §11 item 8; §11 item 9 | Pinnable `Unread (program)` (item 8); test/target filter operands (`Text`) not filesystem paths (item 9). |
| `pipx`, `pipx run` | §11 item 4; §11 item 8 | Bare `["pipx"]` and `["pipx", "run"]` invalid at creation (item 4); `["pipx", "run", "<pkg>"]` pins `Unread (program)` (item 8). |
| `find` | §11 item 2; §11 item 4; §11 item 9; §11 item 10 | `-maxdepth`/`-mindepth`/`-x` → `Unread(UnsupportedFilterForm)` (item 2); bare `["find"]` invalid (item 4); `-D`/`-maxdepth`/`-mindepth` `Text` values not paths (item 9); `-delete` derived destinations (item 10). |
| `cp`, `mv`, `rsync` | §11 item 10 | Derived destinations (`cp -r`/`-R`/`-a`, `mv` of directory, `rsync`) → `Unread`. |
| `tar` | §11 item 2; §11 item 9; §11 item 10 | `-C`/`--directory` → `Unread(UnsupportedBaseDirChange)` (item 2); extract/list `ArchiveMember` operands not host paths (item 9); `-x` derived destinations → `Unread` (item 10). |
| `unzip` | §11 item 9; §11 item 10 | `ArchiveMember` operands not host paths (item 9); derived destinations → `Unread` (item 10). |
| `awk`, `gawk`, `mawk`, `sed` | §11 item 4; §11 item 8 | Bare prefix invalid at creation (item 4); `-f`/`--file` flag-held program file is pinnable `Unread (program)` under `ProgramPin::FlagValue` (item 8). |
| `perl`, `python`, `python3`, `node`, `ruby`, `source`, `.`, `npx` | §11 item 4; §11 item 8 | Bare prefix invalid at creation (item 4); `[tool, script]` pins `Unread (program)` (item 8). |
| `eval`, `xargs`, direct `COMMAND_RUNNERS` (9: `nsenter`, `unshare`, `systemd-run`, `systemd-nspawn`, `firejail`, `bwrap`, `flatpak-spawn`, `setpriv`, `parallel`) | §11 item 4 | Code runners; unpinned Allow prefix invalid at creation. |
| Target-then-code `COMMAND_RUNNERS` (19: `ssh`, `mosh`, `autossh`, `docker`, `podman`, `nerdctl`, `kubectl`, `oc`, `lxc`, `incus`, `machinectl`, `toolbox`, `distrobox`, `distrobox-enter`, `multipass`, `vagrant`, `limactl`, `colima`, `runuser`) | §11 item 4; §11 item 9 | Code runners (item 4); `[0]` host/target operand (`Text`) not a filesystem path (item 9). |
| `OUTSIDE_MODEL` shells (12: `zsh`..`xonsh`) | §11 item 4 | Shells outside §4's modeled `sh`/`bash`/`dash` subset → `Unread`, unpinned Allow prefix invalid. |
| `OUTSIDE_MODEL` wrappers (16: `nohup`..`arch`) | §11 item 10 | Wrappers outside §4's closed list → `Unread`. |

### Round 3 verification

Run by worker (allowed under D-123 + Round 3 instruction):
- `cargo fmt --all` — clean
- `cargo check -p atrium-core --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core --tests -- -D warnings` — clean (0 errors, 0 warnings)

Commands for the orchestrator's verification task (fish-compatible):

```sh
env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```


---

## Round 4 (rework against Codex review round 3 & Rule G per-operand/per-operation access audit)

All 6 findings from `.orchestration/review-input/review-round3.md` are fixed, per-operand and per-operation access modes were audited under Rule G, and every flag added in round 3 was checked against `dev`'s tables. `docs/contracts/m1-policy-rev4-tools.md` is intentionally left untouched for the orchestrator to regenerate via `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.

### Findings, fixes, and commits

| # | Finding | Fix | Commit(s) |
|---|---|---|---|
| 1 | Newly restricted `ListedOnly` rows lacked `§11 item 2` default-change citations (`sort -r`, `rm -f`, etc.) | Defaulted `base_tool`/`base_subcommand` to `changes_default: Some("§11 item 2")`, explicitly set `changes_default: None` on the 14 `AnyExceptForbidden` tools with no other default changes and on the 6 always-`HardDeny` `PRIV` rows (never coverable by `dev`'s rules), added `§11 item 2` to every other `ListedOnly` row/subcommand, added the `listed_only_rows_cite_item_2` integrity test (also checking `AnyExceptForbidden` rows do not spuriously cite item 2), and added fixtures for `sort -r` / `rm -f`. | `ada80e4`, `8ebed58` |
| 2 | Home-forbidden flags on `AnyExceptForbidden` rows were declared `Unread` under every setting | Fixed the module doc, `FlagMode::AnyExceptForbidden`, `HomeReadOnlySpec::forbidden`, `fmt_flags_summary`, and `render_markdown()` intro so `home_readonly.forbidden` **only removes Home read-only eligibility** (`home_readonly_eligible = false`, so Home asks as on `dev`) and never by itself makes the unit `Unread`. Modeled flags (`tree -o`/`--out`, `git diff --output`/`--out`, `rg -f`, `grep -f`) keep their declared effects under covering rules; unlisted flags on `AnyExceptForbidden` (`tree -R`, `file -C`/`-f`, `date -s`) keep conservative `ReadWrite` candidate-path checks; `git diff --ext-diff` stays `Unread` from its declared `runs_code` effect. Added `home_readonly_eligible` to `SimulatedUnit` and tested both Home and covering-rule settings. | `a5707d4`, `8ebed58` |
| 3 | `tar` extraction marked its input archive as `Write` (and `-C` access was tied to row-wide `writes`) | Added `ArchiveOperation` (`Write`, `Extract`, `List`) and `FlagEffects::operation`; replaced `FlagValueAccess::WriteWhenWriting` with per-operation `FlagValueAccess::TarArchive` (`Write` on `-c`/`-r`/`-u`, `Read` on `-x`/`-t`) and `FlagValueAccess::TarDirectory` (`Write` on `-x` extraction destination dir, `Read` on `-c`/`-r`/`-u`/`-t` source/list chdir dir; changed `-C` role to `ValueRole::Path` and `tar` positional `operand_access` to `AllRead`). Added independent fixtures for `-c`, `-r`, `-u`, `-x`, `-t`, flag ordering (`tar -f archive.tar -x`), and `-C` under every operation. | `a5707d4`, `8ebed58` |
| 4 | `sed -f FILE` turned input files into `Write` targets | Separated unit-level possible-writer status (`FlagEffects::writes`, making later units/pipeline siblings `Unread` under §5) from in-place positional input writes (`FlagEffects::writes_positional_inputs` + `OperandAccessMode::InPlaceWithFlag`, removing `AllWriteWhenWriting` and `FlagOnlyWrite`; clarified `ToolEffects::writes` in `model.rs`). On `sed -f FILE`, `FILE` is `ProgramFile` (`Read`, pinned under `ProgramPin::FlagValue`, §11 item 8), positional inputs stay `Read`, and the unit is a possible writer (`writes = true`). Only `-i`/`--in-place` (`sed`, `perl`), `-s`/`-f`/`--remove-signature` (`codesign`), and `-delete` (`find`) enable `writes_positional_inputs` (never marking `ProgramFile` operands `Write`). Removed round 3's erroneous `sed -f` Write assertion and added independent fixtures for `sed -f`, `sed -i -f`, `sed -i`, `sed -e`, `perl`, `perl -i`, `awk`, `codesign`, `ln`, `cp`, `uniq`, `unzip`, and `find`. | `a5707d4`, `8ebed58` |
| 5 | `find -fprintf FILE FORMAT` lacked its second argument role (and `-exec`/`-execdir`/`-ok`/`-okdir` consumed only 1 arg) | Added `FlagArgSpec`, `FlagArity`, `FlagKind::MultiValue(&'static [FlagArgSpec])`, `FlagKind::TerminatedValues { role, terminators }`, and `FlagKind::arity()`. Modeled `find -fprintf` as `MultiValue([Path:Write, Text])`, `-exec`/`-execdir` as `TerminatedValues(Code, &[";", "+"])`, and `-ok`/`-okdir` as `TerminatedValues(Code, &[";"])`; verified `-fprint`/`-fprint0`/`-fls` (1 `Write Path` arg) and left unlisted primaries (`-printf`, `-newerXY`, `-newermt`, `-name`, `-regex`) outside `FIND_FLAGS` (`Unread(UnknownFlag)`). Updated Markdown rendering and added `no_flag_consumes_fewer_arguments_than_documented` integrity test plus fixtures. | `a5707d4`, `8ebed58` |
| 6 | Invented `grep --include-dir` and its parity-test exception | Removed `--include-dir` from `GREP_FLAGS` and removed `"include-dir"` from `grep_dev_val_l` in `dev_lists_and_flag_tables_fully_accounted_for`; updated fixtures to assert `grep --include-dir=sub x proj` produces `UnreadReason::UnknownFlag`. | `a5707d4`, `8ebed58` |

### Flags removed (round-3 audit against `dev`'s tables)

| Tool(s) | Flag removed | Reason |
|---|---|---|
| `grep`, `egrep`, `fgrep`, `zgrep`, `ggrep`, `rgrep` | `--include-dir` | Absent from `dev`'s `GREP_VAL_L` (Finding 6); unknown-flag rule on `ListedOnly` already marks it `Unread(UnknownFlag)`. Parity-test exception removed. |
| `awk`, `gawk`, `mawk` | `-f` / `--file` | Added in round 3 but absent from `dev`'s tables (`words.rs` only handles `-i`/`--include` for `awk`) and not needed to make a form `Unread` (`awk` is `ListedOnly`, so `awk -f` is `Unread(UnknownFlag)` and `awk` returns to `CodeRunnerBehavior::InlineOnly` per contract §4). Removed `§11 item 8` from `awk`/`gawk`/`mawk`. |

### Rows cited for `§11 item 2` (all 151 non-`hard_deny` `ListedOnly` rows)

Every `FlagMode::ListedOnly` row that `dev`'s rules could cover now cites `§11 item 2` in `changes_default` (enforced by `listed_only_rows_cite_item_2`):

- **Already citing `§11 item 2` in round 3 (16):** `sh`, `bash`, `dash`, `rg`, `grep`, `egrep`, `fgrep`, `zgrep`, `ggrep`, `rgrep`, `ag`, `ack`, `git`, `find`, `tar`, `printf`.
- **Newly citing `§11 item 2` in round 4 (135):**
  - **Wrappers (6):** `command`, `exec`, `nice`, `timeout`, `time`, `env`.
  - **Multi-subcommand parents (2):** `cargo`, `pipx`.
  - **Subcommands (19):** `git cat-file`, `git push`, `git clone`, `git checkout`, `git merge-index`, `git filter-branch`, `git bisect`, `git rebase`, `git submodule`, `git difftool`, `git mergetool`; `cargo test`, `cargo build`, `cargo run`, `cargo check`, `cargo bench`, `cargo doc`, `cargo clippy`; `pipx run`.
  - **File, directory, archive, and link tools (6):** `cd`, `ln`, `cp`, `mv`, `rsync`, `unzip`.
  - **`words::WRITERS` (17):** `rm`, `tee`, `touch`, `chmod`, `chown`, `chgrp`, `truncate`, `install`, `mkdir`, `rmdir`, `unlink`, `shred`, `patch`, `wipefs`, `blkdiscard`, `ed`, `ex`.
  - **`words::READERS` (40):** `diff`, `cmp`, `md5`, `md5sum`, `shasum`, `sha1sum`, `sha256sum`, `readlink`, `realpath`, `basename`, `dirname`, `nl`, `od`, `hexdump`, `strings`, `bat`, `column`, `fold`, `expand`, `less`, `more`, `cut`, `sort`, `uniq`, `xxd`, `echo`, `whereis`, `type`, `tr`, `true`, `false`, `id`, `test`, `[`, `jq`, `mdls`, `open`, `spctl`, `mdfind`, `codesign`.
  - **Code runners & interpreters (14):** `awk`, `gawk`, `mawk`, `sed`, `perl`, `python`, `python3`, `node`, `ruby`, `eval`, `source`, `.`, `npx`, `xargs`.
  - **`shell::COMMAND_RUNNERS` (28):** `ssh`, `mosh`, `autossh`, `nsenter`, `unshare`, `docker`, `podman`, `nerdctl`, `kubectl`, `oc`, `lxc`, `incus`, `machinectl`, `systemd-run`, `systemd-nspawn`, `firejail`, `bwrap`, `flatpak-spawn`, `toolbox`, `distrobox`, `distrobox-enter`, `multipass`, `vagrant`, `limactl`, `colima`, `runuser`, `setpriv`, `parallel`.
  - **`deny::DOWNLOADERS` (3):** `curl`, `wget`, `fetch`.
- **Excluded (not coverable by `dev`'s rules):** `deny::PRIV` (`sudo`, `sudoedit`, `su`, `doas`, `pkexec`, `run0`) are always `HardDeny` (`privilege-escalation`) on both `dev` and rev 4 (`changes_default: None`).
- **`AnyExceptForbidden` rows (22, unchanged — do not cite item 2):** `ls`, `pwd`, `cat`, `head`, `tail`, `wc`, `file`, `stat`, `du`, `df`, `which`, `uname`, `date`, `whoami`, `tree`, `git status`, `git log`, `git diff`, `git show`, `git branch`, `git rev-parse`, `git ls-files`.

### Round 4 verification

Run by worker (allowed under D-123):
- `cargo fmt --all` — clean
- `cargo check -p atrium-core --tests` — clean (0 errors, 0 warnings)
- `cargo clippy -p atrium-core --tests -- -D warnings` — clean (0 errors, 0 warnings)

Commands for the orchestrator's verification task (fish-compatible):

```sh
env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
```
