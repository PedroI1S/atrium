// The transcript model: plain mutable data, no DOM. `apply` takes decoded events, `flush` publishes
// the result to Solid once per frame. Only rows that changed (and the row count) are signalled.

import { type Accessor, createSignal } from "solid-js";
import { BlockSplitter } from "./blocks";
import { renderBlock, renderOpen } from "./markdown";
import type { AgentEvent, Ev } from "./wire";

interface Rev {
  rev: Accessor<number>;
  bump: () => void;
}
function makeRev(): Rev {
  const [rev, set] = createSignal(0);
  return { rev, bump: () => set((n) => n + 1) };
}

export class MdRow {
  readonly kind = "md" as const;
  text = "";
  open = true;
  fence = false;
  reasoning: boolean;
  private html: string | null = null;
  private r = makeRev();
  constructor(reasoning: boolean) {
    this.reasoning = reasoning;
  }
  /** Reactive: re-evaluates when the text changes. Closed rows parse once. */
  rendered(): string {
    this.r.rev();
    if (this.open) return renderOpen(this.text, this.fence);
    return (this.html ??= renderBlock(this.text));
  }
  touch(): void {
    this.r.bump();
  }
}

export class ToolRow {
  readonly kind = "tool" as const;
  status = "running";
  title: string;
  output = "";
  private r = makeRev();
  constructor(
    readonly callId: string,
    readonly name: string,
    title: string | null,
  ) {
    this.title = title ?? name;
  }
  rev(): number {
    return this.r.rev();
  }
  touch(): void {
    this.r.bump();
  }
}

export class NoteRow {
  readonly kind = "note" as const;
  constructor(
    readonly text: string,
    readonly tone: "info" | "error" = "info",
  ) {}
}

export type Row = MdRow | ToolRow | NoteRow;

interface MessageState {
  splitter: BlockSplitter;
  /** The row showing the splitter's open block, if one exists. */
  cur: MdRow | null;
  chars: number;
  reasoning: boolean;
  dirty: boolean;
}

export class Transcript {
  readonly rows: Row[] = [];
  readonly count: Accessor<number>;
  private setCount: (n: number) => void;
  private messages = new Map<string, MessageState>();
  private tools = new Map<string, ToolRow>();
  private dirty = new Set<MessageState>();
  private toolsDirty = new Set<ToolRow>();
  private rowsChanged = false;
  /** Highest event id applied to the model (the bridge's cursor for this thread). */
  lastId = 0;
  events = 0;
  /** Events rejected because their id had been applied already (replay after a reconnect). */
  duplicates = 0;
  chars = 0;
  /** `message_completed` whose text length differed from the streamed deltas. */
  completedMismatch = 0;
  ended = false;
  running = true;

  constructor() {
    const [count, setCount] = createSignal(0);
    this.count = count;
    this.setCount = setCount;
  }

  private push(row: Row): void {
    this.rows.push(row);
    this.rowsChanged = true;
  }

  private message(id: string, reasoning: boolean): MessageState {
    let m = this.messages.get(id);
    if (!m) {
      m = { splitter: new BlockSplitter(), cur: null, chars: 0, reasoning, dirty: false };
      this.messages.set(id, m);
    }
    return m;
  }

  private closeBlock(m: MessageState, block: string): void {
    const row = m.cur ?? new MdRow(m.reasoning);
    if (!m.cur) this.push(row);
    row.text = block;
    row.open = false;
    row.fence = false;
    row.touch();
    m.cur = null;
  }

  /**
   * Applies one event. Returns false, changing nothing, for an id at or below the highest one applied:
   * after a reconnect the client replays events that were read but not yet acknowledged, and those
   * have already been applied here. Ids are strictly increasing per thread.
   */
  apply(ev: Ev): boolean {
    if (ev.id <= this.lastId) {
      this.duplicates++;
      return false;
    }
    this.events++;
    this.lastId = ev.id;
    if (ev.delta) {
      const m = this.message(ev.message, ev.mkind === 1);
      m.chars += ev.text.length;
      this.chars += ev.text.length;
      m.splitter.feed(ev.text, (b) => this.closeBlock(m, b));
      if (!m.dirty) {
        m.dirty = true;
        this.dirty.add(m);
      }
      return true;
    }
    this.applyEvent(ev.event);
    return true;
  }

