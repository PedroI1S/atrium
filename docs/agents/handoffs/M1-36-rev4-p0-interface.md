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
2. **Finding 2 (followed searches `rg -L`, `grep -R`):** Contract §10 row 15 makes `rg -L x proj` (with no link out of `proj`, within budget) Allow in Home, while §11 did not list that change. Marked `rg` and `grep` with `"§11 item 15 (proposed; §10 row 15)"` pending driver approval, gated Home followed-search eligibility behind a single constant `tools::ALLOW_HOME_FOLLOWED_SEARCH` (`HomeReadOnlySpec::allow_followed_search`) so rejecting item 15 is a one-line revert to `false`, and updated the `changes_default` citation integrity test to accept item 15.

### Findings, fixes, and commits

| # | Finding | Fix | Commit |
|---|---|---|---|
| 1 | Unrecorded restrictions change Home & covering-rule verdicts | Added `FlagMode::AnyExceptForbidden` across all non-search `HOME_READONLY_COMMANDS` tools/git-subcommands and tools without `dev` flag tables; moved `test`, `[`, `ed`, `ex`, `open`, `spctl`, `mdfind` out of `OUTSIDE_MODEL` into `TOOLS`; kept `pwd`/`uname`/`whoami`/`date` operands as `Path`; added effective Home parity tests and bidirectional `dev` flag-table parity tests. | `f493858`, `8dcf720` |
| 2 | Followed-search relaxations (`rg -L`, `grep -R`) lacked §11 record | Annotated `rg` and `grep` with `"§11 item 15 (proposed; §10 row 15)"`, added single-line switch `ALLOW_HOME_FOLLOWED_SEARCH` + `HomeReadOnlySpec::allow_followed_search`, updated citation test to accept item 15. | `f493858`, `8dcf720` |
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
| `rg`, `grep` | §11 item 9; §11 item 15 (proposed; §10 row 15) | Pattern operand (`[0] Pattern` / `-e`) stops counting as a candidate path (item 9); followed-link search (`rg -L`, `grep -R`) allowed in Home when link walk is within budget and reaches no protected path (proposed item 15, controlled by `ALLOW_HOME_FOLLOWED_SEARCH`). |
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
