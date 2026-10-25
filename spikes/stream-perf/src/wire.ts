// Decoders for the batch encodings produced by src-tauri/src/encode.rs. Every decoder returns the
// same normalised `Batch`, so the transcript never knows which encoding carried the events.

import { decode as msgpackDecode } from "@msgpack/msgpack";

export const ENCODINGS = ["json", "json-slim", "msgpack", "msgpack-slim", "bin"] as const;
export type Encoding = (typeof ENCODINGS)[number];

/** The agent events the UI renders. Fields mirror `crates/atrium-protocol/src/agent.rs`. */
export type AgentEvent =
  | { type: "turn_started"; turn: number }
  | { type: "message_completed"; message_id: string; kind: string; text: string }
  | { type: "tool_call_started"; call_id: string; name: string; kind: string; title: string | null; input: unknown }
  | { type: "tool_call_updated"; call_id: string; title?: string | null; output_delta?: string | null; output?: string | null }
  | { type: "tool_call_finished"; call_id: string; status: string; output: string | null; error: string | null }
  | { type: "turn_completed"; turn: number; outcome: string }
  | { type: "error"; message: string; fatal: boolean }
  | { type: "session_ended"; exit_code: number | null; reason: string | null }
  | { type: string; [k: string]: unknown };

export interface Delta {
  delta: true;
  id: number;
  thread: number;
  /** Unix ms stamped by the agent process when it produced the event. */
  at: number;
  message: string;
  /** 0 assistant, 1 reasoning. */
  mkind: number;
  text: string;
}
export interface Other {
  delta: false;
  id: number;
  thread: number;
  at: number;
  event: AgentEvent;
}
export type Ev = Delta | Other;

export interface Batch {
  seq: number;
  /** Rust wall clock (Unix ms, fractional) when the batch was encoded. */
  sent: number;
  events: Ev[];
}

interface StoredEventJson {
  id: number;
  thread_id: string;
  envelope: { seq: number; at_ms: number; event: AgentEvent };
}

export type ThreadIndex = (threadId: string) => number;

function fromStored(e: StoredEventJson, index: ThreadIndex): Ev {
  const ev = e.envelope.event as { type: string; message_id?: string; kind?: string; text?: string };
  if (ev.type === "message_delta") {
    return { delta: true, id: e.id, thread: index(e.thread_id), at: e.envelope.at_ms, message: ev.message_id!, mkind: ev.kind === "reasoning" ? 1 : 0, text: ev.text! };
  }
  return { delta: false, id: e.id, thread: index(e.thread_id), at: e.envelope.at_ms, event: e.envelope.event };
}

type SlimJson =
  | { t: "d"; i: number; th: number; a: number; m: string; k: number; s: string }
  | { t: "e"; th: number; e: StoredEventJson };

function fromSlim(e: SlimJson): Ev {
  if (e.t === "d") return { delta: true, id: e.i, thread: e.th, at: e.a, message: e.m, mkind: e.k, text: e.s };
  return { delta: false, id: e.e.id, thread: e.th, at: e.e.envelope.at_ms, event: e.e.envelope.event };
}

interface RawBatch<E> {
  seq: number;
  sent: number;
  events: E[];
}

const utf8 = new TextDecoder();

function decodeBin(buf: ArrayBuffer): Batch {
  const view = new DataView(buf);
  const bytes = new Uint8Array(buf);
  if (view.getUint8(0) !== 0xb1) throw new Error("bad bin batch");
  const seq = view.getUint32(1, true);
  const sent = view.getFloat64(5, true);
  const count = view.getUint32(13, true);
  const events: Ev[] = new Array(count);
  let p = 17;
  for (let n = 0; n < count; n++) {
    const tag = view.getUint8(p);
    if (tag === 0) {
      const id = view.getFloat64(p + 1, true);
      const thread = view.getUint16(p + 9, true);
      const at = view.getFloat64(p + 11, true);
      const mkind = view.getUint8(p + 19);
      const mlen = view.getUint8(p + 20);
      p += 21;
      const message = utf8.decode(bytes.subarray(p, p + mlen));
      p += mlen;
      const tlen = view.getUint32(p, true);
      p += 4;
      const text = utf8.decode(bytes.subarray(p, p + tlen));
      p += tlen;
      events[n] = { delta: true, id, thread, at, message, mkind, text };
    } else {
      const thread = view.getUint16(p + 1, true);
      const jlen = view.getUint32(p + 3, true);
      p += 7;
      const stored = JSON.parse(utf8.decode(bytes.subarray(p, p + jlen))) as StoredEventJson;
      p += jlen;
      const ev = fromStored(stored, () => thread);
      events[n] = ev;
    }
  }
  return { seq, sent, events };
}

/**
 * `message` is what Tauri hands `Channel.onmessage`: an already parsed object for the JSON
 * encodings, an `ArrayBuffer` for the raw ones.
 */
export function decodeBatch(enc: Encoding, message: unknown, index: ThreadIndex): Batch {
  switch (enc) {
    case "json": {
      const b = message as RawBatch<StoredEventJson>;
      const events: Ev[] = new Array(b.events.length);
      for (let i = 0; i < events.length; i++) events[i] = fromStored(b.events[i]!, index);
      return { seq: b.seq, sent: b.sent, events };
    }
    case "json-slim": {
      const b = message as RawBatch<SlimJson>;
      const events: Ev[] = new Array(b.events.length);
      for (let i = 0; i < events.length; i++) events[i] = fromSlim(b.events[i]!);
      return { seq: b.seq, sent: b.sent, events };
    }
    case "msgpack": {
      const b = msgpackDecode(new Uint8Array(message as ArrayBuffer)) as RawBatch<StoredEventJson>;
      const events: Ev[] = new Array(b.events.length);
      for (let i = 0; i < events.length; i++) events[i] = fromStored(b.events[i]!, index);
      return { seq: b.seq, sent: b.sent, events };
    }
    case "msgpack-slim": {
      const b = msgpackDecode(new Uint8Array(message as ArrayBuffer)) as RawBatch<SlimJson>;
      const events: Ev[] = new Array(b.events.length);
      for (let i = 0; i < events.length; i++) events[i] = fromSlim(b.events[i]!);
      return { seq: b.seq, sent: b.sent, events };
    }
    case "bin":
      return decodeBin(message as ArrayBuffer);
  }
}

/** Plain-object view of the JSON encodings' text, for the in-webview decode microbenchmark. */
export function messageFromBytes(enc: Encoding, bytes: ArrayBuffer): unknown {
  return enc === "json" || enc === "json-slim" ? JSON.parse(utf8.decode(bytes)) : bytes;
}
