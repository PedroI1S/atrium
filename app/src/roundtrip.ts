// The live check of windows and drafts (`ATRIUM_BENCH=roundtrip`, run by scripts/live-check.sh
// against a real daemon in a throwaway ATRIUM_HOME). The main window drives it and reports
// each check; pop-outs play their part and report through `window_ready`.
//
//   1. a synthetic thread is started and shown; a draft is typed in the main window;
//   2. the thread moves to its own window, which must see the draft and adds to it;
//   3. the pop-out hands the view back when asked: the draft has both parts;
//   4. a second pop-out ignores the request: main forces it back, and what that window had
//      already written is still there;
//   5. main then reports every change of the connection and of the thread list, so the script
//      can kill the daemon and see the app reconnect with its list intact.
//
// `ATRIUM_BENCH=roundtrip-verify` is the relaunch after the app was killed: it reports the
// draft that came back from disk.

import { invoke } from "@tauri-apps/api/core";
import { createEffect, createRoot } from "solid-js";
import { command, daemon } from "./daemon/client";
import type { ThreadSummary } from "./daemon/types";
import { epochNow } from "./harness";
import { type StartupInfo, writeReport } from "./platform";
import { openThread } from "./ui/Sidebar";
import { MAIN, ownerOf } from "./window-model";
import { setAdvancedOpen, setPanel, setScreen, setSettingsSection, setSidebarOpen } from "./state";
import { appliedState, bringBack, ignoreReturnRequests, model, patchSettings, patchView, popOut, popoutThread, viewOf, windowLabel } from "./windows";

const MAIN_TEXT = "typed in main";
const POPOUT_TEXT = " + typed in the pop-out";
const FORCED_TEXT = " + typed before it was forced";
/** The pop-out that plays the hung window: it never answers a return request. */
const STUBBORN = "view-2";
const POLL_MS = 50;
const STEP_TIMEOUT_MS = 15_000;

interface Check { name: string; pass: boolean; detail: string }

async function until(what: string, test: () => boolean | Promise<boolean>): Promise<void> {
  const deadline = Date.now() + STEP_TIMEOUT_MS;
  while (!(await test())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((resolve) => setTimeout(resolve, POLL_MS));
  }
}

type Ready = Record<string, { roundtrip?: { sawDraft: string; wrote: boolean } } | undefined>;
const readyStates = () => invoke<Ready>("ready_states");

/** A pop-out's part: say what draft it found, add to it, and (the stubborn one) refuse to return. */
export async function roundtripPopout(): Promise<void> {
  const thread = popoutThread();
  if (!thread) return;
  const stubborn = windowLabel() === STUBBORN;
  if (stubborn) ignoreReturnRequests();
  const sawDraft = viewOf(thread).draft;
  await patchView(thread, { draft: sawDraft + (stubborn ? FORCED_TEXT : POPOUT_TEXT) });
  await invoke("window_ready", { applied: { ...appliedState(), readyAt: epochNow(), roundtrip: { sawDraft, wrote: true } } });
}

async function popOutAndWait(thread: string, label: string, expectDraft: string, checks: Check[]): Promise<void> {
  await popOut(thread);
  await until(`${label} to own the thread`, () => ownerOf(model(), thread) === label);
  await until(`${label} to report`, async () => (await readyStates())[label]?.roundtrip?.wrote === true);
  const saw = (await readyStates())[label]?.roundtrip?.sawDraft ?? "";
  checks.push({ name: `${label} opened with the draft`, pass: saw === expectDraft, detail: JSON.stringify(saw) });
  // What the pop-out wrote is Rust's now; main's copy is refreshed when the view comes back.
}

export async function runRoundtrip(): Promise<void> {
  const checks: Check[] = [];
  await until("the daemon", () => daemon().connection.type === "connected" && daemon().loaded);
  const started = await command<ThreadSummary>({ type: "start_thread", agent: { type: "fake", tokens_per_second: 50, chunk_chars: 16, duration_ms: 600_000, tool_call_every_ms: 0 }, message: "Round trip" });
  const thread = started.thread_id;
  await until("the thread in the list", () => thread in daemon().threads);
  openThread(thread);
  await patchView(thread, { draft: MAIN_TEXT });

  await popOutAndWait(thread, "view-1", MAIN_TEXT, checks);
  const polite = await bringBack(thread);
  await until("the view to return", () => ownerOf(model(), thread) === MAIN);
  checks.push({ name: "the pop-out returned the view when asked", pass: polite === "returned", detail: polite });
  checks.push({ name: "the draft has what both windows typed", pass: viewOf(thread).draft === MAIN_TEXT + POPOUT_TEXT, detail: JSON.stringify(viewOf(thread).draft) });
  await until("view-1 to be destroyed", async () => !(await invoke<string[]>("window_labels")).includes("view-1"));

  await popOutAndWait(thread, STUBBORN, MAIN_TEXT + POPOUT_TEXT, checks);
  const forced = await bringBack(thread);
  await until("the view to return", () => ownerOf(model(), thread) === MAIN);
  const labels = await invoke<string[]>("window_labels");
  checks.push({ name: "a pop-out that doesn't answer is forced back", pass: forced === "forced" && !labels.includes(STUBBORN), detail: `${forced}; windows: ${labels.join(", ")}` });
  const expected = MAIN_TEXT + POPOUT_TEXT + FORCED_TEXT;
  checks.push({ name: "the forced window's draft is kept", pass: viewOf(thread).draft === expected, detail: JSON.stringify(viewOf(thread).draft) });

  await writeReport({ kind: "roundtrip", thread, expected, pass: checks.every((c) => c.pass), checks });
  followConnection();
}

