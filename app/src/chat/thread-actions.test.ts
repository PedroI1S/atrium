import { describe, expect, it } from "vitest";
import type { ThreadState, ThreadSummary } from "../daemon/types";
import { MAX_TITLE_CHARS, cleanTitle, exportFileName, hasLiveSession, sizeLabel, mainChatLeft, pickNewPath, exportFailureText, EXISTS_TEXT } from "./thread-actions";

describe("rename: the title the daemon will keep", () => {
  it("takes the first line with text and collapses spaces", () => {
    expect(cleanTitle("  Fix   the build  ")).toBe("Fix the build");
    expect(cleanTitle("\n\n  second line\nthird")).toBe("second line");
    expect(cleanTitle("tabs\tand spaces")).toBe("tabs and spaces");
  });
  it("refuses an empty title", () => {
    expect(cleanTitle("")).toBeNull();
    expect(cleanTitle("   \n \t ")).toBeNull();
  });
  it("cuts to the daemon's limit, by characters", () => {
    const long = "é".repeat(MAX_TITLE_CHARS + 10);
    expect([...cleanTitle(long)!].length).toBe(MAX_TITLE_CHARS);
  });
});

describe("export: the suggested file name", () => {
  it("is the title with the format's extension", () => {
    expect(exportFileName("Fix the build", "markdown")).toBe("Fix the build.md");
    expect(exportFileName("Fix the build", "jsonl")).toBe("Fix the build.jsonl");
  });
  it("drops characters file systems refuse, and leading dots", () => {
    expect(exportFileName("a/b\\c:d*e?f\"g<h>i|j", "markdown")).toBe("a b c d e f g h i j.md");
    expect(exportFileName("../../etc/passwd", "jsonl")).toBe("etc passwd.jsonl");
    expect(exportFileName("line\nbreak\u0007bell", "markdown")).toBe("line break bell.md");
  });
  it("falls back to “chat” when nothing is left", () => {
    expect(exportFileName("", "markdown")).toBe("chat.md");
    expect(exportFileName("///", "jsonl")).toBe("chat.jsonl");
  });
  it("keeps names short", () => {
    expect(exportFileName("x".repeat(500), "markdown").length).toBeLessThanOrEqual(64);
  });
});

describe("end session", () => {
  const thread = (state: ThreadState) => ({ state }) as ThreadSummary;
  it("is offered while a session is live", () => {
    for (const s of ["starting", "running", "needs_approval", "needs_input", "idle"] as const) expect(hasLiveSession(thread({ type: s }))).toBe(true);
    expect(hasLiveSession(thread({ type: "paused", resets_at_ms: 1 } as ThreadState))).toBe(true);
  });
  it("is not offered once it has ended", () => {
    expect(hasLiveSession(thread({ type: "ended" }))).toBe(false);
    expect(hasLiveSession(thread({ type: "failed" } as ThreadState))).toBe(false);
    expect(hasLiveSession(thread({ type: "interrupted", cleanup: "verified" } as ThreadState))).toBe(false);
  });
});

describe("sizes", () => {
  it("reads as bytes, KB or MB", () => {
    expect(sizeLabel(12)).toBe("12 bytes");
    expect(sizeLabel(4096)).toBe("4 KB");
    expect(sizeLabel(3 * 1024 * 1024)).toBe("3.0 MB");
  });
});

describe("the main card when its chat leaves the list (round 2, minor 6)", () => {
  it("goes back to a new chat only when a listed chat leaves", () => {
    expect(mainChatLeft("t1", "t1", false, true)).toBe(true);
    expect(mainChatLeft("t1", "t1", true, true)).toBe(false);
    // Not before the list loads, nor for a chat it never listed (a new chat being created).
    expect(mainChatLeft("t1", "t1", false, false)).toBe(false);
    expect(mainChatLeft("t1", null, false, true)).toBe(false);
    expect(mainChatLeft("t2", "t1", false, true)).toBe(false);
    expect(mainChatLeft(null, "t1", false, true)).toBe(false);
  });
});

describe("export never over an existing file (round 2, minor 5)", () => {
  const asker = (picks: ({ path: string; exists: boolean } | null)[]) => {
    const titles: string[] = [];
    const ask = async (title: string, _name: string) => { titles.push(title); return picks.shift() ?? null; };
    return { ask, titles };
  };
  it("asks again, saying why, until the user picks a new name", async () => {
    const { ask, titles } = asker([{ path: "/x/a.md", exists: true }, { path: "/x/b.md", exists: false }]);
    const notes: string[] = [];
    expect(await pickNewPath(ask, "Export", "a.md", (t) => notes.push(t))).toBe("/x/b.md");
    expect(notes).toEqual([EXISTS_TEXT]);
    expect(titles).toEqual(["Export", "Export: pick a new name"]);
  });
  it("stops when the user cancels, or keeps picking taken names", async () => {
    expect(await pickNewPath(asker([{ path: "/x/a.md", exists: true }, null]).ask, "Export", "a.md", () => {})).toBeNull();
    const taken = Array.from({ length: 10 }, () => ({ path: "/x/a.md", exists: true }));
    const notes: string[] = [];
    expect(await pickNewPath(asker(taken).ask, "Export", "a.md", (t) => notes.push(t))).toBeNull();
    expect(notes.length).toBe(5);
  });
  it("words the daemon's refusal of a taken path", () => {
    expect(exportFailureText({ kind: "rejected", code: "invalid", message: "something already exists at the export path" })).toBe(`Couldn't export the chat: ${EXISTS_TEXT}`);
    expect(exportFailureText({ kind: "rejected", code: "invalid", message: "the export folder doesn't exist" })).toBe("Couldn't export the chat: the export folder doesn't exist");
  });
});
