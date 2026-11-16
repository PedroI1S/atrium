# Review: M1-18-policy-followup, fix round 4 (commit `a702441`, D-114)

> Saved by the orchestrator from the reviewer's final message (D-114). Links are relative to the worktree.


Reviewer: Gemini 4 Argon (High), read-only (D-101, D-105, D-110, D-114).

## Verdict: `rework` (for the driver's merge decision: **0 `Allow` regressions vs `dev`**, **0 false hard denies on ordinary lines**, **0 needless asks on ordinary globs/commands**, and **fast evaluation (`< 10ms` per worst-case glob)**; two remaining **`same as dev`** `Any`/`FullAccess` edge cases documented with 1-line known-limit wordings below)

Fix round 4 (`a702441`) closes all 20 named evidence lines from the round-4 review (`8333f0b`):
- **Round-4 Finding 1a (relative `..` re-entry globs):** [search_variants_from_cands](crates/atrium-core/src/policy/deny.rs#L1909-L1967) now takes resolved `roots` from [analyze_with](crates/atrium-core/src/policy/deny.rs#L275-L288) and normalizes both the unjoined pattern and each root-joined `format!("{root}/{gv}")`. All 12 re-entry lines from Round-4 Finding 1a—plus multi-root (`["~/proj", "~/other", "~"]`), `./..`, `..//`, `~/..`, dot/slash classes (`[.][.]/u/.ssh/id_rsa`, `??/u/.ssh/id_rsa`, `[.][.][/]u[/].ssh/id_rsa`), braces (`{..,a}/u/.ssh/id_rsa`), and `wildcard/..` (`*/../../u/.ssh/id_rsa`)—now match their plain-path verdicts (`Hard(credential-read)` for hard-denied paths across all presets; `Ask / Allow / Allow` for CR-12 stores/trees).
- **Round-4 Finding 1b (`glob_search` overflow propagation):** [DefaultPolicy::evaluate](crates/atrium-core/src/policy/engine.rs#L921-L922) checks `a.glob_search.as_ref().is_some_and(|(_, u)| *u)` in `glob_unbounded`, failing closed to `Ask` across `Home`, `Any`, and `FullAccess` whenever `search_variants_from_cands` hits `MAX_GLOB_VARIANTS = 256`.
- **Round-4 Finding 2 (`env -S` separate argument tail offset):** [expand_unknown_env_cmd](crates/atrium-core/src/policy/shell.rs#L3494-L3536) stores `(tail_start, val)` (`m + 2` when `-S`, `--split-string`, `-iS`, or `-vS` takes `a[m + 1]`, and `m + 1` when attached) and slices `&a[*tail..]`. All 6 evidence lines from Round-4 Finding 2—plus 1 to 11 unknown options with `-S`, `--split-string`, `--spl`, `-iS`, `-vS` (attached and separate) and single-word `-S sh -c` / `-S sudo` / `-S eval`—now return `HardDeny` across all 4 settings.
- **Round-4 Finding 3 (worst-case glob performance):** [escape_from_cands](crates/atrium-core/src/policy/deny.rs#L1979-L2001) and [check_escape_variant](crates/atrium-core/src/policy/deny.rs#L2018-L2078) bound cumulative normalized variants (`total_norm`) across all candidates at `MAX_GLOB_VARIANTS = 256`, and [Sensitive::ask_search_glob_hit](crates/atrium-core/src/policy/deny.rs#L657-L730) reuses cached `self.a.glob_search`. The `256 × 128` glob from Round-4 Finding 3 now evaluates in `~6–10ms` across all 3 presets (down from `1.52s`) and returns `["Ask", "Ask", "Ask"]`.

Per the rule that any input reaching `Allow` on a sensitive path goes first and is marked `blocker`, two remaining **`same as dev`** (`Any`/`FullAccess`-only; `Home` returns `Ask`) edge cases are listed below.

---

## Findings

### Blockers

#### 1. [blocker] P1: Search globs where `**` and `..` are separated by `.`, `//`, `literal/..`, or `wildcard/..` (`**/./../.ssh/id_rsa`, `**//../.ssh/id_rsa`, `**/sub/../../.ssh/id_rsa`) do not collapse `**` with `..`, reaching `Allow` under `Any` and `FullAccess` (`Ask` in `Home`) — **`same as dev` on `Any`/`FullAccess` (stricter than `dev` in `Home` when `recursive: false` or root is `~/proj`)**
- **Locations:**
  - [deny.rs:1578-1639](crates/atrium-core/src/policy/deny.rs#L1578-L1639) (`normalize_glob_variants`), [deny.rs:1782-1807](crates/atrium-core/src/policy/deny.rs#L1782-L1807) (`globstar_variants`), [deny.rs:1889-1904](crates/atrium-core/src/policy/deny.rs#L1889-L1904) (`expand_glob_candidates`)
- **Mechanism:**
  - `globstar_variants` ([deny.rs:1782-1807](crates/atrium-core/src/policy/deny.rs#L1782-L1807)) runs before `normalize_glob_variants` and only collapses adjacent `w[0] == "**" && w[1] == ".."` windows on the raw `/` split.
  - When `"."`, `""` (`//`), `"sub/.."`, or `"x*/.."` sits between `"**"` and `".."` (e.g. `"**/./../.ssh/id_rsa"` from `~`, or `"**/./../../.ssh/id_rsa"` from `~/proj`), `globstar_variants` sees no `["**", ".."]` window.
  - Later, `normalize_glob_variants` ([deny.rs:1594-1597](crates/atrium-core/src/policy/deny.rs#L1594-L1597)) strips `"."` / `""` and pops `"sub/.."`, bringing `".."` directly after `"**"`, where `Some("..") | Some("**")` pushes `".."` onto `st` without branching to pop `".."` against `"**"`.
  - Meanwhile, `drop_globstar` removes `"**"` (matching 0 directories), so the remaining `".."` climbs out of `/h/u` to `/h/.ssh/id_rsa`. That sets `glob_leaves = true` (so `Home` always returns `Ask` and CR-12 stores get their required `Ask / Allow / Allow`), but because `/h/u/.ssh/id_rsa` is never produced, `hard_deny` is not triggered and `Any` / `FullAccess` return `Allow`.
- **Evidence** (`[Home, Any, FullAccess]`):
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**/./../.ssh/id_rsa"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **stricter in Home, `same as dev` on Any/FullAccess**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**/./../.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**//../.ssh/id_rsa"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **stricter in Home, `same as dev` on Any/FullAccess**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**/sub/../../.ssh/id_rsa"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **stricter in Home, `same as dev` on Any/FullAccess**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**/x*/../../.ssh/id_rsa"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **stricter in Home, `same as dev` on Any/FullAccess**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**/./../.aws/credentials"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **stricter in Home, `same as dev` on Any/FullAccess**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**/./../.npmrc"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **stricter in Home, `same as dev` on Any/FullAccess**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("**/./../../.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **stricter in Home, `same as dev` on Any/FullAccess**)
- **Suggested known-limit wording (1 line):**
  > Search globs where `**` and `..` only meet after collapsing `.`, `//`, or `seg/..` (`**/./../.ssh/id_rsa`) always ask in Home and under scoped/prefix rules, but are not statically resolved to hard-deny under `Any` or `FullAccess`.

---

#### 2. [blocker] K1: Bounded unknown `env` invocations (`5..=11` mixed unknown options, `<= 24` words) combining `-C <dir>` / `--chdir <dir>` with `-S 'sh -c' '<payload>'` omit `&a[*tail..]` when pairing `chdir_vals` with `split_vals`, reaching `Allow` under `Any` and `FullAccess` — **`same as dev`**
- **Locations:**
  - [shell.rs:3666-3672](crates/atrium-core/src/policy/shell.rs#L3666-L3672) (`expand_unknown_env_cmd`)
- **Mechanism:**
  - In [shell.rs:3527-3536](crates/atrium-core/src/policy/shell.rs#L3527-L3536), fix round 4 updated `for (tail, s) in &split_vals` to append `&a[*tail..]` to `sp` for `readings`.
  - However, at the bottom of `expand_unknown_env_cmd` ([shell.rs:3666-3672](crates/atrium-core/src/policy/shell.rs#L3666-L3672)), the loop pairing `chdir_vals` with `split_vals` still writes `for (_, s) in &split_vals { push_bounded_str(p, format!("cd {} && {s}", quote(d)), unbounded); }` without appending the quoted tail words `&a[*tail..]`.
  - When `env` has 5 to 11 mixed unknown options, `-C ..`, and `-S 'sh -c' 'cat .ssh/id_rsa'` from `cwd=proj`, the `readings` path extracts `cat .ssh/id_rsa` without `cd ..` (resolving against `/h/u/proj`), while the `payloads` path emits `cd '..' && sh -c` without `'cat .ssh/id_rsa'`.
- **Evidence** (`[home, prefix, any, full]`):
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -C .. -S 'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 --chdir .. --split-string 'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 --chdir=.. -S 'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -C.. -S 'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -C .. -iS 'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -C .. -vS 'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -C .. -S'sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 -C .. --split-string='sh -c' 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
- **Suggested known-limit wording (1 line):**
  > `env` commands with 5 to 11 mixed unknown options that combine `-C <dir>` with `-S 'sh -c' '<payload>'` as separate words always ask in Home and under prefix/exact rules, but do not pair `<dir>` with `<payload>` under `Any` or `FullAccess`.

---

### Nit

#### 3. [nit] `search_variants_from_cands` counts both unjoined `gv` and each root-joined `{root}/{gv}` against the same `MAX_GLOB_VARIANTS = 256` budget — **`regression vs dev` (on `>= 65` brace expansions with `**`, or `>= 129` without `**`, which now `Ask` everywhere)**
- **Locations:** [deny.rs:1920-1959](crates/atrium-core/src/policy/deny.rs#L1920-L1959) (`search_variants_from_cands`)
- **Mechanism & Evidence:**
  - For an unanchored glob with `R` search roots, each candidate in `cands.variants` calls `push_norms` `1 + R` times (once unjoined, `R` times root-joined), and `**` doubles `cands.variants` via `drop_globstar`.
  - Consequently, with 1 root, an unanchored glob without `**` hits `total_norm > 256` at **129** brace expansions (`{{1..64}{a,b},extra}.rs` -> `["Ask", "Ask", "Ask"]`, vs `128` in `{1..64}{a,b}.rs` -> `["Allow", "Allow", "Allow"]`), and an unanchored glob with `**` hits `total_norm > 256` at **65** brace expansions (`{{1..32}{a,b},extra}/**/*.rs` -> `["Ask", "Ask", "Ask"]`, vs `64` in `{1..32}{a,b}/**/*.rs` -> `["Allow", "Allow", "Allow"]`).
  - Ordinary brace-and-class globs (e.g. `*.{rs,toml,md,json,yaml,yml,lock}`, `[!0-9]*.{rs,toml,md,json,yaml,yml,lock}`, `?.[a-z]*.{rs,toml,md,json,yaml,yml,lock}`, `**/[!.]*.{rs,toml,md,json,yaml,yml,lock}`, `{src,tests,crates,benches,examples,bins}/**/[a-z_]*.{rs,toml,md,json,yaml,yml,lock}`) generate `14` to `168` total norms (`<= 256`) and remain `["Allow", "Allow", "Allow"]`.
- **Suggested known-limit wording (1 line):**
  > Unanchored search globs whose brace, `**`, and root-joined combinations exceed 256 normalized variants (>128 brace branches without `**`, or >64 with `**` on one root) fail closed to `Ask`.

---

## Check-by-Check Summary (Items 1–4)

1. **Round-4 Findings 1–3 and new spellings in the same families:**
   - **Finding 1a (relative `..` re-entry globs):** Closed for all direct, multi-root, `./..`, `//`, `~/..`, class-widened (`[.][.]`, `.[.]`, `??`, `[/]`), braced, and `wildcard/..` re-entry globs (`recursive: true` and `false`). Remaining edge case: `**` separated from `..` by `.`, `//`, or `seg/..` (`**/./../.ssh/id_rsa`), which returns `Ask` in Home and `Allow` in `Any`/`FullAccess` (Finding 1 above, **`same as dev`** on `Any`/`FullAccess`).
   - **Finding 1b (variant cap reached in each code path):** Tested all 6 cap paths (`brace_expand` overflow, `expand_glob_candidates` cap, `search_variants_from_cands` `total_norm` cap, `search_variants_from_cands` `out.len()` cap, `check_escape_variant` `total_norm` cap, and `check_escape_variant` `out.patterns.len()` cap). All 6 propagate `unbounded = true` to `glob_unbounded` in [DefaultPolicy::evaluate](crates/atrium-core/src/policy/engine.rs#L921-L922) and return `["Ask", "Ask", "Ask"]`.
   - **Finding 2 (`env -S` separate argument):** Closed across 1 to 11 unknown options for `-S`, `--split-string`, `--spl`, `-iS`, `-vS` (attached and separate) and single-word `-S` payloads. Remaining edge case: `5..=11` mixed unknown options combining `-C <dir>` with `-S 'sh -c' '<payload>'` (Finding 2 above, **`same as dev`**).
   - **Finding 3 (performance on `256 × 128` worst-case glob):** Closed. `fix3_finding3_glob_variant_cap_performance` and `fix4_finding3_glob_escape_total_variant_cap_performance` both complete in `0.03s` combined standalone.
2. **New regressions from Round 4:**
   - **Root-joined variants on ordinary searches:** Tested `../proj/*.rs` from `~/proj` (`rec=false/true` -> `Allow` x3), `../proj/src/**/*.rs` from `~/proj` (`Allow` x3), `../sibling/**/*.rs` from `~/proj` (`Ask / Allow / Allow` per D-100), `proj/*.rs` and `proj/**/*.rs` and `../u/proj/*.rs` from `~` (`rec=false` -> `Allow` x3). **0 false hard denies** and **0 new asks on ordinary searches**.
   - **Total variant cap on ordinary brace-and-class globs:** Tested `*.{rs,toml,md,json,yaml,yml,lock}`, `**/*.{rs,toml,md,json,yaml,yml,lock}`, `[a-zA-Z0-9_]*.{rs,toml,md,json,yaml,yml,lock}`, `[!0-9]*.{rs,toml,md,json,yaml,yml,lock}`, `?.[a-z]*.{rs,toml,md,json,yaml,yml,lock}`, `**/[!.]*.{rs,toml,md,json,yaml,yml,lock}`, and `{src,tests,crates,benches,examples,bins}/**/[a-z_]*.{rs,toml,md,json,yaml,yml,lock}`: all return `["Allow", "Allow", "Allow"]` (`widen_pattern_dots` widens all `?`/dot-classes in a single pass, so multiple classes do not multiply combinatorially). Only globs exceeding `64` brace branches with `**` (or `128` without `**`) on 1 root hit the `256` cap (Nit 3 above).
   - **`Analysis::glob_search` made `pub`:** `mod deny` is private to `crates/atrium-core/src/policy/mod.rs` ([mod.rs:16](crates/atrium-core/src/policy/mod.rs#L16)), `Analysis` is not re-exported, and all 13 other fields on `Analysis` were already `pub`. No public API or contract impact.
3. **Performance & `< 150 ms` test threshold:**
   - Standalone timings on an idle machine:
     - `cargo test -p atrium-core --test adversarial_fuzz`: **`13.75s`** (`13.88s` in full `cargo test -p atrium-core --no-fail-fast`; Round 3 was `13.83s`, Round 2 was `13.75s`).
     - `cargo test -p atrium-core --test policy`: **`3.81s`** (`3.80s` in full `cargo test -p atrium-core --no-fail-fast`; Round 3 was `3.82s`, Round 2 was `3.78s`).
     - `fix3_finding3` + `fix4_finding3` worst-case globs: **`0.03s`** combined (`~15ms` per test across 3 `evaluate` calls).
   - **Is relaxing the test assertion from `< 50 ms` to `< 150 ms` across 3 calls (`< 50 ms` per `evaluate`) reasonable?** Yes: each test calls `evaluate` 3 times (`Home`, `Any`, `FullAccess`), and during `cargo test --workspace` all test binaries and `store.rs` run concurrently across threads. `< 150 ms` for 3 evaluations (`< 50 ms` per call) avoids scheduler flakes under parallel load while still catching any `O(N^2)` blowup (which took `1,520 ms` in Round 3).
4. **Tests & Clippy:**
   - `cargo clippy -p atrium-core --all-targets -- -D warnings` and `cargo test -p atrium-core --no-fail-fast` both pass with 0 warnings and 0 failures.

---

## Commands Run

- `git diff 8333f0b..HEAD` -> inspected all changes in `deny.rs`, `engine.rs`, `shell.rs`, `adversarial_m1_18.rs`, and `docs/agents/handoffs/M1-18-policy-followup.md`.
- `cargo clippy -p atrium-core --all-targets -- -D warnings` -> exit `0` (0 warnings).
- `cargo test -p atrium-core --no-fail-fast` -> exit `0` (all test suites passed):
  - `src/lib.rs`: `9 passed; 0 failed; 0 ignored` (`0.00s`)
  - `adversarial_fuzz`: `2 passed; 0 failed; 1 ignored` (`13.88s`)
  - `adversarial_gate`: `9 passed; 0 failed; 2 ignored` (`0.14s`)
  - `adversarial_ledger`: `14 passed; 0 failed; 0 ignored` (`0.19s`)
  - `adversarial_m1_18`: `33 passed; 0 failed; 0 ignored` (`1.96s`)
  - `adversarial_ordinary`: `16 passed; 0 failed; 0 ignored` (`0.11s`)
  - `adversarial_policy`: `16 passed; 0 failed; 3 ignored` (`0.10s`)
  - `adversarial_regression`: `57 passed; 0 failed; 0 ignored` (`1.72s`)
  - `attention`: `8 passed; 0 failed; 0 ignored` (`0.00s`)
  - `history`: `25 passed; 0 failed; 0 ignored` (`1.44s`)
  - `ledger`: `20 passed; 0 failed; 0 ignored` (`0.02s`)
  - `policy`: `44 passed; 0 failed; 0 ignored` (`3.80s`)
  - `store`: `30 passed; 0 failed; 0 ignored` (`36.88s`)
- `cargo test -p atrium-core --test adversarial_fuzz -- --quiet` -> exit `0` (`13.75s`).
- `cargo test -p atrium-core --test policy -- --quiet` -> exit `0` (`3.81s`).
- `cargo test -p atrium-core --test adversarial_m1_18 finding3 -- --nocapture` -> exit `0` (`0.03s` for both tests).
- Temporary probe test `crates/atrium-core/tests/probe_review_round5.rs` run and deleted; `git status --short` is clean (empty).
