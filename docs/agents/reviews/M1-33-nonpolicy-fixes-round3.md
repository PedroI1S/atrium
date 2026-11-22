# Review: M1-33-nonpolicy-fixes
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-22 · round 3

## Verdict

**rework**

Round-2 findings **1–3 are closed**. I found no dependency on the omitted policy work and no new correctness issue in the `conn.rs`/`server.rs` changes. Two new findings remain.

## Findings

1. **Major — [app/src/chat/markdown.ts:820](/home/rodrigo/atrium-wt/M1-33-nonpolicy-fixes/app/src/chat/markdown.ts:820): soft line breaks still cause quadratic parsing.**  
   `parseMarkdown("a\n".repeat(n))` repeatedly inspects the growing `buf` with `endsWith` and indexed trimming, forcing accumulated string concatenations to flatten. Read-only probes took approximately **72 ms / 1.16 s / 10.2 s** for **80/160/320 KB**; equivalent single-line input took 1–2 ms. A folded multiline delta can reach this path despite the splitter’s soft limit. **Fix:** track trailing whitespace independently and accumulate text segments without inspecting the growing buffer; join once.

2. **Major — [claude/adapter.rs:1150](/home/rodrigo/atrium-wt/M1-33-nonpolicy-fixes/crates/atrium-agents/src/claude/adapter.rs:1150), [codex/adapter.rs:2099](/home/rodrigo/atrium-wt/M1-33-nonpolicy-fixes/crates/atrium-agents/src/codex/adapter.rs:2099): delivered-ticket withdrawals bypass the bound on detached cleanup.**  
   Successful delivery disarms `OrphanGuard` and releases its admission permit. Later cancellation uses `queue_withdraw(ticket)`, which supplies `None`; the new drain then detaches that withdrawal on timeout without reserving capacity. Concrete case: wrap a real `SessionGate` with blocked adapter withdrawals, repeatedly deliver questions and decline them through the daemon. The daemon frees each ledger slot before sending `DeclineQuestion`, allowing indefinitely many blocked withdrawal tasks while admission permits remain available. **Fix:** retain admission ownership through delivered-ticket cleanup, or provide bounded cleanup admission that also limits new ticket creation while preserving unfinished withdrawals.

## Test gaps

- Add a multiline-paragraph scaling regression, including input delivered through `BlockSplitter` as one folded delta.
- For both adapters, test blocked withdrawals **after normal ticket delivery**, with ledger slots freed independently; assert outstanding cleanup remains bounded and eventually completes.
- Rerun workspace verification on the integrated tree containing M1-34; the supplied branch log still records N9.

No files were modified. I used the supplied verification results and read-only, in-memory parser probes; I did not run test suites.