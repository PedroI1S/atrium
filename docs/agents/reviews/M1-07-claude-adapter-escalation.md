# Review: M1-07-claude-adapter, escalation round (commits 36fb949, 5b97e6d)

Reviewer: Gemini 4 Hydrogen `high`, read-only (D-074, D-075). Author: Claude Opus 5.5. The review was returned as the agent's final message and saved here verbatim by the orchestrator, minus its first line (a progress note); the `file://` links point at the worker's worktree.

Reviewer: Gemini 4 Hydrogen (read-only) · 2026-11-14 · Commits reviewed: `git diff a2328ac..5b97e6d`

## Verdict
**approve with changes**: Every round-3 blocker, major, and minor finding is closed cleanly, no `Glob` or `Grep` pattern or base classified as understood (`ToolSubject::Paths`) reaches `GateDecision::Allow` on a credential file in `DefaultPolicy`, ordinary workspace globs stay `Allow` under `Preset::Home`, `Proc::drop` safely terminates zombie-led groups on its own runtime without deadlocking or panicking, and all tests and clippy pass. The only remaining item is a `[minor]` CI timing overrun on the exhaustive test (`41.20 s` standalone / `49.29 s` in the full test run vs. the brief's `< 30 s` debug target).

---

## Findings

1. [minor] **Exhaustive test `every_short_pattern_the_policy_allows_reaches_no_credential` exceeds the brief's 30 s debug-build budget on this host (`41.20 s` standalone, `49.29 s` in the full suite)**
   - Where: [claude_policy_e2e.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_policy_e2e.rs#L527-L639) and [briefs/M1-07-claude-adapter.md](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/docs/agents/briefs/M1-07-claude-adapter.md#L62) (`"Keep its run time under 30 seconds in a debug build"`).
   - Evidence:
     - Standalone: `cargo test -p atrium-agents --test claude_policy_e2e every_short_pattern_the_policy_allows_reaches_no_credential -- --nocapture` printed `exhaustive: 444440 patterns (411244 Unknown, 417 of them through the policy; 33196 known, 32727 allowed); subjects in 3.947571958s, all in 41.183757666s` (`41.20 s` total).
     - Full run: `claude_policy_e2e` finished in `49.29 s`.
     - Cause: For each of the 33,196 `known` inputs, lines 596–605 call `SessionGate::check` (`Preset::Home`, then `Preset::FullAccess`), and each `SessionGate::check` ([gate.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-core/src/policy/gate.rs#L178-L296)) spawns a blocking task that runs `expand_roots` (`fs::read_dir` + `metadata` on `.ssh`) and compiles `ShellGlob` regexes per `paths` entry—including calling `Home` on all `path: ".."` patterns where `Home::root_ok` always declines (`Ask`/`HardDeny`).
   - Suggestion: Either call `DefaultPolicy::evaluate` directly inside the exhaustive loop (after verifying `SessionGate` on a small subset, or keeping `SessionGate` only for `cwd` or `FullAccess`), or skip the redundant `Preset::Home` call when `path == ".."` (since `Home::root_ok` rejects `home` itself), or shorten length/alphabet if needed per the brief.

2. [nit] **Non-owned `atrium-core` P1 observation: single-component case folding in `search_glob_hit` and directory-only `Grep` in `FullAccess`**
   - Where: [deny.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-core/src/policy/deny.rs#L1061-L1088) (`Ctx::search_glob_hit`).
   - Evidence: `glob_target` ([tools.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/src/claude/tools.rs#L414-L420)) omits `join(glob)` when `!leaves && !glob.contains('/')`, delegating single-component patterns to `search_glob_hit`. Unlike `Sensitive::glob_hit` (which calls `lower(pattern)`), `search_glob_hit` compiles `glob` case-sensitively against lowercase probes. If a provider glob ever ran case-insensitively on macOS, `{"path":"..","pattern":".NETRC"}` would bypass `search_glob_hit` under `Preset::FullAccess`. This lives in `atrium-core` (outside M1-07's owned paths) and matches standard case-sensitive `globset`/`ripgrep` behavior.

---

## Detailed Check Results

### 1. Round-3 findings (verification from diff & tests)
- **Blocker 1 (interleaved `[` and `{...}`) — Fixed**:
  - Without fix (`a2328ac`): `glob_targets` ran `classes_are_plain` before `expand_braces`, letting `[{a],.}][{a],.}]/.ssh/id_ed25519` expand into `[.][.]/.ssh/id_ed25519` and reach `Allow`.
  - With fix (`5b97e6d`): `in_grammar` ([tools.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/src/claude/tools.rs#L256-L279)) validates raw braces via `brace_body` (`L283-L296`, only ASCII alphanumeric and `_-.,`) and raw classes via `class_body` (`L300-L333`, only ASCII alphanumeric, `_`, and single-run ranges) before `expand_braces`, so `[` and `{` cannot interleave.
  - Tests (fail on `a2328ac`, pass on `5b97e6d`): `patterns_we_cannot_bound_are_unknown_and_never_allowed` ([claude_policy_e2e.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_policy_e2e.rs#L236-L267)) and `r3_globs_outside_the_grammar_are_unknown` ([claude_review.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_review.rs#L1066-L1095)).
- **Blocker 2 (`~user`, `~*`, `~?...`) — Fixed**:
  - Without fix (`a2328ac`): `search_subject` accepted any non-empty `path` and `glob_target` treated any leading `~` as home-relative, letting `expand_home` leave `~bob/*` / `~a*/...` unexpanded under `cwd` and reach `Allow`.
  - With fix (`5b97e6d`): `tilde_ok` ([tools.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/src/claude/tools.rs#L238-L240)) allows only strings not starting with `~`, `"~"`, or `"~/..."`, enforced on `base` in `search_subject` (`L182`) and on `glob` in `in_grammar` (`L257`).
  - Tests (fail on `a2328ac`, pass on `5b97e6d`): `patterns_we_cannot_bound_are_unknown_and_never_allowed` (`L241-L247`) and `r3_globs_outside_the_grammar_are_unknown` (`L1015-L1021`, `L1083-L1094`).
- **Major 3 (multi-component relative credential globs inside `base`) — Fixed**:
  - Without fix (`a2328ac`): `glob_target` returned `Some(vec![])` whenever `!leaves`, omitting `join(glob)` for patterns like `..` + `.ssh/id_ed25519` or `home` + `.aws/cred*`.
  - With fix (`5b97e6d`): `glob_target` ([tools.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/src/claude/tools.rs#L414-L421)) returns `pattern_forms(&join(glob))` whenever `glob.contains('/')`, even when `!leaves`.
  - Tests (fail on `a2328ac`, pass on `5b97e6d`): `r3_credential_globs_that_stay_inside_their_base_are_hard_denied` ([claude_policy_e2e.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_policy_e2e.rs#L297-L337)) and `r3_globs_inside_the_grammar_name_every_expansion_with_a_slash` ([claude_review.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_review.rs#L1100-L1171)).
- **Minor 4 (`*.[a-z]`/`file-[0-9].rs` broken; `[}-\u{7f}]` accepted) — Fixed**:
  - Without fix (`a2328ac`): `classes_are_plain` overwrote `prev = Some('-')` on range entry and only checked `.`/`/`, breaking `[a-z]`/`[0-9]` and missing `~` ranges.
  - With fix (`5b97e6d`): `class_body` ([tools.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/src/claude/tools.rs#L300-L333)) validates `lo <= hi` within one ASCII run (`is_ascii_lowercase`, `is_ascii_uppercase`, or `is_ascii_digit`) and consumes `c-hi` as 3 chars (`k += 3`).
  - Tests (fail on `a2328ac`, pass on `5b97e6d`): `ordinary_patterns_keep_their_plain_subject_and_are_allowed_at_home` ([claude_policy_e2e.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_policy_e2e.rs#L342-L376)), `patterns_we_cannot_bound_are_unknown_and_never_allowed` (`L249`), `r3_globs_outside_the_grammar_are_unknown` (`L1053-L1061`), and `r3_globs_inside_the_grammar_name_every_expansion_with_a_slash`.
- **Minor 5 (`Proc::drop` outside a runtime with zombie leader) — Fixed**:
  - Without fix (`a2328ac`): `Proc::drop` called `supervise::kill_recorded`, which returned `Unverified` without signalling when `identify` returned `Ok(None)` for a zombie leader, then reaped the leader via `try_wait()`.
  - With fix (`5b97e6d`): `Proc::drop` / `terminate_now` ([adapter.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/src/claude/adapter.rs#L432-L460)) runs `supervise::terminate(&mut child, Duration::ZERO)` on a dedicated `current_thread` Tokio runtime (inline outside a runtime, on `std::thread::spawn` inside one), signalling the group before reaping. No `atrium-host` edits were needed.
  - Test (fails on `a2328ac`, passes on `5b97e6d`): `r3_a_start_dropped_outside_a_runtime_after_the_leader_exited_kills_the_group` ([claude_adapter.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_adapter.rs#L1566-L1614)) using `leader-exits` in [claude-fake.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/examples/claude-fake.rs#L82-L86).
- **Minor 6 (oversize `can_use_tool` outside turn / split by `hold`) — Fixed**:
  - Without fix (`a2328ac`): `can_use_tool` called `self.oversize_call` without `establish` and `ensure_turn`.
  - With fix (`5b97e6d`): [normalizer.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/src/claude/normalizer.rs#L934-L936) calls `self.establish(out); self.ensure_turn(out);` immediately before `self.oversize_call`.
  - Tests (fail on `a2328ac`, pass on `5b97e6d`): `r3_an_oversize_can_use_tool_between_turns_is_shown_inside_a_turn` and `r3_an_oversize_can_use_tool_before_init_is_never_split_by_what_waits` ([claude_review.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_review.rs#L950-L1003)).

### 2. Grammar (`in_grammar`) & `pattern_forms` audit
- Checked character classes, negated classes (`[!a]`, `[^a]`, `[[:alpha:]]`), backslashes, braces with nested/empty/dot alternatives, repeated slashes (`~//*`, `*//*a`), dot segments (`./x/./y/`, `..` after wildcard), `**` in odd places (`*sh/**`, `**/.ssh/*`, `a**b`), and caps (`MAX_EXPANSIONS = 32`, `MAX_TARGETS = 64`, `MAX_WILD_COMPONENTS = 4`):
  - `in_grammar` enforces non-overlapping `{...}` and `[...]` on the raw pattern; `brace_body` blocks `/`, `~`, `[`, `]`, `{`, `}`, `*`, `?`, empty, `.`, and `..` alternatives; `class_body` blocks `.`, `/`, `~`, `!`, `^`, `:`, `*`, `?`, `{`, `}`, `[`, `]`, reverse ranges, and cross-run ranges.
  - `pattern_forms` ([tools.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/src/claude/tools.rs#L455-L489)) drops empty and `.` components (matching `globset`/tool path normalization so `ShellGlob`'s split on `/` cannot mismatch `//` or `./`) and enumerates leading-`.` variants for up to 4 wildcard-led components (matching `ShellGlob`'s leading-dot rule); when `joined.contains("**")`, `ShellGlob::new` ([deny.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-core/src/policy/deny.rs#L486-L491)) delegates directly to `globset::GlobBuilder::new(pattern).literal_separator(true)` with no dot rule, matching `pattern_forms`'s `wild = vec![]` branch.
  - `expand_braces` checks `all.len() > MAX_EXPANSIONS` after each branch and avoids exponential blowup because `in_grammar` forbids nested `{...}`; `glob_targets` caps total `paths` additions at `MAX_TARGETS = 64` (well below `words::MAX_USES = 4096`).

### 3. Ordinary patterns in `Preset::Home`
- `**/*.rs`, `src/**/mod.rs`, `*.{ts,tsx}`, `src/{a,b}/mod.rs`, `*.[ch]`, `*.[a-z]`, `file-[0-9].rs`, `*.{d.ts,ts}`, and `[A-Z_]*.md` all produce `ToolSubject::Paths` and `GateDecision::Allow` in `Preset::Home` with base `cwd` for both `Glob` and `Grep` ([ordinary_patterns_keep_their_plain_subject_and_are_allowed_at_home](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_policy_e2e.rs#L342-L376)).

### 4. Exhaustive test oracle soundness
- `oracle_expand`, `normalize`, and `reached` ([claude_policy_e2e.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_policy_e2e.rs#L382-L497)) accurately model `globset` with `literal_separator(true)` against all 10 `CREDENTIALS` probes (matching joined normalized paths and, when `!e.contains('/')`, file names under `base`).
- Sampling 1 in 997 `ToolSubject::Unknown` inputs (`417` checked) is sound because `DefaultPolicy::evaluate` ([engine.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-core/src/policy/engine.rs#L631-L633)) unconditionally returns `PolicyVerdict::Ask` (never `Allow`) whenever `subject` is `ToolSubject::Unknown`.

### 5. `Proc::drop` runtime safety
- When dropped inside a Tokio runtime (`Handle::try_current().is_ok()`), `Proc::drop` moves `SupervisedChild` to a fresh `std::thread::spawn`, so `Builder::new_current_thread().enable_all().build()` never panics from nested runtime creation, never blocks a runtime worker thread, and does not depend on the caller's runtime staying alive during shutdown. When dropped outside a runtime, `terminate_now` runs inline (blocking the dropping thread up to `GROUP_DRAIN_TIMEOUT = 2 s` only if descendants ignore `SIGKILL`).

### 6. Changed expectations in existing tests
- All 5 updates are justified by the Escalation brief:
  - [claude_normalizer.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_normalizer.rs#L1475-L1482): `**/*.rs` now includes joined `"**/*.rs"` in `paths` (brief item 1).
  - [claude_review.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_review.rs#L521-L547): `~*` is `Unknown` (blocker 2); `**/*.{rs,toml}` includes `"src/**/*.rs"` and `"src/**/*.toml"` (brief item 1 / major 3).
  - [claude_review.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_review.rs#L611-L619): `../../x/**` drops the leading `"."` component via `pattern_forms` (item 1b).
  - [claude_review.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_review.rs#L671-L680): renamed `f2_braces_that_hide_a_root_are_unknown` since `/` and `~` inside `{...}` are now outside `in_grammar` (brief item 1).
  - [claude_policy_e2e.rs](file:///Users/pedro/atrium-wt/M1-07-claude-adapter/crates/atrium-agents/tests/claude_policy_e2e.rs#L197-L250): moved 7 round-2 brace patterns (empty, `.`, `..`, nested, `/`, `~` in `{...}`) from `ESCAPES` to `NOT_UNDERSTOOD` (`Unknown`, never `Allow`) per the brief's new brace grammar.

---

## Commands Run
- `cargo build -p atrium-agents --examples` — exit 0
- `cargo clippy -p atrium-agents --all-targets -- -D warnings` — exit 0, no warnings
- `cargo fmt --all --check && git diff --check a2328ac..5b97e6d` — exit 0
- `cargo test -p atrium-agents --no-fail-fast` — exit 0:
  - unit tests (`src/lib.rs`): 25 passed, 0 failed
  - `claude_adapter`: 38 passed, 0 failed
  - `claude_golden`: 20 passed, 0 failed (verified `10.27 s` standalone)
  - `claude_normalizer`: 60 passed, 0 failed (`1.34 s`)
  - `claude_policy_e2e`: 5 passed, 0 failed (`49.29 s`)
  - `claude_review`: 28 passed, 0 failed (`4.04 s`)
  - `consistency`: 11 passed, 0 failed, 1 ignored (`consistency_all_fixtures`)
- `cargo test -p atrium-agents --test claude_policy_e2e every_short_pattern_the_policy_allows_reaches_no_credential -- --nocapture` — exit 0 (`41.20 s`, `444440 patterns (411244 Unknown, 417 of them through the policy; 33196 known, 32727 allowed); subjects in 3.947571958s, all in 41.183757666s`)
- `git status --short` — clean (no files modified or created)

