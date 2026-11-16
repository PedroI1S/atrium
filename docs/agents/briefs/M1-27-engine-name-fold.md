# M1-27-engine-name-fold: fold shell, wrapper and privilege-tool names in the engine (CR-19)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`) · Reviewer: Claude Opus 5.5 (read-only, cross-provider; security) · Depends on: nothing open (runs on `dev` after M1-18)
Go: D-118, CR-19; D-119 (Pedro: the batch split into M1-27 to M1-32) · Branch: `agent/hydrogen/M1-27-engine-name-fold` · Worktree: `../atrium-wt/M1-27-engine-name-fold`

## Goal
CR-19 (`docs/agents/change-requests.md`): the engine compares command basenames as written. macOS filesystems ignore case, so `/bin/BASH -c "…"`, `/usr/bin/ENV rm -rf ~`, `SUDO id` and `/bin/baſh -c "…"` run the real programs, but the wrapper isn't unwrapped, the `-c` payload isn't read, and `SUDO` isn't flagged as privilege escalation, even with no rules. Fold every command name before comparing it.

## Context
- CR-19; `docs/agents/reviews/M1-23-ui-gaps-round3.md` (§5, escalation 1), which names the comparison sites.
- `crates/atrium-core/src/policy/shell.rs`: `base()` (around line 1546) and every name table it feeds (`SHELLS` ~2589, `unwrap_reading`, `payloads_of`, `env_payloads`).
- `crates/atrium-core/src/policy/deny.rs`: `lower()` (~378, the existing path folds: lowercase, which also folds K U+212A, and `ſ`→`s`), `PRIV` and `is_interpreter` (~821–833).
- The app's rule editor already folds names the same way (`app/src/chat/rule-form.ts` `foldName`), for reference only.

## Owned paths
- `shell.rs`: `base()` and the name comparisons that use its result. Prefer one change: have `base()` (or a new `cmd_name()` next to it) return the folded name, and use it at each comparison. Don't touch the word reader (~700–950, another task owns it).
- `deny.rs`: `PRIV`, `is_interpreter` and `reads_stdin_as_code*` (~815–920) only.
- `crates/atrium-core/tests/M1-27-engine-name-fold.rs`; the handoff.

## What to do
1. Fold names as `deny.rs` folds paths (lowercase plus `ſ`→`s`) at every place that compares a command name to a list or a literal, in the owned ranges. Keep the original word for anything printed or matched against rules (rules are matched as written; the rule editor already refuses folded spellings of shells).
2. Tests: `/bin/BASH -c 'cat ~/.ssh/id_rsa'`, `Bash -c`, `/bin/baſh -c`, `/usr/bin/ENV rm -rf ~`, `ENV -S 'sh -c' '…'`, `SUDO id`, `Sudo -i`, `PYTHON3 -c '…'`, `NICE sudo id`, `/bin/Ksh -c` (K as U+212A) each get the same verdict as the lowercase form, in Home, under a prefix rule, Any and FullAccess. Ordinary lines with capitals (`Makefile`-style commands, `./Build.sh`, `CMake`) keep `dev`'s verdict.

## Added scope (D-121: reopened known limits, done in this task's next round)
Owned paths widen to `shell.rs` `shell_c_codes` (~2090–2110), `payloads_of` (~3161) and `expand_unknown_env_cmd` (~3469–3680). Still not the word reader (~700–950).
- **K7:** `sh -c` scripts that are only a positional-parameter expansion (`sh -c '$0' 'cat .ssh/id_rsa'`, `sh -c '"$1"' x 'cat …'`, `bash -c '$@' …`, `${0}`, `"$*"`) must resolve to the words they expand to, so the payload is checked like `sh -c 'cat .ssh/id_rsa'`. Where it can't be resolved exactly, fail closed (Ask everywhere), never Allow.
- **K9:** `env` with more than about 24 words before the command (`MAX_UNKNOWN_STARTS`) asks in every setting, including Any and FullAccess. Make long but readable `env` lines (many known options, many `NAME=value` assignments) get the same verdict as the short form; keep failing closed only when the line truly can't be read within a bounded cost.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
- Five other tasks (M1-27 to M1-32) change the policy code at the same time. Stay inside your owned paths and the line ranges named there. If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it.
- New tests go in your own new test file, `crates/atrium-core/tests/M1-27-engine-name-fold.rs`. In existing test files, only remove the `#[ignore]` lines named in your brief.
- The policy rules: no line more permissive than `dev`; no new hard deny on an ordinary line that `dev` allows or asks about. Fail closed where you can't read something fully.
- Don't create extra target directories or copies of the repo (the disk is small). Remove scratch output you create.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `cargo test -p atrium-core` 3 times in a loop, with the times of `adversarial_fuzz` and the `policy` binary (no slowdown).
- [ ] Show your new tests (and the un-ignored ones) fail on `dev`'s code and pass now: check out `dev`'s versions of the src files you changed, run, restore.
- [ ] `git diff dev --stat` touches only your owned paths.
- [ ] Handoff `docs/agents/handoffs/M1-27-engine-name-fold.md`: what changed and where, a verdict table (dev vs now) for every line you tested, ordinary-line checks, the outputs, anything escalated.

## Added scope (D-120, orchestrator, 2026-11-16): review round 1
The Codex review (`reviews/M1-27-engine-name-fold.md`, finding 1) shows that folded names still miss hard-deny comparisons in `deny.rs`, at the privilege-tool (`PRIV`) sites and any other place that compares a command name. Owned paths widen to **those name comparisons only**, folding them through the same helper as `shell.rs`. Other tasks own other ranges of `deny.rs` (M1-29: the CR-12 list; M1-30: `ASK_TREES` and the glob matchers; M1-31: `check`…`search_glob_hit` and the normalizer). Keep each edit to the comparison line, and list every `deny.rs` hunk in the handoff, so the merges can be checked with `git merge-tree`.
