# M1-29-cr17-stores: four more credential stores (CR-17), and the rule-matching limits reopened by D-121

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`) · Reviewer: Claude Opus 5.5 (read-only, cross-provider; security, D-121) · Depends on: nothing open (runs on `dev` after M1-18)
Go: D-111, CR-17; D-121 (the reopened rule-matching limits); D-119 (Pedro: the batch split into M1-27 to M1-32) · Branch: `agent/hydrogen/M1-29-cr17-stores` · Worktree: `../atrium-wt/M1-29-cr17-stores`

## Goal
CR-17: four credential stores aren't on CR-12's ask list: `~/.gem/credentials`, `~/.terraform.d/credentials.tfrc.json`, `~/.config/op/config`, `~/.yarnrc.yml`. Add them: they ask in Home like the others and are never hard-denied (D-082). The Settings list shows them.

## Context
- CR-17 and CR-12 in `docs/agents/change-requests.md`; contract `docs/contracts/m1.md` §9 (the CR-12 list); D-082.
- The ignored test `finding_6_stores_outside_the_cr_12_list_ask_in_home` (`crates/atrium-core/tests/adversarial_policy.rs` ~1139).
- The lists: `crates/atrium-core/src/policy/deny.rs` (~440–470) and `engine.rs` (~545–560); the Settings list in `app/src/ui/Security.tsx`.

Plus three rule-matching limits that M1-23's reviews escalated and D-121 reopened:
- **Path scope (`engine.rs`):** `all_paths` adds the command's working folder and every word read as a path, and `path_scope_ok` requires all of them to match, for deny rules too. So a scoped command rule never matches in a Home chat, and a scoped deny rule lets a call through once one of its paths is outside the scope (it fails open).
- **`find -exec` and inline code under prefix rules (`engine.rs`):** `allow_covers` checks only top-level units (`parts`), so an allow rule on `find` or `find .` covers `find . -exec <cmd> \;`, and one on `python3` covers `python3 -c '<code>'`, without checking what they run.
- **No daemon-side `CreateRule` guard (`crates/atrium-daemon/src/commands.rs` `create_rule`):** any client on the socket can create an allow rule on `sh`, `bash -c`, `env` or `sudo`, which breaks the engine's assumption (`shell.rs` ~1153) that no rule allows a prefix of a shell.

## Owned paths
- `deny.rs`: the CR-12 store list only (~440–466). Don't touch `ASK_TREES` or anything below it (other tasks own it).
- `engine.rs`: the CR-12 list (~545–560), and the rule matchers `all_paths`, `path_scope_ok`, `parts`, `deny_matches`, `allow_covers` (~34–200).
- `crates/atrium-daemon/src/commands.rs`: `create_rule` (~609), and a shared validator in `atrium-core` (a new `policy/rule_check.rs`, or `policy/mod.rs`) that both the daemon and the engine use.
- Contract §9: the path-scope semantics and the rule refusals, as a CR-17/D-121 wording.
- In `adversarial_policy.rs`: only remove the `#[ignore]` at ~1139.
- Contract §9: the four stores in the CR-12 list. `app/src/ui/Security.tsx` (and its test) if the list is written there rather than read from the core.
- `crates/atrium-core/tests/M1-29-cr17-stores.rs`; the handoff.

## What to do
1. Add the four stores everywhere the CR-12 list lives, so reads ask in Home, `Any` and `FullAccess` keep `dev`'s verdict, and nothing is hard-denied.
2. Tests: reading, copying, searching and `git show HEAD:…` each store, plus home-anchored and relative spellings; ordinary `.yarnrc` (not `.yarnrc.yml`), `~/.gem/specs` and `~/.terraform.d/plugins` keep `dev`'s verdict. Un-ignore finding 6.
3. **Path scope:** a rule's path scope is checked against the paths the call names (not the working folder or words that aren't paths). An allow rule's scope covers a call only if every named path is inside it; a deny rule's scope matches if **any** named path is inside it (fail closed). A command that names no path isn't covered by a scoped allow rule. Tests for allow and deny, single and multi-path calls, commands, file tools, Home and other cwds.
4. **Derived units:** an allow rule covers a call only if it also covers every unit the call runs (`find -exec`/`-execdir`/`-ok` commands, `xargs` targets, inline code), or those units are covered by other rules; otherwise the call asks. `find -delete` is never covered by a prefix rule on `find`. Interpreters with inline code (`-c`, `-e`, …) are never covered by a prefix rule on the interpreter. Tests, plus ordinary lines (`find . -name '*.rs'` under a `find` rule) keeping their verdict.
5. **The `CreateRule` guard:** the daemon refuses an allow rule (prefix or exact) on a shell, `eval`/`source`/`.`, a wrapper, a privilege tool, an interpreter with inline code or with no script, a package runner with `-c`/`--call` or no package, or `find` as a prefix, with the same folding and the same lists as the app's rule editor (`app/src/chat/rule-form.ts` `commandRisk`, read it; the Rust check should be the source of truth for these lists in the core, matching the engine's `shell.rs`/`deny.rs` tables). It refuses path scopes on command and deny rules the same way the editor does. Tests in `crates/atrium-daemon/tests/`.
6. In `app/`, if you touched it: `pnpm typecheck`, `pnpm test`.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
- Five other tasks (M1-27 to M1-32) change the policy code at the same time. Stay inside your owned paths and the line ranges named there. If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it.
- New tests go in your own new test file, `crates/atrium-core/tests/M1-29-cr17-stores.rs`. In existing test files, only remove the `#[ignore]` lines named in your brief.
- The policy rules: no line more permissive than `dev`; no new hard deny on an ordinary line that `dev` allows or asks about. Fail closed where you can't read something fully.
- Don't create extra target directories or copies of the repo (the disk is small). Remove scratch output you create.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `cargo test -p atrium-core` 3 times in a loop, with the times of `adversarial_fuzz` and the `policy` binary (no slowdown).
- [ ] Show your new tests (and the un-ignored ones) fail on `dev`'s code and pass now: check out `dev`'s versions of the src files you changed, run, restore.
- [ ] `git diff dev --stat` touches only your owned paths.
- [ ] Handoff `docs/agents/handoffs/M1-29-cr17-stores.md`: what changed and where, a verdict table (dev vs now) for every line you tested, ordinary-line checks, the outputs, anything escalated.
