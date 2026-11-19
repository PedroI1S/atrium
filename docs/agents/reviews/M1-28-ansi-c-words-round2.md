# Review: M1-28-ansi-c-words
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-18 · round 2

## Verdict
**rework**: fail-closed behavior, globbed directory tracking, fish discovery and late-ticket cleanup remain incomplete; markdown still has a quadratic path.

## Findings

Policy results below are source traces with `home = cwd = /h/u`, no resolved paths, and no rules unless specified. “Any” means Home with an unscoped `Any → Allow` rule.

1. [major] **Unreadable ANSI-C words still allow**
   - Where: `crates/atrium-core/src/policy/shell.rs:929`, `out.reasons.push("malformed escape".into());`. `crates/atrium-core/src/policy/engine.rs:935` computes `unknown` without these decoding failures. The test at `crates/atrium-core/tests/M1-28-ansi-c-words.rs:172` explicitly expects `(r"cat $'\xff'", ["Ask", "Ask", "Allow", "Allow"])`.
   - Case: `cat $'\xff'`, `cat $'\ud800'`, or `cat $'unterminated`.
   - Result: Ask in Home; Allow under Any and FullAccess. The invalid-byte identity fix addresses half of round-1 finding 3, but unreadable words still do not fail closed. Brief step 1 and this review’s explicit requirement require Ask when exact decoding is unavailable, including under Any/FullAccess.
   - Fix: propagate a decoding-failure state that prevents automatic allowance, including through nested payloads; correct the tests that currently require Allow.

2. [major] **Custom Atrium roots remain absent from globbed `cd` candidates**
   - Where: `crates/atrium-core/src/policy/words.rs:1307`, `for rel in SENSITIVE_CD_DIRS.iter().chain(SENSITIVE_CD_TREES.iter())`. Its `walk` API at line 1580 receives no supplied Atrium roots.
   - Case: `atrium_data_paths = ["/h/u/atrium-state"]`; input `cd atrium-st?te/dbs; cat atrium.db`, with `/h/u/atrium-state/dbs` matching.
   - Result: Home asks; Any and FullAccess allow. The reader’s candidates omit `/h/u/atrium-state/dbs`. The literal destination produces HardDeny, as required by contract §9’s protection of every supplied Atrium data path and the added finding-11 scope.
   - Fix: pass the supplied protected roots into directory tracking and derive candidates from the shared protection catalogue. Round-1 finding 4 remains partially open; the handoff’s escalation does not close it.

3. [major] **Directory candidates still discard reachable depths**
   - Where: `crates/atrium-core/src/policy/words.rs:1325`, descendant handling runs only inside `for tree in SENSITIVE_CD_TREES`; `.aws` is instead in `SENSITIVE_CD_DIRS` at line 906. For globstars, line 1237 limits synthesis to `for depth in 0..=3`.
   - Case:
     - `cd .a?s/a/b; cat ../../credentials`, where `.a?s/a/b` uniquely matches `.aws/a/b`.
     - `shopt -s globstar; cd .ss?/**/leaf; cat ../../../../../id_rsa`, where the sole matching `leaf` is `.ssh/a/b/c/d/leaf`.
   - Result: Home asks; Any and FullAccess allow. The first retains no `.aws/a/b` candidate. The second synthesizes at most four components below `.ssh`, so five parent traversals escape every synthetic candidate. Bash’s actual reads reach `.aws/credentials` and `.ssh/id_rsa`, respectively. Both require HardDeny under contract §9 and finding 11’s requirement to consider every matching directory. Round-1 finding 5 is therefore only partially fixed.
   - Fix: preserve directory constraints and traversal depth without silently truncating possibilities; fail closed when the representation cannot cover them.

