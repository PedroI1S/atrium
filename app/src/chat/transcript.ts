// One thread's transcript as a view holds it: a pure model over stored events (no DOM, no
// Solid), tested in transcript.test.ts. The view reads `rows` and the version numbers.
//
// - Events come folded from the client (atrium-client cache.rs): a message's deltas may arrive
//   as one delta with the text so far, a tool call's updates as one update. Applying folded or
//   unfolded events gives the same transcript.
// - Message ids, call ids and request ids are scoped to an incarnation (contract §6.1). A new
//   incarnation closes whatever the old one left open: messages are marked cut off, running
//   tool calls end as interrupted, open requests go stale.
// - The view keeps at most `maxBytes` of transcript (contract §11). Past it the oldest items go
//   (`hasOlder` then says so) and come back with `prepend` from the client's pages. Paging back
//   beyond the bound drops the newest items instead and the view is `detached` from the live
//   tail until it asks for a fresh snapshot.

import type { AgentEvent, EnforcementDescriptor, EventId, MessageKind, PlanEntry, Question, RequestOutcome, StoredEvent, ToolKind, ToolStatus, TurnOutcome, Usage, Value } from "../daemon/types";
import { BlockSplitter, type BlockText } from "./blocks";
import { MAX_LOADED_TRANSCRIPT_BYTES } from "./limits";

interface Base {
  /** Unique within the transcript. */
  key: string;
  /** First and last event applied to it. */
  first: EventId;
  last: EventId;
  incarnation: number;
  /** Bumped on every change. */
  v: number;
  bytes: number;
}

export interface MessageItem extends Base {
  kind: "message";
  role: MessageKind;
  messageId: string;
  splitter: BlockSplitter;
  chars: number;
  open: boolean;
  /** The incarnation ended before the message completed. */
  cut: boolean;
}

export interface ToolItem extends Base {
  kind: "tool";
  callId: string;
  name: string;
  toolKind: ToolKind;
  title: string | null;
  input: Value;
  output: string;
  status: "running" | ToolStatus | "interrupted";
  error: string | null;
  truncatedBytes: number;
}

export interface RequestBase extends Base {
  requestId: string;
  /** The turn that was running when it was asked (null: none known). */
  turn: number | null;
  outcome: RequestOutcome | null;
  /** The turn or session moved on and no resolution arrived: it can't be answered. */
  stale: boolean;
  at: number;
}

export interface ApprovalItem extends RequestBase {
  kind: "approval";
  callId: string | null;
  name: string;
  toolKind: ToolKind;
  title: string | null;
  input: Value;
  reason: string | null;
  rememberable: boolean;
}

export interface QuestionItem extends RequestBase {
  kind: "question";
  questions: Question[];
}

export type RequestItem = ApprovalItem | QuestionItem;

export interface NoteItem extends Base {
  kind: "note";
  note:
    | { type: "session_started"; provider: string; model: string | null; enforcement: EnforcementDescriptor }
    | { type: "session_ended"; exitCode: number | null; reason: string | null }
    | { type: "turn_completed"; turn: number; outcome: TurnOutcome; usage: Usage | null }
    | { type: "error"; message: string; fatal: boolean; details: string[] }
    | { type: "rate_limited"; resetsAtMs: number | null; message: string | null }
    | { type: "plan"; entries: PlanEntry[] }
    | { type: "enforcement_changed"; enforcement: EnforcementDescriptor };
}

export type Item = MessageItem | ToolItem | ApprovalItem | QuestionItem | NoteItem;

/** What the view shows, one entry per visual row (a message has one row per block). */
export type Row =
  | { type: "user"; key: string; item: MessageItem }
  | { type: "block"; key: string; item: MessageItem; block: BlockText; index: number }
  | { type: "tool"; key: string; item: ToolItem }
  | { type: "request"; key: string; item: RequestItem }
  | { type: "note"; key: string; item: NoteItem };

