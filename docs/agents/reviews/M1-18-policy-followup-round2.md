# Review: M1-18-policy-followup, fix round 1 (commits 32c6106, 1259385, 5e0c2e5)

Reviewer: Gemini 4 Argon `high`, read-only (D-101). Authors: Claude Opus 5.5 (round 1), Gemini 4 Hydrogen (fix round 1), the orchestrator (test rows). Saved by the orchestrator from the run's final message.

## Verdict: `rework`

Fix round 1 ([32c6106](crates/atrium-core/src/policy/shell.rs), [1259385](docs/agents/handoffs/M1-18-policy-followup.md), and orchestrator test commit [5e0c2e5](crates/atrium-core/tests/adversarial_m1_18.rs)) resolves the `lex_depth` performance regression from the first review (`adversarial_fuzz` is down from `193.49s` to `12.86s`, and `policy` from `291.77s` to `3.49s`), closes Minors 5–7 without any false hard denies on ordinary commands, and closes the named lines from Blockers 1–3 except the ignored two-unknown-`env`-option test row.

However, probing the invariants behind **K1**, **P1 / D-100**, and **CR-12** shows three remaining `Allow` gaps on sensitive/credential paths (all **`same as dev`**) and one **D-100** `Deny`-rule precedence bug.

---

## Findings

> Inputs reaching `Allow` on a sensitive or credential-store path are listed first, marked `blocker`, with their `regression vs dev` or `same as dev` comparison.

### Blockers

