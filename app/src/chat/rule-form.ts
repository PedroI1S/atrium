// The rules editor in Settings → Approvals (PLAN §3.12, policy rev 4 §7, §11 items 4–5, D-141):
// what the user types, checked before it becomes a `RuleDraft` for `CreateRule`. The daemon
// checks again via `atrium_core::policy::validate_rule_matcher`; this stays in 1-to-1 parity
// with core's frozen D-141 tool table and §7 pinning rules. Pure; tested in rule-form.test.ts.

import type { ListedRule, RuleDraft, RuleMatcher, RuleScope, ToolKind } from "../daemon/types";

export type { ListedRule };

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

export interface StoredRuleValidity {
  valid: boolean;
  reason: string | null;
}

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

// Policy rev 4 §7 & §11 item 4 (D-141 six row kinds): Allow prefix rules that end at a code
// runner in `crates/atrium-core/src/policy/tools.rs` without its required program pin are invalid
// at creation, and stored rules that fail validation stop covering anything (§11 item 5).
// Every tool absent from the frozen table (e.g. `deno`, `bun`, `php`, `lua`, `pwsh`) is an
// unknown tool and is accepted by core, daemon, and app alike.
// Names are CR-19 folded (basename, Unicode lowercase, ſ -> s; U+212A folds to k).

/** Modeled §4 nested shells (`tools.rs` `NestedShell`): pinnable as `[shell, <script>]`. */
const MODELED_SHELLS = new Set(["sh", "bash", "dash"]);
/** Outside-model shells (`tools.rs` `OUTSIDE_MODEL`, `code_runner_no_pin`): unpinnable by prefix. */
const OUTSIDE_SHELLS = new Set(["zsh", "ksh", "fish", "ash", "csh", "tcsh", "mksh", "yash", "rbash", "posh", "elvish", "xonsh"]);
/** Outside-model wrappers (`tools.rs` `OUTSIDE_MODEL`, `code_runner_no_pin`): unpinnable by prefix. */
const OUTSIDE_WRAPPERS = new Set([
  "nohup", "builtin", "busybox", "toybox", "setsid", "unbuffer", "caffeinate", "sandbox-exec",
  "chroot", "ionice", "stdbuf", "chrt", "taskset", "flock", "script", "arch",
]);
/** `tools.rs` `CodeRunner` with `ProgramPin::FirstOperand` (`code_runner_no_pin`). */
const FIRST_OP_RUNNERS = new Set(["perl", "python", "python3", "node", "ruby", "source", ".", "npx"]);
/** `tools.rs` `CodeRunner` with `ProgramPin::FlagValue` `-f`/`--file` (`code_runner_no_pin`). */
const AWK_TOOLS = new Set(["awk", "gawk", "mawk"]);
/** `tools.rs` `CodeRunner` with `program_pin: None` (`eval`, `xargs`, and `COMMAND_RUNNERS`). */
const NO_PIN_RUNNERS = new Set([
  "eval", "xargs",
  "ssh", "mosh", "autossh", "nsenter", "unshare", "docker", "podman", "nerdctl", "kubectl", "oc",
  "lxc", "incus", "machinectl", "systemd-run", "systemd-nspawn", "firejail", "bwrap",
  "flatpak-spawn", "toolbox", "distrobox", "distrobox-enter", "multipass", "vagrant", "limactl",
  "colima", "su", "runuser", "setpriv", "parallel",
]);

/**
 * A command name as the disk resolves it (CR-19): the basename, Unicode lower-cased
 * (which also folds K, U+212A, to k), with ſ (U+017F) folded to s.
 */
export function foldName(word: string): string {
  const b = word.split("/").pop() ?? word;
  return b.toLowerCase().replaceAll("ſ", "s");
}

type ShortOptArity = "bool" | "value" | "opt_attached" | "unknown";

