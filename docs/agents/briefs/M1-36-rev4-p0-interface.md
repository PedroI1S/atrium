# M1-36-rev4-p0-interface: rev 4's shared interface, the tool table and `limits` (P0)

Model: Gemini 4 Neon (`gemini-4-neon-high`, `scripts/agent-run agy`, D-134) · Reviewer: Codex GPT-6.1 Sol `high`, read-only (D-124) · Depends on: nothing open
Go: D-138 · Branch: `agent/neon/M1-36-rev4-p0-interface` · Worktree: `../atrium-wt/M1-36-rev4-p0-interface`
Contract: `docs/contracts/m1-policy-rev4.md` (draft 5, approved by D-136 and D-138). Read all of it before you start. §1, §2 and §5–§8 define what the types must be able to hold.

## Goal
P0 from contract §13. It writes the types that P1, P2, P3 and P5 build on, the normative tool table (§6) as data, and the frozen numbers (§8). **It changes no behaviour:** nothing new is wired into `SessionGate`, and `dev`'s evaluator stays live. P1–P3 then build in parallel behind this interface, so leave no type for them to invent.

## What to write

### 1. `crates/atrium-core/src/policy/model.rs`: the types
Every type here is `pub(crate)`, documented with the contract section it comes from.
- **Words with provenance (§3):** a `Word` holds its bytes and, for each byte, how it was written: `Unquoted`, `SingleQuoted`, `DoubleQuoted`, `Escaped`, or `Derived` (it came out of a resolution or an expansion, so it is never parsed as syntax again). Helpers: `has_unquoted_glob()` and `literal_bytes()`. Keep the per-byte record compact (one byte per input byte, or runs); the contract wants linear work.
- **Units (§3, §4):**
  - argv `Word`s, after the wrappers are removed, with the removed wrappers kept in a list;
  - redirects, from the closed list (`<`, `>`, `>>`, `2>`, `2>>` with a literal target; `n>&m`; `n>&-`);
  - its position in the line: index, the separator before it (`;`, `&&`, `||`, `|`, newline), its pipeline id, and its nesting depth with the `-c` unit it came from.
- **A unit record for the best-effort pass (§2):** the raw text and the units the lexer produced, including those from `-c` scripts. This is separate from exact units, because the model never reads them as Clear.
- **Outcomes (§1):** `PartOutcome` is one of:
  - `HardDenyHit(Hit)`;
  - `StoreHit(Hit)`;
  - `Clear`;
  - `Unread(UnreadReason)`;
  - `Exhausted(ExhaustReason)`;
  - `TooLarge(Cap)`.

  `Hit` records:
  - its source, `Model` or `BestEffort`;
  - what matched: a protected path, a store family, a probe name, or a command word such as `sudo`;
  - the part it came from.

  `UnreadReason` is a **closed** enum with one variant for each "Unread" or "outside the model" case that §2–§7 name: an unquoted byte outside §3's set, with the byte; a reserved word or construct; assignment; `HOME` set earlier; a command-word glob; `MAX_WRAP`; a wrapper form; a nested-shell form; a code runner (inline code); `Program { program words }`; an unknown flag; a process-dependent path; a path that can't be resolved; another equivalence; a variant-only glob match; a filter form; a followed link under a filter; a walk cap; derived destinations; after a possible writer; a pipeline with a possible writer; an unknown cwd; a Git wildcard pathspec; a descriptor outside 0–2; `&>`. Name each variant after its contract wording, and cite its section in a doc comment.

  `CallOutcome` keeps **every** part's outcome, with its unit or part index; it never keeps only the first one (§1: "every hit and every reason for doubt … is kept").
- **Tool effects (§6):** `ToolEffects` holds:
  - for each argv position, its `OperandRole`: `Path`, `Pattern`, `ArchiveMember`, `Ref`, `Text`, `DestinationRoot`, `Code`, `ProgramFile`, `Flag`, or `FlagValue(role)`;
  - whether the unit writes, writes to derived destinations, reads recursively, or runs code, and which operand holds the code;
  - how it follows links, at the root and below the root;
  - the filter dialect: `SearchGlob`, `RgGlob` or `GrepInclude`.
