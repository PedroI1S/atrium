// ⌘J: jump to the next thing that needs the user, from anywhere in the main window.

import { nextJump } from "../chat/inbox";
import { daemon } from "../daemon/client";
import { screen, setScreen, showToast } from "../state";
import { focusPopout, model, ownsView } from "../windows";
import { openThread } from "./Sidebar";

/**
 * The next item in inbox order (contract §8) after the chat being looked at; from the inbox or
 * settings, the first one. Its chat opens (the request's card is above the composer there). An
 * item with no chat (storage full) opens the inbox. With nothing waiting it shows the inbox,
 * which says so.
 */
export function jumpToNext(): void {
  const current = screen() === "chat" ? model().mainThread : null;
  const jump = nextJump(daemon().inbox, current);
  if (jump.kind === "thread") {
    openThread(jump.threadId);
    // A chat that lives in its own window is shown there.
    if (!ownsView(jump.threadId)) void focusPopout(jump.threadId);
    return;
  }
  if (jump.kind === "none") showToast(daemon().inbox.length ? "This is the only chat that needs you" : "Nothing needs you");
  if (daemon().inbox.length === 0 || jump.kind === "inbox") setScreen("inbox");
}
