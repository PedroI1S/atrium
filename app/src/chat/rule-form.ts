// The rules editor in Settings → Approvals (PLAN §3.12, contract §9, CR-06): what the user
// types, checked before it becomes a `RuleDraft` for `CreateRule`. The daemon checks again;
// this catches what would make a rule that never matches, or matches more than it reads.
// Pure; tested in rule-form.test.ts.

import type { RuleDraft, RuleMatcher, RuleScope, ToolKind } from "../daemon/types";

export type MatcherKind = RuleMatcher["type"];

export interface RuleForm {
  decision: "allow" | "deny";
  matcher: MatcherKind;
  /** The command line, for `command_prefix` and `command_exact`. */
  command: string;
  /** The path glob, for `path_glob`. */
  glob: string;
  /** For `path_glob` and `any`; commands are always `execute`. */
  toolKind: ToolKind;
  /** Optional: only the provider's tool of this name. */
  toolName: string;
  /**
   * Optional: every path the call touches must be inside this glob. Only offered where the
   * engine can match it (see `pathScopeOffered`).
   */
  pathScope: string;
  scope: RuleScope["type"];
  /** For `scope === "thread"`. */
  threadId: string | null;
}

export const emptyRuleForm = (): RuleForm => ({
  decision: "allow",
  matcher: "command_prefix",
  command: "",
  glob: "",
  toolKind: "edit",
  toolName: "",
  pathScope: "",
  scope: "global",
  threadId: null,
});

export type Field = "command" | "glob" | "toolName" | "pathScope" | "thread";

export type Validation =
  | { ok: true; draft: RuleDraft; warnings: string[] }
  | { ok: false; errors: Partial<Record<Field, string>> };

/**
 * Tool kinds a path rule can be about. Not `execute`: the engine adds a command's working folder
 * to the paths it checks, so in a Home chat (cwd = ~) a path rule on commands never matches below
 * the home folder (escalated in the M1-23 handoff, round 2).
 */
export const PATH_KINDS: readonly ToolKind[] = ["read", "edit", "delete", "move", "search"];
/** Tool kinds an "every call" rule can be about. */
export const ANY_KINDS: readonly ToolKind[] = ["read", "edit", "delete", "move", "search", "execute", "fetch", "mcp", "other"];

const MAX_TOOL_NAME_CHARS = 100;
const MAX_GLOB_CHARS = 1000;
const MAX_COMMAND_CHARS = 4000;

// What a command allow rule must never be about (round 2, blocker 1). The policy engine
// (`atrium-core` `policy/shell.rs`) reads the code a shell, `eval` or a wrapper runs as derived
// units and drops their reasons, on the assumption that no rule allows a prefix of `sh`; an allow
// rule matches the outer argv only. So a rule on any of these would allow whatever they run.

/** `shell.rs` `SHELLS`. */
const SHELLS = new Set(["sh", "bash", "zsh", "dash", "ksh", "fish", "ash", "csh", "tcsh", "mksh", "yash", "rbash", "posh", "elvish", "xonsh"]);
/** Run their text in this shell (`shell.rs` `runs_here`). */
const RUNS_HERE = new Set(["eval", "source", "."]);
/**
 * The wrappers `shell.rs` `unwrap_reading` strips to find the command they run, and the ones
 * that run it as another user or in another root.
 */
