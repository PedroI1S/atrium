# Review: M1-33-nonpolicy-fixes
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-22 · round 2

## Verdict

**rework**

Round-1 status: **1 partly closed; 2–6 closed.** I found no dependency on policy changes left on the source branch. No files were modified.

## Findings

1. **Major — `crates/atrium-agents/src/claude/adapter.rs:1143`, `crates/atrium-agents/src/codex/adapter.rs:2088`: withdrawal timeout abandons compensation.**  
   Inner checks now have admission limits, but `drain_withdrawals` drops its local `JoinSet` when the deadline expires, aborting unfinished withdrawals and releasing their permits. Concrete case: cancel a check before its `Ask` is delivered, then let it return a ticket; a withdrawal that waits longer than the deadline before calling `SessionGate::withdraw` is aborted permanently. Its invisible ledger row remains pending and consumes a slot. **Fix:** preserve unfinished compensation and its permit until completion; bound the actor’s waiting time without aborting the cleanup. This leaves round-1 finding 1 partly closed.

2. **Major — `app/src/chat/markdown.ts:432`: bare-URL trimming remains quadratic.**  
   `parseInline("https://e.co/" + ")".repeat(n))` recounts parentheses across the remaining URL for every removed character. Read-only probes with the real classifiers took approximately **113/422/1,574 ms** for **20k/40k/80k** closing parentheses. Larger agent messages can freeze synchronous rendering. **Fix:** count parentheses once, then trim using counters and an end index.

3. **Major — `app/src/chat/markdown.ts:43`, `app/src/chat/markdown.ts:49`: block-recognition regexes retain quadratic backtracking.**  
   Concrete cases are `parseMarkdown("# x" + " ".repeat(n) + "x")` and `parseMarkdown("a|b\n---" + " ".repeat(n) + "x")`. Heading probes took approximately **12/46/173 ms** for **5k/10k/20k** spaces; table-delimiter probes showed the same quadratic growth. **Fix:** recognize headings and table delimiters with deterministic scans, removing overlapping whitespace/backtracking paths.

Findings 2–3 also exist on dev, but remain within this task’s parser-hardening area under D-120.

## Test gaps

- Add delayed-withdrawal regressions for both adapters: exceed the wait deadline, verify capacity remains reserved, release the withdrawal, and assert the orphan ledger row is cancelled.
- Add scaling regressions for unmatched URL parentheses and both block-recognition cases.
- The existing admission tests exercise blocked checks followed by prompt withdrawals; they miss finding 1.
- After M1-34 merges, rerun workspace verification on the integrated tree to confirm the known N9 failure disappears.

I reviewed the supplied verification output and ran only read-only, in-memory parser probes.