  private seal(): void {
    for (const m of this.messages.values()) {
      if (m.cur || m.splitter.openText() !== "") m.splitter.seal((b) => this.closeBlock(m, b));
    }
  }

  private applyEvent(e: AgentEvent): void {
    switch (e.type) {
      case "message_completed": {
        const ev = e as Extract<AgentEvent, { type: "message_completed" }>;
        const m = this.messages.get(ev.message_id);
        if (m) {
          if (m.chars !== ev.text.length) this.completedMismatch++;
          m.splitter.seal((b) => this.closeBlock(m, b));
          m.dirty = false;
          this.dirty.delete(m);
          this.messages.delete(ev.message_id);
        } else if (ev.text !== "") {
          // Providers that only send whole messages.
          const mm = this.message(ev.message_id, ev.kind === "reasoning");
          mm.splitter.feed(ev.text + "\n\n", (b) => this.closeBlock(mm, b));
          mm.splitter.seal((b) => this.closeBlock(mm, b));
          this.messages.delete(ev.message_id);
          this.chars += ev.text.length;
        }
        break;
      }
      case "tool_call_started": {
        const ev = e as Extract<AgentEvent, { type: "tool_call_started" }>;
        this.seal();
        const row = new ToolRow(ev.call_id, ev.name, ev.title);
        this.tools.set(ev.call_id, row);
        this.push(row);
        break;
      }
      case "tool_call_updated": {
        const ev = e as Extract<AgentEvent, { type: "tool_call_updated" }>;
        const row = this.tools.get(ev.call_id);
        if (row) {
          if (ev.title) row.title = ev.title;
          if (ev.output != null) row.output = ev.output;
          if (ev.output_delta) row.output += ev.output_delta;
          this.toolsDirty.add(row);
        }
        break;
      }
      case "tool_call_finished": {
        const ev = e as Extract<AgentEvent, { type: "tool_call_finished" }>;
        const row = this.tools.get(ev.call_id);
        if (row) {
          row.status = ev.status;
          if (ev.output != null) row.output = ev.output;
          this.toolsDirty.add(row);
          this.tools.delete(ev.call_id);
        }
        break;
      }
      case "turn_started":
        this.push(new NoteRow(`turn ${(e as { turn: number }).turn} started`));
        break;
      case "turn_completed":
        this.seal();
        this.push(new NoteRow(`turn ${(e as { turn: number }).turn} ${(e as { outcome: string }).outcome}`));
        break;
      case "error":
        this.push(new NoteRow((e as { message: string }).message, "error"));
        break;
      case "session_ended":
        this.ended = true;
        this.running = false;
        this.seal();
        break;
      default:
        break;
    }
  }

  /** Publishes the frame's changes to Solid. Returns whether anything visible changed. */
  flush(): boolean {
    let changed = this.rowsChanged;
    for (const m of this.dirty) {
      m.dirty = false;
      const text = m.splitter.openText();
      if (text === "") {
        continue;
      }
      if (!m.cur) {
        m.cur = new MdRow(m.reasoning);
        this.push(m.cur);
      }
      m.cur.text = text;
      m.cur.fence = m.splitter.isFenceOpen();
      m.cur.touch();
      changed = true;
    }
    this.dirty.clear();
    for (const t of this.toolsDirty) {
      t.touch();
      changed = true;
    }
    this.toolsDirty.clear();
    // Rows closed during `apply` were touched there; they also count as a change.
    if (this.rowsChanged || changed) {
      this.setCount(this.rows.length);
      this.rowsChanged = false;
      changed = true;
    }
    return changed;
  }
}
