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