- **Canonical targets (§5):** a `Target` holds:
  - the operand or redirect it came from;
  - whether it is read or written;
  - one of:
    - `Resolved { canonical: Vec<u8>, identities }`, where `identities` are the (device, inode) pairs of the path and of its existing ancestors, and a dangling link counts under its own name;
    - `Unread(UnreadReason)`.

  Resolved paths are bytes, never patterns.
- **The budget (§8), in `model.rs` or its own `budget.rs`:**
  - one compute budget per check, charged **before** the work: `charge(units)` and `charge_alloc(bytes)` return `Err(Exhausted)`, and once exhausted it stays exhausted;
  - separate filesystem caps: entries read, directory opens, `stat`/`lstat` calls, link reads and canonicalizations. Each is charged before the operation, and running out returns `CapReached`, which callers turn into Unread, not Exhausted (§2).

### 2. `crates/atrium-core/src/policy/limits.rs`: the frozen numbers (§8, Q2)
- `MAX_BYTES`, `MAX_UNITS`, `MAX_USES`, `MAX_PATHS`, `MAX_DEPTH`, `MAX_HOPS`: the same values as `dev`'s (`deny.rs`, `shell.rs`, `words.rs`, `gate.rs`).
- `MAX_WRAP = 8`.
- Walk entries: 4,096. Glob expansion depth: 12. The hard-link walk: 4,096 entries, depth 8, skipping `node_modules` and `.git`.
- A value for each filesystem cap.
- The compute budget. Set it as a function of `MAX_BYTES`, with a comment explaining how it covers linear lexing plus expansion and matching for every row of §10, and for the longest lines in the current adversarial suites (`crates/atrium-core/tests/adversarial*.rs`). The orchestrator will measure it.

To keep one source of truth while both evaluators exist, you may change `dev`'s private `const`s to `pub(super)`. **Change only their visibility, never a value.** Add a test asserting that each `limits` value equals `dev`'s.

### 3. `crates/atrium-core/src/policy/tools.rs`: the normative table (§6), as data
This is the contract's annex, approved like contract text. Write it as static data, `ToolSpec` rows. Each row has:
- the folded name, and the platforms (`Linux`, `MacOs`, both);
- the subcommands (for `git`, `cargo`, …), each with its own flags and operands;
- the flags: boolean or taking a value, short and long, and the role of a flag's value;
- the operand roles, by position or as "every remaining operand";
- the effects: writes, derived destinations, recursive, link following (root/below), runs code (inline code operand, or program file / project build);
- **the program-pinning condition:** which argv words a rule must name to cover Unread (program) (§7: `["python","build.py"]`, `["cargo","test"]`);
- whether the tool is a **code runner with no pinned program**, which makes an Allow prefix that ends there Invalid (§7);
- **Home read-only:** yes or no, with its forbidden flags, mirroring `atrium_protocol::policy::HOME_READONLY_COMMANDS` exactly;
- the filter dialect, for search tools;
- `changes_default: Option<&'static str>`, naming the §11 item when the row changes `dev`'s behaviour. For example, item 9 for `grep`'s pattern operand, item 4 for `awk`, `python`, `pipx run` and the bare `git` prefix, and item 10 for Git wildcard pathspecs.

**Coverage.** The table must account for every tool `dev` knows:
- `HOME_READONLY_COMMANDS`;
- the `rg`/`grep`/`ag`/`ack` flag tables (`engine.rs` `RG_*`/`GREP_*`, `gate.rs` `AG_OPTS`/`ACK_OPTS`);
- `words.rs` `WRITERS`/`READERS`;
- `shell.rs` `SHELLS`, `COMMAND_RUNNERS`, the wrappers and `RESERVED`;
- the git credential guards (`engine.rs` `GIT_CRED_*`);
- `deny.rs` `DOWNLOADERS`/`PRIV`.

