import { describe, expect, it } from "vitest";
import type { ListedRule } from "../daemon/types";
import {
  PATH_KINDS,
  broadGlob,
  checkGlob,
  checkStoredRule,
  commandRisk,
  emptyRuleForm,
  expandsBraces,
  foldName,
  globSyntaxError,
  markStoredRules,
  pathScopeOffered,
  type RuleForm,
  splitCommand,
  validateRule,
} from "./rule-form";

const HOME = "/Users/me";
const ctx = { home: HOME, threadExists: (id: string) => id === "t1" };
const form = (over: Partial<RuleForm>): RuleForm => ({ ...emptyRuleForm(), ...over });

const storedRule = (over: Partial<ListedRule>): ListedRule => ({
  id: "r1",
  scope: { type: "global" },
  tool_kind: "execute",
  tool_name: null,
  matcher: { type: "command_prefix", argv: ["cat"] },
  path_scope: null,
  decision: "allow",
  origin: { type: "settings" },
  created_at_ms: 1,
  invalid_reason: null,
  ...over,
});

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
    expect(runner.ok && runner.warnings[0]).toMatch(/python3 script or package scripts\/build\.py/);
    const every = validateRule(form({ matcher: "any", toolKind: "execute" }), ctx);
    expect(every.ok && every.warnings).toEqual(["This allows every command without asking."]);
    const root = validateRule(form({ matcher: "path_glob", glob: "/**" }), ctx);
    expect(root.ok && root.warnings).toEqual(["This matches every path on this computer."]);
    const deny = validateRule(form({ decision: "deny", command: "bash" }), ctx);
    expect(deny.ok && deny.warnings).toEqual([]);
  });
  it("warns on globs that cover the whole disk or home folder", () => {
    for (const g of ["/", "/**", "/**/*", "/*", "/**/"]) expect(broadGlob(g, HOME), g).toBe("computer");
    for (const g of ["~", "~/**", "~/*", "~/**/*"]) expect(broadGlob(checkGlob(g, HOME).ok ? (checkGlob(g, HOME) as { glob: string }).glob : "", HOME), g).toBe("home");
    expect(broadGlob("/Users/me/src/**", HOME)).toBeNull();
    expect(validateRule(form({ matcher: "path_glob", glob: "/**/*" }), ctx)).toMatchObject({ ok: true, warnings: ["This matches every path on this computer."] });
    expect(validateRule(form({ matcher: "path_glob", glob: "~/**" }), ctx)).toMatchObject({ ok: true, warnings: ["This matches every path in your home folder."] });
  });
  it("keeps the every-call warning when a path limit is set", () => {
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

describe("rules editor: rev 4 §7 & D-141 parity with core", () => {
  const refused = (command: string, matcher: "command_prefix" | "command_exact" = "command_prefix") => {
    const v = validateRule(form({ matcher, command }), ctx);
    return !v.ok && v.errors.command !== undefined;
  };

  it("refuses unpinned Allow prefixes for OUTSIDE_MODEL shells/wrappers, NestedShell, CodeRunner, and ConditionalRunner", () => {
    for (const c of [
      // NestedShell without script pin
      "sh", "bash", "dash", "sh -c", "bash -c 'ls'", "dash -ec",
      // OUTSIDE_MODEL shells & wrappers
      "/bin/zsh", "fish", "ksh", "nohup make", "builtin cd", "busybox ls", "setsid ls",
      "caffeinate make", "chroot /x ls", "stdbuf -oL make", "flock /tmp/l make", "script -q log", "arch -arm64 make",
      // FirstOperand CodeRunners without pin
      "python", "python3", "python3 -c", "python3 -u build.py", "python3 -",
      "node", "node -e '1'", "node --eval=1", "perl", "perl -e '1'", "ruby", "ruby -e '1'",
      "source", ".", "npx", "npx -y prettier",
      // FlagValue CodeRunners (`awk` family)
      "awk", "gawk", "mawk", "awk -f", "awk -f -", "awk -f a.awk -e '{print}'",
      // Multi-subcommand CodeRunners
      "cargo", "cargo --version", "pipx", "pipx run", "pipx install ruff",
      // No-pin CodeRunners (`eval`, `xargs`, `COMMAND_RUNNERS`)
      "eval", "xargs rm", "ssh host", "docker", "docker run img", "kubectl exec pod", "parallel echo", "su -",
      // ConditionalRunner `find`
      "find", "find .", "find . -name x", "find . -exec rm {} +",
      // ConditionalRunner `git`
      "git", "git -c x=y status", "git merge-index", "git filter-branch",
      "git bisect", "git bisect run", "git submodule", "git submodule foreach",
      "git rebase -x id", "git diff --ext-diff", "git push --exec=x", "git push --receive-pack=x",
      // ConditionalRunner `sed`
      "sed", "sed -f", "sed -f -", "sed '1e id'", "sed 's/a/b/w out'", "sed -i p",
    ]) {
      expect(refused(c), c).toBe(true);
    }
    // `m1-demo.ts` expects the bash error text to contain "can't allow bash".
    const bashErr = validateRule(form({ command: "bash -c 'cargo test'" }), ctx);
    expect(!bashErr.ok && bashErr.errors.command).toMatch(/can't allow bash/);
  });

  it("accepts pinned Allow prefixes and safe sed/git/cargo forms matching core (including `sed z`)", () => {
    for (const ok of [
      "sh x.sh", "bash scripts/build.sh", "dash run.sh",
      "source env.sh", ". ./env.sh",
      "python3 scripts/build.py", "python build.py", "node build.js", "perl run.pl", "ruby app.rb", "npx prettier",
      "awk -f prog.awk", "awk -f prog.awk -vfoo=1", "awk -f prog.awk -Fsep", "awk -f prog.awk -F -e",
      "sed -f edit.sed", "sed -f edit.sed -i.ref", "sed -f edit.sed -l80",
      "pipx run ruff",
    ]) {
      const v = validateRule(form({ command: ok }), ctx);
      expect(v.ok && v.warnings.length, ok).toBe(1);
    }
    for (const clean of [
      "cargo test", "git status", "git diff", "git bisect start", "git submodule status",
      "git rebase -Xcontext", "git rebase -S0x1234", "git difftool -txxdiff", "git mergetool -txxdiff",
      "git diff -S --ext-diff", "git push -o --exec",
      "sed 's/error/warn/g'", "sed z", "sed F", "sed '1q 0'", "sed 'l 80'",
    ]) {
      const split = splitCommand(clean);
      expect(split.ok, clean).toBe(true);
      expect(commandRisk(split.ok ? split.argv : []), clean).toBeNull();
    }
    for (const bad of [
      "git rebase -xid", "git rebase -ix id", "git difftool -xid", "git difftool -yx id", "git mergetool -xid",
      "awk -f prog.awk -nf other.awk", "awk -f prog.awk -ne '{print}'",
      "sed -f edit.sed -nf other.sed", "sed -f edit.sed -ne p", "sed -f edit.sed -i", "sed -f edit.sed -l",
    ]) {
      expect(refused(bad), bad).toBe(true);
    }
  });

  it("accepts tools outside the frozen D-141 table (Finding 8: deno, bun, php, lua, pwsh, etc.) in both validateRule and checkStoredRule", () => {
    for (const unknown of ["deno", "deno eval 1", "bun", "bun exec sh", "php -r 'x;'", "lua -e 'x'", "osascript -e x", "pwsh -c x", "powershell -Command x", "uvx", "pnpx", "bunx", "cat", "rm -rf build"]) {
      const v = validateRule(form({ command: unknown }), ctx);
      expect(v.ok, unknown).toBe(true);
      if (v.ok && v.draft.matcher.type === "command_prefix") {
        expect(checkStoredRule(storedRule({ matcher: v.draft.matcher })), unknown).toEqual({ valid: true, reason: null });
      }
    }
  });

  it("accepts CommandExact rules and Deny rules matching core `validate_rule_matcher`", () => {
    for (const c of ["python3 -m pytest", "python3 -u build.py", "find . -name x", "ssh host ls", "docker run img", "bash -c 'ls'"]) {
      expect(refused(c, "command_exact"), c).toBe(false);
    }
    expect(commandRisk(["find", ".", "-delete"], "command_exact")).toMatchObject({ warn: expect.any(String) });
    for (const c of ["bash", "sh -c", "python3 -c", "find . -exec", "awk", "git", "pipx run", "sed", "docker"]) {
      const v = validateRule(form({ decision: "deny", command: c }), ctx);
      expect(v.ok && v.warnings, c).toEqual([]);
    }
  });

  it("folds names via CR-19 (basename, case, ſ and K) and never trips on Object.prototype", () => {
    for (const c of ["/bin/BASH -c 'ls'", "Bash -c x", "Python3 -c 'x'", "/bin/baſh", "/bin/\u212Ash", "FIND . -exec rm {} +", "EVAL x", "NoHup make", "AWK", "/usr/bin/GAWK", "GIT", "CARGO", "PIPX run", "SOURCE"]) {
      expect(refused(c), c).toBe(true);
    }
    for (const c of ["constructor x", "toString", "__proto__ x", "hasOwnProperty", "valueOf -c"]) {
      expect(() => validateRule(form({ command: c }), ctx), c).not.toThrow();
      expect(validateRule(form({ command: c }), ctx).ok, c).toBe(true);
    }
    expect(foldName("/usr/bin/Baſh")).toBe("bash");
    expect(foldName("\u212Ash")).toBe("ksh");
  });

  it("explains the pinning rule in the error text", () => {
    const v = validateRule(form({ command: "python3 -u" }), ctx);
    expect(!v.ok && v.errors.command).toMatch(/right after it \(e\.g\. python3 scripts\/build\.py\)/);
    const f = validateRule(form({ command: "find ." }), ctx);
    expect(!f.ok && f.errors.command).toMatch(/This exact command/);
  });
});

describe("rules editor: stored invalid rules (§11 item 5)", () => {
  it("marks stored rules rejected by rev 4 §7 invalid with a reason, while keeping valid rules valid", () => {
    const valid = storedRule({ id: "r_ok", matcher: { type: "command_prefix", argv: ["cargo", "test"] } });
    const validSedZ = storedRule({ id: "r_sed_z", matcher: { type: "command_prefix", argv: ["sed", "z"] } });
    const validDeno = storedRule({ id: "r_deno", matcher: { type: "command_prefix", argv: ["deno"] } });
    const invAwk = storedRule({ id: "r_awk", matcher: { type: "command_prefix", argv: ["awk"] } });
    const invGit = storedRule({ id: "r_git", matcher: { type: "command_prefix", argv: ["git"] } });
    const invPipx = storedRule({ id: "r_pipx", matcher: { type: "command_prefix", argv: ["pipx", "run"] } });
    const invDaemon = { ...valid, id: "r_daemon", invalid_reason: "daemon marked invalid" };
    const invGlob = storedRule({ id: "r_glob", tool_kind: "edit", matcher: { type: "path_glob", glob: "/a/[" } });
    const denyAwk = storedRule({ id: "r_deny", decision: "deny", matcher: { type: "command_prefix", argv: ["awk"] } });

    for (const ok of [valid, validSedZ, validDeno, denyAwk]) {
      expect(checkStoredRule(ok), ok.id).toEqual({ valid: true, reason: null });
    }
    for (const r of [invAwk, invGit, invPipx, invDaemon, invGlob]) {
      const res = checkStoredRule(r);
      expect(res.valid, r.id).toBe(false);
      expect(typeof res.reason === "string" && res.reason.length > 0, r.id).toBe(true);
    }

    const marked = markStoredRules([valid, invAwk, denyAwk]);
    expect(marked[0]!.invalid_reason).toBeNull();
    expect(marked[1]!.invalid_reason).toMatch(/awk/);
    expect(marked[2]!.invalid_reason).toBeNull();
  });
});

describe("rules editor: what the engine never matches", () => {
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

describe("rules editor: where a path limit is offered", () => {
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
    expect(validateRule(form({ command: "cargo test", pathScope: "relative" }), ctx).ok).toBe(true);
  });
});

describe("rules editor: line breaks, braces, and globset classes", () => {
  it("refuses a line break anywhere, quoted too", () => {
    for (const c of ["echo 'a\nb'", 'echo "a\rb"', "a\nb"]) expect(splitCommand(c), JSON.stringify(c)).toEqual({ ok: false, error: "A rule matches one line: remove the line break" });
  });
  it("reads brace expansion with nested braces", () => {
    for (const bad of ["echo {{a},b}", "echo {a,{b}}", "echo {1..3}", "echo x{a,b}y"]) expect(splitCommand(bad).ok, bad).toBe(false);
    for (const good of ["echo {}", "echo {a}", "echo {{a}}", "find . -exec x {} +", "echo {a b,c}"]) expect(splitCommand(good).ok, good).toBe(true);
    expect(expandsBraces("{{a},b}", 0)).toBe(true);
    expect(expandsBraces("{{a},b}", 1)).toBe(false);
  });
  it("matches globset 0.4.21 on classes", () => {
    for (const bad of ["/a/[a-z-_]", "/a/[a-z-0]", "/a/[\\"]) expect(globSyntaxError(bad), bad).not.toBeNull();
    for (const good of ["/a/[\\]", "/a/[\\]]", "/a/[a-c-e]", "/a/[a-c-b]", "/a/[a\\-z]"]) expect(globSyntaxError(good), good).toBeNull();
  });
});
