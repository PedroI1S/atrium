# Handoff: M1-03-policy

Model: Sonnet 5.5 · Status: done

## Done

All of `crates/atrium-core/src/policy/**` (owned paths only, plus the two `Cargo.toml` dependency lines `sha2` and `globset`, each tagged `# M1-03`, and the resulting `Cargo.lock` lines).

- `shell.rs`: a tolerant shell lexer (own code, not `shell-words`: it needs redirects, assignments, substitutions, here-docs and quoting state). `parse_command` is a thin view over it. Rules and Home match only what it reports as clean.
- `words.rs`: the words of a command that name paths (arguments, `--flag=value`, `-Xvalue`, `key=value`, assignment values, redirect targets, brace expansions), `cd`/`pushd`/`popd`/`cd -` tracking, unknown-cwd handling.
- `deny.rs`: every `HARD_DENY_DEFAULTS` entry, on every segment (through assignments and `env`/`command`/`exec`/`nice`/`timeout`/`busybox`/`xargs`/`nohup`/`time` wrappers), inside `$(…)`, backticks, `<(…)`, `sh -c`/`eval` payloads, `find -exec`, here-doc bodies, and the raw text. Shell-style globs (no leading-dot match) are tested against sensitive paths.
- `engine.rs`: `DefaultPolicy` (hard deny, thread, Home, global rules with deny before allow, then the preset); Home's read-only list with flag guards (including long-option abbreviations), `only`, and `rg`/`grep` search-root analysis.
- `ledger.rs`: `Ledger` (every transition in `lib.rs`, `TooMany` under a lock, stale incarnation, rule created through `approve_with_rule`).
- `gate.rs`: `SessionGate`, `RulesSource`, `StoreRules`, `SessionGateConfig`, `fs_resolve`.
- `mod.rs`: `canonical_input`, `input_sha256`, `parse_command`, the struct definitions.

Public additions (all inside `policy`, no contract item changed): `DefaultPolicy::new`, `SessionGate::new`, `SessionGate::set_preset`, `SessionGateConfig`, `RulesSource`, `StoreRules`, `fs_resolve`. `Ledger::new` is unchanged.

## Not done

- No change request needed.
- Adversarial testing beyond the DoD is M1-13's.
- The non-Home presets are placeholders (Careful asks everything except subject `None`; Normal and Trusted behave like Home; FullAccess allows after hard deny and rules). Contract §9 says M1 uses Home only.

## Definition of Done results

```
cargo fmt --all --check                          -> clean
cargo clippy --workspace --all-targets -- -D warnings -> clean
cargo test -p atrium-core --test ledger          -> 18 passed, 0 ignored
cargo test -p atrium-core --test policy          -> 33 passed, 0 ignored
cargo test -p atrium-core --lib                  -> 0 tests (nothing in the lib yet), ok
```

- The table test is `hard_deny_table` (about 230 lines) plus `hard_deny_evasion_table` (about 55 more). Between them: every hard-deny entry and every D-050 addition, quoting, `env`/`command`/`exec`/`\rm`/`/bin/rm`, `sh -c`, `eval`, `$(…)`, backticks, `<(…)`, `&`, newlines, here-docs, `>` outside the cwd, `~`/`..`/`$HOME`/`${HOME}` paths, braces and globs, symlinks into credential folders (real filesystem, in `path_rules_resolve_symlinks_and_relative_paths` and `redeem_rechecks_hard_deny_after_symlink_swap`), and every flag guard of `HOME_READONLY_COMMANDS`.
- Every finding named in the brief is a test: `ls > ~/.zshrc`, `rg token`, `LD_PRELOAD=… ls`, `cat .npmrc`, `cd .ssh && cat id_ed25519`, `file -f.npmrc`, `grep -f.ssh/id_ed25519 x sub/a`, `rg -e sub`, `dd if=link`, `cd tmp && cd && rm -rf .`, `cd tmp && cd - && cat .npmrc`.
- Concurrency: `concurrent_resolve_and_redeem_have_exactly_one_winner` (50 rounds: two resolvers, then 8 redeemers, exactly one wins), `withdraw_racing_redeem_never_allows_a_cancelled_request`, `concurrent_opens_cannot_pass_the_limit`.
- `engine_never_panics_on_garbage`: 20k random lines through `evaluate` and `parse_command`, plus a 100k-argument line and 5000-deep `$(` nesting (runs in about 2 s).

