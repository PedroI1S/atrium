
// @vitest-environment happy-dom

import { createComponent, createSignal } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonCommand } from "./daemon/types";
import { cardState } from "./chat/requests";
import type { ApprovalItem, QuestionItem } from "./chat/transcript";

const commandMock = vi.fn<(cmd: DaemonCommand) => Promise<null>>();

vi.mock("./daemon/client", () => ({
  command: (cmd: DaemonCommand) => commandMock(cmd),
  pageBefore: vi.fn(),
  viewThread: vi.fn(),
}));

import { ApprovalCard, QuestionCard } from "./ui/RequestCard";
import { Transcript } from "./chat/transcript";

const makeQuestion = (over: Partial<QuestionItem> = {}): QuestionItem => ({
  kind: "question",
  key: "k-q",
  first: 1,
  last: 1,
  incarnation: 1,
  v: 0,
  bytes: 0,
  requestId: "req_1",
  turn: 1,
  outcome: null,
  stale: false,
  at: Date.now(),
  questions: [
    {
      id: "q1",
      prompt: "Pick an option",
      options: [
        { id: "a", label: "Alpha", description: null },
        { id: "b", label: "Beta", description: null },
      ],
      multi_select: false,
      allow_text: true,
    },
  ],
  ...over,
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
}

describe("question card states and Decline button (CR-15)", () => {
  let host: HTMLElement;
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    commandMock.mockReset();
    host = document.createElement("div");
    document.body.appendChild(host);
  });

  afterEach(() => {
    dispose?.();
    dispose = undefined;
    host.remove();
  });

  it("maps question local and remote states accurately", () => {
    const q = makeQuestion();
    expect(cardState(q, undefined)).toEqual({ kind: "fresh" });
    expect(cardState(q, { status: "sending", action: "decline" })).toEqual({ kind: "sending", action: "decline" });
    expect(cardState(q, { status: "accepted", action: "decline" })).toEqual({ kind: "sent", action: "decline" });
    expect(cardState({ ...q, outcome: "cancelled" }, { status: "accepted", action: "decline" })).toEqual({
      kind: "resolved",
      here: true,
      outcome: "cancelled",
      text: "Declined",
    });
    expect(cardState({ ...q, outcome: "cancelled" }, { status: "accepted", action: "cancel" })).toEqual({
      kind: "resolved",
      here: true,
      outcome: "cancelled",
      text: "Withdrawn: the turn ended first",
    });
    expect(cardState({ ...q, outcome: "cancelled" }, undefined)).toEqual({
      kind: "resolved",
      here: false,
      outcome: "cancelled",
      text: "Withdrawn: the turn ended first",
    });
    expect(
      cardState(q, { status: "refused", action: "decline", code: "expired", message: "expired", unknownOutcome: false }),
    ).toMatchObject({ kind: "stale" });
    expect(cardState({ ...q, stale: true }, undefined)).toMatchObject({ kind: "stale" });
  });

  it("renders Decline beside Stop the turn, sends decline_question, and updates card state", async () => {
    const pending = deferred<null>();
    commandMock.mockReturnValueOnce(pending.promise);

    const t = new Transcript();
    t.apply([
      { id: 1, thread_id: "t-decline", envelope: { seq: 1, at_ms: 1, incarnation: 1, event: { type: "turn_started", turn: 1 } } },
      { id: 2, thread_id: "t-decline", envelope: { seq: 2, at_ms: 2, incarnation: 1, event: { type: "question_asked", request_id: "q-decl", questions: makeQuestion().questions } } },
    ]);
    const q = t.items.find((i): i is QuestionItem => i.kind === "question")!;
    const [tick, setTick] = createSignal(0);

    dispose = render(() => createComponent(QuestionCard, { threadId: "t-decline", item: q, tick }), host);

    const actionButtons = Array.from(host.querySelectorAll(".ask-act button")) as HTMLButtonElement[];
    expect(actionButtons.map((b) => b.textContent?.trim())).toEqual(["Answer↵", "Decline", "Stop the turn"]);

    const declineBtn = actionButtons[1]!;
    declineBtn.click();
    await flush();
    setTick((n) => n + 1);

    expect(commandMock).toHaveBeenCalledTimes(1);
    expect(commandMock).toHaveBeenCalledWith({
      type: "decline_question",
      thread_id: "t-decline",
      incarnation: 1,
      request_id: "q-decl",
    });
    expect(host.querySelector(".ask")?.getAttribute("data-state")).toBe("sending");
    expect(host.querySelector(".ask-state")?.textContent).toBe("Declining…");
    expect(host.querySelector(".ask-act")).toBeNull();

    pending.resolve(null);
    await flush();
    setTick((n) => n + 1);

    expect(host.querySelector(".ask")?.getAttribute("data-state")).toBe("sent");
    expect(host.querySelector(".ask-state")?.textContent).toBe("Declined. Waiting for the agent.");

    t.apply([
      { id: 3, thread_id: "t-decline", envelope: { seq: 3, at_ms: 3, incarnation: 1, event: { type: "request_resolved", request_id: "q-decl", outcome: "cancelled" } } },
    ]);
    setTick((n) => n + 1);

    expect(host.querySelector(".ask")?.getAttribute("data-state")).toBe("resolved");
    expect(host.querySelector(".ask-state")?.textContent).toBe("Declined");
  });

  it("allows retrying Decline after a non-expired refusal", async () => {
    commandMock.mockRejectedValueOnce({ kind: "rejected", code: "internal", message: "the store is unavailable" });

    const q = makeQuestion({ requestId: "q-retry" });
    const [tick, setTick] = createSignal(0);

    dispose = render(() => createComponent(QuestionCard, { threadId: "t-retry", item: q, tick }), host);

    const declineBtn = host.querySelectorAll(".ask-act button")[1] as HTMLButtonElement;
    declineBtn.click();
    await flush();
    setTick((n) => n + 1);

    expect(host.querySelector(".ask")?.getAttribute("data-state")).toBe("failed");
    expect(host.querySelector(".ask-state")?.textContent).toBe("the store is unavailable");
    expect(host.querySelectorAll(".ask-act button")).toHaveLength(3);
  });
});

