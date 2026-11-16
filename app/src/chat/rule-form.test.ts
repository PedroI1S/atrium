import { describe, expect, it } from "vitest";
import { checkGlob, emptyRuleForm, type RuleForm, splitCommand, validateRule } from "./rule-form";

const HOME = "/Users/me";
const ctx = { home: HOME, threadExists: (id: string) => id === "t1" };
const form = (over: Partial<RuleForm>): RuleForm => ({ ...emptyRuleForm(), ...over });

describe("rules editor: the command, as argv", () => {
  it("splits on spaces and removes quotes and backslashes", () => {
    expect(splitCommand("git status")).toEqual({ ok: true, argv: ["git", "status"] });
    expect(splitCommand("  rm  -rf   build ")).toEqual({ ok: true, argv: ["rm", "-rf", "build"] });
    expect(splitCommand("echo 'a b' \"c d\" e\\ f")).toEqual({ ok: true, argv: ["echo", "a b", "c d", "e f"] });
    expect(splitCommand("grep -e '' x")).toEqual({ ok: true, argv: ["grep", "-e", "", "x"] });
    expect(splitCommand(`printf "\\"q\\""`)).toEqual({ ok: true, argv: ["printf", '"q"'] });
  });
  it("keeps ~ as typed (the policy reads it so) and quoted specials literally", () => {
    expect(splitCommand("ls ~/src")).toEqual({ ok: true, argv: ["ls", "~/src"] });
    expect(splitCommand("echo '*' '$HOME' 'a;b'")).toEqual({ ok: true, argv: ["echo", "*", "$HOME", "a;b"] });
    expect(splitCommand("find . -exec rm {} +")).toEqual({ ok: true, argv: ["find", ".", "-exec", "rm", "{}", "+"] });
    expect(splitCommand("make VERBOSE=1")).toEqual({ ok: true, argv: ["make", "VERBOSE=1"] });
  });
  it("refuses what makes a command not simple: no rule would match it", () => {
    for (const bad of ["ls; rm x", "a && b", "a | b", "a > out", "cat < in", "(ls)", "echo $HOME", "echo `id`", 'echo "$x"', "rm *.o", "ls ?", "ls [ab]", "echo {a,b}", "FOO=1 make", "ls ~root", "a\nb"]) {
      expect(splitCommand(bad).ok, bad).toBe(false);
    }
  });
  it("refuses unclosed quotes, a trailing backslash and an empty line", () => {
    expect(splitCommand("echo 'x")).toEqual({ ok: false, error: "A single quote isn't closed" });
    expect(splitCommand('echo "x')).toEqual({ ok: false, error: "A double quote isn't closed" });
    expect(splitCommand("echo x\\").ok).toBe(false);
    expect(splitCommand("   ")).toEqual({ ok: false, error: "Type the command" });
  });
});

describe("rules editor: path globs", () => {
  it("writes ~ out as the home folder", () => {
    expect(checkGlob("~/projects/**", HOME)).toEqual({ ok: true, glob: "/Users/me/projects/**" });
    expect(checkGlob("~", HOME)).toEqual({ ok: true, glob: "/Users/me" });
    expect(checkGlob("/tmp/*.log", HOME)).toEqual({ ok: true, glob: "/tmp/*.log" });
  });
  it("refuses relative paths, ~name, . and .., unbalanced brackets, and ~ without a known home", () => {
    for (const bad of ["", "src/**", "./x", "~bob/x", "/a/../b", "/a/./b", "/a/{b", "/a/b}", "/a/[b"]) expect(checkGlob(bad, HOME).ok, bad).toBe(false);
    expect(checkGlob("~/x", null).ok).toBe(false);
  });
});

describe("rules editor: the draft", () => {
  it("builds a command prefix rule, always for commands", () => {
    const v = validateRule(form({ matcher: "command_prefix", command: "cargo test", toolKind: "edit" }), ctx);
    expect(v).toEqual({ ok: true, warnings: [], draft: { scope: { type: "global" }, tool_kind: "execute", tool_name: null, matcher: { type: "command_prefix", argv: ["cargo", "test"] }, path_scope: null, decision: "allow" } });
  });
  it("builds an exact command deny rule in Home, with a path scope", () => {
    const v = validateRule(form({ decision: "deny", matcher: "command_exact", command: "git push", scope: "home", pathScope: "~/work/**" }), ctx);
    expect(v.ok && v.draft).toEqual({ scope: { type: "home" }, tool_kind: "execute", tool_name: null, matcher: { type: "command_exact", argv: ["git", "push"] }, path_scope: "/Users/me/work/**", decision: "deny" });
  });
  it("builds a path rule for one chat and one tool", () => {
    const v = validateRule(form({ matcher: "path_glob", glob: "~/notes/**", toolKind: "edit", toolName: " Edit ", scope: "thread", threadId: "t1" }), ctx);
    expect(v.ok && v.draft).toEqual({ scope: { type: "thread", thread_id: "t1" }, tool_kind: "edit", tool_name: "Edit", matcher: { type: "path_glob", glob: "/Users/me/notes/**" }, path_scope: null, decision: "allow" });
  });
  it("reports every problem at once, per field", () => {
    const v = validateRule(form({ matcher: "path_glob", glob: "notes", toolName: "two words", pathScope: "rel", scope: "thread", threadId: "gone" }), ctx);
    expect(v.ok).toBe(false);
    expect(!v.ok && Object.keys(v.errors).sort()).toEqual(["glob", "pathScope", "thread", "toolName"]);
  });
  it("refuses an empty or compound command", () => {
    expect(validateRule(form({ command: "" }), ctx)).toMatchObject({ ok: false, errors: { command: "Type the command" } });
    expect(validateRule(form({ command: "ls && rm -rf x" }), ctx).ok).toBe(false);
  });
  it("warns when an allow rule is broader than it may look", () => {
    const runner = validateRule(form({ command: "bash" }), ctx);
    expect(runner.ok && runner.warnings[0]).toMatch(/every command run through bash/);
    const every = validateRule(form({ matcher: "any", toolKind: "execute" }), ctx);
    expect(every.ok && every.warnings).toEqual(["This allows every command without asking."]);
    const root = validateRule(form({ matcher: "path_glob", glob: "/**" }), ctx);
    expect(root.ok && root.warnings).toEqual(["This matches every path on this computer."]);
    const deny = validateRule(form({ decision: "deny", command: "bash" }), ctx);
    expect(deny.ok && deny.warnings).toEqual([]);
  });
  it("keeps the tool kind to ones the matcher can use", () => {
    const v = validateRule(form({ matcher: "path_glob", glob: "/x", toolKind: "fetch" }), ctx);
    expect(v.ok && v.draft.tool_kind).toBe("edit");
    const any = validateRule(form({ matcher: "any", toolKind: "think", decision: "deny" }), ctx);
    expect(any.ok && any.draft.tool_kind).toBe("other");
  });
});