function gitSubcommandShortArity(subcommand: string, ch: string): ShortOptArity {
  switch (subcommand) {
    case "rebase":
      if ("mfiqvnkph".includes(ch)) return "bool";
      if ("sXxC".includes(ch)) return "value";
      if ("Sr".includes(ch)) return "opt_attached";
      return "unknown";
    case "difftool":
      if ("dyghpuswbRz".includes(ch)) return "bool";
      if ("txSGOIl".includes(ch)) return "value";
      if ("UCMBW".includes(ch)) return "opt_attached";
      return "unknown";
    case "mergetool":
      if ("ygh".includes(ch)) return "bool";
      if ("tx".includes(ch)) return "value";
      return "unknown";
    case "diff":
      if ("puswbRzh".includes(ch)) return "bool";
      if ("SGOIl".includes(ch)) return "value";
      if ("UCMBW".includes(ch)) return "opt_attached";
      return "unknown";
    case "push":
      if ("vqfudh".includes(ch) || ch === "n") return "bool";
      if (ch === "o") return "value";
      return "unknown";
    default:
      return "unknown";
  }
}

function gitSubcommandLongTakesSeparateValue(subcommand: string, name: string): boolean {
  switch (subcommand) {
    case "rebase":
      return ["onto", "strategy", "strategy-option", "exec", "empty", "whitespace"].includes(name);
    case "difftool":
      return ["tool", "extcmd", "tool-cmd", "output"].includes(name);
    case "mergetool":
      return ["tool", "tool-cmd"].includes(name);
    case "diff":
      return [
        "output", "diff-filter", "src-prefix", "dst-prefix",
        "line-prefix", "inter-hunk-context", "unified", "word-diff-regex",
      ].includes(name);
    case "push":
      return ["repo", "push-option", "receive-pack", "exec"].includes(name);
    default:
      return false;
  }
}

/**
 * Finds the first triggering flag on a Git subcommand (`rebase`, `difftool`,
 * `mergetool`, `diff`, `push`), respecting attached and separate short/long
 * option values so value bytes (e.g. `-Xcontext`, `-S0x1234`, `-txxdiff`) are
 * never read as option letters. Mirrors `v4/tools.rs::git_subcommand_has_flag_trigger_pure`.
 */
function findGitSubcommandTriggerFlag(
  subcommand: string,
  rest: readonly string[],
  triggers: readonly string[],
): string | null {
  let i = 0;
  while (i < rest.length) {
    const tok = rest[i]!;
    if (tok === "--") break;
    if (!tok.startsWith("-") || tok === "-") {
      i++;
      continue;
    }
    if (tok.startsWith("--")) {
      const eq = tok.indexOf("=");
      const name = eq >= 0 ? tok.slice(2, eq) : tok.slice(2);
      const hasEq = eq >= 0;
      if (triggers.some((f) => f.startsWith("--") && f.slice(2) === name)) {
        return tok;
      }
      i += !hasEq && gitSubcommandLongTakesSeparateValue(subcommand, name) ? 2 : 1;
      continue;
    }
    const cluster = tok.slice(1);
    let consumesNext = false;
    for (let k = 0; k < cluster.length; k++) {
      const ch = cluster[k]!;
      const atEnd = k + 1 >= cluster.length;
      if (triggers.some((f) => f.length === 2 && f.startsWith("-") && !f.startsWith("--") && f[1] === ch)) {
        return tok;
      }
      const arity = gitSubcommandShortArity(subcommand, ch);
      if (arity === "bool") continue;
      if (arity === "value") consumesNext = atEnd;
      break;
    }
    i += consumesNext ? 2 : 1;
  }
  return null;
}

/**
 * Mirrors `v4/tools.rs::has_competing_awk_code_flags_pure`: checks for `-f`,
 * `-e`, `--file`, `--source`, `--eval` while respecting attached and separate
 * values of `-F`, `-v`, `-W`, `--field-separator`, and `--assign`.
 */