/** Tool output kept per call; the adapter caps it too (contract §6.1). */
const NOTE_BYTES = 64;

function valueBytes(v: Value): number {
  if (v === null || typeof v !== "object") return String(v).length;
  return JSON.stringify(v).length;
}

export class Transcript {
  items: Item[] = [];
  /** Open things of the current incarnation, by `${incarnation}:${id}`. */
  private messages = new Map<string, MessageItem>();
  private calls = new Map<string, ToolItem>();
  private requests = new Map<string, RequestItem>();
  incarnation = 0;
  /** The turn running now, if any. */
  turn: number | null = null;
  /** The latest usage and context occupancy, for the header. */
  usage: Usage | null = null;
  context: { used: number; window: number | null } | null = null;
  enforcement: EnforcementDescriptor | null = null;
  /** The cursor of the last event applied, and the first event the view holds. */
  last: EventId | null = null;
  /** Older history exists in the client (`page_before` from `firstLoaded`). */
  hasOlder = false;
  /** Older history exists only in the daemon. */
  truncated = false;
  /** Newest items were dropped while paging back; live events don't show until a new snapshot. */
  detached = false;
  /** Live events that arrived while detached. */
  missed = 0;
  bytes = 0;
  /** Bumped on every change; `structure` when rows are added, removed or reordered. */
  v = 0;
  structure = 0;
  private rowsCache: Row[] | null = null;
  private rowsAt = -1;

  constructor(readonly maxBytes: number = MAX_LOADED_TRANSCRIPT_BYTES) {}

  /** The first event the view holds (paging asks for history before it). */
  get firstLoaded(): EventId | null {
    return this.items[0]?.first ?? null;
  }

  /** Open approvals and questions of the current incarnation, oldest first. */
  pending(): RequestItem[] {
    return [...this.requests.values()].filter((r) => r.outcome === null && !r.stale).sort((a, b) => a.first - b.first);
  }

  /** Every request the view holds that is still open (stale ones included). */
  openRequests(): RequestItem[] {
    return [...this.requests.values()];
  }

  /** Replaces everything with a snapshot (folded events, oldest first). */
  load(events: StoredEvent[], hasOlder: boolean, truncated: boolean): void {
    const fresh = new Transcript(this.maxBytes);
    for (const e of events) fresh.applyOne(e);
    fresh.hasOlder = hasOlder;
    fresh.truncated = truncated;
    fresh.evictOldest();
    Object.assign(this, { ...fresh, v: this.v + 1, structure: this.structure + 1, rowsCache: null, rowsAt: -1 });
  }

  /** Applies live events. Returns whether anything changed. */
  apply(events: StoredEvent[]): boolean {
    let changed = false;
    for (const e of events) {
      if (this.detached) {
        if (this.last !== null && e.id <= this.last) continue;
        this.last = e.id;
        this.missed++;
        this.trackDetached(e);
        changed = true;
        continue;
      }
      changed = this.applyOne(e) || changed;
    }
    if (changed) {
      this.v++;
      this.evictOldest();
    }
    return changed;
  }

  /**
   * Older history from the client, oldest first, ending where this view begins. Returns the
   * number of rows added at the top (the view keeps its scroll position with it).
   */
  prepend(events: StoredEvent[], hasOlder: boolean, truncated: boolean): number {
    const first = this.firstLoaded;
    const older = new Transcript(Number.MAX_SAFE_INTEGER);
    for (const e of events) if (first === null || e.id < first) older.applyOne(e);
    const added = older.rows().length;
    this.items = [...older.items, ...this.items];
    this.bytes += older.bytes;
    this.hasOlder = hasOlder;
    this.truncated = truncated;
    this.trimNewest();
    this.v++;
    this.structure++;
    return added;
  }

