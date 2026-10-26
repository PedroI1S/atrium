// End-to-end pop-out check in the real app (`ATRIUM_BENCH=roundtrip`, run by
// `scripts/measure.py roundtrip`). Exercises the paths the review found broken: an edit made
// right before a pop-out closes itself (flush, then bring back), a polite bring-back from main,
// and a forced bring-back when the pop-out cannot answer in time.

import { invoke } from "@tauri-apps/api/core";
import { quitApp, writeReport } from "./platform";
import { setStage } from "./bench";
import { bringBack, model, patchView, popOut, viewId } from "./windows";

/** Set before subscribing, so the database pop-out never answers (see runRoundtrip). */
export const ignoresReturn = (view: string | null) => view === "database";

const WAIT_MS = 10000;
const POLL_MS = 25;
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function until(what: string, ok: () => boolean | Promise<boolean>): Promise<void> {
  const end = performance.now() + WAIT_MS;
  while (performance.now() < end) {
    if (await ok()) return;
    await sleep(POLL_MS);
  }
  throw new Error(`timed out waiting for ${what}`);
}
const ready = async (label: string) => label in (await invoke<Record<string, unknown>>("ready_states"));
/** The native window list (Rust), not readiness: proves a window was actually destroyed. */
const exists = async (label: string) => (await invoke<string[]>("window_labels")).includes(label);

export const POPOUT_EDIT = " + typed in the pop-out right before closing";

/** Pop-out side: the chat edits and closes at once without waiting for the edit to be sent;
 *  the database never answers a return request. */
export async function roundtripPopout(): Promise<void> {
  if (viewId !== "chat") return;
  void patchView("chat", { draft: model().views.chat.draft + POPOUT_EDIT });
  await bringBack("chat");
}

export async function runRoundtrip(): Promise<void> {
  const checks: { name: string; ok: boolean; detail: string }[] = [];
  const check = (name: string, ok: boolean, detail: string) => checks.push({ name, ok, detail });

  setStage("roundtrip: chat closes itself");
  const draft = "draft typed in main";
  await patchView("chat", { draft });
  await popOut("chat");
  check("chat moved to its pop-out", model().views.chat.owner === "view-chat", model().views.chat.owner);
  await until("the chat pop-out to return its view and close", async () => model().views.chat.owner === "main" && !(await exists("view-chat")));
  check("edit made right before closing survives", model().views.chat.draft === draft + POPOUT_EDIT, JSON.stringify(model().views.chat.draft));

  setStage("roundtrip: polite bring-back");
  const content = model().views.editor.content;
  await popOut("editor");
  await until("the editor pop-out to be ready", () => ready("view-editor"));
  const polite = await bringBack("editor");
  check("main brings the editor back politely", polite === "returned" && model().views.editor.owner === "main", String(polite));
  check("editor text unchanged", model().views.editor.content === content, "");

  setStage("roundtrip: forced bring-back");
  // The database pop-out is loaded and listening, but deterministically ignores return requests
  // (`ignoreReturnRequests`, set by index.tsx in this mode): it stands in for a hung window.
  await popOut("database");
  await until("the database pop-out to be ready", () => ready("view-database"));
  check("the database window exists natively", await exists("view-database"), "");
  const forced = await bringBack("database");
  check("main forces the view back from a pop-out that does not answer", forced === "forced" && model().views.database.owner === "main", String(forced));
  await until("the database window to be destroyed", async () => !(await exists("view-database")));
  check("the unanswering window is destroyed (native window list)", !(await exists("view-database")), JSON.stringify(await invoke<string[]>("window_labels")));

  setStage("roundtrip: report");
  await writeReport({ kind: "roundtrip", checks });
  await quitApp();
}