#### 1. [blocker] K1 invariant incomplete: `unwrap_readings`, `unwrap_steps`, and `env_payloads` fix `choices` from `mask == 0` alone, and `scan_env` misses unknown flags in `-ZS…` / `-ZC…` clusters — **`same as dev`**
- **Locations:** [shell.rs:2061-2071](crates/atrium-core/src/policy/shell.rs#L2061-L2071) (`unwrap_readings`), [shell.rs:2134-2141](crates/atrium-core/src/policy/shell.rs#L2134-L2141) (`unwrap_steps`), [shell.rs:3108-3161](crates/atrium-core/src/policy/shell.rs#L3108-L3161) (`scan_env`), [shell.rs:3207-3243](crates/atrium-core/src/policy/shell.rs#L3207-L3243) (`env_payloads`), [adversarial_m1_18.rs:1408-1415](crates/atrium-core/tests/adversarial_m1_18.rs#L1408-L1415)
- **Mechanism:**
  1. **Why the ignored row (`cwd=proj|env -Z X --yyy Y -C .. cat .ssh/id_rsa`) and all multi-unknown-option combinations fail:** In `unwrap_readings` ([shell.rs:2062](crates/atrium-core/src/policy/shell.rs#L2062)), `unwrap_steps` ([shell.rs:2136](crates/atrium-core/src/policy/shell.rs#L2136)), and `env_payloads` ([shell.rs:3224-3226](crates/atrium-core/src/policy/shell.rs#L3224-L3226)), the number of binary choices `choices` is taken exclusively from `mask == 0`. On `mask == 0`, bit 0 is `0`, so the first unknown option (`-Z` or `--xxx`) does not consume its following value word `X`. On the next iteration of the `while j < a.len()` loop in `scan_env` (and `table` in `unwrap_reading_steps`), `X` is neither an option nor an assignment word, so the loop immediately `break`s at `X` with `cur_choices = 1`—before ever reaching `--yyy Y`, `-Y W`, or a second wrapper. The outer `mask` loop then stops at `1u32 << 1` (`mask = 2`), never evaluating `mask = 3` (`0b11`) where both unknown options consume their values.
  2. **`-ZS…` and `-ZC…` clusters with unknown flag letters in `scan_env`:**
     - At [shell.rs:3108-3139](crates/atrium-core/src/policy/shell.rs#L3108-L3139), the `matches!(cluster, Some(('S', _)))` arm never checks `env_flags_ok(w)` or branches with `pick()`. When an unknown option `-Z` precedes `S` with a non-empty remainder (`env -ZSfoo …`, where `-Z` may take `Sfoo` as its attached value), `scan_env` unconditionally splices `foo` as a `-S` command with `unknown = false` and `choices = 0`.
     - At [shell.rs:3150-3160](crates/atrium-core/src/policy/shell.rs#L3150-L3160), when `letter == 'C'`, `chdir` is overwritten with `None` (when `rest.is_empty()` and `!take_two`) or `Some(rest)` (when `!ok && !rest.is_empty()`, without branching on `pick()`), clobbering an earlier `-C ..` in `env -C .. -ZCproj` and `env -C .. -ZC sub`.
- **How to close it:**
  1. In `unwrap_readings`, `unwrap_steps`, and `env_payloads`, update `choices = choices.max(cur_choices)` on every iteration of the `mask` loop (`while mask < (1u32 << choices.min(4))`), and if `choices > 4`, also evaluate `mask = u32::MAX` (because any 1-word reading of an unknown option before a non-option word immediately terminates the option loop at that word, only the 2-word reading continues scanning further options).
  2. In `scan_env`: in the `Some(('S', _))` arm, if `!env_flags_ok(w)`, set `*unknown = true` and branch with `pick()` so one reading treats the `-ZS…` word as a 1-word unknown option (`j += 1; continue;`); in the `letter == 'C'` arm, do not overwrite an existing `chdir` with `None` when `!take_two`, and when `!ok && !rest.is_empty()`, branch with `pick()` on whether the unknown leading flag consumed `C…` as its attached value.
- **Evidence** (`[home, prefix, any, full]`):
  - `cwd=proj|env -Z X --yyy Y -C .. cat .ssh/id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X -Y W -C .. cat .ssh/id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env --xxx X --yyy Y -C .. cat .ssh/id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env --xxx X -Y W --chdir .. cat .ssh/id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X --yyy Y -S 'cat ../.ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X --yyy Y --split-string 'cat ../.ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X --yyy Y eval 'cd ~/.ssh && true' && cat id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X --yyy Y source /dev/stdin <<<'cd ~/.ssh && true' && cat id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X --yyy Y . /dev/stdin <<<'cd ~/.ssh && true' && cat id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X --yyy Y bash <(echo 'rm -rf ~')` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X --yyy Y source <(echo 'rm -rf ~')` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X --yyy Y . <(echo 'rm -rf ~')` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -ZSfoo sudo id` -> `["Ask", "Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -ZSfoo eval 'cd ~/.ssh && true' && cat id_rsa` -> `["Ask", "Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -ZSfoo bash <(echo 'rm -rf ~')` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -C .. -ZCproj cat .ssh/id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -C .. -ZC sub cat .ssh/id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|command -Z X -Y W eval 'cd ~/.ssh && true' && cat id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `cwd=proj|env -Z X command -Y W eval 'cd ~/.ssh && true' && cat id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (**`same as dev`**)
- **Comparison with `dev`:** On `dev` (`git show dev:crates/atrium-core/src/policy/shell.rs`), `env` and `command` do not branch across multiple unknown options or extract `env_payloads`, so all 19 lines return `Allow` under `Any` and `FullAccess` (and `env -ZSfoo …` under prefix rules) on `dev` (**`same as dev`**).

---

#### 2. [blocker] P1 & D-100: `glob_escape` misses `$HOME`/`$PWD` anchors, skips `check_escape_variant` on character-class dot components and `\`, and `search_glob_hit` misses relative/home-anchored hard-deny globs under `~` — **`same as dev`**
- **Locations:** [deny.rs:1019-1042](crates/atrium-core/src/policy/deny.rs#L1019-L1042) (`Ctx::check`), [deny.rs:1225-1252](crates/atrium-core/src/policy/deny.rs#L1225-L1252) (`search_glob_hit`), [deny.rs:1308-1382](crates/atrium-core/src/policy/deny.rs#L1308-L1382) (`glob_escape`, `check_escape_variant`), [engine.rs:832-853](crates/atrium-core/src/policy/engine.rs#L832-L853) (`DefaultPolicy::evaluate`)
- **Mechanism:**
  1. **`$HOME`, `${HOME}`, and `$PWD` anchors in `check_escape_variant`:** [deny.rs:1362](crates/atrium-core/src/policy/deny.rs#L1362) checks `let anchored = variant.starts_with('/') || variant.starts_with('~');` before calling `lexical_resolve`. When `glob` starts with `$HOME/`, `${HOME}/`, or `$PWD/`, `anchored` is `false`, so `check_escape_variant` computes `normalize(&r.join(variant))` = `/h/u/proj/$HOME/.ssh/id_rsa`, which starts with `/h/u/proj` (`inside == true`, `leaves() == false`). Because `ToolSubject::Paths` does not push `glob` into `a.raw`, `raw_hit` never inspects `glob`, and these reach `Allow` in **Home, `Any`, and `FullAccess`**.
  2. **Character-class dot components (`[.][.]`, `.[.]`, `[.]`) and `\` under D-100:** When `glob` has a character-class dot component (`may_be_dot_component` at [deny.rs:1324-L1327](crates/atrium-core/src/policy/deny.rs#L1324-L1327)) or a backslash ([deny.rs:1308-1311](crates/atrium-core/src/policy/deny.rs#L1308-L1311)), `glob_escape` sets `out.unknown = true` and skips `check_escape_variant`, leaving `out.patterns` empty so `Ctx::check` never checks the target path against `hard_deny`. In round 1, `out.unknown` returned `Ask` across all presets; in fix round 1, [engine.rs:836-850](crates/atrium-core/src/policy/engine.rs#L836-L850) allows any `glob_leaves` (`esc.unknown || !esc.patterns.is_empty()`) under `Any` and `FullAccess`. As a result, escaping globs that target `.ssh/id_rsa` through `[.][.]`, `.[.]`, `[.]/..`, `??`, or `\` reach `Allow` under `Any` and `FullAccess` (including 5 rows in [adversarial_m1_18.rs:749-753](crates/atrium-core/tests/adversarial_m1_18.rs#L749-L753)), violating D-100 ("escaping globs that can't hit the hard-deny list are allowed under FullAccess and an Any rule").
  3. **`search_glob_hit` when searching from `~`:** Unlike `ask_search_glob_hit` (updated in fix round 1), `Ctx::search_glob_hit` ([deny.rs:1019-1028](crates/atrium-core/src/policy/deny.rs#L1019-L1028) and [deny.rs:1225-1252](crates/atrium-core/src/policy/deny.rs#L1225-L1252)) is skipped when `recursive == false` and never expands home, normalizes/`drop_globstar`s, lowercases, or matches against `rel` (`under(&probe, &lower(r))`). When `paths: ["~"]`, `/h/u/.ssh/id_rsa` is inside the root `/h/u` (`glob_escape` sees `inside == true`), so `Search ["~"]` with `.ssh/id_rsa`, `~/.ssh/id_rsa`, `.aws/credentials`, or `.npmrc` and `recursive: false` reaches `Allow` in **Home, `Any`, and `FullAccess`** (and `Allow` under `Any` and `FullAccess` when `recursive: true`).
- **Evidence** (`[home, any, full]`):
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("$HOME/.ssh/id_rsa"), recursive: true }` -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("$HOME/.ssh/id_rsa"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("${HOME}/.ssh/id_rsa"), recursive: true }` -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("$PWD/../.ssh/id_rsa"), recursive: true }` -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some(".ssh/id_rsa"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("~/.ssh/id_rsa"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some(".aws/credentials"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some(".npmrc"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**/../.ssh/id_rsa"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some(".ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("~/.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("**/../.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("[.][.]/.ssh/id_ed25519"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some(".[.]/.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("??/.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("[!a][!a]/.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("\\.\\./.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("[.]/../.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("**/[.][.]/.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (**`same as dev`**)
- **Comparison with `dev`:** `dev` (`git show dev:crates/atrium-core/src/policy/deny.rs`) had no `glob_escape` and had the same `search_glob_hit`, so all of these return `Allow` on `dev` (**`same as dev`**).

---

#### 3. [blocker] CR-12: `ask_search_glob_hit` misses non-canonical relative/home globs (`.cargo/./credentials.toml`, `proj/../.cargo/credentials.toml`), and `git_names_credential` misses `:`-terminated or combined short/long pathspec magic (`::.npmrc`, `:/:.npmrc`, `:!(top).env`) in Home — **`same as dev`**
- **Locations:** [deny.rs:510-528](crates/atrium-core/src/policy/deny.rs#L510-L528) (`ask_search_glob_hit`), [engine.rs:589-608](crates/atrium-core/src/policy/engine.rs#L589-L608) (`git_names_credential`)
- **Mechanism:**
  1. `ask_search_glob_hit` only strips a leading `./` (`expanded.trim_start_matches("./")`) without normalizing internal `/./`, `//`, or `..` segments. For `paths: ["~"]` and `recursive: false`, globs such as `.cargo/./credentials.toml`, `.cargo//credentials.toml`, `proj/../.cargo/credentials.toml`, or `~/./.cargo/credentials.toml` normalize inside `~` in `glob_escape` (`leaves() == false`), fail to match `rel = ".cargo/credentials.toml"` or `probe = "/h/u/.cargo/credentials.toml"` in `GlobBuilder`, and skip `root_ok("~")` because `recursive == false`, returning `Allow` in Home.
  2. Per `gitglossary(7)`, Git short pathspec magic `:[/!^]*` may end with an optional terminating colon `:` (`::path`, `:/:path`, `:!:path`, `:^:path`) or be followed by `(long-magic)` (`:!(top)path`, `:/:(icase)path`). In `git_names_credential` ([engine.rs:593-604](crates/atrium-core/src/policy/engine.rs#L593-L604)), `after.trim_start_matches(['/', '!', '^'])` leaves the terminating `:` or `(top)` attached to `name` (`":.npmrc"`, `"(top).env"`), missing `GIT_CRED_NAMES` / `GIT_CRED_SUFFIXES` / `GIT_CRED_DIRS` and returning `Allow` in Home.
- **Evidence:**
  - `ToolSubject::Paths { paths: ["~"], glob: Some(".cargo/./credentials.toml"), recursive: false }` (`[home, any, full]`) -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some(".cargo//credentials.toml"), recursive: false }` (`[home, any, full]`) -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("proj/../.cargo/credentials.toml"), recursive: false }` (`[home, any, full]`) -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("~/./.cargo/credentials.toml"), recursive: false }` (`[home, any, full]`) -> `["Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `git log -p -- '::.npmrc'` (`[home, prefix, any, full]`) -> `["Allow", "Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `git diff HEAD~1 -- '::.env'` (`[home, prefix, any, full]`) -> `["Allow", "Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `git log -p -- ':/:.npmrc'` (`[home, prefix, any, full]`) -> `["Allow", "Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `git diff HEAD~1 -- ':!:.env'` (`[home, prefix, any, full]`) -> `["Allow", "Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `git log -p -- ':^:.pypirc'` (`[home, prefix, any, full]`) -> `["Allow", "Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `git log -p -- ':!(top).env'` (`[home, prefix, any, full]`) -> `["Allow", "Allow", "Allow", "Allow"]` (**`same as dev`**)
  - `git log -p -- ':/:(icase).npmrc'` (`[home, prefix, any, full]`) -> `["Allow", "Allow", "Allow", "Allow"]` (**`same as dev`**)
- **Comparison with `dev`:** Neither `ask_search_glob_hit` nor `git_names_credential` existed on `dev`, which returned `Allow` in Home for all of these (**`same as dev`**).

---

### Majors & Minors

#### 4. [major] D-100 rule precedence: a higher-scope `PathGlob` or `path_scope`d `Allow` rule sets `covered = [true]` on an escaping glob, masking a lower-scope `Deny` rule and returning `Allow` under `FullAccess` — **`same as dev`**
- **Locations:** [engine.rs:116-170](crates/atrium-core/src/policy/engine.rs#L116-L170) (`allow_covers`), [engine.rs:179-233](crates/atrium-core/src/policy/engine.rs#L179-L233) (`apply_rules`), [engine.rs:832-853](crates/atrium-core/src/policy/engine.rs#L832-L853) (`DefaultPolicy::evaluate`)
- **Mechanism:** Under D-100, a `PathGlob` rule or a `path_scope`-restricted `Any` rule cannot cover an escaping glob (`glob_leaves == true`). However, `allow_covers` still returns `vec![true]` when `paths` (the search roots, e.g., `["~/proj/src"]`) match `PathGlob { glob: "/h/u/proj/**" }` or `path_scope: Some("/h/u/proj/**")`. When that `Allow` rule is at `Thread` or `Home` scope (`rank` 0 or 1), `apply_rules` marks `covered = [true]` (with `any_allow = false`). At `Global` scope (`rank` 2), `if !covered.iter().all(|c| *c)` ([engine.rs:195](crates/atrium-core/src/policy/engine.rs#L195)) is then `false`, skipping any `Global` `Deny` rule. `evaluate` then rejects the `PathGlob` allow (`!glob_leaves || out.any_allow` is `false`), falls through to [engine.rs:847-853](crates/atrium-core/src/policy/engine.rs#L847-L853), and returns `Allow` under `Preset::FullAccess` (and `Ask` under `Preset::Home`) instead of `Deny`.
  - *(Note: `fix1_d100_rule_precedence` at [adversarial_m1_18.rs:1318-1358](crates/atrium-core/tests/adversarial_m1_18.rs#L1318-L1358) only tested `Any Allow` + `Any Deny` at the same `Global` scope, and its `PathGlob` test used `paths: ["~/proj"]` with `glob: "/h/u/proj/**"`, which does not match `/h/u/proj` under `literal_separator(true)`.)*
- **Fix:** Pass `glob_leaves` into `allow_covers` (analogous to `command && !simple` at [engine.rs:130-133](crates/atrium-core/src/policy/engine.rs#L130-L133)) so that when `glob_leaves` is `true`, `allow_covers` returns `none` unless `matches!(rule.matcher, RuleMatcher::Any) && rule.path_scope.is_none()`.
- **Evidence:**
  - `ToolSubject::Paths { paths: ["~/proj/src"], glob: Some("../other/*.rs"), recursive: true }` with `[Rule(Thread("t1"), PathGlob("/h/u/proj/**"), Allow), Rule(Global, Any, Deny)]`:
    - `Preset::Home` -> `"Ask"` (wanted `"Deny"`)
    - `Preset::FullAccess` -> `"Allow"` (wanted `"Deny"`)
  - Same check with `[Rule(Thread("t1"), Any + path_scope("/h/u/proj/**"), Allow), Rule(Global, Any, Deny)]` under `Preset::FullAccess` -> `"Allow"` (wanted `"Deny"`).

#### 5. [minor] Performance: `split_words` calls `lex(text)` at `depth = 0` with a new `LexCache`, bypassing `MAX_DEPTH` and `cache` on nested `env -S` — **`regression vs dev` (latency on hostile nested `env -S`)**
- **Locations:** [shell.rs:2353-2368](crates/atrium-core/src/policy/shell.rs#L2353-L2368) (`split_words`), [shell.rs:3130](crates/atrium-core/src/policy/shell.rs#L3130) (`scan_env`)
- **Detail:** `LexCache` (`HashMap<(String, u8), Rc<Lexed>>`) and `refresh_ids` ([shell.rs:405-459](crates/atrium-core/src/policy/shell.rs#L405-L459)) are sound: the cache is local to a single `lex()` call, keyed by `(line, depth)` so `Unit::level` and `MAX_DEPTH` are respected, and `refresh_ids` remaps `ctx`, `pipe`, and `compound` IDs 1-to-1 so `words::walk` and pipeline checks never see stale or colliding IDs. The one remaining uncached recursive path is `split_words(text)` ([shell.rs:2354](crates/atrium-core/src/policy/shell.rs#L2354)), which calls `lex(text)` at `depth = 0` with a fresh `LexCache` on every `scan_env` run (across `unwrap_checked`, `unwrap_readings`, `unwrap_steps`, and `env_payloads`). On 5-deep nested `env -Z X -S "…"` (with backslash-escaped quotes), a 4-setting evaluation takes `580ms` (`~145ms` per `eval_check`, growing ~7× per level). Because `split_words` only extracts the first unit's `assignments` and `argv` (while `code_texts` already lexes `split_texts` as payloads via `lex_depth_fresh`), lexing at `MAX_DEPTH` or threading `depth` and `cache` into `split_words` keeps nested `env -S` linear.

---

## Check-by-Check Summary

1. **First review's blockers 1–4 and minors 5–7:**
   - **Blocker 1 (P1):** `drop_globstar` in `glob_escape` closes `**` before `..` across braces, repeated slashes, curdir segments, and `~`/`/h/u` prefixes (`fix1_p1_globstar_before_parent_dir` passes). Remaining gaps in the invariant: `$HOME`/`$PWD` anchors, character-class dot components + `\` under D-100, and `search_glob_hit` under `~` (Finding 2).
   - **Blocker 2 (K1):** Single unknown short or long options before `-C`, `--chdir`, `-S`, `--split-string`, `eval`, `source`, `.`, and `<(...)` pass (`fix1_k1_env_unknown_options_keep_payloads`). Any unknown option taking a value followed by another unknown option taking a value fails because `choices` is fixed on `mask == 0`, and `-ZS…`/`-ZC…` clusters miss unknown leading flags (Finding 1).
   - **Blocker 3 (CR-12):** Canonical relative and `~`/`$HOME`-anchored globs with `recursive: false` and basic `:(magic)`/`:[/!^]` git pathspecs pass. Non-canonical relative/home globs (`.cargo/./credentials.toml`, `proj/../.cargo/credentials.toml`) and `:`-terminated or combined short/long git pathspec magic (`::.npmrc`, `:/:.npmrc`, `:!(top).env`) still reach `Allow` in Home (Finding 3).
   - **Blocker 4 (Performance):** Closed for `$(...)`, backticks, `<(...)`, wrappers, and long lines (`adversarial_fuzz` `12.86s`, `policy` `3.49s`). Only `split_words` on nested `env -S` bypasses `LexCache` and `MAX_DEPTH` (Finding 5).
   - **Minors 5–7:** All three (`reader_flag_tokens` in `follows_links`, skipping `.git`/`node_modules`/`target` in `links_below`, and flag/`is_git_key_name`/`.zsh_sessions`/`.bash_sessions` handling in `git_names_credential`) are cleanly closed.
2. **D-100:** Escaping globs without hard-deny hits return `Ask` in Home and `Allow` under `FullAccess` and unscoped `Any` rules, but a higher-scope `PathGlob` or `path_scope`d `Allow` rule masks a lower-scope `Deny` rule (Finding 4), and `esc.unknown` globs bypass hard-deny checks under `Any`/`FullAccess` (Finding 2).
3. **Ordinary lines hunt:** Everyday `git`, `cargo`, `rg`, `grep`, `find`, `ls`, `cp`, `tar`, and `env` commands, compound `cd X && ...` chains (`(cd sub && make)`, `{ ...; }`, `if ...; fi`), large repos with `.git`/`node_modules`/`target`, and project files named `env`, `env.rs`, `history`, `history.rs`, `.env`, `.zsh_history`, or `id_rsa_helper.rs` were tested and keep `dev`'s verdicts with zero false hard denies.

---

## Commands Run

- `cargo clippy -p atrium-core --all-targets -- -D warnings` -> exit `0` (`1.84s`, 0 warnings).
- `cargo test -p atrium-core --test adversarial_fuzz` -> exit `0`: `2 passed; 0 failed; 1 ignored; finished in 12.86s`.
- `cargo test -p atrium-core --test policy` -> exit `0`: `44 passed; 0 failed; 0 ignored; finished in 3.49s`.
- `cargo test -p atrium-core --no-fail-fast` -> exit `0` (all test binaries passed):
  - `src/lib.rs`: `9 passed; 0 failed; 0 ignored` (`0.00s`)
  - `adversarial_fuzz`: `2 passed; 0 failed; 1 ignored` (`12.86s`)
  - `adversarial_gate`: `9 passed; 0 failed; 2 ignored` (`0.11s`)
  - `adversarial_ledger`: `14 passed; 0 failed; 0 ignored` (`0.16s`)
  - `adversarial_m1_18`: `17 passed; 0 failed; 1 ignored` (`1.81s`)
  - `adversarial_ordinary`: `16 passed; 0 failed; 0 ignored` (`0.10s`)
  - `adversarial_policy`: `16 passed; 0 failed; 3 ignored` (`0.08s`)
  - `adversarial_regression`: `57 passed; 0 failed; 0 ignored` (`1.63s`)
  - `attention`: `8 passed; 0 failed; 0 ignored` (`0.00s`)
  - `history`: `25 passed; 0 failed; 0 ignored` (`1.38s`)
  - `ledger`: `20 passed; 0 failed; 0 ignored` (`0.01s`)
  - `policy`: `44 passed; 0 failed; 0 ignored` (`3.49s`)
  - `store`: `30 passed; 0 failed; 0 ignored` (`36.59s`)
- Temporary probe test `crates/atrium-core/tests/probe_review.rs` run and deleted; `git status --short` is clean (empty).

