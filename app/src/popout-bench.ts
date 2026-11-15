// `ATRIUM_BENCH=startup-popout` (app/scripts/startup_bench.py): the memory cost of one pop-out
// window (D-028, D-040: measured, not capped). After the startup report, the main window starts
// a short thread from the fake agent, shows it once it is idle and says so ("baseline"); the
// script samples memory; after `BASELINE_MS` the thread moves to its own window, and once that
// window is ready the main window says so ("open") and the script samples again. The cost of the
// pop-out is the difference. Nothing streams while either is sampled.

import { invoke } from "@tauri-apps/api/core";
import { command, daemon } from "./daemon/client";
import type { ThreadSummary } from "./daemon/types";
import { epochNow } from "./harness";
import { writeReport } from "./platform";
import { sleep, until } from "./roundtrip";
import { openThread } from "./ui/Sidebar";
import { MAIN, ownerOf } from "./window-model";
import { model, popOut } from "./windows";

/** How long the main window shows the thread before popping it out: the script samples 5–35 s. */
export const BASELINE_MS = 40_000;

export async function runPopoutBench(): Promise<void> {
  await until("the daemon", () => daemon().connection.type === "connected" && daemon().loaded);
  const started = await command<ThreadSummary>({ type: "start_thread", agent: { type: "fake", tokens_per_second: 50, chunk_chars: 16, duration_ms: 1_000, tool_call_every_ms: 0 }, message: "Pop-out cost" });
  const thread = started.thread_id;
  await until("the thread to be idle", () => daemon().threads[thread]?.state.type === "idle");
  openThread(thread);
  await sleep(1_000);
  await writeReport({ kind: "popout-stage", stage: "baseline", thread, at: epochNow() });
  await sleep(BASELINE_MS);
  const asked = epochNow();
  await popOut(thread);
  let label = "";
  await until("the pop-out to own the thread", () => { label = ownerOf(model(), thread); return label !== MAIN; });
  await until("the pop-out to be ready", async () => label in (await invoke<Record<string, unknown>>("ready_states")));
  await writeReport({ kind: "popout-stage", stage: "open", thread, label, at: epochNow(), readyMs: epochNow() - asked });
}
