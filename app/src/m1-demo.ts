// End-to-end M1 gate demo (`ATRIUM_BENCH=m1-demo`, run by app/scripts/m1_demo.py against a real
// `atriumd` in a throwaway ATRIUM_HOME with the synthetic agent).
//
// Walks through M1's promises in order:
//   1. Home chat with the fake agent streams;
//   2. Fake approval (`approval_at_ms`): Allow once, Deny next turn, "Always" rule + next turn skips;
//   3. Fake question (`question_at_ms`): Answer, Decline next turn (streaming continues both times);
//   4. Inbox lists threads with their states, and marking a thread read works;
//   5. ⌘J jumps through inbox items in rank order, and ⌘⇧J back;
//   6. Settings → Notifications toggle only (daemon recording notifier
//      checked separately by m1_demo.py via `cargo test -p atrium-daemon --test requests_notifications`);
//   7. Stop a turn / End session from the thread menu to Ended / verify fake Resume refusal;
//   8. Kill the daemon mid-turn: the app reconnects and the thread shows `Interrupted`;
//   9. Rename a thread and export it to Markdown and JSONL from the thread menu (the save
//      dialog is bypassed: files go to the demo's folder);
//  10. Archive a thread from the thread menu, after confirming;
//  11. Batch approve and deny in the inbox;
//  12. Settings: add a rule in the rules editor (a bad command is refused first), and
//      Security lists what is always blocked and the known limits;
//  13. The enforcement badge opens the details: coverage, policy and isolation apart.

import { markRead, sendMessage, startChat, type AgentChoice } from "./chat/actions";
import { EXISTS_TEXT, exportFileName } from "./chat/thread-actions";
import { type ThreadModel, holdThread } from "./chat/threads";
import { command, daemon } from "./daemon/client";
import { type AgentLaunch, type Rule, type ThreadSummary, isCommandFailure } from "./daemon/types";
import { setSaveDirOverride, writeReport } from "./platform";
import { sleep } from "./roundtrip";
import { screen, setScreen, setSettingsSection, toast, windowFocused } from "./state";
import { openThread } from "./ui/Sidebar";
import { NEW_THREAD } from "./window-model";
import { model, patchView } from "./windows";

interface Check {
  name: string;
  pass: boolean;
  detail: string;
}

const POLL_MS = 50;
const STEP_TIMEOUT_MS = 25_000;
const RECONNECT_TIMEOUT_MS = 90_000;
const SAMPLE_GAP_MS = 350;

async function waitFor(what: string, test: () => boolean | Promise<boolean>, timeoutMs = STEP_TIMEOUT_MS): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!(await test())) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await sleep(POLL_MS);
  }
}

function fakeChoice(id: string, launch: Extract<AgentLaunch, { type: "fake" }>): AgentChoice {
  return { id, label: `Synthetic (${id})`, launch };
}

function assistantChars(m: ThreadModel): number {
  let chars = 0;
  for (const item of m.transcript.items) {
    if (item.kind === "message" && item.role === "assistant") chars += item.chars;
  }
  return chars;
}

function toolItems(m: ThreadModel) {
  return m.transcript.items.filter((i) => i.kind === "tool");
}

function approvalItems(m: ThreadModel) {
  return m.transcript.items.filter((i) => i.kind === "approval");
}

function questionItems(m: ThreadModel) {
  return m.transcript.items.filter((i) => i.kind === "question");
}

function userCount(m: ThreadModel): number {
  return m.transcript.items.filter((i) => i.kind === "message" && i.role === "user").length;
}

function transcriptClosed(m: ThreadModel): boolean {
  let ended = false;
  let open = 0;
  for (const item of m.transcript.items) {
    if (item.kind === "message" && item.open) open++;
    else if (item.kind === "note" && item.note.type === "session_ended") ended = true;
  }
  return ended && open === 0;
}

async function click(what: string, find: () => HTMLElement | null): Promise<void> {
  let target: HTMLElement | null = null;
  await waitFor(what, () => {
    target = find();
    return target !== null && !(target as HTMLButtonElement).disabled;
  });
  target!.click();
}

/** Opens the main card's thread menu and picks an action. */
async function menuAction(id: string): Promise<void> {
  await click("thread menu button", () => document.querySelector<HTMLElement>('.card.main [data-atrium-part="thread-menu-button"]'));
  await click(`thread menu: ${id}`, () => document.querySelector<HTMLElement>(`[data-atrium-part="thread-menu"] [data-action="${id}"]`));
}

/** Types into a field the way a user does: the value, then an input event (Solid's onInput). */
function typeInto(el: HTMLInputElement, text: string): void {
  el.focus();
  el.value = text;
  el.dispatchEvent(new InputEvent("input", { bubbles: true, data: text, inputType: "insertText" }));
}

function key(target: EventTarget, init: KeyboardEventInit): void {
  target.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }));
}

function formatErr(e: unknown): string {
  if (isCommandFailure(e)) {
    return e.kind === "rejected" ? `${e.kind} (${e.code}): ${e.message}` : `${e.kind}: ${e.message}`;
  }
  return e instanceof Error ? e.message : String(e);
}

export async function runM1Demo(exportDir: string | null): Promise<void> {
  const checks: Check[] = [];
  try {
    await runM1DemoInner(exportDir, checks);
  } catch (e) {
    const err = formatErr(e);
    try {
      await writeReport({ kind: "m1-demo-failure", checks, error: err });
    } catch {
      // Best-effort partial check report; index.tsx's reportFailure still reports the error.
    }
    throw new Error(err);
  }
}

