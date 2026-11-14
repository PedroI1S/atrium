# Review: M1-09b-home-chat, round 2 (review fixes)
Reviewer: Gemini 4 Hydrogen (read-only) · 2026-11-14

## Verdict
**approve**: All three round-1 findings are closed cleanly with no regressions found in `softCut`, `check_reveal`, or `RequestCard.tsx` (macOS screenshots/benches and idle-memory baseline from round 1 remain owed separately).

## Round 1 findings
1. **[minor] Single-line fenced code blocks are not bounded by `SOFT_BLOCK_CHARS`** — **closed**.
   - `app/src/chat/blocks.ts:18` adds `SOFT_FENCE_CHARS = 4 * SOFT_BLOCK_CHARS` (16,384).
   - `app/src/chat/blocks.ts:169-193` (`BlockSplitter.softCut`) now gates on `f.lines < SOFT_FENCE_LINES && this.openText().length <= SOFT_FENCE_CHARS` (`blocks.ts:172`), closes any accumulated `this.body !== reopen` (`blocks.ts:175-180`), and splits `this.partial` in `while (this.partial.length > SOFT_FENCE_CHARS)` (`blocks.ts:183-191`) without splitting UTF-16 surrogate pairs (`blocks.ts:185-187`).
   - Covered by `app/src/chat/transcript.test.ts:108-137` (1 MiB single-line emoji fence streamed and whole, plus 150 lines × 1,000 chars).

2. **[nit] `check_reveal` does not check for control characters in Rust** — **closed**.
   - `app/src-tauri/src/links.rs:82-84` adds `if path.chars().any(char::is_control) { return Err(RevealError::NotAbsolute); }` at the top of `check_reveal`, matching `links::check` (`links.rs:30`).
   - Covered by `app/src-tauri/src/links.rs:144-157` (`a_path_with_a_control_character_is_refused_even_when_it_exists`).

3. **[nit] Escape on the `Always…` button denies while the menu is open** — **closed**.
   - `app/src/ui/RequestCard.tsx:62-63` intercepts `e.key === "Escape" && menu()` in `.ask`'s `onKeyDown` before the `ARM_AFTER_MS` check and `deny()` branch (`RequestCard.tsx:64-67`), calling `e.preventDefault(); setMenu(false); return;`.

## New findings
None.

## Tried and held
- **`softCut` on open fences (`app/src/chat/blocks.ts:169-193`, `app/src/chat/markdown.ts:103-119`, `app/src/ui/Transcript.tsx:63`)**:
  - Cannot emit an empty block (`this.body !== reopen` only runs when at least one complete line sits after `reopen`, and `while` slices `at >= SOFT_FENCE_CHARS - 1 > 0`).
  - Preserves all characters without loss or duplication, reduces `this.partial.length` by at least `SOFT_FENCE_CHARS - 1` per iteration (cannot loop), and backs up 1 code unit when `at - 1` is a high surrogate.
  - Every emitted closed part starts with `f.opener + "\n"` and ends with `f.marker + "\n"`, and the trailing open part starts with `f.opener + "\n"`, so `parseMarkdown` (`markdown.ts:103-119`) renders every part as a `{ t: "code" }` block and `BlockRow` (`Transcript.tsx:63`) applies `classList={{ cont: props.block.cont, ... }}`.
  - When a closing fence arrives right after a cut, `feed()`'s line loop processes `this.line(line)` with `this.fence` still set and closes the trailing part normally.
- **`check_reveal` (`app/src-tauri/src/links.rs:79-97`)**:
  - `char::is_control` rejects only Unicode `Cc` control characters (`U+0000..=U+001F`, `U+007F..=U+009F`); spaces (`' '`) and non-ASCII printable characters are allowed.
- **`RequestCard.tsx` Escape handling (`app/src/ui/RequestCard.tsx:60-68, 87-88`)**:
  - With `menu()` closed, line 63 does not match and `Escape` (after `ARM_AFTER_MS` and `!e.repeat`) reaches line 67 and calls `deny()`.
  - With `menu()` open, `Escape` on the `Always…` button or elsewhere in `.ask` hits line 63 (`return`s without calling `deny()`), and `Escape` inside `.menu` hits line 88 (`e.stopPropagation(); setMenu(false);`), so `Escape` never closes the menu and denies in one press.
- **Tests run at HEAD (`8e7dffd`)**:
  - `cargo test -p atrium-app`: 45 passed in `atrium_app`, 2 passed in `delivery_shapes`.
  - `pnpm --dir app test`: 13 test files, 261 tests passed.
