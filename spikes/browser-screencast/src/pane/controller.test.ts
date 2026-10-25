import { beforeEach, describe, expect, it, vi } from "vitest";

// A fake Tauri: `invoke` records calls, `Channel` lets the test push packets as the backend would.
const calls: { cmd: string; args: Record<string, unknown> }[] = [];
const channels: { onmessage?: (b: ArrayBuffer) => void }[] = [];
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args: Record<string, unknown> = {}) => {
    calls.push({ cmd, args });
    return undefined;
  }),
  Channel: class {
    onmessage?: (b: ArrayBuffer) => void;
    constructor() {
      channels.push(this);
    }
  },
}));

import { HEADER_LEN } from "./frame";
import { PaneController } from "./controller";

function packet(stream: number, seq: number, ackId: number): ArrayBuffer {
  const buf = new ArrayBuffer(HEADER_LEN + 4);
  const v = new DataView(buf);
  v.setUint32(0, ackId, true);
  v.setUint32(8, seq, true);
  v.setUint32(48, stream, true);
  return buf;
}

/** createImageBitmap that stays pending until the test resolves it (a slow decoder). */
let pendingDecodes: { resolve: () => void; bitmap: { width: number; height: number; close: () => void } }[] = [];
const closed: unknown[] = [];

function makeController() {
  const draws: unknown[] = [];
  const ctx = { drawImage: (b: unknown) => draws.push(b) };
  const canvas = {
    width: 100,
    height: 100,
    getContext: () => ctx,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 100, height: 100 }),
  } as unknown as HTMLCanvasElement;
  const c = new PaneController(canvas, { quality: 80, format: "jpeg", deviceScaleFactor: 1 });
  return { c, draws };
}

beforeEach(() => {
  calls.length = 0;
  channels.length = 0;
  pendingDecodes = [];
  closed.length = 0;
  vi.stubGlobal("createImageBitmap", () => {
    let resolve!: () => void;
    const bitmap = { width: 100, height: 100, close: () => closed.push(bitmap) };
    const p = new Promise<typeof bitmap>((r) => (resolve = () => r(bitmap)));
    pendingDecodes.push({ resolve, bitmap });
    return p;
  });
  vi.stubGlobal("performance", { now: () => 0 });
  vi.stubGlobal("requestAnimationFrame", () => 1);
  vi.stubGlobal("cancelAnimationFrame", () => undefined);
});

const acks = () => calls.filter((c) => c.cmd === "pane_ack").map((c) => c.args);
const flush = () => new Promise((r) => setTimeout(r, 0));

describe("context switching with a slow decoder", () => {
  it("never draws or acks a frame of the context we switched away from, and keeps the new one", async () => {
    const { c, draws } = makeController();
    await c.attach(0);
    channels[0]!.onmessage!(packet(1, 50, 7)); // context A, sequence 50, decode still pending
    expect(pendingDecodes).toHaveLength(1);

    await c.attach(1); // switch to B while A's frame is decoding
    channels[1]!.onmessage!(packet(2, 1, 3)); // B starts again at sequence 1
    expect(pendingDecodes).toHaveLength(2);
    pendingDecodes[1]!.resolve();
    await flush();
    expect(draws).toEqual([pendingDecodes[1]!.bitmap]); // B drawn although its seq (1) < A's (50)

    pendingDecodes[0]!.resolve(); // A's decode finally completes
    await flush();
    expect(draws).toHaveLength(1); // A was not drawn over B
    expect(closed).toContain(pendingDecodes[0]!.bitmap); // and its bitmap was released
    // Only B's ack went out, tagged with B's stream; A's ack would have hit the wrong page.
    expect(acks()).toEqual([{ stream: 2, ackId: 3 }]);
  });

  it("drops packets from a detached channel without decoding them", async () => {
    const { c } = makeController();
    await c.attach(0);
    await c.detach();
    channels[0]!.onmessage!(packet(1, 1, 1));
    expect(pendingDecodes).toHaveLength(0);
    expect(acks()).toEqual([]);
  });
});

describe("held mouse buttons", () => {
  const down = { clientX: 20, clientY: 30, button: 0, buttons: 1, detail: 1, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false };
  const mouseCalls = () => calls.filter((c) => c.cmd === "pane_mouse").map((c) => c.args.params as { type: string; x: number; y: number });

  it("releases a button the pointer never released (pointer left the window, cancel, blur)", async () => {
    const { c } = makeController();
    c.setUserControl(true);
    c.mouseDown(down);
    expect(c.hasHeldButtons()).toBe(true);
    await c.releaseAll();
    expect(mouseCalls().map((p) => p.type)).toEqual(["mousePressed", "mouseReleased"]);
    expect(mouseCalls()[1]).toMatchObject({ x: 20, y: 30 });
    expect(c.hasHeldButtons()).toBe(false);
    await c.releaseAll(); // idempotent
    expect(mouseCalls()).toHaveLength(2);
  });

  it("a normal mouseup clears the held state so nothing is released twice", async () => {
    const { c } = makeController();
    c.setUserControl(true);
    c.mouseDown(down);
    c.mouseUp({ ...down, buttons: 0 });
    await c.releaseAll();
    expect(mouseCalls().map((p) => p.type)).toEqual(["mousePressed", "mouseReleased"]);
  });

  it("handing control back releases held buttons", async () => {
    const { c } = makeController();
    c.setUserControl(true);
    c.mouseDown(down);
    c.setUserControl(false);
    await flush();
    expect(mouseCalls().map((p) => p.type)).toEqual(["mousePressed", "mouseReleased"]);
  });

  it("switching context releases the button on the context where it was pressed", async () => {
    const { c } = makeController();
    c.setUserControl(true);
    await c.attach(0);
    c.mouseDown(down);
    await c.attach(1);
    const release = calls.filter((x) => x.cmd === "pane_mouse").at(-1)!;
    expect((release.args.params as { type: string }).type).toBe("mouseReleased");
    expect(release.args.context).toBe(0);
  });
});
