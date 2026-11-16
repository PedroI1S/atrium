// End-to-end M1 gate demo (`ATRIUM_BENCH=m1-demo`, run by app/scripts/m1_demo.py against a real
// `atriumd` in a throwaway ATRIUM_HOME with the synthetic agent).
//
// Walks through M1's promises in order:
//   1. Home chat with the fake agent streams;
//   2. Fake approval (`approval_at_ms`): Allow once, Deny next turn, "Always" rule + next turn skips;
//   3. Fake question (`question_at_ms`): Answer, Decline next turn (streaming continues both times);
//   4. Inbox lists threads with their states, and marking a thread read works;
//   5. ⌘J jumps through inbox items in rank order;
//   6. Settings → Notifications toggles persist in daemon settings (daemon recording notifier
//      checked separately by m1_demo.py via `cargo test -p atrium-daemon --test requests_notifications`);
//   7. Stop a turn / Shutdown to Ended / attempt Resume after Ended;
//   8. Kill the daemon mid-turn: the app reconnects and the thread shows `Interrupted`;
//   9. Export a thread to Markdown and JSONL;
//  10. Archive a thread (`archive_thread`).

import { markRead, sendMessage, startChat, type AgentChoice } from "./chat/actions";
import { type ThreadModel, holdThread } from "./chat/threads";
import { command, daemon } from "./daemon/client";
import { type AgentLaunch, type Rule, type ThreadSummary, isCommandFailure } from "./daemon/types";
import { writeReport } from "./platform";
import { sleep } from "./roundtrip";
import { screen, setScreen, setSettingsSection, windowFocused } from "./state";
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

function formatErr(e: unknown): string {
  if (isCommandFailure(e)) {
    return e.kind === "rejected" ? `${e.kind} (${e.code}): ${e.message}` : `${e.kind}: ${e.message}`;
  }
  return e instanceof Error ? e.message : String(e);
}

export async function runM1Demo(exportDir: string | null): Promise<void> {
  try {
    await runM1DemoInner(exportDir);
  } catch (e) {
    throw new Error(formatErr(e));
  }
}

