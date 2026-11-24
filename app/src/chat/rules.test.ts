import { describe, expect, it } from "vitest";
import type { Rule } from "../daemon/types";
import { argvLine, quoteArg, ruleText } from "./rules";

const rule = (over: Partial<Rule>): Rule => ({
  id: "r1",
  scope: { type: "home" },
  tool_kind: "execute",
  tool_name: null,
  matcher: { type: "command_exact", argv: ["rm", "-rf", "build"] },
  path_scope: null,
  decision: "allow",
  origin: { type: "approval", request_id: "q1" },
  created_at_ms: 0,
  ...over,
});
const noTitle = () => undefined;

describe("how a remembered rule reads (D-054, D-062)", () => {
  it("shows the exact command, argument by argument", () => {
    const text = ruleText(rule({}), noTitle);
    expect(text.decision).toBe("Allow");
    expect(text.what).toBe("this exact command");
    expect(text.code).toBe("rm -rf build");
  });

  it("quotes arguments so two different argvs never read the same", () => {
    expect(argvLine(["rm", "-rf", "build dir"])).toBe("rm -rf 'build dir'");
    expect(argvLine(["rm", "-rf", "build", "dir"])).toBe("rm -rf build dir");
    expect(quoteArg("")).toBe("''");
    expect(quoteArg("it's")).toBe("'it'\\''s'");
    expect(quoteArg("a;b")).toBe("'a;b'");
    expect(quoteArg("$HOME")).toBe("'$HOME'");
    expect(quoteArg("src/main.rs")).toBe("src/main.rs");
    // Round 2 nit 8: ~ and ~/… read as the home folder, so they stay unquoted.
    expect(argvLine(["ls", "~/src"])).toBe("ls ~/src");
    expect(quoteArg("~")).toBe("~");
    expect(quoteArg("~bob")).toBe("'~bob'");
    expect(quoteArg("a~b")).toBe("'a~b'");
    expect(quoteArg("~/a b")).toBe("~/'a b'");
  });

  it("says where it applies, and that it is in any folder", () => {
    expect(ruleText(rule({ scope: { type: "home" } }), noTitle).where).toBe("In every Home chat, in any folder");
    expect(ruleText(rule({ scope: { type: "global" } }), noTitle).where).toBe("In every chat, in any folder");
    expect(ruleText(rule({ scope: { type: "thread", thread_id: "t1" } }), (id) => (id === "t1" ? "Fix the build" : undefined)).where).toBe("In the chat “Fix the build”, in any folder");
    expect(ruleText(rule({ scope: { type: "thread", thread_id: "gone" } }), noTitle).where).toBe("In one chat that is no longer listed, in any folder");
    expect(ruleText(rule({ path_scope: "/home/u/project/**" }), noTitle).where).toBe("In every Home chat, only for paths inside /home/u/project/**");
  });

  it("describes path, prefix and any matchers, and deny rules", () => {
    const path = ruleText(rule({ tool_kind: "edit", tool_name: "Write", matcher: { type: "path_glob", glob: "/home/u/notes.md" } }), noTitle);
    expect(path.what).toBe("every file edit (Write) of");
    expect(path.code).toBe("/home/u/notes.md");
    const prefix = ruleText(rule({ matcher: { type: "command_prefix", argv: ["git", "status"] }, origin: { type: "settings" } }), noTitle);
    expect(prefix.what).toBe("commands that start with");
    expect(prefix.code).toBe("git status …");
    expect(prefix.origin).toMatch(/^Added in settings on /);
    const any = ruleText(rule({ tool_kind: "fetch", matcher: { type: "any" }, decision: "deny" }), noTitle);
    expect(any.decision).toBe("Deny");
    expect(any.what).toBe("every web request");
    expect(any.code).toBeNull();
    expect(any.invalidReason).toBeNull();
  });

  it("surfaces invalidReason for stored invalid rules (§11 item 5)", () => {
    const inv = ruleText(
      { ...rule({ matcher: { type: "command_prefix", argv: ["awk"] } }), invalid_reason: "unpinned awk" },
      noTitle,
    );
    expect(inv.invalidReason).toBe("unpinned awk");

    const invLocal = ruleText(
      rule({ matcher: { type: "command_prefix", argv: ["awk"] } }),
      noTitle,
    );
    expect(invLocal.invalidReason).toMatch(/awk/);

    const okDeno = ruleText(
      rule({ matcher: { type: "command_prefix", argv: ["deno"] } }),
      noTitle,
    );
    expect(okDeno.invalidReason).toBeNull();
  });
});
