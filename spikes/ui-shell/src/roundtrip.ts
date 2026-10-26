// End-to-end pop-out check in the real app (`ATRIUM_BENCH=roundtrip`, run by
// `scripts/measure.py roundtrip`). Exercises the paths the review found broken: an edit made
// right before a pop-out closes itself (flush, then bring back), a polite bring-back from main,
// and a forced bring-back when the pop-out cannot answer in time.

import { invoke } from "@tauri-apps/api/core";
import { quitApp, writeReport } from "./platform";
import { setStage } from "./bench";
import { bringBack, model, patchView, popOut, viewId } from "./windows";

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

export const POPOUT_EDIT = " + typed in the pop-out right before closing";

/** Pop-out side: edit, and close at once without waiting for the edit to be sent. */
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
  await until("the chat pop-out to return its view", async () => model().views.chat.owner === "main" && !(await ready("view-chat")));
  check("edit made right before closing survives", model().views.chat.draft === draft + POPOUT_EDIT, JSON.stringify(model().views.chat.draft));

  setStage("roundtrip: polite bring-back");
  const content = model().views.editor.content;
  await popOut("editor");
  await until("the editor pop-out to be ready", () => ready("view-editor"));
  const polite = await bringBack("editor");
  check("main brings the editor back politely", polite === "returned" && model().views.editor.owner === "main", String(polite));
  check("editor text unchanged", model().views.editor.content === content, "");

  setStage("roundtrip: forced bring-back");
  await popOut("database");
  // Ask before the pop-out has loaded its listener: it cannot answer, so main must force it.
  const forced = await bringBack("database");
  await until("the database window to be gone", async () => !(await ready("view-database")));
  check("main recovers a view from a pop-out that does not answer", model().views.database.owner === "main", String(forced));

  setStage("roundtrip: report");
  await writeReport({ kind: "roundtrip", checks });
  await quitApp();
}