const WRAPPERS = new Set([
  "time", "nohup", "builtin", "command", "setsid", "unbuffer", "caffeinate", "sandbox-exec", "exec", "nice", "ionice", "stdbuf", "xargs", "busybox", "toybox",
  "timeout", "chrt", "taskset", "arch", "chroot", "flock", "script", "env",
  "sudo", "doas", "su", "runuser", "setpriv", "pkexec",
]);
/** `shell.rs` `COMMAND_RUNNERS`: run a command somewhere else (a host, a container). */
const REMOTE_RUNNERS = new Set([
  "ssh", "mosh", "autossh", "nsenter", "unshare", "docker", "podman", "nerdctl", "kubectl", "oc", "lxc", "incus", "machinectl", "systemd-run", "systemd-nspawn",
  "firejail", "bwrap", "flatpak-spawn", "toolbox", "distrobox", "distrobox-enter", "multipass", "vagrant", "limactl", "colima", "parallel",
]);
/** Interpreters, and how each is given code inline: short letters (also in a cluster, `perl -ne`), long options, subcommands. */
const INLINE_CODE: Record<string, { short: string; long: readonly string[]; sub?: readonly string[] }> = {
  python: { short: "c", long: [] },
  node: { short: "ep", long: ["--eval", "--print"] },
  deno: { short: "", long: [], sub: ["eval"] },
  bun: { short: "ep", long: ["--eval", "--print"] },
  ruby: { short: "e", long: [] },
  perl: { short: "eE", long: [] },
  php: { short: "rRBEF", long: ["--run", "--process-code", "--process-begin", "--process-end", "--process-file"] },
  lua: { short: "e", long: [] },
  osascript: { short: "e", long: [] },
};
/** Run scripts or packages whose content the rule doesn't pin. */
const SCRIPT_RUNNERS = new Set(["npx", "pnpx", "bunx", "uvx", "pipx"]);
/** `find` options that run a command for each file (`shell.rs` `find_exec_units`). */
const FIND_EXEC = new Set(["-exec", "-execdir", "-ok", "-okdir"]);

const base = (word: string): string => word.split("/").pop() ?? word;
const interpreterOf = (name: string): keyof typeof INLINE_CODE | null =>
  /^python[0-9.]*$/.test(name) ? "python" : name in INLINE_CODE ? (name as keyof typeof INLINE_CODE) : null;

export type CommandRisk = { refuse: string } | { warn: string } | null;

/**
 * Whether an allow rule on this argv (a prefix or an exact command) would allow more than the
 * command it names. Refused: a shell, `eval`/`source`/`.`, a wrapper, an interpreter given code
 * inline, `find -exec`. Warned: an interpreter or a runner (it runs whatever script or package
 * it is given), `find`. Deny rules on any of them are fine.
 */
export function commandRisk(argv: readonly string[]): CommandRisk {
  const first = argv[0];
  if (first === undefined) return null;
  const name = base(first);
  if (SHELLS.has(name)) return { refuse: `A rule can't allow ${name}: it would allow every command run through it. Write the command itself.` };
  if (RUNS_HERE.has(name)) return { refuse: `A rule can't allow ${name}: it runs whatever text follows it. Write the command itself.` };
  if (WRAPPERS.has(name)) return { refuse: `A rule can't allow ${name}: it runs the command that follows it, and Atrium wouldn't check that one. Write the command without ${name}.` };
  const interp = interpreterOf(name);
  if (interp !== null) {
    const { short, long, sub } = INLINE_CODE[interp]!;
    for (const [i, w] of argv.entries()) {
      if (i === 0) continue;
      if (w === "--") break;
      const inline = long.some((l) => w === l || w.startsWith(`${l}=`))
        || (short !== "" && /^-[A-Za-z]+$/.test(w) && [...w.slice(1)].some((c) => short.includes(c)))
        || (i === 1 && sub?.includes(w) === true);
      if (inline) return { refuse: `A rule can't allow ${name} ${w}: it runs code written on the command line, and Atrium wouldn't check it.` };
    }
    return { warn: `This allows every script ${name} is given${argv.length > 1 ? " after these words" : ""}, whatever the script does.` };
  }
  if (name === "find") {
    const exec = argv.find((w) => FIND_EXEC.has(w));
    if (exec) return { refuse: `A rule can't allow find ${exec}: it runs another command for each file, and Atrium wouldn't check that one.` };
    return { warn: "This also allows find … -exec and -delete, which run commands or delete files." };
  }
  if (REMOTE_RUNNERS.has(name)) return { warn: `This allows whatever ${name} runs (on a host, in a container), whatever follows these words.` };
  if (SCRIPT_RUNNERS.has(name)) return { warn: `This allows every package ${name} runs, whatever it does.` };
  return null;
}