4. [minor] **Fish discovery still corrupts the final PATH entry**
   - Where: `crates/atrium-daemon/src/lib.rs:112`, `"printf '__ATRIUM_PATH_START__'; string join -n : -- $PATH; printf '__ATRIUM_PATH_END__'"`. The fake’s incorrect assumption is documented at `crates/atrium-daemon/tests/M1-28-followups.rs:307`: `"which appends a trailing newline unless `-n` is given"`.
   - Case: fish PATH ends in `/h/u/.local/share/mise/shims`, containing the only fake Claude executable; call `ProbeProviders { preset: Home }`.
   - Result: discovery retains `/h/u/.local/share/mise/shims\n`, which fails the directory-existence filter, so the provider is not discovered. Fish’s `-n` means **exclude empty strings**, not suppress the newline. [Fish documentation](https://fishshell.com/docs/current/cmds/string-join.html) confirms that option’s meaning. D-122 requires discovery from this directory.
   - Fix: emit PATH using `printf` without a trailing newline and correct the fake to reproduce real fish behavior. Round-1 finding 7 remains open.

5. [major] **Late orphan withdrawals can enter a channel that is never drained again**
   - Where: `crates/atrium-agents/src/claude/adapter.rs:418`, `self.tx.send(Internal::Withdraw(ticket))`; direct withdrawal runs only if that send fails. After the final drain at line 1028, line 1036 awaits `self.events_tx.send(ev)` while keeping `internal_rx` open. Codex has the same combination at `codex/adapter.rs:452` (`try_send`) and lines 1227–1229 (final drain followed by `flush_final`).
   - Case: under Careful, a Claude `PreToolUse` for `Read {"file_path":"x"}` inserts an Ask ticket but pauses before returning it. Fill the session’s event channel, send Shutdown, let the 500 ms in-flight wait expire, then release the check while terminal delivery is blocked. Finally drain the events.
   - Result: `OrphanGuard` successfully queues `Internal::Withdraw`, so its direct fallback does not run. The actor never reads that channel again; dropping it discards the withdrawal and leaves the ticket pending. D-122 requires cancellation of these orphaned tickets. The new test releases after 50 ms (`tests/M1-28-followups.rs:782`), before this window.
   - Fix: close the internal receiver before the final drain, ensuring subsequent guards take the direct withdrawal path, or give withdrawal an independent cleanup owner. Test completion after the 500 ms cap with terminal backpressure.

6. [minor] **Malformed link destinations still cause quadratic markdown parsing**
   - Where: `app/src/chat/markdown.ts:287`, `const close = s.indexOf(">", i + 1);`; line 547 repeatedly calls `linkTail` whenever a later `)` exists.
   - Case: `parseInline("[x](<".repeat(20_000) + ")")`. Preset: not applicable.
   - Result: each of the 20,000 `](` sequences scans its remaining suffix for a nonexistent `>`. The final `)` defeats the new `lastParen` shortcut, producing quadratic aggregate scanning. This is a source-traced cost, not a measured duration. N12’s markdown-cost work remains incomplete.
   - Fix: cache failed destination scans or enforce an aggregate scanning budget; add a malformed-link scaling test.

## Suspected, not shown

The supplied brief still restricts `shell.rs` to the word reader and does not list `tests/binary_adapters.rs`. The rework changes substitution scanners (`shell.rs:210`) and that fixture (`binary_adapters.rs:61`). A separate ownership grant was not available in the review inputs.

No additional connection-loss, duplication or healthy-connection-close defect shown.

## Test gaps

- Cargo could not start `cargo test -p atrium-core --test M1-28-ansi-c-words`: the read-only filesystem refused `target/debug/.cargo-build-lock`. No files were changed.
- Add the cases above, escaped ANSI-C redirection targets, and late-return cleanup tests for both adapters.
- Round-1 findings 1, 2, 6 and 9, plus M1-30’s exact `timeout 5 cd ~/.a?s; xxd credentials;` case, are addressed in source/tests.
- No further finding from batch-error logging, damaged-plus-open export coverage, parallel withdrawal deadlines, stderr routing or the M1-21 corrections.
- N10’s test rejects a never-closing daemon at `adversarial_socket.rs:500`, `peer_closed`. The close watch and graceful control-lane draining are distinct; no additional defect found in those paths. N12’s socket and Codex saturation changes were inspected but not executed here.