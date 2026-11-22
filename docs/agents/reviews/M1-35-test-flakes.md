## Verdict

**rework**

N13’s inherited `SIG_IGN` diagnosis fits the failure and background-job reproduction; the daemon installs explicit handlers before startup. The `pre_exec` hook contains no apparent allocation or locking, and `signal` is [async-signal-safe](https://man7.org/linux/man-pages/man7/signal-safety.7.html).

N14’s cause is supported by `claude-fake.rs:154` writing directly to a file the old helper immediately read. N15’s inherited-writer mechanism is credible, and the lock addresses it, although the supplied evidence does not capture the original failing spawn’s errno.

All changed paths are owned. No files were modified during this review.

## Findings

1. **minor — [crates/atrium-agents/tests/claude_adapter.rs:1893](/home/rodrigo/atrium-wt/M1-35-test-flakes/crates/atrium-agents/tests/claude_adapter.rs:1893): Completed malformed records are silently discarded.**  
   With `invalid\n` followed by valid matching responses, `wait_response` now succeeds; previously parsing panicked. This weakens validation beyond ignoring an unfinished trailing line. **Fix:** retain the newline boundary handling, but require every completed line to parse successfully, with a useful diagnostic.

2. **minor — [crates/atrium-daemon/src/lib.rs:248](/home/rodrigo/atrium-wt/M1-35-test-flakes/crates/atrium-daemon/src/lib.rs:248): A retry can launch the shell after the deadline.**  
   When the final backoff consumes the remaining budget, execution loops directly into `cmd.spawn()` without checking time again. If the writer closes during that sleep, shell startup executes after discovery has expired. **Fix:** check the deadline immediately before each retry and return `None` when exhausted.

3. **minor — [crates/atrium-daemon/tests/M1-28-followups.rs:217](/home/rodrigo/atrium-wt/M1-35-test-flakes/crates/atrium-daemon/tests/M1-28-followups.rs:217): The ETXTBSY recovery regression depends on scheduling.**  
   If the test thread is descheduled for over 50 ms before entering discovery, the releaser closes the writer before the first spawn. The test then passes with the original non-retrying implementation. The permanently busy case also passes with that implementation because it checks only `None` and an upper time bound. **Fix:** synchronize release with an observed ETXTBSY attempt through a test seam, or inject deterministic busy/success outcomes and time progression.

## Test gaps

- Add coverage for malformed **completed** JSONL records and expiry immediately before a spawn retry.
- Demonstrate that the ETXTBSY regression fails when recovery is removed.
- The [verification record](/home/rodrigo/atrium/.orchestration/verify/M1-35-test-flakes.txt:11) documents 30 repeats for `claude_adapter`, but does not list `codex_adapter` repeats.
- The supplied clean checks and three loaded workspace runs are strong evidence, but do not resolve the cases above. I ran no tests during this read-only review.