  /** The rows to show, rebuilt only when the structure changed. */
  rows(): Row[] {
    for (const item of this.items) {
      if (item.kind === "message" && item.splitter.structural) {
        item.splitter.structural = false;
        this.structure++;
      }
    }
    if (this.rowsCache && this.rowsAt === this.structure) return this.rowsCache;
    const rows: Row[] = [];
    for (const item of this.items) {
      switch (item.kind) {
        case "message":
          if (item.role === "user") rows.push({ type: "user", key: item.key, item });
          else item.splitter.blocks.forEach((block, index) => rows.push({ type: "block", key: `${item.key}#${index}`, item, block, index }));
          break;
        case "tool":
          rows.push({ type: "tool", key: item.key, item });
          break;
        case "approval":
        case "question":
          rows.push({ type: "request", key: item.key, item });
          break;
        case "note":
          if (this.noteShown(item)) rows.push({ type: "note", key: item.key, item });
          break;
      }
    }
    this.rowsCache = rows;
    this.rowsAt = this.structure;
    return rows;
  }

  /** A completed turn shows only when something is worth saying (not every "completed"). */
  private noteShown(item: NoteItem): boolean {
    return !(item.note.type === "turn_completed" && item.note.outcome === "completed" && !item.note.usage);
  }

  // -- applying ----------------------------------------------------------------------------

  private scoped(id: string, incarnation = this.incarnation): string {
    return `${incarnation}:${id}`;
  }

  private add<T extends Item>(item: T): T {
    this.items.push(item);
    this.bytes += item.bytes;
    this.structure++;
    return item;
  }

  private grow(item: Item, bytes: number): void {
    item.bytes += bytes;
    this.bytes += bytes;
  }

  private base(e: StoredEvent, key: string, bytes: number) {
    return { key, first: e.id, last: e.id, incarnation: e.envelope.incarnation, v: 0, bytes };
  }

  /** A new incarnation: nothing of the old one can continue. */
  private newIncarnation(incarnation: number): void {
    for (const m of this.messages.values()) {
      m.open = false;
      m.cut = true;
      m.splitter.seal();
      m.v++;
    }
    for (const c of this.calls.values()) {
      c.status = "interrupted";
      c.v++;
    }
    this.closeRequests(() => true);
    this.messages.clear();
    this.calls.clear();
    this.requests.clear();
    this.incarnation = incarnation;
    this.turn = null;
  }

  /** Open requests matching `which` can no longer be answered. */
  private closeRequests(which: (r: RequestItem) => boolean): void {
    for (const r of this.requests.values()) {
      if (r.outcome === null && !r.stale && which(r)) {
        r.stale = true;
        r.v++;
      }
    }
  }

  /** While detached only the state that cards and the header need is kept. */
  private trackDetached(e: StoredEvent): void {
    if (e.envelope.incarnation !== this.incarnation) this.newIncarnation(e.envelope.incarnation);
    const ev = e.envelope.event;
    if (ev.type === "turn_started") this.turn = ev.turn;
    else if (ev.type === "turn_completed") {
      this.closeRequests((r) => r.turn === ev.turn || r.turn === null);
      this.turn = null;
    } else if (ev.type === "session_ended") this.closeRequests(() => true);
    else if (ev.type === "request_resolved") {
      const r = this.requests.get(this.scoped(ev.request_id));
      if (r) {
        r.outcome = ev.outcome;
        r.v++;
        this.requests.delete(this.scoped(ev.request_id));
      }
    } else if (ev.type === "approval_requested" || ev.type === "question_asked") {
      // Shown in the dock even while the transcript is detached.
      this.request(e, ev, false);
    }
  }

