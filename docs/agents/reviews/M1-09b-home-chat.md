# Review: M1-09b-home-chat
Reviewer: Gemini 4 Hydrogen (read-only) · 2026-11-14
## Verdict
**approve with changes**: §12 content security, approval/question safety, inbox/⌘J ordering, transcript eviction/keying, and all 28 Linux screenshots hold up cleanly, with two minor hardening items and the handoff's macOS/idle-memory items owed.

## Findings
1. [minor] **Single-line fenced code blocks are not bounded by `SOFT_BLOCK_CHARS`**
   - Where: `app/src/chat/blocks.ts:168-178` (`BlockSplitter.softCut`):
     `if (this.fence) { if (this.fence.lines < SOFT_FENCE_LINES) return; ... }`
   - Case: A streaming message opens a fenced code block (```` ``` ````) and emits a single 1 MiB line with no newlines.
   - Result: `softCut` only checks `this.fence.lines < SOFT_FENCE_LINES` (200 lines), so the 1 MiB block stays in one `BlockText` row and is re-rendered via `el.replaceChildren(...)` on every streaming batch until a newline or closing fence arrives (paragraphs > `SOFT_BLOCK_CHARS` = 4096 chars are split at `blocks.ts:180-205`).
   - Fix: In `softCut()`, if `this.fence` is set and `this.openText().length > SOFT_BLOCK_CHARS`, close the current fence chunk and reopen with `this.fence.opener`, the same as when `SOFT_FENCE_LINES` is reached.

2. [nit] **`check_reveal` does not check for control characters in Rust**
   - Where: `app/src-tauri/src/links.rs:79-83` (`check_reveal`):
     `if !path.is_absolute() || path.as_os_str().len() > MAX_LINK_BYTES { return Err(RevealError::NotAbsolute); }`
   - Case: A caller invokes `reveal_path` directly with a path containing an ASCII control character (e.g. `\n`) that happens to exist inside `$HOME`.
   - Result: Unlike `links::check` (`links.rs:30`, `if input.chars().any(char::is_control)`), `check_reveal` does not reject control characters itself (relying on `links.ts:44` `UNSAFE_CHARS` in the webview, `canonicalize` rejecting `\0`, and `Command::arg` avoiding a shell).
   - Fix: Add `if path.chars().any(char::is_control) { return Err(RevealError::NotAbsolute); }` at the top of `check_reveal` so Rust enforces the same character hygiene as `open_link`.

3. [nit] **Escape on the `Always…` button denies while the menu is open**
   - Where: `app/src/ui/RequestCard.tsx:65, 86` (`ApprovalCard`):
     `else if (e.key === "Escape") { e.preventDefault(); deny(); }` and `<div class="menu" ... onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); setMenu(false); } }}>`
   - Case: User clicks or focuses the `Always…` button to open the remember menu, leaves focus on the `Always…` button (outside `.menu`), and presses `Escape` expecting to close the menu.
   - Result: The keydown event does not pass through `.menu`, bubbles to `.ask`, and calls `deny()` instead of closing the menu.
   - Fix: In `.ask`'s `onKeyDown`, if `menu()` is true and `e.key === "Escape"`, call `e.preventDefault(); setMenu(false);` and return before `deny()`.

## Screenshots
- `chat-approval-refused-light.png`, `chat-approval-refused-dark.png`: header pill still reads "Needs approval" because the seeded stale request sits on a `NeedsApproval` daemon summary (noted in handoff).
- `chat-streaming-light.png`, `chat-streaming-dark.png`: inline code inside parentheses `( 4c1e9aa )` has chip padding that reads slightly like spaces around it (noted in handoff).
- All other 24 screenshots (`chat-approval-*`, `chat-question-*`, `inbox*`, `incompatible`, `main-*`, `panel-minimized`, `popout`, `settings-*`, `sidebar-hidden`): no remark.

## Suspected, not shown
- None.

## Tried and held
- **§12 rendered content security**: `rg` across `app/src/` confirms zero `innerHTML`, `insertAdjacentHTML`, `outerHTML`, `DOMParser`, `createContextualFragment`, or `document.write` calls. `render.ts` only sets `a.href` for `http:`/`https:`/`mailto:` (`links.ts:47-52`), `img.src` for validated base64 raster `data:` URLs ≤ `MAX_INLINE_IMAGE_BYTES` (`links.ts:75-87`) or clicked `https:` non-SVG URLs (`links.ts:89-93`, `render.ts:80-100`). `javascript:`, `data:`, `vbscript:`, relative, and protocol-relative URLs return `{ kind: "none" }` and render as `<span class="md-nolink">` with `title`. `file:` links have no `href`, call `revealPath` → `reveal_path`, which canonicalizes against `$HOME` (`lib.rs:640-647`, `links.rs:79-120`) and reveals without opening/executing. Deeply nested lists/quotes stop at `MAX_DEPTH = 12` (`markdown.ts:36`).
- **Approval and question card safety**: Stale approvals (`turn_completed`, `session_ended`, `newIncarnation`, or daemon `expired` refusal) set `canAct(state)` to `false` (`requests.ts:46-62`), hiding action buttons and blocking `onKeyDown` (`RequestCard.tsx:61, 76`), and `InboxCard` only acts on `transcript.pending()` (`InboxCard.tsx:114`). `ApprovalCard` never steals focus from `<textarea>`/`<input>` (`RequestCard.tsx:38, 59`), ignores `Enter`/`Escape` for `ARM_AFTER_MS = 600` ms (`RequestCard.tsx:36, 62`), and lets `Enter` on a focused `<button>` activate that button (`RequestCard.tsx:64`). `rememberChoices` returns `[]` when `!request.rememberable` and words all three scopes with `"in any folder"` (`requests.ts:67-75`). `subjectOf` displays exact command/paths (`requests.ts:82-100`).
- **Inbox and ⌘J**: `REASON_RANK` and `rankInbox` (`inbox.ts:9-14`) match §8; `<For>` keys by group name and `itemKey` (`InboxCard.tsx:141-158`) so updates don't drop or duplicate items. `nextJump` and `jumpToNext` (`inbox.ts:50-61`, `jump.ts:15-26`) skip the currently viewed thread, open the inbox on `storage_full`, and toast + open inbox when empty (or toast "This is the only chat that needs you" when only the current chat is waiting).
- **Memory, streaming, and bench outputs**: `Transcript` enforces `MAX_LOADED_TRANSCRIPT_BYTES` via `evictOldest()`/`trimNewest()` (`transcript.ts:498-530`) and pages back via `pageOlder()` (`threads.ts:113-127`, `Transcript.tsx:250-257`). Row-keyed `<For each={keys()}>` (`Transcript.tsx:277-305`) mounts/unmounts cleanly without duplicating keys, and `createEffect` on `[structure(), virt.getTotalSize()]` (`Transcript.tsx:265-267`) preserves tail-following. All numbers in the handoff match `.orchestration/stream-bench-3runs.txt` and `.orchestration/startup-bench.txt` line by line.
- **Owned paths**: `git diff --stat dev...HEAD` touches only `app/src/**`, `app/scripts/**`, `app/src-tauri/src/{lib,links}.rs` (`reveal_path`), `app/package.json` (`happy-dom`), `app/pnpm-lock.yaml`, `docs/agents/handoffs/M1-09b-home-chat.md`, and `docs/agents/change-requests.md` (CR-13..CR-15). The M1-09a files `Sidebar.tsx` and `index.tsx` are noted in the handoff.
- **Rust tests**: `cargo test -p atrium-app` ran and passed (44 unit tests in `atrium_app`, 2 tests in `delivery_shapes`).

## Test gaps
- `pnpm test` (260 vitest tests) was not re-run in this read-only review; frontend test results are taken from the handoff.
- Live end-to-end approval/question roundtrip against a real provider (M1-07) or fake-agent option (CR-13) is not yet exercised outside recorded `ServerMessage` fixtures (`approvals.json`) and seeded scenes.
- macOS screenshots, macOS `startup_bench.py`, and a 60 Hz §14 `stream_bench.py` run remain owed to a macOS shift; Linux idle private PSS (212.5–216.3 MiB) is over 150 MiB pending an M1-09a baseline on the same host.