/** Reports the connection and the thread list whenever either changes. */
function followConnection(): void {
  createRoot(() => {
    createEffect(() => {
      const state = daemon();
      void writeReport({
        kind: "connection", state: state.connection.type, loaded: state.loaded,
        threads: state.order.map((id) => ({ id, state: state.threads[id]?.state.type })),
      });
    });
  });
}

/** After the app was killed and started again: the draft and the thread shown come back from disk. */
export async function runRoundtripVerify(): Promise<void> {
  await until("the daemon", () => daemon().connection.type === "connected" && daemon().loaded);
  const thread = model().mainThread;
  await writeReport({
    kind: "roundtrip-verify", thread, draft: thread ? viewOf(thread).draft : null,
    inList: thread !== null && thread in daemon().threads,
    threads: daemon().order.map((id) => ({ id, state: daemon().threads[id]?.state.type })),
  });
  followConnection();
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
/** After a state change: let it render and its transitions end before the picture. */
const SETTLE_MS = 450;

async function shot(name: string): Promise<void> {
  await sleep(SETTLE_MS);
  await invoke("take_snapshot", { name });
}

/** A pop-out in a screenshot run: a picture of itself, then it says it is done. */
export async function shotsPopout(): Promise<void> {
  await until("the thread's summary", () => { const id = popoutThread(); return id !== null && id in daemon().threads; });
  await shot("popout");
  await invoke("window_ready", { applied: { ...appliedState(), readyAt: epochNow(), roundtrip: { sawDraft: "", wrote: true } } });
}

/**
 * `ATRIUM_BENCH=shots` with `ATRIUM_SHOT_DIR`: seeds a few synthetic threads in a throwaway
 * daemon and takes the screenshots the review asks for, one scene after another.
 */
export async function runShots(): Promise<void> {
  await until("the daemon", () => daemon().connection.type === "connected" && daemon().loaded);
  const fake = (message: string, duration_ms: number) =>
    command<ThreadSummary>({ type: "start_thread", agent: { type: "fake", tokens_per_second: 80, chunk_chars: 16, duration_ms, tool_call_every_ms: 0 }, message });
  const done = await fake("Summarise the notes in ~/Documents/notes", 1_500);
  await fake("Which folders take the most space?", 600_000);
  const shown = await fake("Draft a reply to the landlord's email", 600_000);
  await until("a finished turn", () => daemon().threads[done.thread_id]?.state.type === "idle");
  openThread(shown.thread_id);
  await patchView(shown.thread_id, { draft: "Keep it short, and ask about the boiler too." });

  await patchSettings({ theme: "light" });
  await shot("main-light");
  await patchSettings({ theme: "dark" });
  await shot("main-dark");
  setSidebarOpen(false);
  await shot("sidebar-hidden");
  setSidebarOpen(true);
  setPanel({ open: false });
  await shot("panel-minimized");
  setPanel({ open: true });
  setScreen("inbox");
  await shot("inbox");
  setScreen("settings");
  setSettingsSection("appearance");
  setAdvancedOpen(true);
  await shot("settings-advanced-dark");
  await patchSettings({ theme: "light" });
  await shot("settings-advanced-light");
  setScreen("chat");

  await popOut(shown.thread_id);
  const label = ownerOf(model(), shown.thread_id);
  await until("the pop-out's picture", async () => (await readyStates())[label]?.roundtrip?.wrote === true);
  await shot("main-with-popout");
  await bringBack(shown.thread_id);
  await until("the view to return", () => ownerOf(model(), shown.thread_id) === MAIN);
  await sleep(SETTLE_MS);
  await writeReport({ kind: "shots", pass: true });
}

/** `ATRIUM_BENCH=shots-incompatible`: against a daemon that refuses this app (contract §10). */
export async function runShotsIncompatible(): Promise<void> {
  await until("the refusal", () => daemon().connection.type === "incompatible");
  await patchSettings({ theme: "light" });
  await shot("incompatible");
  await sleep(SETTLE_MS);
  await writeReport({ kind: "shots", pass: true });
}

export const isRoundtrip = (info: StartupInfo): boolean => info.benchMode === "roundtrip" || info.benchMode === "roundtrip-verify" || (info.benchMode ?? "").startsWith("shots");