  private request(e: StoredEvent, ev: Extract<AgentEvent, { type: "approval_requested" | "question_asked" }>, listed: boolean): RequestItem {
    const key = `${this.scoped(ev.request_id, e.envelope.incarnation)}:r`;
    const common = { ...this.base(e, key, NOTE_BYTES), requestId: ev.request_id, turn: this.turn, outcome: null, stale: false, at: e.envelope.at_ms };
    const item: RequestItem =
      ev.type === "approval_requested"
        ? { ...common, kind: "approval", callId: ev.call_id, name: ev.name, toolKind: ev.kind, title: ev.title, input: ev.input, reason: ev.reason, rememberable: ev.rememberable ?? false, bytes: NOTE_BYTES + valueBytes(ev.input) }
        : { ...common, kind: "question", questions: ev.questions, bytes: NOTE_BYTES + valueBytes(ev.questions as unknown as Value) };
    this.requests.set(this.scoped(ev.request_id, e.envelope.incarnation), item);
    if (listed) this.add(item);
    return item;
  }

  private note(e: StoredEvent, note: NoteItem["note"]): void {
    this.add<NoteItem>({ ...this.base(e, `${e.id}:n`, NOTE_BYTES), kind: "note", note });
  }

  private applyOne(e: StoredEvent): boolean {
    if (this.last !== null && e.id <= this.last) return false;
    this.last = e.id;
    const inc = e.envelope.incarnation;
    if (inc !== this.incarnation) this.newIncarnation(inc);
    const ev = e.envelope.event;
    switch (ev.type) {
      case "session_started":
        this.enforcement = ev.enforcement;
        this.note(e, { type: "session_started", provider: ev.provider, model: ev.model, enforcement: ev.enforcement });
        break;
      case "enforcement_changed":
        this.enforcement = ev.enforcement;
        this.note(e, { type: "enforcement_changed", enforcement: ev.enforcement });
        break;
      case "turn_started":
        this.turn = ev.turn;
        break;
      case "message_delta": {
        const id = this.scoped(ev.message_id);
        let m = this.messages.get(id);
        if (!m) {
          m = this.add<MessageItem>({ ...this.base(e, `${id}:m`, 0), kind: "message", role: ev.kind, messageId: ev.message_id, splitter: new BlockSplitter(), chars: 0, open: true, cut: false });
          this.messages.set(id, m);
        }
        m.splitter.feed(ev.text);
        m.chars += ev.text.length;
        m.last = e.id;
        m.v++;
        this.grow(m, ev.text.length);
        break;
      }
      case "message_completed": {
        const id = this.scoped(ev.message_id);
        let m = this.messages.get(id);
        if (!m) {
          // A completion-only message (or one whose deltas the view doesn't hold).
          m = this.add<MessageItem>({ ...this.base(e, `${id}:m`, 0), kind: "message", role: ev.kind, messageId: ev.message_id, splitter: new BlockSplitter(), chars: 0, open: true, cut: false });
          if (ev.body.type === "inline") {
            m.splitter.feed(ev.body.text);
            m.chars = ev.body.text.length;
            this.grow(m, ev.body.text.length);
          }
        }
        m.splitter.seal();
        m.open = false;
        m.last = e.id;
        m.v++;
        this.messages.delete(id);
        break;
      }
      case "tool_call_started": {
        const id = this.scoped(ev.call_id);
        const t = this.add<ToolItem>({ ...this.base(e, `${id}:t`, valueBytes(ev.input)), kind: "tool", callId: ev.call_id, name: ev.name, toolKind: ev.kind, title: ev.title, input: ev.input, output: "", status: "running", error: null, truncatedBytes: 0 });
        this.calls.set(id, t);
        break;
      }
      case "tool_call_updated": {
        const t = this.calls.get(this.scoped(ev.call_id));
        if (!t) break;
        if (ev.title !== null) t.title = ev.title;
        if (ev.kind !== null) t.toolKind = ev.kind;
        if (ev.input !== null) {
          this.grow(t, valueBytes(ev.input) - valueBytes(t.input));
          t.input = ev.input;
        }
        // "output" replaces the whole output so far, then a delta in the same update appends.
        if (ev.output !== null) {
          this.grow(t, ev.output.length - t.output.length);
          t.output = ev.output;
        }
        if (ev.output_delta !== null) {
          this.grow(t, ev.output_delta.length);
          t.output += ev.output_delta;
        }
        t.last = e.id;
        t.v++;
        break;
      }
      case "tool_call_finished": {
        const id = this.scoped(ev.call_id);
        const t = this.calls.get(id);
        if (!t) break;
        t.status = ev.status;
        if (ev.output !== null) {
          this.grow(t, ev.output.length - t.output.length);
          t.output = ev.output;
        }
        t.error = ev.error;
        t.truncatedBytes = ev.truncated_bytes ?? 0;
        t.last = e.id;
        t.v++;
        this.calls.delete(id);
        break;
      }
      case "approval_requested":
      case "question_asked":
        this.request(e, ev, true);
        break;
      case "request_resolved": {
        const id = this.scoped(ev.request_id);
        const r = this.requests.get(id);
        if (!r) break;
        r.outcome = ev.outcome;
        r.last = e.id;
        r.v++;
        this.requests.delete(id);
        break;
      }
      case "plan_updated":
        this.note(e, { type: "plan", entries: ev.entries });
        break;
      case "usage":
        this.usage = ev.usage;
        break;
      case "context_updated":
        this.context = { used: ev.used_tokens, window: ev.window_tokens };
        break;
      case "quota_updated":
        break;
      case "rate_limited":
        this.note(e, { type: "rate_limited", resetsAtMs: ev.resets_at_ms, message: ev.message });
        break;
      case "turn_completed":
        if (ev.usage) this.usage = ev.usage;
        // The ledger expires a turn's requests when it ends (contract §5).
        this.closeRequests((r) => r.turn === ev.turn || r.turn === null);
        this.turn = null;
        this.note(e, { type: "turn_completed", turn: ev.turn, outcome: ev.outcome, usage: ev.usage });
        break;
      case "error":
        this.note(e, { type: "error", message: ev.message, fatal: ev.fatal, details: ev.details ?? [] });
        break;
      case "unmapped":
        break;
      case "session_ended":
        this.newIncarnation(this.incarnation);
        this.note(e, { type: "session_ended", exitCode: ev.exit_code, reason: ev.reason });
        break;
    }
    return true;
  }

