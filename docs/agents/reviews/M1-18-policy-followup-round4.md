# Review: M1-18-policy-followup, fix round 3 (commit `8333f0b`, D-110)

Reviewer: Gemini 4 Argon (High), read-only (D-101, D-105, D-110).

Saved by the orchestrator from the reviewer's final message. The orchestrator checked that 6 of 29 `adversarial_m1_18` tests fail on round 2's code and that the branch merges cleanly with dev.
Authors: Claude Opus 5.5 (round 1), Gemini 4 Hydrogen (fix rounds 1, 2, and 3), the orchestrator (test rows).

## Verdict: `rework` (for the driver's D-110 merge decision: **0 `Allow` regressions vs `dev`**, **0 false hard denies on ordinary lines**, **0 new needless asks on ordinary lines**, and **no policy slowdown** on an idle machine; remaining **`same as dev`** edge cases documented with 1-line known-limit wordings below)

Fix round 3 (`8333f0b`) closes all 48 named evidence lines from the round-3 review across Findings 1–4:
- **Finding 1 (P1 & CR-12):** Unified `normalize_glob_variants` in [deny.rs:1543-1604](crates/atrium-core/src/policy/deny.rs#L1543-L1604) pops `wildcard/..` (`*/..`, `p*/..`, `[a-z]/..`, `?/..`) in both `glob_escape` and `glob_search_variants`; `expand_glob_candidates` in [deny.rs:1827-1853](crates/atrium-core/src/policy/deny.rs#L1827-L1853) chains `widen_pattern_dots` and `widen_pattern_slashes` in both orders (`[.][.][/].ssh/id_rsa`, `??[/].ssh/id_rsa`); brace overflow fails closed (`unbounded = true` -> `Ask` across Home, `Any`, and `FullAccess` without sampling); and `ask_search_glob_hit` in [deny.rs:675-702](crates/atrium-core/src/policy/deny.rs#L675-L702) catches non-recursive filename-filtered searches of `ASK_TREES` (`.zsh_sessions/*.session`).
- **Finding 2 (K1):** `unknown_words_could_be_dangerous` in [deny.rs:110-165](crates/atrium-core/src/policy/deny.rs#L110-L165) splits separate argument words after `-S`/`--split-string`, `-C`/`--chdir`, and `script`/`flock` `-c`/`--command`; checks `security`, `secret-tool`, and piped interpreters (`inv.piped && is_interpreter(tok)`); and `unwrap_reading_steps` in [shell.rs:2387-2524](crates/atrium-core/src/policy/shell.rs#L2387-L2524) records `unknown_invs` for every wrapper the parser unwraps (`nice`, `nohup`, `time`, `xargs`, `stdbuf`, `setsid`, `timeout`, `ionice`, `chrt`, `taskset`, `chroot`, `flock`, `script`, `exec`, `builtin`, `unbuffer`, `caffeinate`, `sandbox-exec`, `arch`, `env`, `command`).
- **Finding 3 (Performance):** `MAX_GLOB_VARIANTS = 256` caps `expand_glob_candidates`, `search_variants_from_cands`, `normalize_glob_variants`, and `check_escape_variant`; `Analysis` caches `glob_escape` and `glob_search`; and `class_may_match_dot` skips regex compilation for ordinary `[a-z]`/`[A-Z]`/`[0-9]` classes (`{1..64}{a,b,c,d}/[a-z]/$VAR/**/../**/../**/../**/../*.rs` evaluates in `< 30ms`).
- **Finding 4 (`git log -L` & `[,-0]`):** `git_args_name_credential` and `parse_git_line_range` in [engine.rs:600-643](crates/atrium-core/src/policy/engine.rs#L600-L643) parse `-L <spec>`, `-L<spec>`, `--line-range <spec>`, and `--line-range=<spec>`, restoring `Allow` across all 4 settings on `git log -L :main:src/lib.rs` while keeping `Ask` in Home when `<file>` names a credential (`git log -L :main:.env`). Direct test of `globset` 0.4 confirms `[,-0]` matches `'/'` even with `literal_separator(true)`, so keeping slash widening on `[,-0]` is right.

Per the rule that any input still reaching `Allow` on a sensitive or credential-store path goes first and is marked `blocker`, two remaining **`same as dev`** edge cases are listed below.

---

## Findings

### Blockers

#### 1. [blocker] P1 & CR-12: Relative search globs that climb out of a root with `..` and re-enter it (`../u/.ssh/id_rsa` from `~`, `../.aws/credentials` from `~/.aws`) or enter another root in `paths`, and `search_variants_from_cands` variant-cap overflows (`a.glob_search` unbounded while `a.glob_escape` is not), still reach `Allow` — **`same as dev`**
- **Locations:**
  - [deny.rs:648-654](crates/atrium-core/src/policy/deny.rs#L648-L654) (`ask_search_glob_hit`), [deny.rs:1400-1416](crates/atrium-core/src/policy/deny.rs#L1400-L1416) (`search_glob_hit`), [deny.rs:1874-1894](crates/atrium-core/src/policy/deny.rs#L1874-L1894) (`search_variants_from_cands`), [deny.rs:1968-1993](crates/atrium-core/src/policy/deny.rs#L1968-L1993) (`check_escape_variant`)
  - [engine.rs:918-935](crates/atrium-core/src/policy/engine.rs#L918-L935) (`DefaultPolicy::evaluate`)
- **Mechanism:**
  1. **Relative globs that climb out of a root with leading `..` and re-enter it (or enter another root in `paths`):** `search_glob_hit` and `ask_search_glob_hit` run `normalize_glob_variants` on the unjoined relative `glob` (`"../u/.ssh/id_rsa"` or `"../.aws/credentials"`), where the leading `".."` has no preceding segment to pop, so the normalized pattern still starts with `"../"` and fails to match `probe` (`"/h/u/.ssh/id_rsa"`), `rel` (`".ssh/id_rsa"`), or `name` (`"id_rsa"`). Meanwhile, `check_escape_variant` ([deny.rs:1969-1985](crates/atrium-core/src/policy/deny.rs#L1969-L1985)) joins `variant` to the root (`/h/u/../u/.ssh/id_rsa` or `/h/u/.aws/../.aws/credentials`), normalizes it to `/h/u/.ssh/id_rsa` or `/h/u/.aws/credentials`, sees that `start.starts_with(r)` is `true` (`inside == true`), and does not add `abs` to `out.patterns` (or when `paths` is `["~/proj", "~"]` and `glob` is `"../.ssh/id_rsa"`, `"/h/u/proj/../.ssh/id_rsa"` normalizes to `"/h/u/.ssh/id_rsa"`, which satisfies `start.starts_with("/h/u")` for the second root). As a result, neither `search_glob_hit` nor `glob_escape` checks `/h/u/.ssh/id_rsa`.
  2. **`a.glob_search` overflow not propagated to `glob_unbounded`:** When `cands.variants` has `<= 256` entries (e.g. 193 brace expansions) that stay inside `~` and contain `wildcard/..`, `search_variants_from_cands` ([deny.rs:1886](crates/atrium-core/src/policy/deny.rs#L1886)) hits `MAX_GLOB_VARIANTS = 256` across the loop and returns `(vec![], true)`, causing `search_glob_hit` ([deny.rs:1414](crates/atrium-core/src/policy/deny.rs#L1414)) to return `None`. However, in `check_escape_variant` ([deny.rs:1969-1985](crates/atrium-core/src/policy/deny.rs#L1969-L1985)), each single `j` normalizes to only 2 strings (`<= 256`) and stays inside `~` (`inside == true`), so `a.glob_escape` has `unbounded = false`. Because `DefaultPolicy::evaluate` ([engine.rs:920](crates/atrium-core/src/policy/engine.rs#L920)) only checks `esc.unbounded` and not `a.glob_search`'s `unbounded` flag, it allows the call under `Any` and `FullAccess` (`Ask` in Home).
- **Evidence** (`[Home, Any, FullAccess]`):
  - `ToolSubject::Paths { paths: ["~"], glob: Some("../u/.ssh/id_rsa"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("../u/.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("../../h/u/.ssh/id_rsa"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("../u/.aws/credentials"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("../u/.npmrc"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("../u/.cargo/credentials.toml"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("../u/.pypirc"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("../u/.zsh_sessions/*.session"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/.aws"], glob: Some("../.aws/credentials"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/.aws"], glob: Some("../.aws/credentials"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
  - `ToolSubject::Paths { paths: ["~/.cargo"], glob: Some("../.cargo/credentials.toml"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj", "~"], glob: Some("../.ssh/id_rsa"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("{{1..64}{a,b,c}/x*/../foo,.ssh/id_rsa}"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
- **Suggested known-limit wording (1 line):**
  > Search globs that climb out of a search root with `..` and re-enter a root on the subject list (`../u/.ssh/id_rsa` from `~`), or that overflow the 256 normalized-variant cap only after `wildcard/..` expansion inside a root, are not caught by static glob analysis.

---

#### 2. [blocker] K1: Bounded unknown `env` invocations (`5..=11` mixed unknown options, `<= 24` words) with a separate `-S` / `--split-string` argument (`-S 'sh -c' 'cat .ssh/id_rsa'`) slice from `m + 1` instead of `m + 2` in `expand_unknown_env_cmd`, reaching `Allow` under `Any` and `FullAccess` — **`same as dev`**
- **Locations:**
  - [shell.rs:3483-3537](crates/atrium-core/src/policy/shell.rs#L3483-L3537) (`expand_unknown_env_cmd`)
- **Mechanism:**
  - When `env` has 5 or more unknown options mixing 1-word and 2-word flags (so `choices > 4` and the active reading is not `mask = 0..16` or `u32::MAX`) and `a.len() <= MAX_UNKNOWN_STARTS` (24), `expand_unknown_env_cmd` handles `-S` / `--split-string`.
  - When `-S` or `--split-string` (or `-iS`) takes its string from the next word `a[m + 1]` ([shell.rs:3495-3497](crates/atrium-core/src/policy/shell.rs#L3495-L3497) and [shell.rs:3511-3513](crates/atrium-core/src/policy/shell.rs#L3511-L3513)), `split_vals` records `(m, v)` (the index of `-S`), and [shell.rs:3536](crates/atrium-core/src/policy/shell.rs#L3536) appends `&a[*m + 1..]` instead of `&a[*m + 2..]`.
  - On `-S 'sh -c' 'cat .ssh/id_rsa'`, `sw` is `["sh", "-c"]` and `&a[*m + 1..]` is `["sh -c", "cat .ssh/id_rsa"]`, producing `sp = ["sh", "-c", "sh -c", "cat .ssh/id_rsa"]`. `shell_c_codes` then extracts `sp[2]` (`"sh -c"`) as the `-c` script and ignores `sp[3]` (`"cat .ssh/id_rsa"`) as `$0`.
- **Evidence** (`[home, prefix, any, full]`):
  - `env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -S 'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 --split-string 'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -S 'sh -c' 'sudo id'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 --split-string 'sh -c' 'sudo id'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -S 'sh -c' 'rm -rf ~'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -iS 'sh -c' 'sudo id'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
- **Suggested known-limit wording (1 line):**
  > `env` commands with 5 to 11 mixed unknown options before a separate `-S 'sh -c' '<payload>'` argument always ask in Home and under prefix/exact rules, but do not extract `<payload>` under `Any` or `FullAccess`.

---

### Minor

#### 3. [minor] Performance: `escape_from_cands` + `ask_search_glob_hit` can still evaluate up to `256 × 128 = 32,768` normalized variants (`~0.5s` per `evaluate`) when 256 brace expansions combine with 7 `wildcard/..` pairs inside a root — **`regression vs dev` (latency on synthetic worst-case globs only)**
- **Locations:** [deny.rs:650](crates/atrium-core/src/policy/deny.rs#L650) (`ask_search_glob_hit`), [deny.rs:1924-1929](crates/atrium-core/src/policy/deny.rs#L1924-L1929) (`escape_from_cands`), [deny.rs:1969-1993](crates/atrium-core/src/policy/deny.rs#L1969-L1993) (`check_escape_variant`)
- **Evidence:** On `ToolSubject::Paths { paths: ["~/proj"], glob: Some("{1..64}{a,b,c,d}/x1*/../x2*/../x3*/../x4*/../x5*/../x6*/../x7*/../*.rs"), recursive: true }`, each of the 256 `cands.variants` produces `2^7 = 128 <= 256` normalized strings in `check_escape_variant` (and `ask_search_glob_hit` recomputes `glob_search_variants` instead of reading `a.glob_search`), taking `1.52s` across 3 `evaluate` calls (`~505ms` per call). The round-3 Finding 3 input (`{1..64}{a,b,c,d}/[a-z]/$VAR/**/../**/../**/../**/../*.rs`) is now fast (`< 30ms`).
- **Suggested known-limit wording (1 line):**
  > Search globs combining 256 brace expansions with 7 `wildcard/..` steps inside a root can take ~0.5s to evaluate.

---

## Check-by-Check Summary (Items 1–5)

1. **Round-3 Findings 1–4 (and the Round-2 blockers they extend):**
   - **Round-3 Finding 1 (P1 & CR-12):** All 18 evidence lines from Round-3 Finding 1 now pass (`Hard(credential-read)` for `.ssh`/`credentials` hits, `Ask / Allow / Allow` for CR-12 stores/trees, `Ask / Ask / Ask` for brace overflows). Remaining edge cases: relative globs that climb out of a root with `..` and re-enter a root (`../u/.ssh/id_rsa`), and `search_variants_from_cands` variant-cap overflow when `a.glob_escape` does not overflow (Finding 1 above, **`same as dev`**).
   - **Round-3 Finding 2 (K1):** All 10 evidence lines from Round-3 Finding 2 (plus neighbours across `time`, `xargs`, `stdbuf`, `setsid`, `timeout`, and all 9 interpreters) now pass (`Ask / Ask / Ask / Ask` on unbounded dangerous wrappers; `Hard(credential-read)` on bounded `nohup` `eval`). Remaining edge case: off-by-one slice (`&a[*m + 1..]` vs `&a[*m + 2..]`) for separate `-S`/`--split-string` arguments in `expand_unknown_env_cmd` when `choices > 4` and `a.len() <= 24` (Finding 2 above, **`same as dev`**).
   - **Round-3 Finding 3 (Glob variant cap & performance):** Closed for the round-3 worst-case glob (`< 30ms`, `["Ask", "Ask", "Ask"]`). Remaining synthetic case: `256` brace expansions × `128` `wildcard/..` normalizations inside a root (`~505ms` per evaluation, Finding 3 above).
   - **Round-3 Finding 4 (`git log -L` & `[,-0]`):** Closed. `git log -L :main:src/lib.rs`, `git log -L 1,20:src/lib.rs`, `git log --line-range=1,20:src/lib.rs`, and `git log --line-range :main:src/lib.rs` all return `Allow` across all 4 settings, while `git log -L :main:.env` and `git log -L 1,20:.env` return `Ask` in Home. `globset` 0.4's `GlobBuilder::new("[,-0]").literal_separator(true)` matches `'/'`, so keeping slash widening on `[,-0]` is justified.
2. **Fail-closed cost on ordinary lines:**
   - Tested 34 everyday commands (`env FOO=1 BAR=2 cargo test`, `env -i PATH=/usr/bin:/bin make`, `env -u RUSTFLAGS cargo check`, `cwd=proj|env -C sub cargo build`, `cwd=proj|env --chdir=sub make`, `cwd=proj|env -S 'cargo test --lib'`, `command -v rg`, `command -V git`, `command -p ls`, `command ls -la`, `command git status`, `nice`, `nohup`, `time`, `xargs`, `timeout`, git pathspecs `':!vendor'`, `':(exclude)target'`, `':/README.md'`, `':^Cargo.lock'`, `':(top,exclude)dist'`, `'::src/main.rs'`, `HEAD:src/main.rs`, `:0:src/lib.rs`, `-L 1,20:src/lib.rs`, `-L :main:src/lib.rs`, `--line-range=1,20:src/lib.rs`, `--line-range :main:src/lib.rs`, `':(glob)crates/**/*.rs'`, `':(icase)readme.md'`, `rg`/`grep`/`find`/`fd` with `'*.{rs,toml}'`, `'[A-Z]*.md'`, `'?.txt'`, `'!target/**'`, and `cd proj && ...` chains) plus 13 `ToolSubject::Paths` searches in `~/proj` and `~` (`proj/**/*.rs`, `proj/*.{rs,toml}`, `proj/[A-Z]*.md`, `proj/?.txt`, `proj/src/*.rs`, `*/../src/*.rs`, `[!.]*.rs`, `[0-9]*.sql`, `{src,tests}/**/*.rs`): **0 false hard denies** and **0 new asks on ordinary lines**.
3. **Performance (idle machine comparison vs Round 2):**
   - On an idle machine (once concurrent builds finished):
     - `cargo test -p atrium-core --test adversarial_fuzz`: **`13.83s`** standalone (`13.98s` during full `cargo test -p atrium-core --no-fail-fast`), compared to **`13.73s`–`13.76s`** in Round 2 (+0.07s / +0.5%).
     - `cargo test -p atrium-core --test policy`: **`3.82s`** standalone (`3.86s` during full `cargo test -p atrium-core --no-fail-fast`), compared to **`3.76s`–`3.78s`** in Round 2 (+0.04s / +1.0%).
   - **Conclusion:** Round 3 did **not** slow the policy; the worker's `16–23s` and `4.9–5.5s` measurements were caused by CPU contention from another worker's builds.
4. **D-100 with `glob_leaves` passed into `allow_covers`:**
   - Verified in [engine.rs:129-132](crates/atrium-core/src/policy/engine.rs#L129-L132) and [engine.rs:918-955](crates/atrium-core/src/policy/engine.rs#L918-L955):
     - Only an unscoped `RuleMatcher::Any` (`rule.path_scope.is_none()`) covers a call when `glob_leaves == true`.
     - Higher-scope `PathGlob` or `path_scope`-restricted `Any` `Allow` rules do not mask lower-scope `Deny` rules.
     - Thread/Home/Global `Deny` rules still beat unscoped `Any` `Allow` and `Preset::FullAccess`.
     - `hard_deny` hits (`../.ssh/id_rsa`) always win first (`Hard(credential-read)`), and `glob_unbounded` forces `Ask` under unscoped `Any` and `FullAccess` while still respecting `Deny` rules.
5. **Test & lint verification:**
   - `cargo clippy -p atrium-core --all-targets -- -D warnings` and `cargo test -p atrium-core --no-fail-fast` both pass cleanly with 0 warnings and 0 failures.

---

## Commands Run

- `cargo clippy -p atrium-core --all-targets -- -D warnings` -> exit `0` (0 warnings).
- `cargo test -p atrium-core --test adversarial_fuzz -- --quiet` (idle) -> exit `0` (`2 passed; 0 failed; 1 ignored; finished in 13.83s`).
- `cargo test -p atrium-core --test policy -- --quiet` (idle) -> exit `0` (`44 passed; 0 failed; 0 ignored; finished in 3.82s`).
- `cargo test -p atrium-core --no-fail-fast` -> exit `0` (all test suites passed):
  - `src/lib.rs`: `9 passed; 0 failed; 0 ignored` (`0.00s`)
  - `adversarial_fuzz`: `2 passed; 0 failed; 1 ignored` (`13.98s`)
  - `adversarial_gate`: `9 passed; 0 failed; 2 ignored` (`0.13s`)
  - `adversarial_ledger`: `14 passed; 0 failed; 0 ignored` (`0.18s`)
  - `adversarial_m1_18`: `29 passed; 0 failed; 0 ignored` (`2.16s`)
  - `adversarial_ordinary`: `16 passed; 0 failed; 0 ignored` (`0.11s`)
  - `adversarial_policy`: `16 passed; 0 failed; 3 ignored` (`0.10s`)
  - `adversarial_regression`: `57 passed; 0 failed; 0 ignored` (`1.74s`)
  - `attention`: `8 passed; 0 failed; 0 ignored` (`0.00s`)
  - `history`: `25 passed; 0 failed; 0 ignored` (`1.44s`)
  - `ledger`: `20 passed; 0 failed; 0 ignored` (`0.01s`)
  - `policy`: `44 passed; 0 failed; 0 ignored` (`3.86s`)
  - `store`: `30 passed; 0 failed; 0 ignored` (`37.26s`)
- Temporary probe test `crates/atrium-core/tests/probe_review_round4.rs` run and deleted; `git status --short` is clean (empty).
