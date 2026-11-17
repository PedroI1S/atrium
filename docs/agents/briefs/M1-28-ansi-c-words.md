# M1-28-ansi-c-words: decode ANSI-C quoted words (S7), and the open minors from M1-21 to M1-26

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`) · Reviewer: Claude Sonnet 5.5 (read-only, cross-provider) · Depends on: nothing open (runs on `dev` after M1-18)
Go: D-111; D-122 (the follow-up minors folded in); D-119 (Pedro: the batch split into M1-27 to M1-32) · Branch: `agent/hydrogen/M1-28-ansi-c-words` · Worktree: `../atrium-wt/M1-28-ansi-c-words`

## Goal
S7 (`docs/agents/m1-must-fix.md`, M1-13 finding 4): ANSI-C quoted words (`$'…'`) aren't decoded, so `cat $'\x2essh/id_rsa'` or `$'\163h' -c …` hide what they name. Decode them the way bash does.

## Context
- The ignored test `finding_4_ansi_c_quoted_words_are_decoded` (`crates/atrium-core/tests/adversarial_policy.rs` ~977).
- `crates/atrium-core/src/policy/shell.rs`, the word reader (roughly lines 700–950: quoting, `$`, expansion), and `words.rs` if decoding belongs there.
- bash's `$'…'` escapes: `\a \b \e \E \f \n \r \t \v \\ \' \" \?`, `\nnn` octal, `\xHH`, `\uHHHH`, `\UHHHHHHHH`, `\cX`.

Plus every minor and nit still open in the reviews of tasks merged before D-120 (D-122). Check each against `dev` first; if it's already fixed, say so with the evidence:
- **M1-22** (`docs/agents/reviews/M1-22-daemon-adapters.md`, handoff "Login shell `PATH` discovery"):
  - merge the login shell's `PATH` into the provider `PATH` before the runtime starts (`$SHELL -ilc 'printf %s "$PATH"'` or equivalent, short timeout, fish-aware, existing entries first, de-duplicated, skipped under `ATRIUM_NO_PATH_EXTRAS=1`), so `nvm`/`asdf`/`mise`/custom-init CLIs are found; never fails startup;
  - the daemon's stderr is piped but read only on early exit, so a chatty daemon can fill the pipe: send it to a log file or `Stdio::null()`;
  - nits: `wait_for_socket` waits for the file, not `listen`; the spawn-error match is a substring.
- **M1-24** (`docs/agents/reviews/M1-24-bridge-batching.md` finding 4): `client.ts` `receiveFrame` rethrows only the first error in a batch; log every later one (`console.error`) or collect them.
- **M1-25** (`docs/agents/reviews/M1-25-export-damaged.md` finding 1): add a test with one damaged message and another still open at the end of the export.
- **M1-26** (`docs/agents/reviews/M1-26-gate-withdraw.md`):
  - withdrawals are awaited one at a time under the gate deadline (`claude/adapter.rs` ~993–1003, `codex/adapter.rs` ~1869–1876), so a hung gate stalls the actor for N × the deadline: one `JoinSet` with one overall deadline, or detached tasks;
  - `clear_requests` aborts in-flight gate checks before draining (`claude/adapter.rs` ~1043–1047), so a check that already opened a ticket can leak it until the turn expires: withdraw those tickets too;
  - a fake-provider test for the Claude `tool_result`-without-`can_use_tool` withdrawal.
- **M1-21** (`docs/agents/reviews/M1-21-demo.md`): confirm every finding was addressed in `fdabc1b8`; fix any that wasn't.

## Owned paths
- `shell.rs`: the word reader (~700–950) only; `words.rs` if decoding lives there. Don't touch `base()` (~1546) or the name tables (another task owns them).
- In `adversarial_policy.rs`: only remove the `#[ignore]` at ~977.
- For the minors (D-122): `crates/atrium-daemon/src/lib.rs` and `main.rs` (PATH; not `commands.rs`, another task owns `create_rule`); the app's daemon launcher in `app/src-tauri/src/`; `app/src/daemon/client.ts` and its test; `crates/atrium-agents/src/{claude,codex}/adapter.rs` (the withdraw and `clear_requests` code only); `app/scripts/m1_demo.py` and `app/src/m1-demo.ts` if an M1-21 item is still open. In `crates/atrium-core/src/policy/gate.rs`, nothing (tests only).
- New tests: `crates/atrium-core/tests/M1-28-ansi-c-words.rs`, `crates/atrium-daemon/tests/M1-28-followups.rs`, `crates/atrium-agents/tests/M1-28-followups.rs`, and app tests next to the code.
- The handoff.

