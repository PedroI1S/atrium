// This window's thread models, wired to the bridge. A view (the chat card, a pop-out, the inbox
// for its approval buttons) holds a thread; while anyone holds it the window views it in the
// client and keeps its `Transcript`. Deliveries are applied once per animation frame (M0-07),
// with a timer as the fallback when frames stop (a hidden window still keeps its model current
// and its acknowledgements flowing).
//
// Commands about requests and turns go through `command` (contract §11), and what this window
// did with each request is kept for the cards (requests.ts).

import { type Accessor, createSignal } from "solid-js";
import { command, pageBefore, viewThread } from "../daemon/client";
import { type CommandFailure, type DaemonCommand, type QuestionAnswer, type RememberScope, type ThreadDelivery, isCommandFailure } from "../daemon/types";
import { type Action, type Local, decisionOf } from "./requests";
import { Transcript, type RequestItem } from "./transcript";

/** How long a delivery may wait for an animation frame before a timer applies it. */
const FRAME_FALLBACK_MS = 100;
/** History asked for per page when the view scrolls to its top. */
export const PAGE_BYTES = 1024 * 1024;

export type LoadState = { kind: "loading" } | { kind: "ready" } | { kind: "failed"; message: string };

export interface ThreadModel {
  readonly threadId: string;
  readonly transcript: Transcript;
  /** Bumped after each applied batch: read it to follow the transcript. */
  readonly tick: Accessor<number>;
  readonly load: Accessor<LoadState>;
  /** Asks for older history; resolves with the rows added at the top (0: none). */
  pageOlder(): Promise<number>;
  /** Drops the detached view and asks for the live tail again. */
  reattach(): void;
}

interface Entry {
  model: ThreadModel;
  holders: number;
  queue: ThreadDelivery[];
  scheduled: boolean;
  bump: () => void;
  setLoad: (s: LoadState) => void;
  stop: () => void;
  paging: Promise<number> | null;
}

const entries = new Map<string, Entry>();

function flush(entry: Entry): void {
  entry.scheduled = false;
  const queue = entry.queue;
  if (!queue.length) return;
  entry.queue = [];
  const t = entry.model.transcript;
  let batch: Parameters<Transcript["apply"]>[0] = [];
  const applyBatch = () => {
    if (batch.length) t.apply(batch);
    batch = [];
  };
  for (const d of queue) {
    if (d.type === "snapshot") {
      batch = [];
      t.load(d.events, d.has_older, d.truncated);
      entry.setLoad({ kind: "ready" });
    } else if (d.type === "event") {
      batch.push(d.event);
    } else {
      applyBatch();
      entry.setLoad({ kind: "failed", message: d.message });
    }
  }
  applyBatch();
  entry.bump();
}

function schedule(entry: Entry): void {
  if (entry.scheduled) return;
  entry.scheduled = true;
  let done = false;
  const run = () => {
    if (done) return;
    done = true;
    clearTimeout(timer);
    flush(entry);
  };
  const timer = setTimeout(run, FRAME_FALLBACK_MS);
  if (typeof requestAnimationFrame === "function") requestAnimationFrame(run);
}

/** Holds a thread's model; call the returned `release` when the view goes. */
export function holdThread(threadId: string): { model: ThreadModel; release: () => void } {
  let entry = entries.get(threadId);
  if (!entry) {
    const [tick, setTick] = createSignal(0, { equals: false });
    const [load, setLoad] = createSignal<LoadState>({ kind: "loading" });
    const transcript = new Transcript();
    const e: Entry = {
      model: null as unknown as ThreadModel,
      holders: 0,
      queue: [],
      scheduled: false,
      bump: () => setTick((n) => n + 1),
      setLoad,
      stop: () => {},
      paging: null,
    };
    e.model = {
      threadId,
      transcript,
      tick,
      load,
      pageOlder: () => {
        if (e.paging) return e.paging;
        const first = transcript.firstLoaded;
        if (!transcript.hasOlder || first === null) return Promise.resolve(0);
        e.paging = pageBefore(threadId, first, PAGE_BYTES)
          .then((page) => {
            if (!page || transcript.firstLoaded !== first) return 0;
            const added = transcript.prepend(page.events, page.has_older, page.truncated);
            e.bump();
            return added;
          })
          .catch(() => 0)
          .finally(() => { e.paging = null; });
        return e.paging;
      },
      reattach: () => {
        // Viewing again sends a fresh snapshot of the tail.
        e.stop();
        e.stop = viewThread(threadId, (d) => { e.queue.push(d); schedule(e); });
      },
    };
    e.stop = viewThread(threadId, (d) => { e.queue.push(d); schedule(e); });
    entries.set(threadId, e);
    entry = e;
  }
  entry.holders++;
  const held = entry;
  let released = false;
  return {
    model: held.model,
    release: () => {
      if (released) return;
      released = true;
      held.holders--;
      if (held.holders === 0) {
        held.stop();
        entries.delete(threadId);
      }
    },
  };
}

/** Models this window holds now (the inbox looks up pending requests here). */
export const heldModel = (threadId: string): ThreadModel | undefined => entries.get(threadId)?.model;

// -- requests --------------------------------------------------------------------------------

const [locals, setLocals] = createSignal<Record<string, Local>>({});
const localKey = (threadId: string, r: RequestItem) => `${threadId}:${r.incarnation}:${r.requestId}`;
export const localOf = (threadId: string, r: RequestItem): Local | undefined => locals()[localKey(threadId, r)];

function setLocal(key: string, local: Local): void {
  setLocals((m) => ({ ...m, [key]: local }));
}

function refusal(action: Action, e: unknown): Local {
  if (isCommandFailure(e)) {
    const f = e as CommandFailure;
    return { status: "refused", action, code: f.kind === "rejected" ? f.code : null, message: f.message, unknownOutcome: f.kind === "unknown_outcome" };
  }
  return { status: "refused", action, code: null, message: String(e), unknownOutcome: false };
}

async function send(threadId: string, r: RequestItem, action: Action, cmd: DaemonCommand): Promise<void> {
  const key = localKey(threadId, r);
  const current = locals()[key];
  // One answer per request from this window, unless the last one failed.
  if (current && current.status !== "refused") return;
  setLocal(key, { status: "sending", action });
  try {
    await command(cmd);
    setLocal(key, { status: "accepted", action });
  } catch (e) {
    setLocal(key, refusal(action, e));
  }
}

/** Allow or deny an approval, optionally remembering it ("always…", contract §5). */
export function resolveApproval(threadId: string, r: RequestItem, action: "allow" | "deny", remember: RememberScope | null = null): Promise<void> {
  return send(threadId, r, action, {
    type: "resolve_approval",
    thread_id: threadId,
    incarnation: r.incarnation,
    request_id: r.requestId,
    decision: decisionOf(action),
    remember: action === "allow" ? remember : null,
  });
}

export function answerQuestion(threadId: string, r: RequestItem, answers: QuestionAnswer[]): Promise<void> {
  return send(threadId, r, "answer", { type: "answer_question", thread_id: threadId, incarnation: r.incarnation, request_id: r.requestId, answers });
}

/**
 * Declining a question: the wire has no "decline" (CR in the handoff), so the turn is stopped,
 * which withdraws the question.
 */
export function stopTurnForQuestion(threadId: string, r: RequestItem): Promise<void> {
  return send(threadId, r, "cancel", { type: "thread_command", thread_id: threadId, command: { type: "interrupt" } });
}
