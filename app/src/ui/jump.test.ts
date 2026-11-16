// ⌘J / ⌘⇧J routing (PLAN §3.4): which chat opens, and what happens with nothing to jump to.
// The window and daemon modules are replaced by plain state here.

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { InboxItem, InboxReason } from "../daemon/types";

const env = vi.hoisted(() => ({
  inbox: [] as InboxItem[],
  screen: "chat" as string,
  mainThread: null as string | null,
  owned: new Set<string>(),
  opened: [] as (string | null)[],
  focused: [] as string[],
  toasts: [] as string[],
}));

vi.mock("../daemon/client", () => ({ daemon: () => ({ inbox: env.inbox }) }));
vi.mock("../state", () => ({
  screen: () => env.screen,
  setScreen: (s: string) => { env.screen = s; },
  showToast: (t: string) => env.toasts.push(t),
}));
vi.mock("../windows", () => ({
  model: () => ({ mainThread: env.mainThread }),
  ownsView: (id: string) => env.owned.has(id),
  focusPopout: async (id: string) => { env.focused.push(id); },
}));
vi.mock("./Sidebar", () => ({
  openThread: (id: string | null) => { env.opened.push(id); env.mainThread = id; env.screen = "chat"; },
}));

const { jumpToNext, jumpToPrev } = await import("./jump");

const item = (thread_id: string | null, reason: InboxReason, since_ms: number): InboxItem => ({ thread_id, reason, since_ms, title: thread_id ?? "", detail: null });

beforeEach(() => {
  env.inbox = [item("r", "reply", 1), item("a2", "approval", 30), item("q", "question", 5), item("a1", "approval", 10)];
  env.screen = "chat";
  env.mainThread = null;
  env.owned = new Set(["r", "a1", "a2", "q"]);
  env.opened = [];
  env.focused = [];
  env.toasts = [];
});

describe("⌘⇧J", () => {
  it("from Home goes to the last item, then walks back through the inbox order", () => {
    jumpToPrev();
    jumpToPrev();
    jumpToPrev();
    jumpToPrev();
    jumpToPrev();
    expect(env.opened).toEqual(["r", "q", "a2", "a1", "r"]);
  });

  it("from the inbox starts at the last item, whatever chat the main card had", () => {
    env.screen = "inbox";
    env.mainThread = "a1";
    jumpToPrev();
    expect(env.opened).toEqual(["r"]);
  });

  it("is the reverse of ⌘J", () => {
    jumpToNext();
    jumpToNext();
    jumpToPrev();
    expect(env.opened).toEqual(["a1", "a2", "a1"]);
  });

  it("shows a chat that lives in its own window there", () => {
    env.owned.delete("r");
    jumpToPrev();
    expect(env.opened).toEqual(["r"]);
    expect(env.focused).toEqual(["r"]);
  });

  it("with nothing waiting, says so and shows the inbox", () => {
    env.inbox = [];
    jumpToPrev();
    expect(env.opened).toEqual([]);
    expect(env.toasts).toEqual(["Nothing needs you"]);
    expect(env.screen).toBe("inbox");
  });

  it("on the only item, says so and stays", () => {
    env.inbox = [item("only", "approval", 1)];
    env.mainThread = "only";
    jumpToPrev();
    expect(env.opened).toEqual([]);
    expect(env.toasts).toEqual(["This is the only chat that needs you"]);
    expect(env.screen).toBe("chat");
  });

  it("opens the inbox for storage full", () => {
    env.inbox = [item(null, "storage_full", 1), item("a1", "approval", 2)];
    env.mainThread = "a1";
    jumpToPrev();
    expect(env.screen).toBe("inbox");
  });
});