function hasCompetingAwkFlags(rest: readonly string[], pinFlags: readonly string[]): boolean {
  const longBad = [...pinFlags.filter((f) => f.startsWith("--")), "--source", "--eval"];
  let i = 0;
  while (i < rest.length) {
    const tok = rest[i]!;
    if (tok === "--") break;
    if (!tok.startsWith("-") || tok === "-") {
      i++;
      continue;
    }
    if (tok.startsWith("--")) {
      const eq = tok.indexOf("=");
      const name = eq >= 0 ? tok.slice(2, eq) : tok.slice(2);
      const hasEq = eq >= 0;
      if (name.length > 0 && longBad.some((f) => f.slice(2).startsWith(name))) {
        return true;
      }
      i += !hasEq && (name === "field-separator" || name === "assign") ? 2 : 1;
      continue;
    }
    const cluster = tok.slice(1);
    let consumesNext = false;
    for (let k = 0; k < cluster.length; k++) {
      const ch = cluster[k]!;
      const atEnd = k + 1 >= cluster.length;
      const isCompeting =
        ch === "e" ||
        pinFlags.some((f) => f.length === 2 && !f.startsWith("--") && f[1] === ch);
      if (isCompeting) return true;
      if ("bcCgkMNnOPrSstV".includes(ch)) continue;
      if (ch === "F" || ch === "v" || ch === "W") {
        consumesNext = atEnd;
      }
      break;
    }
    i += consumesNext ? 2 : 1;
  }
  return false;
}

/**
 * Mirrors `v4/tools.rs::has_competing_sed_flags_pure`: attached `-i.ref` and
 * `-l80` do not scan their suffix as option letters, while bare `-i`, bare `-l`,
 * `-f`, `-e`, `--file`, `--expression`, and unknown flags disable pinning.
 */
function hasCompetingSedFlags(rest: readonly string[]): boolean {
  let i = 0;
  while (i < rest.length) {
    const tok = rest[i]!;
    if (tok === "--") break;
    if (!tok.startsWith("-") || tok === "-") {
      i++;
      continue;
    }
    if (tok.startsWith("--")) {
      const eq = tok.indexOf("=");
      const name = eq >= 0 ? tok.slice(2, eq) : tok.slice(2);
      const hasEq = eq >= 0;
      if (name.length === 0) {
        i++;
        continue;
      }
      if ("file".startsWith(name) || "expression".startsWith(name)) return true;
      if (name === "in-place") {
        if (!hasEq) return true;
        i++;
        continue;
      }
      if (name === "line-length") {
        if (!hasEq && i + 1 >= rest.length) return true;
        i += hasEq ? 1 : 2;
        continue;
      }
      if ([
        "quiet", "silent", "regexp-extended", "separate", "unbuffered",
        "null-data", "posix", "sandbox", "debug", "follow-symlinks",
      ].includes(name)) {
        if (hasEq) return true;
        i++;
        continue;
      }
      return true;
    }
    const cluster = tok.slice(1);
    for (let k = 0; k < cluster.length; k++) {
      const ch = cluster[k]!;
      const atEnd = k + 1 >= cluster.length;
      if (ch === "f" || ch === "e") return true;
      if (ch === "i" || ch === "l") {
        if (atEnd) return true;
        break;
      }
      if ("nErsuz".includes(ch)) continue;
      return true;
    }
    i++;
  }
  return false;
}

/**
 * Linear `sed` script inspector mirroring `crates/atrium-core/src/policy/v4/tools/sed.rs`
 * 1-to-1 (`e`, `w`, `r`, `W`, `R`, `s///e|w|W`, safe commands including `z`, `F`, `l`, `q`, `Q`).
 */