async function runM1DemoInner(exportDir: string | null, checks: Check[]): Promise<void> {
  const check = async (name: string, pass: boolean, detail: unknown) => {
    const detailStr = typeof detail === "string" ? detail : JSON.stringify(detail);
    checks.push({ name, pass, detail: detailStr });
    await writeReport({ kind: "m1-demo-check", name, pass, detail: detailStr });
  };

  const connected = () => daemon().connection.type === "connected" && daemon().loaded;
  await waitFor("the daemon connection", connected);

  // ---------------------------------------------------------------------------
  // 1. Home chat with the fake agent streams
  // ---------------------------------------------------------------------------
  openThread(null);
  const step1Msg = "Step 1: Home chat stream";
  await patchView(NEW_THREAD, { draft: step1Msg });
  const t1Summary = await startChat(
    step1Msg,
    fakeChoice("stream", { type: "fake", tokens_per_second: 200, chunk_chars: 16, duration_ms: 1_200, tool_call_every_ms: 400 }),
  );
  if (!t1Summary) throw new Error("Step 1 chat did not start");
  const t1 = t1Summary.thread_id;
  const h1 = holdThread(t1);
  await waitFor("step 1 first markdown and tool row", () => assistantChars(h1.model) > 0 && document.querySelectorAll(".thread-row .tool").length > 0 && document.querySelectorAll(".thread-row .md").length > 0);
  const c1 = assistantChars(h1.model);
  await sleep(SAMPLE_GAP_MS);
  const c2 = assistantChars(h1.model);
  await waitFor("step 1 turn to become idle", () => daemon().threads[t1]?.state.type === "idle");
  const tools1 = toolItems(h1.model);
  await check("1. Home chat with the fake agent streams", model().mainThread === t1 && c2 > c1 && tools1.length > 0 && document.querySelector(".you")?.textContent === step1Msg, {
    thread: t1,
    chars: [c1, c2, assistantChars(h1.model)],
    tools: tools1.length,
    state: daemon().threads[t1]?.state.type,
  });
  h1.release();

  // ---------------------------------------------------------------------------
  // 2. Fake approval (`approval_at_ms`): Allow, Deny, Always
  // ---------------------------------------------------------------------------
  const t2Summary = await startChat(
    "Step 2: approval turn 1 (allow)",
    fakeChoice("approval", { type: "fake", tokens_per_second: 160, chunk_chars: 16, duration_ms: 300, tool_call_every_ms: 0, approval_at_ms: 40 }),
  );
  if (!t2Summary) throw new Error("Step 2 chat did not start");
  const t2 = t2Summary.thread_id;
  const h2 = holdThread(t2);

  await waitFor("step 2a NeedsApproval", () =>
    daemon().threads[t2]?.state.type === "needs_approval" &&
    daemon().threads[t2]?.pending_approvals === 1 &&
    document.querySelector<HTMLElement>('[data-atrium-part="approval"]')?.dataset.state === "fresh",
  );
  const inQueue = daemon().inbox.some((i) => i.thread_id === t2 && i.reason === "approval");
  await click("Allow once button", () => document.querySelector<HTMLElement>('[data-atrium-part="approval"] .btn.pri'));
  await waitFor("step 2a turn to finish idle", () => daemon().threads[t2]?.state.type === "idle");
  const req2a = approvalItems(h2.model)[0];
  const tool2a = toolItems(h2.model)[0];
  await check("2a. Fake approval shows NeedsApproval in queue and Allow succeeds", inQueue && req2a?.outcome === "approved" && tool2a?.status === "succeeded", {
    inQueue,
    outcome: req2a?.outcome,
    toolStatus: tool2a?.status,
  });

  // 2b. Next turn: Deny
  const sent2b = await sendMessage(daemon().threads[t2]!, "Step 2b: approval turn 2 (deny)");
  if (!sent2b) throw new Error("Step 2b sendMessage failed");
  await waitFor("step 2b NeedsApproval", () =>
    daemon().threads[t2]?.state.type === "needs_approval" &&
    approvalItems(h2.model).length === 2 &&
    document.querySelector<HTMLElement>('[data-atrium-part="approval"]')?.dataset.state === "fresh",
  );
  await click("Deny button", () => document.querySelector<HTMLElement>('[data-atrium-part="approval"] .ask-act .btn.ghost'));
  await waitFor("step 2b turn to finish idle", () => daemon().threads[t2]?.state.type === "idle");
  const req2b = approvalItems(h2.model)[1];
  const tool2b = toolItems(h2.model)[1];
  await check("2b. Next turn approval Deny denies the call", req2b?.outcome === "denied" && tool2b?.status === "denied", {
    outcome: req2b?.outcome,
    toolStatus: tool2b?.status,
  });

  // 2c. Next turn: "Always" creates a rule and the next turn doesn't ask
  const sent2c = await sendMessage(daemon().threads[t2]!, "Step 2c: approval turn 3 (always)");
  if (!sent2c) throw new Error("Step 2c sendMessage failed");
  await waitFor("step 2c NeedsApproval", () =>
    daemon().threads[t2]?.state.type === "needs_approval" &&
    approvalItems(h2.model).length === 3 &&
    document.querySelector<HTMLElement>('[data-atrium-part="approval"]')?.dataset.state === "fresh",
  );
  await click("Always… menu button", () => document.querySelector<HTMLElement>('[data-atrium-part="approval"] button[aria-haspopup="menu"]'));
  // Pick the first scope ("Always in this chat") so later threads can still trigger approvals.
  await click("Always in this chat option", () => document.querySelector<HTMLElement>('[data-atrium-part="approval"] .menu .menu-item'));
  await waitFor("step 2c turn to finish idle", () => daemon().threads[t2]?.state.type === "idle");
  const rulesAfterAlways = await command<Rule[]>({ type: "list_rules" });
  const ruleCreated = rulesAfterAlways.some((r) => r.scope.type === "thread" && r.scope.thread_id === t2 && r.decision === "allow");

  const sent2d = await sendMessage(daemon().threads[t2]!, "Step 2d: turn 4 skips asking");
  if (!sent2d) throw new Error("Step 2d sendMessage failed");
  await waitFor("step 2d turn 4 to complete", () =>
    userCount(h2.model) === 4 &&
    toolItems(h2.model).length === 4 &&
    toolItems(h2.model)[3]?.status === "succeeded" &&
    daemon().threads[t2]?.state.type === "idle",
  );
  await check("2c. Always creates a rule and the next turn runs without asking", ruleCreated && approvalItems(h2.model).length === 3 && toolItems(h2.model)[3]?.status === "succeeded", {
    rulesCount: rulesAfterAlways.length,
    approvalsAsked: approvalItems(h2.model).length,
    turn4ToolStatus: toolItems(h2.model)[3]?.status,
  });
  h2.release();

  // ---------------------------------------------------------------------------
  // 3. Fake question (`question_at_ms`): Answer, Decline
  // ---------------------------------------------------------------------------
  const t3Summary = await startChat(
    "Step 3: question turn 1 (answer)",
    fakeChoice("question", { type: "fake", tokens_per_second: 160, chunk_chars: 16, duration_ms: 400, tool_call_every_ms: 0, question_at_ms: 40 }),
  );
  if (!t3Summary) throw new Error("Step 3 chat did not start");
  const t3 = t3Summary.thread_id;
  const h3 = holdThread(t3);

  await waitFor("step 3a NeedsInput", () =>
    daemon().threads[t3]?.state.type === "needs_input" &&
    daemon().threads[t3]?.pending_questions === 1 &&
    document.querySelector<HTMLElement>('[data-atrium-part="question"]')?.dataset.state === "fresh",
  );
  const charsBeforeAnswer = assistantChars(h3.model);
  await click("first question option", () => document.querySelector<HTMLElement>('[data-atrium-part="question"] .q-opt'));
  await click("Answer button", () => document.querySelector<HTMLElement>('[data-atrium-part="question"] .btn.pri'));
  await waitFor("step 3a turn to finish idle", () => daemon().threads[t3]?.state.type === "idle");
  const q3a = questionItems(h3.model)[0];
  const charsAfterAnswer = assistantChars(h3.model);
  await check("3a. Fake question shows NeedsInput form and Answer resumes streaming", q3a?.outcome === "answered" && charsAfterAnswer > charsBeforeAnswer, {
    outcome: q3a?.outcome,
    chars: [charsBeforeAnswer, charsAfterAnswer],
  });

  // 3b. Next turn: Decline
  const sent3b = await sendMessage(daemon().threads[t3]!, "Step 3b: question turn 2 (decline)");
  if (!sent3b) throw new Error("Step 3b sendMessage failed");
  await waitFor("step 3b NeedsInput", () =>
    daemon().threads[t3]?.state.type === "needs_input" &&
    questionItems(h3.model).length === 2 &&
    document.querySelector<HTMLElement>('[data-atrium-part="question"]')?.dataset.state === "fresh",
  );
  const charsBeforeDecline = assistantChars(h3.model);
  await click("Decline button", () => {
    const btns = Array.from(document.querySelectorAll<HTMLButtonElement>('[data-atrium-part="question"] .ask-act button'));
    return btns.find((b) => b.textContent?.trim() === "Decline") ?? null;
  });
  await waitFor("step 3b turn to finish idle", () => daemon().threads[t3]?.state.type === "idle");
  const q3b = questionItems(h3.model)[1];
  const charsAfterDecline = assistantChars(h3.model);
  await check("3b. Next turn Decline resolves question as cancelled and streaming continues", q3b?.outcome === "cancelled" && charsAfterDecline > charsBeforeDecline, {
    outcome: q3b?.outcome,
    chars: [charsBeforeDecline, charsAfterDecline],
  });
  h3.release();

  // ---------------------------------------------------------------------------
  // 4. Inbox lists threads with their states, and marking a thread read works
  // ---------------------------------------------------------------------------
  // Clear unread flags on earlier demo threads (if the window lacked OS focus during Steps 1-3)
  // so Step 4 and Step 5 observe only the threads created for the inbox/Cmd+J checks.
  for (const prev of [t1, t2, t3]) {
    if (daemon().threads[prev]?.unread) {
      await command({ type: "mark_read", thread_id: prev });
    }
  }
  // Move away from any thread so the new threads stay unread/unfocused while arriving.
  openThread(null);
  setScreen("inbox");

  const tInboxApproval = (await command<ThreadSummary>({
    type: "start_thread",
    agent: { type: "fake", tokens_per_second: 160, chunk_chars: 16, duration_ms: 400, tool_call_every_ms: 0, approval_at_ms: 40 },
    message: "Inbox item: pending approval",
  })).thread_id;
  const tInboxQuestion = (await command<ThreadSummary>({
    type: "start_thread",
    agent: { type: "fake", tokens_per_second: 160, chunk_chars: 16, duration_ms: 400, tool_call_every_ms: 0, question_at_ms: 40 },
    message: "Inbox item: pending question",
  })).thread_id;
  const tInboxUnread = (await command<ThreadSummary>({
    type: "start_thread",
    agent: { type: "fake", tokens_per_second: 160, chunk_chars: 16, duration_ms: 200, tool_call_every_ms: 0 },
    message: "Inbox item: unread reply",
  })).thread_id;

  await waitFor("inbox items in daemon and DOM", () => {
    const reasons = new Map(daemon().inbox.map((i) => [i.thread_id, i.reason]));
    const domCount = document.querySelectorAll('[data-atrium-part="inbox-item"]').length;
    return reasons.get(tInboxApproval) === "approval" &&
      reasons.get(tInboxQuestion) === "question" &&
      reasons.get(tInboxUnread) === "reply" &&
      domCount >= 3;
  });

  const domReasons = Array.from(document.querySelectorAll<HTMLElement>('[data-atrium-part="inbox-item"]')).map((el) => el.dataset.reason);
  const idxApp = domReasons.indexOf("approval");
  const idxQ = domReasons.indexOf("question");
  const idxRep = domReasons.indexOf("reply");
  await check("4a. Inbox lists threads with their states in rank order", idxApp !== -1 && idxQ !== -1 && idxRep !== -1 && idxApp < idxQ && idxQ < idxRep, {
    domReasons,
    inboxCount: daemon().inbox.length,
  });

  // Mark the unread thread read by opening it (auto-marks when focused) or falling back to markRead().
  const wasFocused = windowFocused();
  openThread(tInboxUnread);
  let markMode = "auto-on-view";
  try {
    await waitFor("unread flag cleared on view", () => daemon().threads[tInboxUnread]?.unread === false, 2_200);
  } catch {
    markMode = `explicit-markRead (windowFocused=${wasFocused})`;
    markRead(tInboxUnread);
    await waitFor("unread flag cleared by mark_read", () => daemon().threads[tInboxUnread]?.unread === false);
  }
  await waitFor("reply item removed from inbox", () => !daemon().inbox.some((i) => i.thread_id === tInboxUnread));
  await check("4b. Marking a thread read clears unread and removes its reply inbox item", daemon().threads[tInboxUnread]?.unread === false && !daemon().inbox.some((i) => i.thread_id === tInboxUnread), {
    mode: markMode,
    remainingInbox: daemon().inbox.map((i) => ({ thread: i.thread_id, reason: i.reason })),
  });

  // ---------------------------------------------------------------------------
  // 5. ⌘J jumps through inbox items
  // ---------------------------------------------------------------------------
  setScreen("inbox");
  const dispatchCmdJ = () => {
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "j", metaKey: true, bubbles: true, cancelable: true }));
  };
  dispatchCmdJ();
  await waitFor("⌘J jump 1 to open top approval thread", () => screen() === "chat" && model().mainThread === tInboxApproval);
  const jump1 = model().mainThread;

  dispatchCmdJ();
  await waitFor("⌘J jump 2 to open question thread", () => screen() === "chat" && model().mainThread === tInboxQuestion);
  const jump2 = model().mainThread;

  dispatchCmdJ();
  await waitFor("⌘J jump 3 to wrap back to approval thread", () => screen() === "chat" && model().mainThread === tInboxApproval);
  const jump3 = model().mainThread;

  await check("5. Cmd+J cycles through inbox threads in rank order", jump1 === tInboxApproval && jump2 === tInboxQuestion && jump3 === tInboxApproval, {
    jump1,
    jump2,
    jump3,
    expected: [tInboxApproval, tInboxQuestion, tInboxApproval],
  });

  // ⌘⇧J goes back: from the approval (first) it wraps to the question (last), then back again.
  const dispatchCmdShiftJ = () => key(window, { key: "J", metaKey: true, shiftKey: true });
  dispatchCmdShiftJ();
  await waitFor("⌘⇧J jump 1 back to the question thread (wraps)", () => screen() === "chat" && model().mainThread === tInboxQuestion);
  const back1 = model().mainThread;
  dispatchCmdShiftJ();
  await waitFor("⌘⇧J jump 2 back to the approval thread", () => screen() === "chat" && model().mainThread === tInboxApproval);
  const back2 = model().mainThread;
  setScreen("inbox");
  dispatchCmdShiftJ();
  await waitFor("⌘⇧J from the inbox to the last item", () => screen() === "chat" && model().mainThread === tInboxQuestion);
  const back3 = model().mainThread;
  await check("5b. Cmd+Shift+J goes to the previous item, wrapping; from the inbox to the last", back1 === tInboxQuestion && back2 === tInboxApproval && back3 === tInboxQuestion, {
    back1,
    back2,
    back3,
    expected: [tInboxQuestion, tInboxApproval, tInboxQuestion],
  });

  // Clean up the pending inbox approval/question threads so they don't linger.
  await command({ type: "thread_command", thread_id: tInboxApproval, command: { type: "interrupt" } });
  await command({ type: "thread_command", thread_id: tInboxQuestion, command: { type: "interrupt" } });

  // ---------------------------------------------------------------------------
  // 6. Notifications: Settings → Notifications toggle only (posting & suppression tested via
  //    the daemon recording notifier in `cargo test -p atrium-daemon --test requests_notifications`)
  // ---------------------------------------------------------------------------
  setScreen("settings");
  setSettingsSection("notifications");
  await waitFor("notification switch to be enabled", () => {
    const sw = document.querySelector<HTMLButtonElement>('#setting-notify-approval button[role="switch"]');
    return sw !== null && !sw.disabled && sw.getAttribute("aria-checked") === "true";
  });
  await click("disable notify_approval toggle", () => document.querySelector<HTMLElement>('#setting-notify-approval button[role="switch"]'));
  await waitFor("notify_approval=false in daemon settings", async () => (await command<Record<string, unknown>>({ type: "get_settings" }))["notify_approval"] === false);
  const offSettings = await command<Record<string, unknown>>({ type: "get_settings" });

  await click("re-enable notify_approval toggle", () => document.querySelector<HTMLElement>('#setting-notify-approval button[role="switch"]'));
  await waitFor("notify_approval=true in daemon settings", async () => (await command<Record<string, unknown>>({ type: "get_settings" }))["notify_approval"] === true);
  const onSettings = await command<Record<string, unknown>>({ type: "get_settings" });

  await check("6a. Settings -> Notifications toggle persists in daemon settings (settings toggle only)", offSettings["notify_approval"] === false && onSettings["notify_approval"] === true, {
    afterOff: offSettings["notify_approval"],
    afterOn: onSettings["notify_approval"],
  });

  // ---------------------------------------------------------------------------
  // 7. Stop a turn; Shutdown to Ended; verify fake resume refusal
  // ---------------------------------------------------------------------------
  setScreen("chat");
  const t7Summary = await startChat(
    "Step 7: stop and resume",
    fakeChoice("stop", { type: "fake", tokens_per_second: 160, chunk_chars: 16, duration_ms: 60_000, tool_call_every_ms: 0 }),
  );
  if (!t7Summary) throw new Error("Step 7 chat did not start");
  const t7 = t7Summary.thread_id;
  const h7 = holdThread(t7);
  await waitFor("step 7 thread running with streamed text", () => daemon().threads[t7]?.state.type === "running" && assistantChars(h7.model) > 0);
  await click("Composer Stop button", () => document.querySelector<HTMLElement>('[data-atrium-part="composer"] button[aria-label="Stop the turn"]'));
  await waitFor("step 7 thread state after Stop and Stopped note", () =>
    daemon().threads[t7]?.state.type !== "running" &&
    h7.model.transcript.items.some((i) => i.kind === "note" && i.note.type === "turn_completed" && i.note.outcome === "interrupted"),
  );
  const stateAfterStop = daemon().threads[t7]?.state.type;
  const hasStoppedNote = h7.model.transcript.items.some(
    (i) => i.kind === "note" && i.note.type === "turn_completed" && i.note.outcome === "interrupted",
  );
  const charsAtStop = assistantChars(h7.model);
  await sleep(SAMPLE_GAP_MS);
  const charsAfterStopWait = assistantChars(h7.model);
  await check("7a. Stop ends the running turn", (stateAfterStop === "idle" || stateAfterStop === "ended") && hasStoppedNote && charsAfterStopWait === charsAtStop, {
    stateAfterStop,
    hasStoppedNote,
    chars: [charsAtStop, charsAfterStopWait],
    note: stateAfterStop === "idle" ? "Stop sends Interrupt (ends turn to idle); Shutdown ends session to ended" : "ended",
  });

  // End the session from the thread menu (Shutdown) so it reaches `ended`, after confirming,
  // then verify `resume_thread` on a synthetic (`atrium-fake:`) thread is refused (real resume
  // is tested in the human demo + 7d).
  await menuAction("end");
  await click("End session confirmation", () => document.querySelector<HTMLElement>('[data-atrium-part="confirm-ok"]'));
  await waitFor("step 7 thread to reach ended", () => daemon().threads[t7]?.state.type === "ended");
  await click("thread menu button (after End session)", () => document.querySelector<HTMLElement>('.card.main [data-atrium-part="thread-menu-button"]'));
  const endDisabled = document.querySelector<HTMLButtonElement>('[data-atrium-part="thread-menu"] [data-action="end"]')?.disabled === true;
  key(document.querySelector('[data-atrium-part="thread-menu"]')!, { key: "Escape" });
  await check("7b. End session from the thread menu (confirmed) reaches Ended; the item is then off", daemon().threads[t7]?.state.type === "ended" && endDisabled, {
    state: daemon().threads[t7]?.state.type,
    endItemDisabled: endDisabled,
  });
  let refused = false;
  let resumeDetail = "";
  try {
    const resumed = await command<ThreadSummary>({ type: "resume_thread", thread_id: t7 });
    resumeDetail = `unexpectedly resumed (incarnation=${resumed.incarnation})`;
  } catch (e) {
    refused = isCommandFailure(e) && e.kind === "rejected" && e.code === "invalid" && e.message.includes("can't be resumed");
    resumeDetail = formatErr(e);
  }
  await check("7c. resume of a synthetic thread is refused (fake-agent limit; real resume: human demo + 7d)", refused, resumeDetail);
  h7.release();

  // ---------------------------------------------------------------------------
  // 8. Kill the daemon mid-turn: the app reconnects and the thread shows Interrupted
  // ---------------------------------------------------------------------------
  const t8Summary = await startChat(
    "Step 8: kill daemon mid-turn",
    fakeChoice("kill", { type: "fake", tokens_per_second: 160, chunk_chars: 16, duration_ms: 180_000, tool_call_every_ms: 0 }),
  );
  if (!t8Summary) throw new Error("Step 8 chat did not start");
  const t8 = t8Summary.thread_id;
  const h8 = holdThread(t8);
  await waitFor("step 8 thread running with streamed text", () => daemon().threads[t8]?.state.type === "running" && assistantChars(h8.model) > 0);
  const charsBeforeKill = assistantChars(h8.model);

  await writeReport({ kind: "m1-demo-kill-daemon", thread: t8, chars: charsBeforeKill });
  await waitFor("app to notice daemon is gone", () => daemon().connection.type !== "connected", RECONNECT_TIMEOUT_MS);
  await waitFor("app to reconnect and show Interrupted", () => connected() && daemon().threads[t8]?.state.type === "interrupted", RECONNECT_TIMEOUT_MS);
  await waitFor("step 8 transcript to close", () => transcriptClosed(h8.model), RECONNECT_TIMEOUT_MS);
  const charsAfterKill = assistantChars(h8.model);
  await check("8. Kill daemon mid-turn: app reconnects and thread shows Interrupted", daemon().threads[t8]?.state.type === "interrupted" && transcriptClosed(h8.model) && charsAfterKill >= charsBeforeKill, {
    state: daemon().threads[t8]?.state.type,
    chars: [charsBeforeKill, charsAfterKill],
  });
  h8.release();

  // ---------------------------------------------------------------------------
  // 9. Rename, then export to Markdown and JSONL, from the thread menu
  // ---------------------------------------------------------------------------
  if (!exportDir) throw new Error("m1-demo requires an export directory (ATRIUM_BENCH=m1-demo:<dir>)");
  // The save dialog is bypassed: "Export…" saves into the demo's folder under the suggested name.
  setSaveDirOverride(exportDir);
  setScreen("chat");
  openThread(t1);
  await waitFor("step 9 thread header", () => model().mainThread === t1 && document.querySelector('.card.main [data-atrium-part="thread-menu-button"]') !== null);
  const renamed = "M1 demo: renamed chat";
  await menuAction("rename");
  let renameField: HTMLInputElement | null = null;
  await waitFor("the rename field", () => (renameField = document.querySelector<HTMLInputElement>('[data-atrium-part="rename"]')) !== null && document.activeElement === renameField);
  typeInto(renameField!, `  ${renamed}  `);
  key(renameField!, { key: "Enter" });
  await waitFor("the daemon to store the new title", () => daemon().threads[t1]?.title === renamed);
  await waitFor("the header to show the new title", () => document.querySelector(".card.main .head .title")?.textContent === renamed && document.querySelector('[data-atrium-part="rename"]') === null);
  await check("9a. Rename from the thread menu stores the title and the header shows it", daemon().threads[t1]?.title === renamed, {
    thread: t1,
    title: daemon().threads[t1]?.title,
    sidebar: Array.from(document.querySelectorAll(".chat-row .t")).some((el) => el.textContent === renamed),
  });

  const mdPath = `${exportDir}/${exportFileName(renamed, "markdown")}`;
  const jsonlPath = `${exportDir}/${exportFileName(renamed, "jsonl")}`;
  await menuAction("export-markdown");
  await waitFor("the Markdown export toast", () => toast()?.text.includes(mdPath) === true);
  const mdToast = toast()?.text;
  await menuAction("export-jsonl");
  await waitFor("the JSONL export toast", () => toast()?.text.includes(jsonlPath) === true);
  const jsonlToast = toast()?.text;
  // Round 2 minor 5: the same name again is refused, never replaced, and the app says so.
  await menuAction("export-markdown");
  await waitFor("the already-exists toast", () => toast()?.text === `Couldn't export the chat: ${EXISTS_TEXT}`);
  const againToast = toast()?.text;
  await check("9b. Export as Markdown and as JSONL from the thread menu writes both files; the same name again is refused with a clear message", Boolean(mdToast?.startsWith("Exported") && jsonlToast?.startsWith("Exported")), {
    thread: t1,
    mdPath,
    mdToast,
    jsonlPath,
    jsonlToast,
    againToast,
  });

  // ---------------------------------------------------------------------------
  // 10. Archive from the thread menu, after confirming
  // ---------------------------------------------------------------------------
  await menuAction("archive");
  await waitFor("the archive confirmation", () => document.querySelector('[data-atrium-part="confirm"]') !== null);
  // Cancelling keeps the thread.
  await click("Cancel in the confirmation", () => Array.from(document.querySelectorAll<HTMLElement>('[data-atrium-part="confirm"] button')).find((b) => b.textContent === "Cancel") ?? null);
  await waitFor("the confirmation to close", () => document.querySelector('[data-atrium-part="confirm"]') === null);
  await sleep(SAMPLE_GAP_MS);
  const keptAfterCancel = t1 in daemon().threads;
  await menuAction("archive");
  await click("Archive confirmation", () => document.querySelector<HTMLElement>('[data-atrium-part="confirm-ok"]'));
  await waitFor("archived thread to leave daemon state", () => !(t1 in daemon().threads) && !daemon().order.includes(t1));
  await waitFor("the main card to go back to Home", () => model().mainThread === null);
  const listedAfterArchive = await command<ThreadSummary[]>({ type: "list_threads" });
  let rejectedAfterArchive = false;
  try {
    await command({ type: "mark_read", thread_id: t1 });
  } catch (e) {
    rejectedAfterArchive = isCommandFailure(e) && e.kind === "rejected" && e.code === "unknown_thread";
  }
  await check("10. Archive from the thread menu (cancel keeps it; confirm archives) removes it from lists and inbox", keptAfterCancel && !listedAfterArchive.some((t) => t.thread_id === t1) && rejectedAfterArchive, {
    archived: t1,
    keptAfterCancel,
    inList: listedAfterArchive.some((t) => t.thread_id === t1),
    rejectedUnknownThread: rejectedAfterArchive,
  });

  // ---------------------------------------------------------------------------
  // 11. Batch approve and deny in the inbox
  // ---------------------------------------------------------------------------
  setScreen("inbox");
  const batchThreads: string[] = [];
  for (let i = 1; i <= 3; i++) {
    batchThreads.push((await command<ThreadSummary>({
      type: "start_thread",
      agent: { type: "fake", tokens_per_second: 160, chunk_chars: 16, duration_ms: 300, tool_call_every_ms: 0, approval_at_ms: 40 },
      message: `Batch approval ${i}`,
    })).thread_id);
  }
  const batchHolds = batchThreads.map((id) => holdThread(id));
  const pickOf = (id: string) => document.querySelector<HTMLInputElement>(`[data-atrium-part="inbox-item"][data-thread="${id}"] [data-atrium-part="batch-pick"]`);
  await waitFor("three approvals with checkboxes in the inbox", () => batchThreads.every((id) => daemon().threads[id]?.state.type === "needs_approval" && pickOf(id) !== null));
  await click("Select all approvals", () => document.querySelector<HTMLElement>('[data-atrium-part="batch-all"]'));
  await waitFor("all three selected", () => batchThreads.every((id) => pickOf(id)?.checked === true));
  // Leave the third one out of the Allow batch: X on its focused checkbox (round 2: the inbox's
  // keys work from a checkbox, and X toggles the card that holds focus).
  const pickedRow = () => document.querySelector<HTMLElement>('[data-atrium-part="inbox-item"].on')?.dataset.thread;
  pickOf(batchThreads[2]!)!.focus();
  // Round 3 blocker 2: focus moving into a row picks it.
  await waitFor("focus to pick the third row", () => pickedRow() === batchThreads[2]);
  key(pickOf(batchThreads[2]!)!, { key: "x" });
  await waitFor("two selected", () => pickOf(batchThreads[2]!)?.checked === false && document.querySelector('[data-atrium-part="batch-allow"]')?.textContent === "Allow once (2)");
  const xFromCheckbox = pickOf(batchThreads[0]!)?.checked === true && pickOf(batchThreads[1]!)?.checked === true;
  // J from a checkbox moves the pick and focus together; X then toggles the newly picked row,
  // never the row whose checkbox had focus.
  pickOf(batchThreads[0]!)!.focus();
  await waitFor("focus to pick the first row", () => pickedRow() === batchThreads[0]);
  key(pickOf(batchThreads[0]!)!, { key: "j" });
  await waitFor("J to move the pick", () => pickedRow() !== batchThreads[0]);
  const afterJ = pickedRow();
  const focusOnList = document.activeElement === document.querySelector('[data-atrium-part="inbox"]');
  const before = batchThreads.map((id) => pickOf(id)?.checked);
  key(document.activeElement ?? document.body, { key: "x" });
  const pickedBatch = afterJ !== undefined && batchThreads.includes(afterJ);
  if (pickedBatch) await waitFor("X to toggle the picked row", () => pickOf(afterJ)?.checked !== before[batchThreads.indexOf(afterJ)]);
  else await sleep(SAMPLE_GAP_MS);
  // Only the picked row changed (or nothing, when the picked row has no approval).
  const othersKept = batchThreads.every((id, i) => id === afterJ || pickOf(id)?.checked === before[i]);
  if (pickedBatch) {
    key(document.activeElement ?? document.body, { key: "x" });
    await waitFor("X again to restore it", () => batchThreads.every((id, i) => pickOf(id)?.checked === before[i]));
  }
  const jThenX = focusOnList && othersKept;
  await click("Allow once (2)", () => document.querySelector<HTMLElement>('[data-atrium-part="batch-allow"]'));
  await waitFor("the two allowed approvals to resolve", () => batchHolds.slice(0, 2).every((h) => approvalItems(h.model)[0]?.outcome === "approved"));
  await waitFor("the Allow summary", () => document.querySelector('[data-atrium-part="batch-result"]')?.textContent?.startsWith("Allowed 2.") === true);
  const allowSummary = document.querySelector('[data-atrium-part="batch-result"]')?.textContent;
  await click("the third approval's checkbox", () => pickOf(batchThreads[2]!));
  await click("Deny (1)", () => document.querySelector<HTMLElement>('[data-atrium-part="batch-deny"]'));
  await waitFor("the denied approval to resolve", () => approvalItems(batchHolds[2]!.model)[0]?.outcome === "denied");
  await waitFor("the Deny summary", () => document.querySelector('[data-atrium-part="batch-result"]')?.textContent?.startsWith("Denied 1.") === true);
  const denySummary = document.querySelector('[data-atrium-part="batch-result"]')?.textContent;
  // Leaving the inbox drops the last result (round 2, minor 7).
  setScreen("settings");
  await waitFor("the inbox to close", () => document.querySelector('[data-atrium-part="inbox"]') === null);
  setScreen("inbox");
  await waitFor("the inbox again", () => document.querySelector('[data-atrium-part="inbox"]') !== null);
  const resultAfterReturn = document.querySelector('[data-atrium-part="batch-result"]') !== null;
  await waitFor("the batch threads to finish their turns", () => batchThreads.every((id) => daemon().threads[id]?.state.type === "idle"));
  const outcomes = batchHolds.map((h) => approvalItems(h.model)[0]?.outcome);
  const toolStatuses = batchHolds.map((h) => toolItems(h.model)[0]?.status);
  await check("11. Batch: select all, X unselects the third from its checkbox, J from a checkbox then X acts on the new pick, Allow once two together, Deny the third; the result clears on leaving", outcomes.join() === "approved,approved,denied" && toolStatuses.join() === "succeeded,succeeded,denied" && xFromCheckbox && jThenX && !resultAfterReturn, {
    outcomes,
    xFromCheckbox,
    jThenX,
    afterJ,
    pickedBatch,
    focusOnList,
    resultAfterReturn,
    toolStatuses,
    allowSummary,
    denySummary,
  });
  for (const h of batchHolds) h.release();

  // ---------------------------------------------------------------------------
  // 12. Settings: add a rule in the rules editor; Security lists what is always blocked
  // ---------------------------------------------------------------------------
  setScreen("settings");
  setSettingsSection("approvals");
  await click("Add a rule…", () => document.querySelector<HTMLElement>('[data-atrium-part="setting-action-add-rule"]'));
  let commandField: HTMLInputElement | null = null;
  await waitFor("the rules editor", () => (commandField = document.querySelector<HTMLInputElement>('[data-atrium-part="rule-command"]')) !== null);
  const rulesBefore = (await command<Rule[]>({ type: "list_rules" })).length;
  typeInto(commandField!, "cargo test && rm -rf target");
  await click("Add allow rule (refused)", () => document.querySelector<HTMLElement>('[data-atrium-part="rule-add"]'));
  await waitFor("the command's error", () => document.querySelector('[data-atrium-part="rule-editor"] .err') !== null);
  const refusedText = document.querySelector('[data-atrium-part="rule-editor"] .err')?.textContent;
  await sleep(SAMPLE_GAP_MS);
  // Round 2 blocker 1: an allow rule on a shell would allow whatever it runs.
  typeInto(commandField!, "bash -c 'cargo test'");
  await click("Add allow rule (refused: a shell)", () => document.querySelector<HTMLElement>('[data-atrium-part="rule-add"]'));
  await waitFor("the shell's error", () => document.querySelector('[data-atrium-part="rule-editor"] .err')?.textContent?.includes("can't allow bash") === true);
  const shellRefusedText = document.querySelector('[data-atrium-part="rule-editor"] .err')?.textContent;
  // Round 2 blocker 2: a command rule has no "Only for paths inside".
  const commandPathScope = document.querySelector('[data-atrium-part="rule-path-scope"]') !== null;
  await sleep(SAMPLE_GAP_MS);
  const rulesAfterRefused = (await command<Rule[]>({ type: "list_rules" })).length;
  typeInto(commandField!, "cargo test");
  await waitFor("the rule preview", () => document.querySelector('[data-atrium-part="rule-preview"]')?.textContent?.includes("cargo test") === true);
  const preview = document.querySelector('[data-atrium-part="rule-preview"]')?.textContent;
  await click("Add allow rule", () => document.querySelector<HTMLElement>('[data-atrium-part="rule-add"]'));
  await waitFor("the rules editor to close", () => document.querySelector('[data-atrium-part="rule-editor"]') === null);
  const rulesAfter = await command<Rule[]>({ type: "list_rules" });
  const made = rulesAfter.find((r) => r.origin.type === "settings" && r.matcher.type === "command_prefix" && r.matcher.argv.join(" ") === "cargo test");
  await waitFor("the new rule in the list", () => Array.from(document.querySelectorAll('[data-atrium-part="rules"] .s-item')).some((el) => el.textContent?.includes("commands that start with") && el.textContent.includes("cargo test")));
  await check("12a. Rules editor refuses a compound command and an allow rule on bash -c, offers no path limit on commands, then adds a command-prefix allow rule", rulesAfterRefused === rulesBefore && !commandPathScope && made !== undefined && made.decision === "allow" && made.scope.type === "global" && made.tool_kind === "execute" && made.path_scope === null, {
    refusedText,
    shellRefusedText,
    commandPathScope,
    preview,
    rulesBefore,
    rulesAfterRefused,
    made,
  });

  setSettingsSection("security");
  await waitFor("Security's lists", () => document.querySelectorAll('[data-atrium-part="always-blocked"] .s-item').length > 0 && document.querySelector('[data-atrium-part="known-limits"]') !== null);
  const blocked = Array.from(document.querySelectorAll<HTMLElement>('[data-atrium-part="always-blocked"] .s-item')).map((el) => el.dataset.id);
  const limits = Array.from(document.querySelectorAll<HTMLElement>('[data-atrium-part="known-limits"] .s-item')).map((el) => el.dataset.id);
  const heading = document.querySelector("#setting-always-blocked .rl b")?.textContent;
  await check("12b. Settings → Security lists Always blocked (HARD_DENY_DEFAULTS) and the git config and same-user limits", heading === "Always blocked (for calls Atrium sees)" && blocked.length === 7 && blocked.includes("credential-read") && limits.includes("git-config") && limits.includes("same-user"), {
    heading,
    blocked,
    limits,
  });

  // ---------------------------------------------------------------------------
  // 13. The enforcement badge opens the details
  // ---------------------------------------------------------------------------
  setScreen("chat");
  openThread(batchThreads[0]!);
  await click("the enforcement badge", () => document.querySelector<HTMLElement>('.card.main [data-atrium-part="enforcement-badge"]'));
  await waitFor("the enforcement details", () => document.querySelectorAll('[data-atrium-part="enforcement-coverage"] .enf-row').length > 0);
  const coverageRowsShown = document.querySelectorAll('[data-atrium-part="enforcement-coverage"] .enf-row').length;
  const sections = ["coverage", "policy", "isolation"].filter((x) => document.querySelector(`[data-atrium-part="enforcement-${x}"]`) !== null);
  key(document.querySelector('[data-atrium-part="enforcement-details"]')!, { key: "Escape" });
  await waitFor("the details to close", () => document.querySelector('[data-atrium-part="enforcement-details"]') === null);
  await check("13. Enforcement badge opens the details: coverage per tool class, then policy and isolation apart", coverageRowsShown === 10 && sections.length === 3, {
    level: daemon().threads[batchThreads[0]!]?.enforcement?.level,
    coverageRowsShown,
    sections,
  });

  await writeReport({
    kind: "m1-demo",
    pass: checks.every((c) => c.pass),
    checks,
    exportPaths: {
      markdown: mdPath,
      jsonl: jsonlPath,
      expectedText: step1Msg,
    },
  });
}
