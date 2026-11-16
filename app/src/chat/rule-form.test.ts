import { describe, expect, it } from "vitest";
import { PATH_KINDS, broadGlob, expandsBraces, foldName, checkGlob, commandRisk, emptyRuleForm, globSyntaxError, pathScopeOffered, type RuleForm, splitCommand, validateRule } from "./rule-form";

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
    expect(runner.ok && runner.warnings[0]).toMatch(/python3 script it names, with whatever arguments follow/);
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
  it("warns on interpreters and runners that name their script or package, and on remote runners", () => {
    for (const c of ["python3 scripts/build.py", "node build.js", "npx prettier", "ssh host", "docker", "docker run img", "deno run x.ts", "bun x.ts"]) {
      const v = validateRule(form({ command: c }), ctx);
      expect(v.ok && v.warnings.length, c).toBe(1);
    }
    expect(commandRisk(["cargo", "test"])).toBeNull();
    expect(commandRisk(["git", "-c", "x=y", "status"])).toBeNull();
    // Exact rules can name the interpreter's options: nothing can be added after them.
    expect(commandRisk(["python3", "-m", "pytest"], "command_exact")).toMatchObject({ warn: expect.any(String) });
    expect(commandRisk(["find", ".", "-name", "x"], "command_exact")).toBeNull();
    expect(commandRisk(["find", ".", "-delete"], "command_exact")).toMatchObject({ warn: expect.any(String) });
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

describe("rules editor: the allow check fails closed (round 3, blocker 1)", () => {
  const refused = (command: string, matcher: "command_prefix" | "command_exact" = "command_prefix") => {
    const v = validateRule(form({ matcher, command }), ctx);
    return !v.ok && v.errors.command !== undefined;
  };
  it("folds names as the disk does: case, ſ and K (U+212A), any folder", () => {
    for (const c of ["/bin/BASH -c 'ls'", "Bash -c x", "/usr/bin/ENV ls", "SUDO ls", "Python3 -c 'x'", "/bin/baſh", "/bin/\u212Ash", "FIND . -exec rm {} +", "Find . -EXEC rm {} +", "EVAL x", "Source x", "NoHup make", "RUN0 ls", "/usr/bin/SUDOEDIT f", "PERL -E x"]) {
      expect(refused(c), c).toBe(true);
      expect(refused(c, "command_exact"), c).toBe(true);
    }
  });
  it("never trips on Object.prototype names", () => {
    for (const c of ["constructor x", "toString", "__proto__ x", "hasOwnProperty", "valueOf -c"]) {
      expect(() => validateRule(form({ command: c }), ctx), c).not.toThrow();
      expect(validateRule(form({ command: c }), ctx).ok, c).toBe(true);
    }
  });
  it("finds a code flag anywhere in an option word: attached code, digit clusters, any case", () => {
    for (const c of [`python3 -c'import os; os.system("sh")'`, `python3 -c"print(1)"`, "python3 -Ic x", "python3 -IC x", `node -e"1"`, "node -pe 1", "node --EVAL=1", "node --print", `perl -e'system("sh")'`, `perl -0e 'x'`, `perl -0777ne 'x'`, `ruby -0e 'x'`, `ruby -e'x'`, `php -r'x;'`, `lua -e'x'`, "osascript -e x", "bun -e 1", "bun --eval=1", "python3.12 -c x", "python3-dbg -c x"]) {
      expect(refused(c), c).toBe(true);
      expect(refused(c, "command_exact"), c).toBe(true);
    }
  });
  it("checks the subcommand on the first word that isn't an option", () => {
    for (const c of ["deno eval 1", "deno -q eval 1", "deno --quiet eval 1", "bun eval 1", "bun --silent eval 1", "deno repl --eval 1", "Deno EVAL 1"]) {
      expect(refused(c), c).toBe(true);
      expect(refused(c, "command_exact"), c).toBe(true);
    }
  });
  it("refuses PowerShell given code, by any prefix PowerShell accepts", () => {
    for (const c of ["pwsh -c x", "pwsh -Command x", "pwsh -com x", "pwsh --command x", "pwsh /c x", "pwsh -EncodedCommand AA", "pwsh -ec AA", "pwsh -e AA", "pwsh -en AA", "pwsh -cwa x", "pwsh -CommandWithArgs x", "powershell -NoProfile -Command x", "pwsh.exe -c x", "pwsh -command:x"]) {
      expect(refused(c), c).toBe(true);
      expect(refused(c, "command_exact"), c).toBe(true);
    }
    // Only a named script is clear: Windows PowerShell reads bare words as a command.
    expect(refused("pwsh")).toBe(true);
    expect(refused("powershell build")).toBe(true);
    expect(refused("pwsh -File build.ps1")).toBe(false);
    expect(refused("pwsh -NoProfile -ExecutionPolicy Bypass -File build.ps1")).toBe(false);
  });
  it("refuses deny.rs's PRIV wrappers", () => {
    for (const c of ["run0 ls", "sudoedit /etc/hosts", "pkexec ls", "doas ls", "su -c x"]) expect(refused(c), c).toBe(true);
  });
  it("refuses a prefix rule on an interpreter, runner or find unless the script, package or file comes right after it", () => {
    for (const c of ["python3", "python3 -u", "python3 -m pytest", "python3 -W ignore", "python3 -- x.py", "node", "node --inspect", "perl", "deno", "deno run", "deno run --allow-all x.ts", "bun", "bun run", "bun test", "npx", "npx -y prettier", "npx -p pkg cmd", "uvx", "find", "find .", "find . -name x", "python3 -", "python3 x.py -"]) {
      expect(refused(c), c).toBe(true);
    }
    for (const c of ["python3 scripts/build.py", "python3 x.py --flag", "node build.js", "deno run x.ts", "bun run build", "npx prettier --write", "uvx ruff"]) {
      expect(refused(c), c).toBe(false);
    }
    // Fail closed: a code flag anywhere counts, even after the script (where it is the script's).
    expect(refused("perl tool.pl -e", "command_exact")).toBe(true);
    // Exact rules can't grow, so options are fine there.
    for (const c of ["python3 -m pytest", "python3 -u build.py", "npx -y prettier", "find . -name x"]) expect(refused(c, "command_exact"), c).toBe(false);
    for (const c of ["npx -c 'sh'", "npx --call=x", "pnpx -c x"]) expect(refused(c, "command_exact"), c).toBe(true);
  });
  it("explains the rule in the error text", () => {
    const v = validateRule(form({ command: "python3 -u" }), ctx);
    expect(!v.ok && v.errors.command).toMatch(/right after it, like python3 scripts\/build.py/);
    const f = validateRule(form({ command: "find ." }), ctx);
    expect(!f.ok && f.errors.command).toMatch(/This exact command/);
    expect(foldName("/usr/bin/Baſh")).toBe("bash");
    expect(foldName("\u212Ash")).toBe("ksh");
  });
});

describe("rules editor: round 3 minors", () => {
  it("refuses a line break anywhere, quoted too", () => {
    for (const c of ["echo 'a\nb'", 'echo "a\rb"', "a\nb"]) expect(splitCommand(c), JSON.stringify(c)).toEqual({ ok: false, error: "A rule matches one line: remove the line break" });
  });
  it("reads brace expansion with nested braces, as shell.rs does", () => {
    for (const bad of ["echo {{a},b}", "echo {a,{b}}", "echo {1..3}", "echo x{a,b}y"]) expect(splitCommand(bad).ok, bad).toBe(false);
    for (const good of ["echo {}", "echo {a}", "echo {{a}}", "find . -exec x {} +", "echo {a b,c}"]) expect(splitCommand(good).ok, good).toBe(true);
    expect(expandsBraces("{{a},b}", 0)).toBe(true);
    expect(expandsBraces("{{a},b}", 1)).toBe(false);
  });
  it("matches globset 0.4.21 on classes (checked by building each glob with it)", () => {
    // globset refuses these ("invalid range"): a `-` after a range extends it from its start.
    for (const bad of ["/a/[a-z-_]", "/a/[a-z-0]", "/a/[\\"]) expect(globSyntaxError(bad), bad).not.toBeNull();
    // and builds these: no escapes inside a class.
    for (const good of ["/a/[\\]", "/a/[\\]]", "/a/[a-c-e]", "/a/[a-c-b]", "/a/[a\\-z]"]) expect(globSyntaxError(good), good).toBeNull();
  });
});

describe("rules editor: round 3 follow-up (review 3, finding 1)", () => {
  const refused = (command: string, matcher: "command_prefix" | "command_exact" = "command_prefix") => {
    const v = validateRule(form({ matcher, command }), ctx);
    return !v.ok && v.errors.command !== undefined;
  };
  it("refuses bun exec, which runs a shell string", () => {
    for (const c of ["bun exec 'rm -rf build'", "bun exec echo", "bun --silent exec sh", "BUN EXEC x"]) {
      expect(refused(c), c).toBe(true);
      expect(refused(c, "command_exact"), c).toBe(true);
    }
  });
  it("refuses -c in a package runner's option cluster or with its value attached", () => {
    for (const c of ["npx -yc sh", "npx -c'sh'", "npx prettier -yc sh", "npx prettier -c'sh'", "pnpx -Yc x", "bunx -c x", "uvx --CALL=x pkg"]) {
      expect(refused(c, "command_exact"), c).toBe(true);
      expect(refused(c), c).toBe(true);
    }
  });
  it("refuses a script read from standard input, by any name", () => {
    for (const c of ["python3 /dev/stdin", "node /dev/fd/0", "perl /DEV/STDIN", "ruby -", "python3 -u /proc/self/fd/0"]) {
      expect(refused(c), c).toBe(true);
      expect(refused(c, "command_exact"), c).toBe(true);
    }
  });
  it("refuses exact rules with no script or package: the interpreter would read code from its input", () => {
    for (const c of ["python3", "python3 -u", "node", "perl", "ruby", "php", "lua", "osascript", "deno", "deno run", "bun", "bun run", "npx", "npx -y", "uvx"]) {
      expect(refused(c, "command_exact"), c).toBe(true);
    }
    for (const c of ["python3 -m pytest", "python3 -u build.py", "npx -y prettier", "node build.js", "deno run x.ts", "bun run build"]) {
      expect(refused(c, "command_exact"), c).toBe(false);
    }
  });
});