function isSedScriptSafe(script: string): boolean {
  const n = script.length;
  let i = 0;

  const scanRegexToDelim = (delim: string): boolean => {
    let inClass = false;
    let classFirst = false;
    while (i < n) {
      const ch = script[i]!;
      if (inClass) {
        if (ch === "[" && i + 1 < n && (script[i + 1] === ":" || script[i + 1] === "." || script[i + 1] === "=")) {
          const marker = script[i + 1]!;
          i += 2;
          let closed = false;
          while (i + 1 < n) {
            if (script[i] === marker && script[i + 1] === "]") {
              i += 2;
              closed = true;
              break;
            }
            i++;
          }
          if (!closed) return false;
          classFirst = false;
          continue;
        }
        if (ch === "]" && !classFirst) {
          inClass = false;
          i++;
          continue;
        }
        if (ch === "^" && classFirst) {
          i++;
          continue;
        }
        classFirst = false;
        i++;
        continue;
      }
      if (ch === "\\") {
        if (i + 1 >= n) return false;
        i += 2;
        continue;
      }
      if (ch === delim) {
        i++;
        return true;
      }
      if (ch === "\n") return false;
      if (ch === "[") {
        inClass = true;
        classFirst = true;
        i++;
        continue;
      }
      i++;
    }
    return false;
  };

  const scanReplacementToDelim = (delim: string): boolean => {
    while (i < n) {
      const ch = script[i]!;
      if (ch === "\\") {
        if (i + 1 >= n) return false;
        i += 2;
        continue;
      }
      if (ch === delim) {
        i++;
        return true;
      }
      if (ch === "\n") return false;
      i++;
    }
    return false;
  };

  const skipAddr = (): "yes" | "no" | "bad" => {
    if (i >= n) return "no";
    const ch = script[i]!;
    if (ch === "$") { i++; return "yes"; }
    if (ch >= "0" && ch <= "9") {
      while (i < n && script[i]! >= "0" && script[i]! <= "9") i++;
      if (i < n && (script[i] === "~" || script[i] === "+")) {
        i++;
        const start = i;
        while (i < n && script[i]! >= "0" && script[i]! <= "9") i++;
        if (i === start) return "bad";
      }
      return "yes";
    }
    if (ch === "+" || ch === "~") {
      i++;
      const start = i;
      while (i < n && script[i]! >= "0" && script[i]! <= "9") i++;
      return i === start ? "bad" : "yes";
    }
    if (ch === "/") {
      i++;
      if (!scanRegexToDelim("/")) return "bad";
      while (i < n && (script[i] === "I" || script[i] === "M")) i++;
      return "yes";
    }
    if (ch === "\\") {
      if (i + 1 >= n) return "bad";
      const d = script[i + 1]!;
      if (d === "\n" || d === "\\") return "bad";
      i += 2;
      if (!scanRegexToDelim(d)) return "bad";
      while (i < n && (script[i] === "I" || script[i] === "M")) i++;
      return "yes";
    }
    return "no";
  };

  let blockDepth = 0;
  while (i < n) {
    while (i < n && (script[i] === " " || script[i] === "\t" || script[i] === "\r" || script[i] === "\n" || script[i] === ";")) i++;
    if (i >= n) break;
    if (script[i] === "#") {
      while (i < n && script[i] !== "\n") i++;
      continue;
    }
    if (script[i] === "}") {
      if (blockDepth === 0) return false;
      blockDepth--;
      i++;
      continue;
    }
    const a1 = skipAddr();
    if (a1 === "bad") return false;
    if (a1 === "yes") {
      while (i < n && (script[i] === " " || script[i] === "\t")) i++;
      if (i < n && script[i] === ",") {
        i++;
        while (i < n && (script[i] === " " || script[i] === "\t")) i++;
        if (skipAddr() !== "yes") return false;
      }
    }
    while (i < n && (script[i] === " " || script[i] === "\t" || script[i] === "!")) i++;
    if (i >= n) return false;
    const cmd = script[i]!;
    i++;
    if (cmd === "{") { blockDepth++; continue; }
    if (cmd === "e" || cmd === "w" || cmd === "r" || cmd === "W" || cmd === "R") return false;
    if (cmd === "s") {
      if (i >= n) return false;
      const delim = script[i]!;
      if (delim === "\n" || delim === "\\") return false;
      i++;
      if (!scanRegexToDelim(delim)) return false;
      if (!scanReplacementToDelim(delim)) return false;
      while (i < n) {
        const f = script[i]!;
        if (f === ";" || f === "\n" || f === "}" || f === "#") break;
        if (f === " " || f === "\t" || f === "\r") { i++; continue; }
        if (f === "e" || f === "w" || f === "W") return false;
        if ("gpIiMm0123456789".includes(f)) { i++; continue; }
        return false;
      }
      continue;
    }
    if (cmd === "y") {
      if (i >= n) return false;
      const delim = script[i]!;
      if (delim === "\n" || delim === "\\") return false;
      i++;
      if (!scanReplacementToDelim(delim)) return false;
      if (!scanReplacementToDelim(delim)) return false;
      continue;
    }
    if ("dDgGhHnNpPx=zF".includes(cmd)) {
      while (i < n && (script[i] === " " || script[i] === "\t" || script[i] === "\r")) i++;
      if (i < n && script[i] !== ";" && script[i] !== "\n" && script[i] !== "}" && script[i] !== "#") return false;
      continue;
    }
    if (cmd === "q" || cmd === "Q" || cmd === "l") {
      while (i < n && (script[i] === " " || script[i] === "\t" || script[i] === "\r")) i++;
      while (i < n && script[i]! >= "0" && script[i]! <= "9") i++;
      while (i < n && (script[i] === " " || script[i] === "\t" || script[i] === "\r")) i++;
      if (i < n && script[i] !== ";" && script[i] !== "\n" && script[i] !== "}" && script[i] !== "#") return false;
      continue;
    }
    if (cmd === ":" || cmd === "b" || cmd === "t" || cmd === "T") {
      while (i < n && script[i] !== ";" && script[i] !== "\n" && script[i] !== "}" && script[i] !== "#") i++;
      continue;
    }
    if (cmd === "a" || cmd === "i" || cmd === "c") {
      while (i < n && (script[i] === " " || script[i] === "\t")) i++;
      if (i < n && script[i] === "\\") {
        i++;
        if (i < n && script[i] === "\n") i++;
      }
      while (i < n) {
        if (script[i] === "\\" && i + 1 < n) { i += 2; continue; }
        if (script[i] === "\n") { i++; break; }
        i++;
      }
      continue;
    }
    return false;
  }
  return blockDepth === 0;
}

