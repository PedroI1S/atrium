// What the thread header's menu and the palette do to a thread (PLAN M1, contract §10 and §13):
// rename, export, end its session, archive. Each is one daemon command; failures are shown,
// never retried. The pure parts are tested in thread-actions.test.ts.

import { createSignal } from "solid-js";
import { command, daemon } from "../daemon/client";
import { type ExportFormat, type ThreadSummary, isCommandFailure } from "../daemon/types";
import { askSavePath } from "../platform";
import { showToast } from "../state";

/** The daemon's limit (`atrium-daemon` `MAX_TITLE_CHARS`): longer titles are cut there. */
export const MAX_TITLE_CHARS = 120;
/** How much of a title goes into a suggested file name. */
const MAX_FILE_STEM_CHARS = 60;
const KIB = 1024;

/**
 * A title as the daemon will store it: the first line with text, spaces collapsed, cut to the
 * limit. Null when there is nothing left (the daemon refuses an empty title).
 */
export function cleanTitle(input: string): string | null {
  const line = input.split(/\r?\n/).map((l) => l.trim()).find((l) => l !== "");
  if (line === undefined) return null;
  return [...line.split(/\s+/).join(" ")].slice(0, MAX_TITLE_CHARS).join("");
}

export const EXTENSION: Record<ExportFormat, string> = { markdown: "md", jsonl: "jsonl" };
export const FORMAT_LABEL: Record<ExportFormat, string> = { markdown: "Markdown", jsonl: "JSONL" };

/** The file name the save dialog suggests: the title without characters file systems refuse. */
export function exportFileName(title: string, format: ExportFormat): string {
  const stem = [...title.replace(/[\p{Cc}/\\:*?"<>|]+/gu, " ").replace(/\s+/g, " ").trim()]
    .slice(0, MAX_FILE_STEM_CHARS)
    .join("")
    .replace(/^[.\s]+|[.\s]+$/g, "");
  return `${stem || "chat"}.${EXTENSION[format]}`;
}

/** Whether "End session" has a session to end (contract §10: `Shutdown` ends it). */
export function hasLiveSession(thread: ThreadSummary): boolean {
  switch (thread.state.type) {
    case "starting":
    case "running":
    case "needs_approval":
    case "needs_input":
    case "idle":
    case "paused":
      return true;
    case "failed":
    case "interrupted":
    case "ended":
      return false;
  }
}

export function sizeLabel(bytes: number): string {
  if (bytes < KIB) return `${bytes} bytes`;
  if (bytes < KIB * KIB) return `${Math.round(bytes / KIB)} KB`;
  return `${(bytes / (KIB * KIB)).toFixed(1)} MB`;
}

const failureText = (e: unknown): string => (isCommandFailure(e) ? e.message : String(e));
const titleOf = (threadId: string): string => daemon().threads[threadId]?.title || "Untitled chat";

/** The thread whose title is being edited in its header (one at a time per window). */
export const [renaming, setRenaming] = createSignal<string | null>(null);

/** Renames a thread. Resolves true when the daemon took it. */
export async function renameThread(threadId: string, input: string): Promise<boolean> {
  const title = cleanTitle(input);
  if (title === null) {
    showToast("A chat needs a name");
    return false;
  }
  if (title === daemon().threads[threadId]?.title) return true;
  try {
    await command({ type: "rename_thread", thread_id: threadId, title });
    return true;
  } catch (e) {
    showToast(`Couldn't rename the chat: ${failureText(e)}`);
    return false;
  }
}

/**
 * Exports a thread to a file the user picks (contract §13). The daemon writes it: redacted, mode
 * 0600, and only as a new file. Resolves with the path, or null when cancelled or refused.
 */
export async function exportThread(threadId: string, format: ExportFormat): Promise<string | null> {
  let path: string | null;
  try {
    path = await askSavePath(`Export “${titleOf(threadId)}” as ${FORMAT_LABEL[format]}`, exportFileName(titleOf(threadId), format));
  } catch (e) {
    showToast(`Couldn't open the save dialog: ${failureText(e)}`);
    return null;
  }
  if (path === null) return null;
  try {
    const { bytes } = await command<{ bytes: number }>({ type: "export_thread", thread_id: threadId, format, path });
    showToast(`Exported ${sizeLabel(bytes)} to ${path}`);
    return path;
  } catch (e) {
    showToast(`Couldn't export the chat: ${failureText(e)}`);
    return null;
  }
}

/** Ends the thread's session (`Shutdown`): the agent's process stops and the thread is Ended. */
export async function endSession(threadId: string): Promise<boolean> {
  try {
    await command({ type: "thread_command", thread_id: threadId, command: { type: "shutdown" } });
    return true;
  } catch (e) {
    showToast(`Couldn't end the session: ${failureText(e)}`);
    return false;
  }
}

/** Archives a thread: its session ends and it leaves every list (contract §10). */
export async function archiveThread(threadId: string): Promise<boolean> {
  try {
    await command({ type: "archive_thread", thread_id: threadId });
    return true;
  } catch (e) {
    showToast(`Couldn't archive the chat: ${failureText(e)}`);
    return false;
  }
}