## What to do
1. Decode `$'…'` into the word's literal text, with every escape above. If a word can't be decoded exactly (an unknown or malformed escape, a NUL, a decoded control character the reader refuses elsewhere), fail closed the way the reader already does for words it can't read.
2. Tests for S7: hidden paths and commands through each escape form (`$'\x2e'ssh`, octal, `\u`, `\U`, `\c`, mixed with other quoting), in a path, a command name and a `-c` payload; ordinary `$'…'` use (`printf $'a\tb\n'`, `IFS=$'\n'`) keeps `dev`'s verdict. Un-ignore finding 4.

3. The minors above, each with a test, never running a real provider (fake CLIs and a fake `$SHELL` in a temp folder for the PATH work).
4. In `app/`: `pnpm typecheck`, `pnpm test`, `pnpm build`.

## Rules
- Work only in your worktree; commit on your branch only. Never push, merge or rebase. Never run a real provider.
- Five other tasks (M1-27 to M1-32) change the policy code at the same time. Stay inside your owned paths and the line ranges named there. If the fix needs a change elsewhere, stop that part and say so in the handoff: don't make it.
- New tests go in your own new test file, `crates/atrium-core/tests/M1-28-ansi-c-words.rs`. In existing test files, only remove the `#[ignore]` lines named in your brief.
- The policy rules: no line more permissive than `dev`; no new hard deny on an ordinary line that `dev` allows or asks about. Fail closed where you can't read something fully.
- Don't create extra target directories or copies of the repo (the disk is small). Remove scratch output you create.

## Definition of Done
**D-123:** you write code and tests only (plus `cargo fmt` and `cargo check` on your crate). A separate verification task runs everything below on your branch; in your handoff, list the exact commands it should run, including the check that your new tests fail on `dev`'s code.

- [ ] `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --no-fail-fast`.
- [ ] `cargo test -p atrium-core` 3 times in a loop, with the times of `adversarial_fuzz` and the `policy` binary (no slowdown).
- [ ] Show your new tests (and the un-ignored ones) fail on `dev`'s code and pass now: check out `dev`'s versions of the src files you changed, run, restore.
- [ ] `git diff dev --stat` touches only your owned paths.
- [ ] Handoff `docs/agents/handoffs/M1-28-ansi-c-words.md`: what changed and where, a verdict table (dev vs now) for every line you tested, ordinary-line checks, the outputs, anything escalated.

## Added scope (D-120, from M1-30; orchestrator, 2026-11-16)
- **M1-13 finding 11 in `adversarial_fuzz.rs`** (`random_spellings_of_sensitive_paths_are_always_hard_denied`, `#[ignore = "M1-13 finding 11"]` at ~422): its `shape_pick == 8` builds `cd <glob>; <reader> <file>`, and the fix is in `words.rs` `cd_effect` (~628). M1-30 can't reach it, because that is outside its paths. Owned paths widen to `cd_effect` and to removing that `#[ignore]`. A `cd` into a glob that could reach a credential or Atrium folder must leave the reader's file judged as if it were under every folder the glob can match; fail closed (Ask, or hard deny where the plain path is hard-denied), never Allow.

## Added scope (D-120, orchestrator, 2026-11-16): N10
`crates/atrium-daemon/tests/adversarial_socket.rs` `flooding_unread_list_threads_past_8_mib_control_lane_closes_connection` fails on Linux every run, on `dev` too: "the 8 MiB control lane was not enforced: all 17 answers arrived" (0.6 s). Find out whether the test is sized for macOS socket buffers (Linux's are larger) or the daemon doesn't enforce the lane on Linux. Make the test size its flood from the lane limit, not from the kernel buffer, so that it fails for a daemon that never closes the connection, and fix the daemon if that is the cause. Owned paths widen to that test and the daemon's per-connection lane code.

## Added scope (D-120, orchestrator, 2026-11-17): N12
`dev` fails three more tests on Linux, some only intermittently:
- `atrium-daemon` `socket::replay_then_live_delivers_every_event_once_and_in_order`;
- `socket::a_reader_that_stops_lags_alone`, which this task's rework 4 already worked on;
- the app's `markdown.test.ts` "pathological emphasis and brackets parse quickly".

Make all three deterministic on Linux. Owned paths widen to those tests, the daemon's connection and lane code, and `app/src/chat/markdown.ts`. Fix the code if it is a race or a slowdown; fix the test only if the test is wrong.