function isSedPrefixSafe(args: readonly string[]): boolean {
  const scripts: string[] = [];
  let firstPos: string | null = null;
  let i = 0;
  let dd = false;
  while (i < args.length) {
    const tok = args[i]!;
    if (dd || !tok.startsWith("-") || tok === "-") {
      if (firstPos === null) firstPos = tok;
      i++;
      continue;
    }
    if (tok === "--") { dd = true; i++; continue; }
    if (tok.startsWith("--")) {
      const eq = tok.indexOf("=");
      const name = eq >= 0 ? tok.slice(2, eq) : tok.slice(2);
      const val = eq >= 0 ? tok.slice(eq + 1) : null;
      if (name === "expression") {
        if (val !== null) { scripts.push(val); i++; }
        else if (i + 1 < args.length) { scripts.push(args[i + 1]!); i += 2; }
        else return false;
      } else if (["quiet", "silent", "regexp-extended", "separate", "unbuffered", "null-data", "posix", "sandbox"].includes(name)) {
        if (val !== null) return false;
        i++;
      } else {
        return false;
      }
    } else {
      let consumedNext = false;
      for (let k = 1; k < tok.length; k++) {
        const ch = tok[k]!;
        if (ch === "e") {
          if (k + 1 < tok.length) scripts.push(tok.slice(k + 1));
          else if (i + 1 < args.length) { scripts.push(args[i + 1]!); consumedNext = true; }
          else return false;
          break;
        }
        if (!"nErsuz".includes(ch)) return false;
      }
      i += consumedNext ? 2 : 1;
    }
  }
  if (scripts.length === 0) {
    if (firstPos === null) return false;
    scripts.push(firstPos);
  }
  return scripts.every(isSedScriptSafe);
}

export type CommandRisk = { refuse: string } | { warn: string } | null;

/**
 * Checks an allow rule's argv against rev 4 §7's code-runner pinning rules and the frozen D-141
 * tool table (1-to-1 with `atrium_core::policy::validate_rule_matcher`).
 *
 * Refuses ONLY Allow `command_prefix` rules that core rejects; `command_exact` and Deny rules
 * are never refused here (pinned programs and exact code runners carry a warning when allowed).
 */
