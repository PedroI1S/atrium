// The live check of the chat (`ATRIUM_BENCH=chat-live`, run by app/scripts/chat_check.py against
// a real `atriumd` in a throwaway ATRIUM_HOME, with the synthetic agent; M1-09b Definition of
// Done):
//
//   1. a Home chat is started the way the composer starts one, and its transcript streams in
//      the main window (model and DOM both grow, tool calls show, the view follows the tail);
//   2. the thread moves to its own window while the main window keeps a model of it: the second
//      window shows the same thread, streaming;
//   3. the script kills the daemon mid-stream: the app reconnects (starting a new daemon) and
//      both windows recover: the thread is interrupted, its transcript is closed and shown;
//   4. a new chat started after the reconnect streams too.
//
// Approvals and questions are not part of it: the synthetic agent can't raise them (CR-13).

import { invoke } from "@tauri-apps/api/core";
import { type AgentChoice, startChat, stopTurn } from "./chat/actions";
import { type ThreadModel, holdThread } from "./chat/threads";
import { daemon } from "./daemon/client";
import { epochNow } from "./harness";
import { writeReport } from "./platform";
import { sleep } from "./roundtrip";
import { MAIN, NEW_THREAD, ownerOf } from "./window-model";
import { appliedState, model, patchView, popOut, popoutThread, viewOf } from "./windows";

const MESSAGE = "Live check: stream a long answer";
const AGAIN = "Live check: a chat after the reconnect";
/** Long enough that the daemon is killed mid-stream. */
const LIVE_AGENT: AgentChoice = { id: "live", label: "Synthetic", launch: { type: "fake", tokens_per_second: 200, chunk_chars: 16, duration_ms: 180_000, tool_call_every_ms: 1_500 } };
const POLL_MS = 50;
const STEP_TIMEOUT_MS = 20_000;
/** Killing the daemon, noticing, starting a new one and its recovery take a while. */
const RECONNECT_TIMEOUT_MS = 90_000;
const SAMPLE_GAP_MS = 1_000;
const POPOUT_REPORT_MS = 200;
/** The two windows are sampled a moment apart: two seconds of stream (800 characters a second) is "the same place". */
const KEEP_UP_CHARS = 1_600;
/** Distance from the bottom (px) that still counts as following the tail while rows arrive. */
const TAIL_GAP = 120;

interface Check { name: string; pass: boolean; detail: string }

async function waitFor(what: string, test: () => boolean | Promise<boolean>, timeoutMs = STEP_TIMEOUT_MS): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!(await test())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(POLL_MS);
  }
}

interface Sample {
  /** The model: the last event applied, assistant characters, tool calls, whether the session ended, open messages. */
  last: number | null; chars: number; tools: number; ended: boolean; open: number;
  /** The DOM of this window's transcript. */
  rows: number; height: number; tailGap: number; user: string | null; md: number; toolRows: number;
}

function sample(m: ThreadModel): Sample {
  const t = m.transcript;
  let chars = 0, tools = 0, open = 0, ended = false;
  for (const item of t.items) {
    if (item.kind === "message") {
      if (item.role === "assistant") chars += item.chars;
      if (item.open) open++;
    } else if (item.kind === "tool") tools++;
    else if (item.kind === "note" && item.note.type === "session_ended") ended = true;
  }
  const scroller = document.querySelector<HTMLElement>('[data-atrium-part="transcript"]');
  return {
    last: t.last, chars, tools, ended, open,
    rows: document.querySelectorAll(".thread-row").length,
    height: scroller?.scrollHeight ?? 0,
    tailGap: scroller ? scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight : -1,
    user: document.querySelector(".you")?.textContent ?? null,
    md: document.querySelectorAll(".thread-row .md").length,
    toolRows: document.querySelectorAll(".thread-row .tool").length,
  };
}

type Ready = Record<string, { chat?: Sample } | undefined>;
const readyStates = () => invoke<Ready>("ready_states");

/** A pop-out's part: it shows its thread and keeps saying what it shows. */
export async function chatLivePopout(): Promise<void> {
  const thread = popoutThread();
  if (!thread) return;
  const held = holdThread(thread);
  setInterval(() => {
    void invoke("window_ready", { applied: { ...appliedState(), readyAt: epochNow(), chat: sample(held.model) } });
  }, POPOUT_REPORT_MS);
}

