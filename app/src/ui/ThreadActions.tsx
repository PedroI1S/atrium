// The thread header's ⋯ menu and the palette's commands for one thread: rename, export to
// Markdown or JSONL, end the session, archive. Archive and End session ask first.

import { Ellipsis } from "lucide-solid";
import { For, Show, createSignal, onCleanup } from "solid-js";
import { FORMAT_LABEL, archiveThread, endSession, exportThread, hasLiveSession, setRenaming } from "../chat/thread-actions";
import { daemon } from "../daemon/client";
import type { ExportFormat, ThreadSummary } from "../daemon/types";
import { model } from "../windows";
import { askConfirm } from "./Confirm";
import { IconButton } from "./controls";
import { openThread } from "./Sidebar";

export interface ThreadAction {
  id: string;
  label: string;
  /** Shown under the label in the menu. */
  hint?: string;
  run: () => void;
  disabled?: boolean;
}

const titleOf = (t: ThreadSummary) => t.title || "Untitled chat";

export async function confirmEndSession(threadId: string): Promise<void> {
  const t = daemon().threads[threadId];
  if (!t) return;
  const ok = await askConfirm({
    title: "End this chat's session?",
    body: `The agent of “${titleOf(t)}” stops, and a running turn is cut off. The chat and its history stay.`,
    confirm: "End session",
    danger: true,
  });
  if (ok) await endSession(threadId);
}

export async function confirmArchive(threadId: string): Promise<void> {
  const t = daemon().threads[threadId];
  if (!t) return;
  const ok = await askConfirm({
    title: "Archive this chat?",
    body: `“${titleOf(t)}” leaves every list and the inbox, and its session ends if it is running. Atrium keeps its history.`,
    confirm: "Archive",
    danger: true,
  });
  if (!ok) return;
  if (await archiveThread(threadId)) {
    // The main card doesn't keep showing a chat that left the list.
    if (model().mainThread === threadId) openThread(null);
  }
}

const exportAction = (t: ThreadSummary, format: ExportFormat): ThreadAction => ({
  id: `export-${format}`,
  label: `Export as ${FORMAT_LABEL[format]}…`,
  hint: format === "markdown" ? "A readable copy, secrets redacted" : "Every event, one per line, secrets redacted",
  run: () => void exportThread(t.thread_id, format),
});

/** What can be done to a thread, in menu order. */
export function threadActions(t: ThreadSummary): ThreadAction[] {
  return [
    { id: "rename", label: "Rename", run: () => setRenaming(t.thread_id) },
    exportAction(t, "markdown"),
    exportAction(t, "jsonl"),
    { id: "end", label: "End session…", hint: hasLiveSession(t) ? "Stops the agent; the chat stays" : "The session has already ended", disabled: !hasLiveSession(t), run: () => void confirmEndSession(t.thread_id) },
    { id: "archive", label: "Archive…", hint: "Hides the chat from every list", run: () => void confirmArchive(t.thread_id) },
  ];
}

/** The ⋯ button in a thread's header and the menu it opens under itself. */
export function ThreadMenu(props: { thread: ThreadSummary; small?: boolean }) {
  const [open, setOpen] = createSignal(false);
  let wrap!: HTMLSpanElement;
  let menu: HTMLDivElement | undefined;
  const items = () => Array.from(menu?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
  const close = (refocus: boolean) => {
    setOpen(false);
    document.removeEventListener("pointerdown", outside, true);
    if (refocus) wrap.querySelector<HTMLButtonElement>("button")?.focus();
  };
  const outside = (e: PointerEvent) => { if (!wrap.contains(e.target as Node)) close(false); };
  const show = () => {
    setOpen(true);
    document.addEventListener("pointerdown", outside, true);
    queueMicrotask(() => items()[0]?.focus());
  };
  onCleanup(() => document.removeEventListener("pointerdown", outside, true));
  const toggle = () => (open() ? close(false) : show());
  return (
    <span class="menu-wrap" ref={wrap}>
      <IconButton label="Chat actions" small={props.small} aria-haspopup="menu" aria-expanded={open()} data-atrium-part="thread-menu-button" onClick={toggle}>
        <Ellipsis class={props.small ? "i sm" : "i"} aria-hidden="true" />
      </IconButton>
      <Show when={open()}>
        <div
          class="menu pop"
          role="menu"
          aria-label="Chat actions"
          data-atrium-part="thread-menu"
          ref={menu}
          onKeyDown={(e) => {
            const list = items();
            const at = list.indexOf(document.activeElement as HTMLButtonElement);
            if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); close(true); }
            else if (e.key === "ArrowDown") { e.preventDefault(); list[(at + 1) % list.length]?.focus(); }
            else if (e.key === "ArrowUp") { e.preventDefault(); list[(at - 1 + list.length) % list.length]?.focus(); }
            else if (e.key === "Tab") close(false);
          }}
        >
          <For each={threadActions(props.thread)}>
            {(a) => (
              <button type="button" class="menu-item" role="menuitem" data-action={a.id} disabled={a.disabled} onClick={() => { close(true); a.run(); }}>
                <b>{a.label}</b><Show when={a.hint}><span>{a.hint}</span></Show>
              </button>
            )}
          </For>
        </div>
      </Show>
    </span>
  );
}
