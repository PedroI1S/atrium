# Handoff: M1-13-adversarial

Model: Sonnet 5.5 · Status: **partial** (stopped at the end of the driver's shift)

## Top of the list: inputs that reach Allow on a sensitive path
All need a broad allow rule, FullAccess, or (finding 2 only) two ordinary prefix rules; in Home with no rules every one of them Asks.
- Finding 2: `cd nope; cat .ssh/id_rsa`. A failing `cd` leaves the shell where it was, but the tracker assumes it worked. With ordinary prefix rules `cd` and `cat` allowed it gives **Allow** (test `finding_2_with_plain_prefix_rules_a_failing_cd_reaches_allow`). Medium.
- Findings 1, 3, 4, 5, 7, 8, 9, 10, 11: same class, hard deny misses, Allow only under an `Any` rule or FullAccess.
- Finding 6: other credential stores outside the contract's list are **auto-allowed in Home with no rules** (`cat ~/.cargo/credentials.toml`, `~/.pypirc`, `~/.pgpass`, `~/.vault-token`, `~/.azure/accessTokens.json`, `~/.aws/sso/cache/*.json`, `~/.config/git/credentials`, `~/.config/op/config`, `~/.terraform.d/credentials.tfrc.json`, shell history, `~/.env`). Needs a contract change to `HARD_DENY_DEFAULTS`: change request NOT yet written (`change-requests.md` is not my path).

## Incident (my mistake)
A heredoc in one of my shell commands ended early on an `EOF` line inside the data, so the following lines ran as shell commands in `/Users/pedro` (they included `cat` of `~/.aws/credentials`, `~/Library/Keychains/login.keychain-db`, Chrome `Login Data`, if those exist). The output (64 MB) went to a tool-result file I did not read: `/Users/pedro/.claude/projects/-Users-pedro-Harness/ff733f9c-d28e-4b8b-bedc-9602d0508ee8/tool-results/blmbq5fi1.txt`. I tried to delete it; the permission classifier refused. **Pedro: please delete that file.** Nothing was written outside the worktree.

## Fixes (policy leftovers, all with table tests that fail on the old code)
- Round 6 finding 1: `$(…)` no longer makes a here-doc body "not text" (only process substitutions do; `!` is no longer a reserved owner). `Unit.proc_sub` added.
- Round 6 finding 3: `unwrap` knows `caffeinate`, `arch`, `sandbox-exec`, `flock` (incl. `-c`), `chroot`, `script`, `unbuffer`, `env -P`, `env -S` forms (`-S'…'`, `--split-string=…`, shell-style splitting); `SHELLS` gains `mksh yash rbash posh elvish xonsh`; a command word starting with `$` or a backtick is a shell; `at`, `batch`, `crontab -`, `make -f -`, `script`, `chroot DIR` are stdin script readers (also feed pipe-to-shell).
- M1-03 finding 5: `~/.ssh` members use `symlink_metadata` (`identity_nofollow`).
- Hard links: a regular file under Home with `nlink > 1` that would Allow now Asks (gate).
- Round 6 finding 2 left as a documented limit (see finding 9).

## Attacks tried (about 1,500 command lines, in tables)
Lexer (quotes, ANSI-C, backslash-newline, unicode, NUL, long lines), cwd tracking (`cd -`, `pushd/popd`, CDPATH, subshells, groups, failing cd), flag tables of `rg`, `grep`, `git`, `file`, `date`, `tree`, redirects, path aliases (`/System/Volumes/Data`, case, `//`, `..`), `~user`, globs, real symlink/hard link/dangling/loop/`..`-through-link cases, subject `cwd`, file tools, Unknown subjects, wrappers, remembered exact/prefix/path rules, ledger (replay, stale, cross-thread, cross-incarnation, wrong call, concurrent redeem and allow/deny, expiry paths) over the real SQLite store.

## Findings (ignored tests `#[ignore = "M1-13 finding N"]`)
1. Medium: `cd` inside `( … )` or a pipeline is not followed (`(cd .ssh; cat id_rsa)`). Fix: add the cd target to `seen` before `make_unknown` in `words::apply_cd`.
2. Medium: failing `cd` assumed to succeed (`cd nope; cat .ssh/id_rsa`). Fix: after a `cd`, candidates = new dir plus the previous ones.
3. Low: `pushd /tmp; popd; cd .ssh; …`, `pushd +1`, CDPATH leave cwd unknown.
4. Low: ANSI-C words `$'.ssh/\x69d_rsa'` not decoded.
5. Low: recursive search of a folder holding a credential file (`rg x ~/.aws`, `.docker`, `.kube`, `.config/gh`, `.claude`, `.codex`) only Asks.
6. Medium: credential stores outside the contract list (above). Also `git show HEAD:.npmrc` and `/proc/<pid>/environ` (Linux) allowed.
7. Low: `sh -c -- 'payload'`: `payload_of` returns `--`.
8. Low-Medium: `echo rm -rf ~ | sh` (the `-rf` is dropped as an echo flag), `printf '%s' '…' | sh`.
9. Low (known limit): `bash < <(echo 'rm -rf ~')`, `source <(…)`.
10. Low: a glob that reaches a credential only through a symlink (`cat proj/*/id_ed25519`).
11. Low-Medium: a glob wildcard inside the name of a credential tree or Atrium folder (`.g*/…`, `.local/share/atr*/…`, `rm .config/atr*/config.toml`). The structured fuzz test is ignored for this.
Info: `--exclude-dir=.ssh` and `ls >/dev/null/x` are hard-denied (false hard denies, harmless); a redeem with a hard-denied variant withdraws the ticket (by design); nested JSON in an `Unknown` input is not scanned (can only Ask).

## Test counts (`cargo test -p atrium-core`, all pass)
New files in `crates/atrium-core/tests/`: `adversarial_policy.rs` 7 pass + 10 ignored, `adversarial_ledger.rs` 14, `adversarial_gate.rs` 9 + 2 ignored, `adversarial_fuzz.rs` 2 + 1 ignored (about 12 s), helpers in `common_adversarial/`. Existing `policy` (44) and `ledger` (20) still pass.

## Definition of Done
- [x] Four leftover fixes with tests; round 1-6 tables pass.
- [~] New adversarial files: core done; **daemon and host not started**.
- [x] Fuzz-style tests (xorshift, no `proptest`: `Cargo.toml` not mine), under a minute.
- [x] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` clean. `cargo test -p atrium-core` pass. **`-p atrium-daemon -p atrium-host` not run.**
- [~] This handoff.

## Not done / next step
1. `crates/atrium-daemon/tests/adversarial_*.rs` (use `#[path = "common/mod.rs"] mod common;`): socket roles (App with a wrong identity, Dev off, pre-Hello commands), `Hello` byte by byte across `HELLO_TIMEOUT`, oversize frames, a full 8 MiB control lane (flood unread `ListThreads`), many replaced subscriptions and replacement while live (D-058), a failing store read in replay, recovery with `kill_recorded` `Unverified` (copy `crash_child`, then rewrite `start_time` through `SqliteStore::set_process`), failed closure in recovery, slow-command limit.
2. `crates/atrium-host/tests/adversarial_*.rs` if anything is missing for process identity.
3. Write the change request for finding 6, decide on findings 1, 2, 7, 8 (small fixes in `words.rs` / `shell.rs`; Pedro/orchestrator to approve, outside my brief's list).
4. Run the full DoD commands.
