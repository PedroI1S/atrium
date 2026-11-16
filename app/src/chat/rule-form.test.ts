import { describe, expect, it } from "vitest";
import { PATH_KINDS, broadGlob, checkGlob, commandRisk, emptyRuleForm, globSyntaxError, pathScopeOffered, type RuleForm, splitCommand, validateRule } from "./rule-form";

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
  it("builds an exact command deny rule in Home, never with a path scope", () => {
    // Round 2 blocker 2: the engine adds the cwd (~ in Home) to a command's paths, so a scoped
    // command rule never matches; a leftover scope from another matcher is dropped.
    const v = validateRule(form({ decision: "deny", matcher: "command_exact", command: "git push", scope: "home", pathScope: "~/work/**" }), ctx);
    expect(v.ok && v.draft).toEqual({ scope: { type: "home" }, tool_kind: "execute", tool_name: null, matcher: { type: "command_exact", argv: ["git", "push"] }, path_scope: null, decision: "deny" });
  });
  it("builds a file rule limited to a folder", () => {
    const v = validateRule(form({ matcher: "any", toolKind: "edit", pathScope: "~/notes/**" }), ctx);
    expect(v.ok && v.draft).toMatchObject({ tool_kind: "edit", matcher: { type: "any" }, path_scope: "/Users/me/notes/**", decision: "allow" });
    expect(v.ok && v.warnings).toEqual(["This allows every call of this kind inside that folder without asking."]);
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
    const runner = validateRule(form({ command: "python3 scripts/build.py" }), ctx);
    expect(runner.ok && runner.warnings[0]).toMatch(/every script python3 is given after these words/);
    const every = validateRule(form({ matcher: "any", toolKind: "execute" }), ctx);
    expect(every.ok && every.warnings).toEqual(["This allows every command without asking."]);
    const root = validateRule(form({ matcher: "path_glob", glob: "/**" }), ctx);
    expect(root.ok && root.warnings).toEqual(["This matches every path on this computer."]);
    const deny = validateRule(form({ decision: "deny", command: "bash" }), ctx);
    expect(deny.ok && deny.warnings).toEqual([]);
  });
  it("warns on globs that cover the whole disk or home folder (round 2, minor 4)", () => {
    for (const g of ["/", "/**", "/**/*", "/*", "/**/"]) expect(broadGlob(g, HOME), g).toBe("computer");
    for (const g of ["~", "~/**", "~/*", "~/**/*"]) expect(broadGlob(checkGlob(g, HOME).ok ? (checkGlob(g, HOME) as { glob: string }).glob : "", HOME), g).toBe("home");
    expect(broadGlob("/Users/me/src/**", HOME)).toBeNull();
    expect(validateRule(form({ matcher: "path_glob", glob: "/**/*" }), ctx)).toMatchObject({ ok: true, warnings: ["This matches every path on this computer."] });
    expect(validateRule(form({ matcher: "path_glob", glob: "~/**" }), ctx)).toMatchObject({ ok: true, warnings: ["This matches every path in your home folder."] });
  });
  it("keeps the every-call warning when a path limit is set (round 2, minor 4)", () => {
    expect(validateRule(form({ matcher: "any", toolKind: "read", pathScope: "/**" }), ctx)).toMatchObject({ ok: true, warnings: ["This allows every call of this kind without asking."] });
    expect(validateRule(form({ matcher: "any", toolKind: "read", pathScope: "~/**" }), ctx)).toMatchObject({ ok: true, warnings: ["This allows every call of this kind in your home folder without asking."] });
  });
  it("keeps the tool kind to ones the matcher can use", () => {
    const v = validateRule(form({ matcher: "path_glob", glob: "/x", toolKind: "fetch" }), ctx);
    expect(v.ok && v.draft.tool_kind).toBe("edit");
    const any = validateRule(form({ matcher: "any", toolKind: "think", decision: "deny" }), ctx);
    expect(any.ok && any.draft.tool_kind).toBe("other");
  });
});

describe("rules editor: allow rules on shells, wrappers and inline code (round 2, blocker 1)", () => {
  const refused = (command: string, matcher: "command_prefix" | "command_exact" = "command_prefix") => {
    const v = validateRule(form({ matcher, command }), ctx);
    return !v.ok && v.errors.command !== undefined;
  };
  it("refuses shells, eval/source/., and every wrapper the engine strips, with or without a path or more words", () => {
    for (const c of ["bash", "sh -c", "bash -c 'ls'", "/bin/zsh", "dash -ec", "fish", "eval", "source", ". ./env.sh", "env", "env -i", "/usr/bin/env FOO=1 make", "exec ls", "command ls", "nohup make", "time cargo test", "nice -n 10 make", "xargs rm", "sudo ls", "doas ls", "timeout 60 cargo test", "stdbuf -oL make", "flock /tmp/l make", "script -q log", "chroot /x ls", "builtin cd", "busybox ls", "su -", "setsid ls", "caffeinate make", "arch -arm64 make"]) {
      expect(refused(c), c).toBe(true);
      expect(refused(c, "command_exact"), c).toBe(true);
    }
  });
  it("refuses interpreters given code inline, also in an option cluster", () => {
    for (const c of ["python3 -c", "python -c 'print(1)'", "python3.12 -Ic x", "node -e", "node --eval", "node --eval=1", "node -p", "ruby -e", "ruby -ne x", "perl -e", "perl -lne", "perl -E", "osascript -e", "deno eval", "bun -e", "php -r", "lua -e", "find . -exec rm {} +", "find . -execdir x", "find . -ok rm"]) {
      expect(refused(c), c).toBe(true);
    }
  });
  it("warns, whatever the argv length, on interpreters, script and remote runners, and find", () => {
    for (const c of ["python3", "python3 -m pytest", "node", "node build.js", "npx", "npx -y prettier", "ssh host", "docker", "docker run img", "find", "find . -name x"]) {
      const v = validateRule(form({ command: c }), ctx);
      expect(v.ok && v.warnings.length, c).toBe(1);
    }
    expect(commandRisk(["deno", "run", "x.ts"])).toMatchObject({ warn: expect.any(String) });
    expect(commandRisk(["python3", "--", "-c"])).toMatchObject({ warn: expect.any(String) });
    expect(commandRisk(["cargo", "test"])).toBeNull();
    expect(commandRisk(["git", "-c", "x=y", "status"])).toBeNull();
  });
  it("allows deny rules on all of them", () => {
    for (const c of ["bash", "sh -c", "sudo", "env", "python3 -c", "find . -exec"]) {
      const v = validateRule(form({ decision: "deny", command: c }), ctx);
      expect(v.ok && v.warnings, c).toEqual([]);
    }
  });
});

