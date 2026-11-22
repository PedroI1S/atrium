# Handoff: M1-33-nonpolicy-fixes (rework round 1)

Worker: Gemini 4 Neon (D-134) · Branch: `agent/orchestrator/M1-33-nonpolicy-fixes`

## Review findings closed

1. **Finding 1 (Major — bounded inner gate calls):** Fixed in `crates/atrium-agents/src/claude/adapter.rs` and `crates/atrium-agents/src/codex/adapter.rs` (`fcc5438`) using a `MAX_IN_FLIGHT_GATE_CALLS` (`MAX_PENDING_REQUESTS = 64`) `Semaphore` whose `OwnedSemaphorePermit` is held across inner `gate.check`/`gate.question_ticket` execution and any `OrphanGuard` compensation withdrawal (`Internal::Withdraw` -> `drain_withdrawals`, or direct fallback withdrawal), explicitly declining excess requests on the provider wire and feeding deny/declined decisions to the normalizer; pinned by `claude_bounded_inner_gate_calls_decline_excess_and_recover` and `codex_bounded_inner_gate_calls_decline_excess_and_recover` in `crates/atrium-agents/tests/M1-28-followups.rs`.
2. **Finding 2 (Major — image bracket quadratic scanning):** Fixed in `app/src/chat/markdown.ts` (`b033e47`) by tracking active non-image `[` openers in a dedicated `activeLinks` stack so closing a link visits each `[` at most once and never scans `![` image openers; pinned by `image openers before many links scale linearly (M1-33 finding 2)` in `app/src/chat/markdown.test.ts`.
3. **Finding 3 (Major — distinct backtick run lengths rescanning suffixes):** Fixed in `app/src/chat/markdown.ts` (`b033e47`) by lazily indexing maximal backtick runs by length in a single linear pass with monotonic per-length cursors; pinned by `distinct unmatched backtick run lengths scale linearly (M1-33 finding 3)` in `app/src/chat/markdown.test.ts`.
4. **Finding 4 (Major — inline nesting stack overflow):** Fixed in `app/src/chat/markdown.ts` (`b033e47`) by capping inline container depth at `MAX_INLINE_DEPTH = 16` and flattening excess nesting in O(1) via singly-linked `InlineSeq` segments in `processEmphasis` (plus making `plainText` iterative); pinned by `deep inline emphasis nesting flattens without overflowing the stack (M1-33 finding 4)` in `app/src/chat/markdown.test.ts`.
5. **Finding 5 (Minor — malformed link prefixes disabling later valid links):** Fixed in `app/src/chat/markdown.ts` (`b033e47`) by replacing the shared scan budget in `linkTail` with monotonic scan-end pointers (`angleEnd`, `parenTitleEnd`, `dquoteTitleEnd`, `squoteTitleEnd`), cached failed post-dest/post-title indices, and deferred destination slicing; pinned by `malformed link prefixes never disable later valid links (M1-33 finding 5)` in `app/src/chat/markdown.test.ts`.
6. **Finding 6 (Minor — login-shell timeout leaving descendants and reader threads running):** Fixed in `crates/atrium-daemon/src/lib.rs` (`3df4985`) by spawning `$SHELL` in its own process group (`CommandExt::process_group(0)`), supervising stdout directly on the calling thread via a `UnixStream::pair()` with bounded read timeouts (no detached reader thread), peeking exit via `rustix::process::waitid(..., NOWAIT)` so `pgid` stays pinned, and killing the entire process group + reaping + draining via `LoginShellChild` on every return path; pinned by `discover_login_shell_path_kills_process_group_descendants_on_timeout` and `discover_login_shell_path_cleans_up_background_descendant_retaining_stdout` in `crates/atrium-daemon/tests/M1-28-followups.rs`.

## Verification performed (per D-123)

- Ran `cargo fmt --all` (clean).
- Ran `cargo check -p atrium-agents --tests` and `cargo check -p atrium-daemon --tests` (both passed with 0 errors and 0 warnings).

## Not done (left to orchestrator verification per D-123)

- Did not run `cargo test`, `cargo clippy`, or `pnpm` (prohibited for workers by D-123). Exact commands for verification:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace --no-fail-fast`
  - `pnpm -C app typecheck`
  - `pnpm -C app test`