export function commandRisk(argv: readonly string[], matcher: "command_prefix" | "command_exact" = "command_prefix"): CommandRisk {
  const first = argv[0];
  if (first === undefined) return null;
  const name = foldName(first);
  const args = argv.slice(1);
  const prefix = matcher === "command_prefix";

  // 1. Outside-model shells (`zsh`..`xonsh`) and outside-model wrappers (`nohup`..`arch`).
  if (OUTSIDE_SHELLS.has(name)) {
    if (prefix) {
      return { refuse: `An Allow prefix rule can't allow ${name}: shells outside the model cannot be allowed by prefix.` };
    }
    return { warn: `This allows ${name}, which runs outside Atrium's command model.` };
  }
  if (OUTSIDE_WRAPPERS.has(name)) {
    if (prefix) {
      return { refuse: `An Allow prefix rule can't allow ${name}: wrappers outside the model cannot be allowed by prefix.` };
    }
    return { warn: `This allows ${name}, which runs outside Atrium's command model.` };
  }

  // 2. Modeled nested shells (`sh`, `bash`, `dash`): Allow prefix pins `[shell, <script>]`.
  if (MODELED_SHELLS.has(name)) {
    if (prefix) {
      if (args.length >= 1 && args[0] !== "" && !args[0]!.startsWith("-")) {
        return { warn: `This allows the ${name} script ${args[0]}, with whatever arguments follow, whatever the script does.` };
      }
      return { refuse: `An Allow prefix rule can't allow ${name}${args.length ? ` ${args[0]}` : ""}: pin a script file right after it (e.g. ${name} script.sh).` };
    }
    return { warn: `This allows the ${name} command it names, whatever it runs.` };
  }

  // 3. First-operand code runners (`python`, `python3`, `perl`, `node`, `ruby`, `source`, `.`, `npx`).
  if (FIRST_OP_RUNNERS.has(name)) {
    if (prefix) {
      if (args.length >= 1 && args[0] !== "" && !args[0]!.startsWith("-")) {
        return { warn: `This allows the ${name} script or package ${args[0]} it names, with whatever arguments follow, whatever it does.` };
      }
      return { refuse: `An Allow prefix rule can't end at ${name}${args.length ? ` ${args[0]}` : ""}: name the script or package right after it (e.g. python3 scripts/build.py), with no options before it.` };
    }
    return { warn: `This allows the ${name} program it names, whatever it does.` };
  }

  // 4. `awk` / `gawk` / `mawk` (§7, §10 row 13, §11 item 4): Allow prefix pins `-f <file>` / `--file <file>`.
  if (AWK_TOOLS.has(name)) {
    if (prefix) {
      if (
        args.length >= 2 &&
        (args[0] === "-f" || args[0] === "--file") &&
        args[1] !== "" &&
        args[1] !== "-" &&
        !hasCompetingAwkFlags(args.slice(2), ["-f", "--file"])
      ) {
        return { warn: `This allows the ${name} program ${args[1]}, whatever it does.` };
      }
      return { refuse: `An Allow prefix rule can't end at ${name} without pinning a program file with -f <file> or --file <file>.` };
    }
    return { warn: `This allows the ${name} program it names, whatever it does.` };
  }

  // 5. `cargo` (§7): Allow prefix pins a subcommand (e.g. `cargo test`).
  if (name === "cargo") {
    if (prefix && (args.length === 0 || args[0] === "" || args[0]!.startsWith("-"))) {
      return { refuse: "An Allow prefix rule can't end at cargo without pinning a subcommand (e.g. cargo test)." };
    }
    return null;
  }

  // 6. `pipx` (§7, §11 item 4): Allow prefix pins `pipx run <package>`.
  if (name === "pipx") {
    if (prefix) {
      if (args.length >= 2 && args[0] === "run" && args[1] !== "" && !args[1]!.startsWith("-")) {
        return { warn: `This allows the package pipx run ${args[1]} runs, whatever it does.` };
      }
      return { refuse: "An Allow prefix rule on pipx must pin a package after pipx run (e.g. pipx run ruff)." };
    }
    return { warn: "This allows pipx to run the command it names." };
  }

  // 7. No-pin code runners (`eval`, `xargs`, `COMMAND_RUNNERS`: `ssh`, `docker`, `kubectl`, `parallel`, ...).
  if (NO_PIN_RUNNERS.has(name)) {
    if (prefix) {
      return { refuse: `An Allow prefix rule can't end at ${name}: it has no pinnable prefix form. Use “This exact command” instead.` };
    }
    return { warn: `This allows whatever ${name} runs.` };
  }

  // 8. Conditional runner `find` (§7): Allow prefix is always refused (could append `-exec`).
  if (name === "find") {
    if (prefix) {
      return { refuse: "An Allow prefix rule can't end at find (it would allow appending -exec): use “This exact command” instead." };
    }
    if (args.some((w) => w.toLowerCase() === "-delete")) {
      return { warn: "This allows find to delete what it matches." };
    }
    return null;
  }

  // 9. Conditional runner `git` (§7, §11 item 4): Allow prefix pins a non-code-running subcommand.
  if (name === "git") {
    if (prefix) {
      if (args.length === 0 || args[0] === "" || args[0]!.startsWith("-")) {
        return { refuse: "An Allow prefix rule can't end at git without pinning a non-code-running subcommand (e.g. git status)." };
      }
      const sub = args[0]!;
      const rest = args.slice(1);
      if (sub === "merge-index" || sub === "filter-branch") {
        return { refuse: `An Allow prefix rule can't allow git ${sub}: it runs external programs.` };
      }
      if (sub === "bisect" || sub === "submodule") {
        const triggerSub = sub === "bisect" ? "run" : "foreach";
        const ddIdx = rest.indexOf("--");
        const beforeDd = ddIdx >= 0 ? rest.slice(0, ddIdx) : rest;
        const firstNonFlag = beforeDd.find((a) => !a.startsWith("-"));
        if (firstNonFlag === undefined || firstNonFlag === triggerSub) {
          return { refuse: `An Allow prefix rule on git ${sub} must pin a non-code-running action (it would cover git ${sub} ${triggerSub}).` };
        }
      }
      const subFlagTriggers: Record<string, readonly string[]> = {
        rebase: ["-x", "--exec"],
        difftool: ["-x", "--tool-cmd"],
        mergetool: ["-x", "--tool-cmd"],
        diff: ["--ext-diff"],
        push: ["--receive-pack", "--exec"],
      };
      const flags = subFlagTriggers[sub];
      if (flags) {
        const hit = findGitSubcommandTriggerFlag(sub, rest, flags);
        if (hit !== null) {
          return { refuse: `An Allow prefix rule can't allow git ${sub} with ${hit}: it runs an external command.` };
        }
      }
    }
    return null;
  }

  // 10. Conditional runner `sed` (§7): Allow prefix pins `-f <file>` / `--file <file>` or a safe inline script.
  if (name === "sed") {
    if (prefix) {
      if (args.length === 0) {
        return { refuse: "An Allow prefix rule can't end at sed without pinning a program file (sed -f script.sed) or a safe inline script." };
      }
      if (args[0] === "-f" || args[0] === "--file") {
        if (
          args.length >= 2 &&
          args[1] !== "" &&
          args[1] !== "-" &&
          !hasCompetingSedFlags(args.slice(2))
        ) {
          return { warn: `This allows the sed script ${args[1]}, whatever it does.` };
        }
        return { refuse: "An Allow prefix rule on sed -f must pin a program file (e.g. sed -f edit.sed)." };
      }
      if (!isSedPrefixSafe(args)) {
        return { refuse: "An Allow prefix rule on sed can't use e, w, r, W, or R commands or unmodeled flags: pin a safe inline script or use sed -f <file>." };
      }
      return null;
    }
    return null;
  }

  // Every other tool (HomeReadOnly, Search, §4 Wrapper, DerivedDestinationWriter,
  // UNKNOWN_BY_DESIGN, and unknown tools such as `deno`, `bun`, `php`, `lua`, `pwsh`)
  // is accepted by `atrium_core::policy::validate_rule_matcher`.
  return null;
}

