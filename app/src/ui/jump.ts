// ⌘J and ⌘⇧J: jump to the next or previous thing that needs the user, from anywhere in the
// main window. Tested in jump.test.ts.

import { type Jump, nextJump, prevJump } from "../chat/inbox";
import { daemon } from "../daemon/client";
import { screen, setScreen, showToast } from "../state";
import { focusPopout, model, ownsView } from "../windows";
import { openThread } from "./Sidebar";

/** Where a jump starts: the chat being looked at; nothing from the inbox or settings. */
const current = (): string | null => (screen() === "chat" ? model().mainThread : null);

/**
 * Goes where a jump says. Its chat opens (the request's card is above the composer there). An
 * item with no chat (storage full) opens the inbox. With nothing waiting it shows the inbox,
 * which says so.
 */
function go(jump: Jump): void {
  if (jump.kind === "thread") {
    openThread(jump.threadId);
    // A chat that lives in its own window is shown there.
    if (!ownsView(jump.threadId)) void focusPopout(jump.threadId);
    return;
  }
  if (jump.kind === "none") showToast(daemon().inbox.length ? "This is the only chat that needs you" : "Nothing needs you");
  if (daemon().inbox.length === 0 || jump.kind === "inbox") setScreen("inbox");
}

/** ⌘J: the next item in inbox order (contract §8) after the chat being looked at; from the inbox or settings, the first one. */
export function jumpToNext(): void {
  go(nextJump(daemon().inbox, current()));
}

/** ⌘⇧J: the previous item in inbox order before the chat being looked at; from the inbox or settings, the last one. */
export function jumpToPrev(): void {
  go(prevJump(daemon().inbox, current()));
}