const makeApproval = (over: Partial<ApprovalItem> = {}): ApprovalItem => ({
  kind: "approval",
  key: "k-a",
  first: 1,
  last: 1,
  incarnation: 1,
  v: 0,
  bytes: 0,
  requestId: "req_a1",
  turn: 1,
  outcome: null,
  stale: false,
  at: Date.now(),
  callId: "c1",
  name: "Bash",
  toolKind: "execute",
  title: 'for f in *.rs; do wc -l "$f"; done',
  input: { command: 'for f in *.rs; do wc -l "$f"; done' },
  reason: null,
  rememberable: false,
  unread: null,
  ...over,
});

describe("approval card Unread and Exhausted notice (contract rev 4 §9)", () => {
  let host: HTMLElement;
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    commandMock.mockReset();
    host = document.createElement("div");
    document.body.appendChild(host);
  });

  afterEach(() => {
    dispose?.();
    dispose = undefined;
    host.remove();
  });

  it("shows no unread notice and offers Always… for an ordinary rememberable approval", () => {
    const a = makeApproval({ requestId: "req-plain", rememberable: true, unread: null });
    const [tick] = createSignal(0);

    dispose = render(() => createComponent(ApprovalCard, { threadId: "t-plain", item: a, tick }), host);

    expect(host.querySelector('[data-atrium-part="approval-unread"]')).toBeNull();
    expect(host.querySelector('button[aria-haspopup="menu"]')).not.toBeNull();
    const labels = Array.from(host.querySelectorAll(".ask-act button")).map((b) => b.textContent?.trim());
    expect(labels).toEqual(["Allow once↵", "Always…", "Denyesc"]);
  });

  it("shows the Unread notice, hides Always… even if rememberable is true, and keeps Allow once working", async () => {
    commandMock.mockResolvedValueOnce(null);
    const a = makeApproval({ requestId: "req-unread", rememberable: true, unread: "unread" });
    const [tick, setTick] = createSignal(0);

    dispose = render(() => createComponent(ApprovalCard, { threadId: "t-unread", item: a, tick }), host);

    const notice = host.querySelector('[data-atrium-part="approval-unread"]');
    expect(notice?.textContent).toBe("Atrium can't read this command in full.");
    expect(notice?.getAttribute("data-unread")).toBe("unread");
    expect(host.querySelector('button[aria-haspopup="menu"]')).toBeNull();

    const buttons = Array.from(host.querySelectorAll(".ask-act button")) as HTMLButtonElement[];
    expect(buttons.map((b) => b.textContent?.trim())).toEqual(["Allow once↵", "Denyesc"]);

    buttons[0]!.click();
    await flush();
    setTick((n) => n + 1);

    expect(commandMock).toHaveBeenCalledTimes(1);
    expect(commandMock).toHaveBeenCalledWith({
      type: "resolve_approval",
      thread_id: "t-unread",
      incarnation: 1,
      request_id: "req-unread",
      decision: { type: "allow" },
      remember: null,
    });
  });

  it("shows the Exhausted notice with second sentence, hides Always…, and keeps Deny working", async () => {
    commandMock.mockResolvedValueOnce(null);
    const a = makeApproval({ requestId: "req-exhausted", rememberable: true, unread: "exhausted" });
    const [tick, setTick] = createSignal(0);

    dispose = render(() => createComponent(ApprovalCard, { threadId: "t-exhausted", item: a, tick }), host);

    const notice = host.querySelector('[data-atrium-part="approval-unread"]');
    expect(notice?.textContent).toBe("Atrium can't read this command in full. It was too long or too complex to check.");
    expect(notice?.getAttribute("data-unread")).toBe("exhausted");
    expect(host.querySelector('button[aria-haspopup="menu"]')).toBeNull();

    const buttons = Array.from(host.querySelectorAll(".ask-act button")) as HTMLButtonElement[];
    expect(buttons.map((b) => b.textContent?.trim())).toEqual(["Allow once↵", "Denyesc"]);

    buttons[1]!.click();
    await flush();
    setTick((n) => n + 1);

    expect(commandMock).toHaveBeenCalledTimes(1);
    expect(commandMock).toHaveBeenCalledWith({
      type: "resolve_approval",
      thread_id: "t-exhausted",
      incarnation: 1,
      request_id: "req-exhausted",
      decision: { type: "deny", message: null },
      remember: null,
    });
  });
});