describe("rules editor: what the engine never matches (round 2, minor 4)", () => {
  it("refuses a word-initial #, which starts a comment, but not a # inside a word or quoted", () => {
    expect(splitCommand("cargo test # note").ok).toBe(false);
    expect(splitCommand("#x").ok).toBe(false);
    expect(splitCommand("echo a#b")).toEqual({ ok: true, argv: ["echo", "a#b"] });
    expect(splitCommand("echo '#'")).toEqual({ ok: true, argv: ["echo", "#"] });
  });
  it("refuses control characters and the Unicode line separators", () => {
    for (const bad of ["ls\u0000", "ls\u001b[0m", "ls\u007f", "ls\u0085", "ls\u2028x", "ls\u2029"]) {
      expect(splitCommand(bad), JSON.stringify(bad)).toMatchObject({ ok: false, error: expect.stringMatching(/control character/) });
    }
    // A tab splits words, as in the shell.
    expect(splitCommand("ls\t-l")).toEqual({ ok: true, argv: ["ls", "-l"] });
  });
  it("refuses an empty command name and reserved first words", () => {
    expect(splitCommand("'' x").ok).toBe(false);
    expect(splitCommand('""').ok).toBe(false);
    for (const w of ["if", "for", "while", "!", "{", "}", "case", "function", "coproc", "'if'"]) expect(splitCommand(`${w} x`).ok, w).toBe(false);
    expect(splitCommand("iffy x").ok).toBe(true);
  });
  it("refuses globs globset can't build", () => {
    for (const bad of ["/a/[", "/a/[]", "/a/[!]", "/a/][", "/a/[z-a]", "/a/[z--]", "/a/x\\", "/a/{b", "/a/b}", "/a/{b}}"]) {
      expect(globSyntaxError(bad), bad).not.toBeNull();
      expect(checkGlob(bad, HOME).ok, bad).toBe(false);
    }
    for (const good of ["/a/[ab]", "/a/[]]", "/a/[!]x]", "/a/[a-]", "/a/[-a]", "/a/]", "/a/\\[x", "/a/{b,c}", "/a/{b,{c,d}}", "/a/[[]"]) {
      expect(globSyntaxError(good), good).toBeNull();
    }
  });
});

describe("rules editor: where a path limit is offered (round 2, blocker 2)", () => {
  it("only on allow rules about file calls; never on commands or deny rules", () => {
    expect(pathScopeOffered({ decision: "allow", matcher: "path_glob", toolKind: "edit" })).toBe(true);
    expect(pathScopeOffered({ decision: "allow", matcher: "any", toolKind: "read" })).toBe(true);
    expect(pathScopeOffered({ decision: "allow", matcher: "any", toolKind: "execute" })).toBe(false);
    expect(pathScopeOffered({ decision: "allow", matcher: "command_prefix", toolKind: "execute" })).toBe(false);
    expect(pathScopeOffered({ decision: "allow", matcher: "command_exact", toolKind: "execute" })).toBe(false);
    for (const matcher of ["path_glob", "any", "command_prefix", "command_exact"] as const) expect(pathScopeOffered({ decision: "deny", matcher, toolKind: "edit" }), matcher).toBe(false);
  });
  it("path rules are never about commands", () => {
    expect(PATH_KINDS).not.toContain("execute");
    const v = validateRule(form({ matcher: "path_glob", glob: "/x", toolKind: "execute" }), ctx);
    expect(v.ok && v.draft.tool_kind).toBe("edit");
  });
  it("drops the path limit from command and deny rules", () => {
    for (const over of [{ command: "cargo test" }, { matcher: "any" as const, toolKind: "execute" as const }, { decision: "deny" as const, matcher: "path_glob" as const, glob: "/x" }]) {
      const v = validateRule(form({ ...over, pathScope: "~/projects/**" }), ctx);
      expect(v.ok && v.draft.path_scope, JSON.stringify(over)).toBeNull();
    }
    // An invalid leftover limit doesn't block a rule that can't have one.
    expect(validateRule(form({ command: "cargo test", pathScope: "relative" }), ctx).ok).toBe(true);
  });
});