  // -- the bound ---------------------------------------------------------------------------

  /** An item that may still change can't leave the view. */
  private isOpen(item: Item): boolean {
    if (item.kind === "message") return item.open;
    if (item.kind === "tool") return item.status === "running";
    if (item.kind === "approval" || item.kind === "question") return item.outcome === null && !item.stale;
    return false;
  }

  /**
   * Drops the oldest items while over the bound. A cut is made only where every dropped item's
   * events come before the first kept one, so a page from the client fills the gap exactly.
   * Like the client's cache (segments), the bound holds at such cuts: an item that is still
   * open, or one that closed after later items began, holds everything after it until the
   * next cut point arrives.
   */
  private evictOldest(): void {
    if (this.bytes <= this.maxBytes) return;
    let dropped = 0;
    let lastDropped = -1;
    let best = 0;
    let bestBytes = 0;
    for (let k = 0; k < this.items.length - 1; k++) {
      const item = this.items[k]!;
      if (this.isOpen(item)) break;
      dropped += item.bytes;
      lastDropped = Math.max(lastDropped, item.last);
      if (lastDropped < this.items[k + 1]!.first) {
        best = k + 1;
        bestBytes = dropped;
        if (this.bytes - dropped <= this.maxBytes) break;
      }
    }
    if (best === 0) return;
    this.items = this.items.slice(best);
    this.bytes -= bestBytes;
    this.hasOlder = true;
    this.structure++;
  }

  /** After paging back past the bound: the newest closed items go and the view detaches. */
  private trimNewest(): void {
    while (this.bytes > this.maxBytes && this.items.length > 1) {
      const item = this.items[this.items.length - 1]!;
      this.items.pop();
      this.bytes -= item.bytes;
      this.detached = true;
    }
  }
}
