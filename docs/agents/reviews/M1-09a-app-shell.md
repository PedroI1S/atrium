# Review: M1-09a-app-shell
Reviewer: Gemini 3.8 Flash (read-only) · 2026-11-12

## Verdict
**approve** — The implementation cleanly satisfies all contract and Definition of Done requirements across security, window coordination, token architecture, and macOS/Linux ergonomics with zero production panics.

## Findings

1. [nit] **`replace_daemon` initiates immediate reconnect while daemon may be waiting for turns to finish**
   - **Where:** `app/src-tauri/src/lib.rs:619`
   - **Problem:** When `replace_daemon` is invoked with `force = false` (`upgrade::Stop::WhenIdle`), it sends `SIGTERM` to the older daemon and immediately invokes `state.bridge.client().reconnect()`. If the older daemon still has active turns running, it will not exit immediately, causing the initial reconnection attempt to encounter the `Incompatible` response again before the older daemon eventually drains its turns and shuts down.
   - **Fix:** The frontend in `ConnectionBanner.tsx` already anticipates this by entering the `"Waiting for its running turns to finish…"` state and exposing the "Interrupt and restart now" action. A code comment in `replace_daemon` clarifying that the client's connection backoff loop resolves once the old daemon terminates would document this intentional flow.

2. [nit] **Uncommitted working-tree improvements from Opus 5.5 unsafe audit**
   - **Where:** `app/src-tauri/src/macos.rs:105` and `docs/agents/reviews/M1-09a-app-shell-unsafe.md`
   - **Problem:** The working tree contains the safety fix for `apply_to_new_window` (`objc2_foundation::MainThreadMarker::new().is_some()` guard) and updated documentation from Opus 5.5's unsafe audit (`docs/agents/reviews/M1-09a-app-shell-unsafe.md`), but they have not yet been committed to the task branch.
   - **Fix:** Stage and include `app/src-tauri/src/macos.rs` and `docs/agents/reviews/M1-09a-app-shell-unsafe.md` when preparing the branch for merge into `dev`.