/**
 * Checks whether an existing stored rule is valid under rev 4 §7 (§11 item 5).
 * Stored rules that fail validation stay visible in the rules list, marked invalid,
 * and stop covering anything in the evaluator.
 */
export function checkStoredRule(rule: ListedRule): StoredRuleValidity {
  if (typeof rule.invalid_reason === "string" && rule.invalid_reason.length > 0) {
    return { valid: false, reason: rule.invalid_reason };
  }
  if (rule.path_scope !== null && rule.path_scope !== undefined) {
    if (rule.path_scope === "") {
      return { valid: false, reason: "The rule's path limit is empty." };
    }
    const scopeErr = globSyntaxError(rule.path_scope);
    if (scopeErr) return { valid: false, reason: scopeErr };
  }
  switch (rule.matcher.type) {
    case "any":
      return { valid: true, reason: null };
    case "path_glob": {
      if (!rule.matcher.glob) return { valid: false, reason: "The rule's path glob is empty." };
      const err = globSyntaxError(rule.matcher.glob);
      return err ? { valid: false, reason: err } : { valid: true, reason: null };
    }
    case "command_exact": {
      if (rule.matcher.argv.length === 0 || rule.matcher.argv[0] === "") {
        return { valid: false, reason: "The rule's command is empty." };
      }
      return { valid: true, reason: null };
    }
    case "command_prefix": {
      if (rule.matcher.argv.length === 0 || rule.matcher.argv[0] === "") {
        return { valid: false, reason: "The rule's command is empty." };
      }
      if (rule.decision === "deny") return { valid: true, reason: null };
      const risk = commandRisk(rule.matcher.argv, "command_prefix");
      if (risk && "refuse" in risk) return { valid: false, reason: risk.refuse };
      return { valid: true, reason: null };
    }
  }
}

