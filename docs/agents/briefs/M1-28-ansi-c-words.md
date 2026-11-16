# M1-28-ansi-c-words: decode ANSI-C quoted words (S7)

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`) · Reviewer: Claude Sonnet 5.5 (read-only, cross-provider) · Depends on: nothing open (runs on `dev` after M1-18)
Go: D-111; D-119 (Pedro: the batch split into M1-27 to M1-32) · Branch: `agent/hydrogen/M1-28-ansi-c-words` · Worktree: `../atrium-wt/M1-28-ansi-c-words`

## Goal
S7 (`docs/agents/m1-must-fix.md`, M1-13 finding 4): ANSI-C quoted words (`$'…'`) aren't decoded, so `cat $'\x2essh/id_rsa'` or `$'\163h' -c …` hide what they name. Decode them the way bash does.

## Context
- The ignored test `finding_4_ansi_c_quoted_words_are_decoded` (`crates/atrium-core/tests/adversarial_policy.rs` ~977).
- `crates/atrium-core/src/policy/shell.rs`, the word reader (roughly lines 700–950: quoting, `$`, expansion), and `words.rs` if decoding belongs there.
- bash's `$'…'` escapes: `\a \b \e \E \f \n \r \t \v \\ \' \" \?`, `\nnn` octal, `\xHH`, `\uHHHH`, `\UHHHHHHHH`, `\cX`.

## Owned paths
- `shell.rs`: the word reader (~700–950) only; `words.rs` if decoding lives there. Don't touch `base()` (~1546) or the name tables (another task owns them).
- In `adversarial_policy.rs`: only remove the `#[ignore]` at ~977.
- `crates/atrium-core/tests/M1-28-ansi-c-words.rs`; the handoff.

## What to do
1. Decode `$'…'` into the word's literal text, with every escape above. If a word can't be decoded exactly (an unknown or malformed escape, a NUL, a decoded control character the reader refuses elsewhere), fail closed the way the reader already does for words it can't read.
2. Tests: hidden paths and commands through each escape form (`$'\x2e'ssh`, octal, `\u`, `\U`, `\c`, mixed with other quoting), in a path, a command name and a `-c` payload; ordinary `$'…'` use (`printf $'a\tb\n'`, `IFS=$'\n'`) keeps `dev`'s verdict. Un-ignore finding 4.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
- Five other tasks (M1-27 to M1-32) change the policy code at the same time. Stay inside your owned paths and the line ranges named there. If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it.
- New tests go in your own new test file, `crates/atrium-core/tests/M1-28-ansi-c-words.rs`. In existing test files, only remove the `#[ignore]` lines named in your brief.
- The policy rules: no line more permissive than `dev`; no new hard deny on an ordinary line that `dev` allows or asks about. Fail closed where you can't read something fully.
- Don't create extra target directories or copies of the repo (the disk is small). Remove scratch output you create.

## Definition of Done
- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `cargo test -p atrium-core` 3 times in a loop, with the times of `adversarial_fuzz` and the `policy` binary (no slowdown).
- [ ] Show your new tests (and the un-ignored ones) fail on `dev`'s code and pass now: check out `dev`'s versions of the src files you changed, run, restore.
- [ ] `git diff dev --stat` touches only your owned paths.
- [ ] Handoff `docs/agents/handoffs/M1-28-ansi-c-words.md`: what changed and where, a verdict table (dev vs now) for every line you tested, ordinary-line checks, the outputs, anything escalated.