## Gotchas

- **`resolved_paths` is aligned, not just a set.** `words::walk` enumerates the words of a call in a fixed order, and for each word one resolution per candidate cwd (one when the cwd is known). The gate resolves against the filesystem; the engine redoes it lexically and, when `resolved_paths.len()` matches, pairs the gate's paths with its words (symlink-aware delete checks). If the length differs (a caller that builds `PolicyInput` by hand, as the tests do) the engine falls back to its own lexical resolution plus the flat list for credential checks. M1-13 and the daemon should build inputs through `SessionGate`, not by hand, when they want symlinks honoured.
- **`redeem` always re-evaluates** the policy (minus the rule this approval created), not only when the revision or preset changed. It is a superset of the contract and equivalent, but it means a deny rule present at `check` time and unchanged would also have denied. A refusal withdraws the ticket (it can't be redeemed later). The gate remembers each ticket's `PolicyContext` in memory; a ticket from another gate is refused.
- **A `Mismatch` in the ledger's `redeem` does not consume the grant** (a wrong-input attempt can't burn the user's approval, and can't use it either).
- Ledger error mapping: `cancelled` and `expired` rows give `Expired`; any other final state gives `AlreadyResolved`; `redeem` of a still-`pending` row gives `Unknown`. A request id is `req_<start-nanos hex>_<counter>`; a rule's id is `rule_` plus 10 bytes of SHA-256 over thread, incarnation and request id (one rule per request).
- `parse_command` returns `Unparseable` for anything with a glob, `$`, braces, `~user`, here-doc or here-string, backgrounding, subshell or a compound command (`if`, `for`, `{`, …). A single-quoted `$` also counts (the lexer doesn't keep quote state per word, but reports expansion only for unquoted or double-quoted `$`; `'$X'` is literal and stays parseable).
- `serde_json` has `preserve_order` on: `canonical_input` sorts keys itself. Numbers print as serde_json prints them (`1` and `1.0` hash differently on purpose). The prefix is `<len>:<kind>\n<len>:<name>\n<json>`.
- Real-filesystem tests use `std::env::temp_dir()` with unique names (no `tempfile` dependency) and clean up.

## Stricter or different than the contract text (Pedro, D-013: please confirm)

All of these err toward Ask or Hard, never toward Allow.

1. **Hard deny reaches further than a literal reading in a few places.**
   - Any word of any command that resolves to a credential path is blocked, so `ssh -i ~/.ssh/id_rsa host` and `git` calls that name a key are blocked too.
   - The `~/.ssh` folder itself may be listed (`ls`), `chmod`ed or `mkdir`ed, but searched, copied, tarred or written into only by being blocked.
   - Writing to `~/.ssh/config` or `authorized_keys` is blocked (contract), reading them is allowed.
   - A here-doc body or any raw text that mentions a home-anchored credential path or an Atrium data path is blocked (`bash <<EOF … ~/.npmrc … EOF`), but plain prose in a file tool's content is not scanned.
   - MCP and unknown-subject inputs: any string value that looks like a single path is checked. For `Unknown`, every string is also lexed as a command line.
2. **`find … -delete`** with a filter (`-name x -delete`) at the cwd itself is not blocked (it only removes matches); with no filter, or rooted at `~`, `/` or outside the folder, it is. `rm -rf <glob>` is blocked when the glob's parent is `~` or an ancestor (`rm -rf ~/*`, `rm -rf *` in Home).
3. **Force-push**: `--mirror`, `--all`, `--tags` with a force flag, and any refspec that isn't a plain branch name (`refs/tags/…`, `HEAD`, `src:`, globs) count as protected destinations.
4. **Home recursive search** (`rg`, `grep -r`, and the Paths-kind search tool) asks when the root is `~`, an ancestor of `~`, outside `~`, or an ancestor of a credential or Atrium location (`~/.config`, `~/Library`), not only when it is `~` itself. Searches rooted exactly in a credential location are blocked.

## Command shapes I chose to Ask rather than match (Home)

- `cd`, `pushd`, `popd` (not in `HOME_READONLY_COMMANDS`; `cd sub && git status` asks, though the hard-deny list still follows the `cd`).
- `echo`, `env`, `sh -c`, `eval`, `xargs`, `command`, `exec`, `time`, `nohup`, absolute or relative command names (`/bin/ls`, `./ls`), anything not in the list.
- `git` with options before the subcommand (`git -C dir status`, `git --no-pager log`), `git -c`, `--config-env`, `--exec-path`, other subcommands, `git branch` with any positional argument.
- Any command with: a leading `NAME=value`, a redirect other than `>/dev/null` and fd duplication (including every input redirect), a newline, here-doc, here-string, subshell, compound-command keyword, backgrounding, any `$` expansion or substitution (`$HOME` included, even though hard deny understands `$HOME`/`${HOME}`/`$PWD`), unquoted glob, brace expansion, or `~user`.
- `rg` or `grep -r` with no explicit root, a root of `.`, `~`, `..`, `-` (stdin), an absolute path outside `~`, or an ancestor of a credential or Atrium folder; and any flag the matcher doesn't know (`rg -L`/`--follow`, `--rec` and other abbreviations count as recursive). `grep` asks on unknown flags even when not recursive.
- `-f`/`--file`, `-z`, `--pre`, `--pre-glob`, `--hostname-bin` (rg); `-f`, `--file` (grep); `-C`, `-m`, `-f` and long forms (file); `-s`, `--set` (date); `-o`, `--output`, `-R` (tree); `--output`, `--ext-diff`, `-c` and abbreviations of the long ones (git), by the guard lists in `atrium-protocol`.
- `rm`, `touch`, `cp`, `mv`, `chmod`, `mkdir`, `curl`, `wget`, `dd` (not to a device), `sudo`-less package managers, everything that edits or runs code.
- File tools: `Edit`, `Delete`, `Move` and any `Other` kind with paths; `Fetch`, `Mcp`, `Unknown` subjects; a `Read` or `Search` tool whose subject isn't `Paths`.

## Known limits (not matched by design or by effort)

- Hard deny sees only what a call names: `python -c "shutil.rmtree('/')"`, `node -e`, scripts that run later, `curl … > f && sh f`, `git clean -fdx`, `rsync --delete`, `xargs rm -rf` (targets on stdin), shell variables other than `$HOME`, `${HOME}` and `$PWD`, and ANSI-C `$'\x2e…'` words are not resolved (a line using them asks, but is not blocked).
- Writes to Atrium's protected paths are recognised for redirects, `of=`, `sed -i`/`perl -i` and a fixed list of writer commands (`rm mv cp tee touch chmod chown chgrp ln truncate install mkdir rmdir rsync unlink shred patch ed ex wipefs blkdiscard`); other programs that write are only caught if they name the path in a redirect.
- Path comparison is ASCII-case-insensitive (macOS), so a Linux user with a `~/.SSH` folder would see a false positive.
- Symlink swaps between `check` and `redeem` are re-checked, but a swap after `redeem` returns is the adapter's race (TOCTOU), as the contract says.

## Follow-ups

- M1-05 (daemon): build a `SessionGate` per incarnation with `StoreRules(store)`, `Ledger::new(store.requests())`, the resolved Atrium paths, and call `expire_*` from stored events as §5 says. Call `set_preset` if a thread's preset changes.
- M1-13: the places most worth attacking are `shell::lex` (quote and expansion edge cases), `words::apply_cd` and `deny::delete_blocked` (cwd tracking), the `rg`/`grep` flag tables in `engine.rs`, and `Sensitive::hit` (the `~/.ssh` name rules).
- If Pedro wants `cd <literal>` or `echo` auto-allowed in Home, it is a one-line change to `HOME_READONLY_COMMANDS` (his list, D-013).