export async function runChatLive(): Promise<void> {
  const checks: Check[] = [];
  const check = (name: string, pass: boolean, detail: unknown) => checks.push({ name, pass, detail: typeof detail === "string" ? detail : JSON.stringify(detail) });
  const connected = () => daemon().connection.type === "connected" && daemon().loaded;
  await waitFor("the daemon", connected);

  // 1. A Home chat from the empty view's composer.
  await patchView(NEW_THREAD, { draft: MESSAGE });
  const started = await startChat(MESSAGE, LIVE_AGENT);
  if (!started) throw new Error("the chat didn't start");
  const thread = started.thread_id;
  check("a new Home chat starts from the composer and becomes the chat shown", model().mainThread === thread && viewOf(NEW_THREAD).draft === "", `main shows ${model().mainThread}; draft ${JSON.stringify(viewOf(NEW_THREAD).draft)}`);
  const held = holdThread(thread);
  await waitFor("the first tool call in the transcript", () => sample(held.model).toolRows > 0 && sample(held.model).md > 0);
  const a = sample(held.model);
  await sleep(SAMPLE_GAP_MS);
  const b = sample(held.model);
  check("the chat streams: the model and the rendered transcript both grow", b.chars > a.chars && b.height > a.height && b.md > 0, { chars: [a.chars, b.chars], height: [a.height, b.height], rows: b.rows });
  check("the user's message and the tool calls are shown", b.user === MESSAGE && b.toolRows > 0 && b.tools > 0, { user: b.user, toolRows: b.toolRows, tools: b.tools });
  check("the view follows the tail", b.tailGap >= 0 && b.tailGap < TAIL_GAP, `${b.tailGap}px from the bottom`);

  // 2. The same thread in a second window (the main window keeps its model).
  await popOut(thread);
  await waitFor("the pop-out to own the thread", () => ownerOf(model(), thread) !== MAIN);
  const label = ownerOf(model(), thread);
  const theirs = async () => (await readyStates())[label]?.chat;
  const mark = held.model.transcript.last ?? 0;
  await waitFor("the pop-out to catch up", async () => ((await theirs())?.last ?? -1) >= mark);
  const p1 = (await theirs())!;
  await sleep(SAMPLE_GAP_MS);
  const p2 = (await theirs())!;
  const mine = sample(held.model);
  check("a second window shows the same thread, streaming", p2.chars > p1.chars && p2.height > p1.height && p2.user === MESSAGE && p2.toolRows > 0, { window: label, chars: [p1.chars, p2.chars], height: [p1.height, p2.height], rows: p2.rows });
  check("both windows keep up with the same events", mine.last !== null && p2.last !== null && Math.abs(mine.chars - p2.chars) <= KEEP_UP_CHARS, { main: { last: mine.last, chars: mine.chars }, popout: { last: p2.last, chars: p2.chars } });

  // 3. The script kills the daemon now.
  const before = sample(held.model);
  await writeReport({ kind: "chat-live-streaming", thread, chars: before.chars });
  await waitFor("the app to notice the daemon is gone", () => daemon().connection.type !== "connected", RECONNECT_TIMEOUT_MS);
  await waitFor("the reconnect and the thread's new state", () => connected() && daemon().threads[thread]?.state.type === "interrupted", RECONNECT_TIMEOUT_MS);
  await waitFor("the main window's transcript to close", () => { const s = sample(held.model); return s.ended && s.open === 0; }, RECONNECT_TIMEOUT_MS);
  await waitFor("the pop-out's transcript to close", async () => { const s = await theirs(); return s !== undefined && s.ended && s.open === 0 && s.last === held.model.transcript.last; }, RECONNECT_TIMEOUT_MS);
  const after = sample(held.model);
  const popAfter = (await theirs())!;
  check("after the daemon was killed mid-stream, the thread is interrupted and its transcript is closed", after.ended && after.open === 0 && held.model.load().kind === "ready", { state: daemon().threads[thread]?.state.type, load: held.model.load().kind });
  check("what had streamed is still there", after.chars >= a.chars && after.chars > 0, { firstSample: a.chars, beforeKill: before.chars, after: after.chars });
  check("the second window recovered to the same transcript and shows it", popAfter.last === after.last && popAfter.chars === after.chars && popAfter.rows > 0 && popAfter.md > 0, { main: { last: after.last, chars: after.chars }, popout: { last: popAfter.last, chars: popAfter.chars, rows: popAfter.rows } });

  // 4. The view works after the reconnect: a new chat streams in the main window.
  const again = await startChat(AGAIN, LIVE_AGENT);
  if (!again) throw new Error("the chat after the reconnect didn't start");
  const second = holdThread(again.thread_id);
  await waitFor("the new chat's first rows", () => model().mainThread === again.thread_id && sample(second.model).md > 0 && sample(second.model).user === AGAIN);
  const c = sample(second.model);
  await sleep(SAMPLE_GAP_MS);
  const d = sample(second.model);
  check("a new chat streams after the reconnect", d.chars > c.chars && d.height >= c.height && d.user === AGAIN, { chars: [c.chars, d.chars], rows: d.rows });
  await stopTurn(again.thread_id);
  await waitFor("the stopped turn", () => daemon().threads[again.thread_id]?.state.type !== "running");
  check("Stop ends the turn", ["idle", "ended"].includes(daemon().threads[again.thread_id]?.state.type ?? ""), String(daemon().threads[again.thread_id]?.state.type));

  await writeReport({ kind: "chat-live", thread, pass: checks.every((x) => x.pass), checks });
}