async function runM1DemoInner(exportDir: string | null): Promise<void> {
  const checks: Check[] = [];
  const check = (name: string, pass: boolean, detail: unknown) => {
    checks.push({ name, pass, detail: typeof detail === "string" ? detail : JSON.stringify(detail) });
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
  check("1. Home chat with the fake agent streams", model().mainThread === t1 && c2 > c1 && tools1.length > 0 && document.querySelector(".you")?.textContent === step1Msg, {
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
  check("2a. Fake approval shows NeedsApproval in queue and Allow succeeds", inQueue && req2a?.outcome === "approved" && tool2a?.status === "succeeded", {
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
  check("2b. Next turn approval Deny denies the call", req2b?.outcome === "denied" && tool2b?.status === "denied", {
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
  check("2c. Always creates a rule and the next turn runs without asking", ruleCreated && approvalItems(h2.model).length === 3 && toolItems(h2.model)[3]?.status === "succeeded", {
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
  check("3a. Fake question shows NeedsInput form and Answer resumes streaming", q3a?.outcome === "answered" && charsAfterAnswer > charsBeforeAnswer, {
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
  check("3b. Next turn Decline resolves question as cancelled and streaming continues", q3b?.outcome === "cancelled" && charsAfterDecline > charsBeforeDecline, {
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
  check("4a. Inbox lists threads with their states in rank order", idxApp !== -1 && idxQ !== -1 && idxRep !== -1 && idxApp < idxQ && idxQ < idxRep, {
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
  check("4b. Marking a thread read clears unread and removes its reply inbox item", daemon().threads[tInboxUnread]?.unread === false && !daemon().inbox.some((i) => i.thread_id === tInboxUnread), {
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

  check("5. Cmd+J cycles through inbox threads in rank order", jump1 === tInboxApproval && jump2 === tInboxQuestion && jump3 === tInboxApproval, {
    jump1,
    jump2,
    jump3,
    expected: [tInboxApproval, tInboxQuestion, tInboxApproval],
  });

  // Clean up the pending inbox approval/question threads so they don't linger.
  await command({ type: "thread_command", thread_id: tInboxApproval, command: { type: "interrupt" } });
  await command({ type: "thread_command", thread_id: tInboxQuestion, command: { type: "interrupt" } });

  // ---------------------------------------------------------------------------
  // 6. Notifications: Settings → Notifications toggles (daemon notifier checked by m1_demo.py)
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

  check("6a. Settings -> Notifications toggles persist in daemon settings", offSettings["notify_approval"] === false && onSettings["notify_approval"] === true, {
    afterOff: offSettings["notify_approval"],
    afterOn: onSettings["notify_approval"],
  });

  // ---------------------------------------------------------------------------
  // 7. Stop a turn; thread ends; resume it
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
  await waitFor("step 7 thread state after Stop", () => daemon().threads[t7]?.state.type !== "running");
  const stateAfterStop = daemon().threads[t7]?.state.type;
  check("7a. Stop ends the running turn", stateAfterStop === "idle" || stateAfterStop === "ended", {
    stateAfterStop,
    note: stateAfterStop === "idle" ? "Stop sends Interrupt (ends turn to idle); Shutdown ends session to ended" : "ended",
  });

  // End the session via Shutdown so it reaches `ended`, then attempt `resume_thread` right after `ended`.
  await command({ type: "thread_command", thread_id: t7, command: { type: "shutdown" } });
  await waitFor("step 7 thread to reach ended", () => daemon().threads[t7]?.state.type === "ended");
  let resumeOk = false;
  let resumeDetail = "";
  try {
    const resumed = await command<ThreadSummary>({ type: "resume_thread", thread_id: t7 });
    await waitFor("resumed incarnation to stream", () => resumed.incarnation === 2 && daemon().threads[t7]?.incarnation === 2);
    resumeOk = true;
    resumeDetail = `incarnation=${resumed.incarnation}`;
  } catch (e) {
    resumeOk = false;
    resumeDetail = isCommandFailure(e)
      ? e.kind === "rejected"
        ? `${e.kind} (${e.code}): ${e.message}`
        : `${e.kind}: ${e.message}`
      : String(e);
  }
  check("7b. Resume synthetic thread right after Ended streams a new incarnation", resumeOk, resumeDetail);
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
  check("8. Kill daemon mid-turn: app reconnects and thread shows Interrupted", daemon().threads[t8]?.state.type === "interrupted" && transcriptClosed(h8.model) && charsAfterKill >= charsBeforeKill, {
    state: daemon().threads[t8]?.state.type,
    chars: [charsBeforeKill, charsAfterKill],
  });
  h8.release();

  // ---------------------------------------------------------------------------
  // 9. Export the thread to Markdown and JSONL
  // ---------------------------------------------------------------------------
  if (!exportDir) throw new Error("m1-demo requires an export directory (ATRIUM_BENCH=m1-demo:<dir>)");
  const mdPath = `${exportDir}/thread.md`;
  const jsonlPath = `${exportDir}/thread.jsonl`;
  const mdRes = await command<{ bytes: number }>({ type: "export_thread", thread_id: t1, format: "markdown", path: mdPath });
  const jsonlRes = await command<{ bytes: number }>({ type: "export_thread", thread_id: t1, format: "jsonl", path: jsonlPath });
  check("9a. Export command writes Markdown and JSONL files", mdRes.bytes > 0 && jsonlRes.bytes > 0, {
    thread: t1,
    mdPath,
    mdBytes: mdRes.bytes,
    jsonlPath,
    jsonlBytes: jsonlRes.bytes,
  });

  // ---------------------------------------------------------------------------
  // 10. Archive, if M1 has it
  // ---------------------------------------------------------------------------
  await command({ type: "archive_thread", thread_id: t1 });
  await waitFor("archived thread to leave daemon state", () => !(t1 in daemon().threads) && !daemon().order.includes(t1));
  const listedAfterArchive = await command<ThreadSummary[]>({ type: "list_threads" });
  let rejectedAfterArchive = false;
  try {
    await command({ type: "mark_read", thread_id: t1 });
  } catch (e) {
    rejectedAfterArchive = isCommandFailure(e) && e.kind === "rejected" && e.code === "unknown_thread";
  }
  check("10. Archive removes thread from lists and inbox", !listedAfterArchive.some((t) => t.thread_id === t1) && rejectedAfterArchive, {
    archived: t1,
    inList: listedAfterArchive.some((t) => t.thread_id === t1),
    rejectedUnknownThread: rejectedAfterArchive,
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