Add the wrappers from §4 in exactly their listed forms, `sh`/`bash`/`dash -c`, `ln`, `cp`, `mv`, `rsync`, `tar`, `unzip`, `cd`, `find`, and the git subcommands that run programs from §6.

A tool `dev` knows that the model leaves out goes in an explicit `OUTSIDE_MODEL` list, with the reason, and makes its unit Unread. Copy `dev`'s flag tables faithfully; don't invent flags. Where `dev` has no data for a form, leave the form out, which makes it Unread.

Write accessor functions (`lookup(folded_name)`, `subcommand(...)`, `flag(...)`) as plain table reads. Parsing argv against the table is P3's job, not yours.

**Integrity tests** (in `tools.rs` or a test file):
- no duplicate names, subcommands or flags;
- each value-taking flag has a role;
- each code-running row names its code operand or its pinning condition;
- the Home read-only rows equal `HOME_READONLY_COMMANDS`;
- every name in `dev`'s lists above is either in the table or in `OUTSIDE_MODEL`;
- each `changes_default` names an item 1–14.

**A readable copy for review:** a test renders the table as Markdown and compares it to `docs/contracts/m1-policy-rev4-tools.md`. When `UPDATE_TOOL_TABLE=1` is set, the test rewrites that file instead. Write the file by hand as well as you can. The orchestrator will regenerate it, and Pedro and rodrigo review that file.

### 4. `crates/atrium-core/src/policy/v4/`: the frozen function-level ownership (§13)
- **`mod.rs`,** with `#![allow(dead_code)]` and a doc comment saying it is not wired until P5.
- **Stub files with `todo!()` bodies,** whose signatures use only `model.rs` types:
  - `lex.rs` and `units.rs`: P1;
  - `paths.rs`: P2, filesystem access only through a resolver trait that `SessionGate`'s resolver will implement; define the trait here;
  - `tools.rs`: P3, applying the table to a unit to produce `ToolEffects`;
  - `rules.rs`: P3, rule coverage of a unit's outcome;
  - `evaluate.rs`: P5, §1's decision procedure.
- **Doc comments:** each function's says which task owns it and which contract section it implements.
- **The shared data:** where P1–P3 need the same data (the protected set, `Sensitive`, CR-19's folds), make the signatures take it as a parameter rather than leaving each task to import its own.

Add `mod model; mod limits; mod tools; mod v4;` to `policy/mod.rs`. Change nothing else in `mod.rs`.

## Owned paths
- New: `crates/atrium-core/src/policy/model.rs`, `limits.rs`, `tools.rs`, `budget.rs` (if you split it), `policy/v4/*`, `docs/contracts/m1-policy-rev4-tools.md`.
- `crates/atrium-core/src/policy/mod.rs`: the module lines only.
- `deny.rs`, `shell.rs`, `words.rs`, `gate.rs`, `engine.rs`: visibility of constants and tables only (`const` → `pub(super) const`), no other edit.
- The handoff `docs/agents/handoffs/M1-36-rev4-p0-interface.md`.

## Rules
- Follow the worker rules in `docs/agents/README.md`. Under D-123, run only `cargo fmt --all` and `cargo check -p atrium-core --tests`. No `cargo test`, `cargo clippy` or `pnpm`.
- **No behaviour change.** No existing test's expectation changes, and nothing in `dev`'s evaluator calls the new code.
- Never put test data in shell heredocs. Commit in small steps. Never push, merge or switch branches.
- **The handoff:**
  - list every choice the contract left open (type shapes, cap values, table rows you left out and why);
  - list every row marked `changes_default`;
  - list the exact commands the verification should run, including `UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools` to regenerate the Markdown.

## Definition of Done (the orchestrator's verification)
- [ ] The Markdown table is regenerated and committed by the orchestrator.
- [ ] fmt, workspace clippy and `cargo test --workspace --no-fail-fast` are clean. No existing test changed.
- [ ] Codex review: approve with no findings (D-120). The review checks the types against the contract, and the table against `dev`'s tables and §6.
- [ ] The tool table is approved by rodrigo (D-138). Its `changes_default` rows are listed for Pedro.