/** Annotates every stored rule with its rev 4 §7 invalid reason (`null` when valid). */
export function markStoredRules(rules: readonly ListedRule[]): Array<ListedRule & { invalid_reason: string | null }> {
  return rules.map((rule) => {
    const v = checkStoredRule(rule);
    return { ...rule, invalid_reason: v.valid ? null : v.reason };
  });
}

/**
 * Words that start a compound command: a command starting with one is never matched by a rule.
 */
const RESERVED = new Set(["if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac", "function", "select", "{", "}", "!", "[[", "coproc"]);

export type Split = { ok: true; argv: string[] } | { ok: false; error: string };

/**
 * Whether the `{` at `at` starts a brace expansion: a `,` or `..` at depth 1 before its `}`,
 * counting nested braces, stopping at a blank or an operator; too far to tell (256 chars) counts.
 */
export function expandsBraces(line: string, at: number): boolean {
  let depth = 1;
  let has = false;
  for (let j = at + 1; j < line.length && depth > 0; j++) {
    if (j - at > 256) return true;
    const c = line[j]!;
    if (c === "{") depth++;
    else if (c === "}") depth--;
    else if (c === "," && depth === 1) has = true;
    else if (c === "." && depth === 1 && line[j + 1] === ".") has = true;
    else if (" \t\n;&|".includes(c)) break;
  }
  return depth === 0 && has;
}

export function splitCommand(line: string): Split {
  if (line.length > MAX_COMMAND_CHARS) return { ok: false, error: "The command is too long" };
  if (/[\n\r]/.test(line)) return { ok: false, error: "A rule matches one line: remove the line break" };
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
    if (c === "{" && expandsBraces(line, i)) return fail("Quote { } to match it literally: brace expansion never matches a rule");
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
 * What `globset` (the engine's glob parser) refuses: mirrors `globset` 0.4.21 `Parser`.
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
 * Whether "Only for paths inside" can be set. Offered only on allow rules about file calls.
 */
export function pathScopeOffered(form: Pick<RuleForm, "decision" | "matcher" | "toolKind">): boolean {
  if (form.decision !== "allow") return false;
  return form.matcher === "path_glob" || (form.matcher === "any" && PATH_KINDS.includes(form.toolKind));
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
        const risk = form.decision === "allow" ? commandRisk(split.argv, form.matcher) : null;
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