/**
 * Words that start a compound command (`shell.rs` `RESERVED`): a command starting with one is
 * never matched by a rule.
 */
const RESERVED = new Set(["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "function", "select", "{", "}", "!", "[[", "coproc"]);

export type Split = { ok: true; argv: string[] } | { ok: false; error: string };

/**
 * Splits a command line into argv the way the policy's POSIX parser reads a simple command
 * (contract §9): quotes and backslashes are removed, `~` stays as typed. Anything that makes a
 * command not simple (operators, redirects, `$`, backquotes, unquoted globs, brace expansion,
 * a leading NAME=value) is refused: no rule would ever match it.
 */
export function splitCommand(line: string): Split {
  if (line.length > MAX_COMMAND_CHARS) return { ok: false, error: "The command is too long" };
  // The policy's lexer refuses control characters (but tab, which splits words, and the line
  // breaks below) and the Unicode line and paragraph separators.
  if (/[\x00-\x08\x0b\x0c\x0e-\x1f\x7f-\x9f\u2028\u2029]/.test(line)) return { ok: false, error: "Remove the control character: a command with one never matches a rule" };
  const argv: string[] = [];
  let word = "";
  let inWord = false;
  let quoted = false;
  let i = 0;
  const fail = (error: string): Split => ({ ok: false, error });
  const end = () => {
    if (inWord) argv.push(word);
    word = "";
    inWord = false;
    quoted = false;
  };
  while (i < line.length) {
    const c = line[i]!;
    if (c === " " || c === "\t") { end(); i++; continue; }
    if (c === "\n" || c === "\r") return fail("A rule matches one line: remove the line break");
    if (c === "'") {
      const close = line.indexOf("'", i + 1);
      if (close < 0) return fail("A single quote isn't closed");
      word += line.slice(i + 1, close);
      inWord = true;
      quoted = true;
      i = close + 1;
      continue;
    }
    if (c === '"') {
      let j = i + 1;
      let out = "";
      for (;;) {
        if (j >= line.length) return fail("A double quote isn't closed");
        const d = line[j]!;
        if (d === '"') break;
        if (d === "$" || d === "`") return fail("Rules match commands as written: remove $ and backquotes");
        if (d === "\\" && j + 1 < line.length && '"\\$`'.includes(line[j + 1]!)) { out += line[j + 1]; j += 2; continue; }
        out += d;
        j++;
      }
      word += out;
      inWord = true;
      quoted = true;
      i = j + 1;
      continue;
    }
    if (c === "\\") {
      if (i + 1 >= line.length) return fail("A backslash ends the line");
      word += line[i + 1];
      inWord = true;
      quoted = true;
      i += 2;
      continue;
    }
    if (c === "#" && !inWord) return fail("A word starting with # starts a comment: quote the #, or remove the comment");
    if (";&|<>()".includes(c)) return fail("A rule matches one simple command: remove ; & | < > ( )");
    if (c === "$" || c === "`") return fail("Rules match commands as written: remove $ and backquotes");
    if ("*?[".includes(c)) return fail("Quote * ? [ to match them literally: a command with a glob never matches a rule");
    if (c === "{") {
      const close = line.indexOf("}", i);
      const inner = close < 0 ? "" : line.slice(i + 1, close);
      if (inner.includes(",") || inner.includes("..")) return fail("Quote { } to match it literally: brace expansion never matches a rule");
    }
    if (c === "~" && !inWord) {
      const next = line[i + 1];
      if (next !== undefined && next !== "/" && next !== " " && next !== "\t") return fail("~name isn't supported: write the full path");
    }
    if (c === "=" && !quoted && argv.length === 0 && /^[A-Za-z_][A-Za-z0-9_]*$/.test(word)) {
      return fail("Rules match commands without a leading NAME=value");
    }
    word += c;
    inWord = true;
    i++;
  }
  end();
  if (!argv.length) return fail("Type the command");
  if (argv[0] === "") return fail("The command's name is empty");
  if (RESERVED.has(argv[0]!)) return fail(`${argv[0]} starts a compound command: a rule matches a simple command`);
  return { ok: true, argv };
}

/**
 * What `globset` (the engine's glob parser) refuses: a rule with such a glob would never match,
 * even a deny rule. Mirrors `globset` 0.4 `Parser`: `\\` escapes the next character (a trailing one
 * dangles); a class `[…]` takes a leading `!` or `^`, then a leading `]` or `-` literally, and
 * must close with a later `]`; its ranges go up; `{` `}` nest and pair.
 */
export function globSyntaxError(glob: string): string | null {
  const cs = [...glob];
  let depth = 0;
  for (let i = 0; i < cs.length; i++) {
    const c = cs[i]!;
    if (c === "\\") {
      if (i + 1 >= cs.length) return "A \\ ends the glob: remove it, or write \\\\";
      i++;
    } else if (c === "{") depth++;
    else if (c === "}") {
      if (--depth < 0) return "A } has no {";
    } else if (c === "[") {
      let k = i + 1;
      if (cs[k] === "!" || cs[k] === "^") k++;
      let first = true;
      let inRange = false;
      let last: string | null = null;
      for (;;) {
        const d = cs[k++];
        if (d === undefined) return "A [ isn't closed: write [[] for a literal [";
        if (d === "]" && !first) break;
        if (d === "-" && !first && !inRange) inRange = true;
        else if (inRange) {
          if (last !== null && d < last) return `The range ${last}-${d} goes down: write it the other way`;
          inRange = false;
        } else last = d;
        first = false;
      }
      i = k - 1;
    }
  }
  if (depth !== 0) return "A { isn't closed";
  return null;
}

export type Glob = { ok: true; glob: string } | { ok: false; error: string };

/**
 * A path glob as the policy engine matches it: absolute, after symlinks are resolved. `~/…` is
 * written out with the home folder, since the engine doesn't expand it.
 */
export function checkGlob(input: string, home: string | null): Glob {
  const g = input.trim();
  if (!g) return { ok: false, error: "Type a path or a glob, like ~/projects/**" };
  if (g.length > MAX_GLOB_CHARS) return { ok: false, error: "The glob is too long" };
  if (/[\n\r\0]/.test(g)) return { ok: false, error: "A glob is one line" };
  let abs = g;
  if (g === "~" || g.startsWith("~/")) {
    if (!home) return { ok: false, error: "Atrium doesn't know your home folder here: write the full path" };
    abs = home.replace(/\/+$/, "") + g.slice(1);
  } else if (g.startsWith("~")) {
    return { ok: false, error: "~name isn't supported: write the full path" };
  }
  if (!abs.startsWith("/")) return { ok: false, error: "Start with / or ~/: paths are matched as full paths" };
  if (/(^|\/)\.\.?(\/|$)/.test(abs)) return { ok: false, error: "Write the path without . or .. parts" };
  const syntax = globSyntaxError(abs);
  if (syntax) return { ok: false, error: syntax };
  return { ok: true, glob: abs };
}

/**
 * Whether "Only for paths inside" can be set (round 2, blocker 2). The engine checks a path scope
 * against every path of the call, a command's working folder included, and with `all` for deny
 * rules too. So it is offered only on allow rules about file calls: on a command rule it would
 * never match in a Home chat (cwd = ~), and on a deny rule it would let a call through as soon as
 * one of its paths is outside. Escalated in the M1-23 handoff.
 */
export function pathScopeOffered(form: Pick<RuleForm, "decision" | "matcher" | "toolKind">): boolean {
  if (form.decision !== "allow") return false;
  if (form.matcher === "path_glob") return true;
  return form.matcher === "any" && form.toolKind !== "execute" && ANY_KINDS.includes(form.toolKind);
}

/** How much of the disk a glob covers, when it is everything or the whole home folder. */
export function broadGlob(glob: string, home: string | null): "computer" | "home" | null {
  if (/^\/(\*\*(\/\*)?|\*)?\/?$/.test(glob)) return "computer";
  if (home) {
    const h = home.replace(/\/+$/, "");
    if (glob === h || glob === `${h}/` || glob === `${h}/*` || glob === `${h}/**` || glob === `${h}/**/*`) return "home";
  }
  return null;
}
const BROAD_TEXT = { computer: "every path on this computer", home: "every path in your home folder" } as const;

/** Checks the form and builds the draft, with warnings for rules broader than they may look. */
export function validateRule(form: RuleForm, ctx: { home: string | null; threadExists: (id: string) => boolean }): Validation {
  const errors: Partial<Record<Field, string>> = {};
  const warnings: string[] = [];
  let matcher: RuleMatcher | null = null;
  let toolKind: ToolKind = form.toolKind;

  let pathScope: string | null = null;
  const scoped = pathScopeOffered(form) && form.pathScope.trim() !== "";
  if (scoped) {
    const glob = checkGlob(form.pathScope, ctx.home);
    if (!glob.ok) errors.pathScope = glob.error;
    else pathScope = glob.glob;
  }
  const scopeBroad = pathScope === null ? null : broadGlob(pathScope, ctx.home);

  switch (form.matcher) {
    case "command_prefix":
    case "command_exact": {
      toolKind = "execute";
      const split = splitCommand(form.command.trim());
      if (!split.ok) errors.command = split.error;
      else {
        const risk = form.decision === "allow" ? commandRisk(split.argv) : null;
        if (risk && "refuse" in risk) errors.command = risk.refuse;
        else {
          matcher = { type: form.matcher, argv: split.argv };
          if (risk) warnings.push(risk.warn);
        }
      }
      break;
    }
    case "path_glob": {
      if (!PATH_KINDS.includes(toolKind)) toolKind = "edit";
      const glob = checkGlob(form.glob, ctx.home);
      if (!glob.ok) errors.glob = glob.error;
      else {
        matcher = { type: "path_glob", glob: glob.glob };
        const broad = broadGlob(glob.glob, ctx.home);
        if (form.decision === "allow" && broad) warnings.push(`This matches ${BROAD_TEXT[broad]}.`);
      }
      break;
    }
    case "any":
      if (!ANY_KINDS.includes(toolKind)) toolKind = "other";
      matcher = { type: "any" };
      if (form.decision === "allow") {
        const what = toolKind === "execute" ? "every command" : "every call of this kind";
        if (pathScope === null || scopeBroad === "computer") warnings.push(`This allows ${what} without asking.`);
        else if (scopeBroad === "home") warnings.push(`This allows ${what} in your home folder without asking.`);
        else warnings.push(`This allows ${what} inside that folder without asking.`);
      }
      break;
  }
  if (form.matcher !== "any" && scopeBroad) warnings.push(`The path limit covers ${BROAD_TEXT[scopeBroad]}: it doesn't narrow the rule.`);

  const toolName = form.toolName.trim();
  if (toolName && (/\s/.test(toolName) || toolName.length > MAX_TOOL_NAME_CHARS)) errors.toolName = "A tool name is one word, like Bash or Edit";

  let scope: RuleScope;
  if (form.scope === "thread") {
    if (!form.threadId || !ctx.threadExists(form.threadId)) {
      errors.thread = "Pick a chat";
      scope = { type: "global" };
    } else scope = { type: "thread", thread_id: form.threadId };
  } else scope = { type: form.scope };

  if (Object.keys(errors).length || matcher === null) return { ok: false, errors };
  return { ok: true, draft: { scope, tool_kind: toolKind, tool_name: toolName || null, matcher, path_scope: pathScope, decision: form.decision }, warnings };
}
