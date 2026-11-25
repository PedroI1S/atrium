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

/**
 * Matches a Git flag trigger (`-x`, `--exec`, `--tool-cmd`, `--ext-diff`,
 * `--receive-pack`) against one token without modelling option arities (D-156).
 * Mirrors `v4/tools.rs::matches_git_flag_pure` 1-to-1.
 */
function matchesGitFlag(tok: string, trigger: string): boolean {
  if (trigger.startsWith("--")) {
    if (!tok.startsWith("--")) return false;
    const eq = tok.indexOf("=");
    const name = eq >= 0 ? tok.slice(2, eq) : tok.slice(2);
    const long = trigger.slice(2);
    if (name === long) return true;
    return name.length >= 2 && long.startsWith(name) && !(long === "tool-cmd" && name === "tool");
  }
  if (trigger.length === 2 && trigger.startsWith("-") && tok.startsWith("-") && !tok.startsWith("--") && tok.length > 1) {
    return tok.slice(1).includes(trigger[1]!);
  }
  return tok === trigger;
}

/**
 * Finds the first triggering flag on a Git subcommand (`rebase`, `difftool`,
 * `mergetool`, `diff`, `push`) without consuming any word as an option's value
 * (D-156). Honors `--` only when no preceding flag could have consumed `--`.
 * Mirrors `v4/tools.rs::git_subcommand_has_flag_trigger_pure`.
 */
function findGitSubcommandTriggerFlag(
  rest: readonly string[],
  triggers: readonly string[],
): string | null {
  let prevMayConsumeNext = false;
  for (const tok of rest) {
    if (tok === "--") {
      if (!prevMayConsumeNext) break;
      prevMayConsumeNext = false;
      continue;
    }
    if (!tok.startsWith("-") || tok === "-") {
      prevMayConsumeNext = false;
      continue;
    }
    if (triggers.some((trigger) => matchesGitFlag(tok, trigger))) {
      return tok;
    }
    prevMayConsumeNext = !(tok.startsWith("--") && tok.includes("="));
  }
  return null;
}

/**
 * Checks whether `rest` contains `subarg` before an unconsumed `--` without
 * consuming any word as an option's value (D-156). Mirrors
 * `v4/tools.rs::git_subcommand_has_subarg_trigger_pure`.
 */
function hasGitSubcommandSubargTrigger(rest: readonly string[], subarg: string): boolean {
  let prevMayConsumeNext = false;
  for (const tok of rest) {
    if (tok === "--") {
      if (!prevMayConsumeNext) break;
      prevMayConsumeNext = false;
      continue;
    }
    if (tok.startsWith("-") && tok !== "-") {
      prevMayConsumeNext = !(tok.startsWith("--") && tok.includes("="));
      continue;
    }
    prevMayConsumeNext = false;
    if (tok === subarg) return true;
  }
  return false;
}

const AWK_EXTRA_CODE_LONG_FLAGS = ["source", "eval", "exec", "include", "load"] as const;
const AWK_EXTRA_CODE_SHORT_FLAGS = new Set(["e", "E", "i", "l", "W"]);

/**
 * Mirrors `v4/tools.rs::has_competing_awk_code_flags_pure`: checks for any form
 * that could supply code (`-f`, `-e`, `-E`, `-i`, `-l`, `-W…`, `--file`,
 * `--source`, `--eval`, `--exec`, `--include`, `--load`) without consuming any
 * word as an option's value (D-156).
 */
function hasCompetingAwkFlags(rest: readonly string[], pinFlags: readonly string[]): boolean {
  const longBad = [...pinFlags.filter((f) => f.startsWith("--")).map((f) => f.slice(2)), ...AWK_EXTRA_CODE_LONG_FLAGS];
  const shortBad = new Set([
    ...AWK_EXTRA_CODE_SHORT_FLAGS,
    ...pinFlags.filter((f) => f.length === 2 && f.startsWith("-") && !f.startsWith("--")).map((f) => f[1]!),
  ]);
  let prevMayConsumeNext = false;
  for (const tok of rest) {
    if (tok === "--") {
      if (!prevMayConsumeNext) break;
      prevMayConsumeNext = false;
      continue;
    }
    if (!tok.startsWith("-") || tok === "-") {
      prevMayConsumeNext = false;
      continue;
    }
    if (tok.startsWith("--")) {
      const eq = tok.indexOf("=");
      const name = eq >= 0 ? tok.slice(2, eq) : tok.slice(2);
      if (name.length > 0 && longBad.some((l) => l.startsWith(name))) {
        return true;
      }
      prevMayConsumeNext = eq < 0;
      continue;
    }
    for (const ch of tok.slice(1)) {
      if (shortBad.has(ch)) return true;
    }
    prevMayConsumeNext = true;
  }
  return false;
}

const SED_SAFE_BOOL_LONG = new Set([
  "quiet", "silent", "regexp-extended", "separate", "unbuffered",
  "null-data", "posix", "sandbox",
]);
const SED_SAFE_BOOL_SHORT = new Set(["n", "E", "r", "s", "u", "z"]);

/**
 * Mirrors `v4/tools.rs::has_competing_sed_flags_pure`: only pure zero-arg
 * boolean flags are accepted after `sed -f PROG`; never consumes a word as an
 * option's value and rejects `-f`, `-e`, `-i`, `-i.ref`, `-l`, `-l80`,
 * `--in-place`, `--line-length`, and unknown flags (D-156).
 */
function hasCompetingSedFlags(rest: readonly string[]): boolean {
  for (const tok of rest) {
    if (tok === "--") break;
    if (!tok.startsWith("-") || tok === "-") continue;
    if (tok.startsWith("--")) {
      const eq = tok.indexOf("=");
      const name = eq >= 0 ? tok.slice(2, eq) : tok.slice(2);
      if (eq >= 0 || name.length === 0 || !SED_SAFE_BOOL_LONG.has(name)) {
        return true;
      }
      continue;
    }
    for (const ch of tok.slice(1)) {
      if (!SED_SAFE_BOOL_SHORT.has(ch)) return true;
    }
  }
  return false;
}

const UTF8_ENCODER = new TextEncoder();

const asciiByte = (ch: string): number => ch.charCodeAt(0);
const byteSet = (chars: string): ReadonlySet<number> => new Set([...chars].map((ch) => ch.charCodeAt(0)));

const B_SPACE = asciiByte(" ");
const B_TAB = asciiByte("\t");
const B_CR = asciiByte("\r");
const B_LF = asciiByte("\n");
const B_SEMI = asciiByte(";");
const B_HASH = asciiByte("#");
const B_LBRACE = asciiByte("{");
const B_RBRACE = asciiByte("}");
const B_COMMA = asciiByte(",");
const B_BANG = asciiByte("!");
const B_DOLLAR = asciiByte("$");
const B_0 = asciiByte("0");
const B_9 = asciiByte("9");
const B_PLUS = asciiByte("+");
const B_TILDE = asciiByte("~");
const B_SLASH = asciiByte("/");
const B_BSLASH = asciiByte("\\");
const B_LBRACK = asciiByte("[");
const B_RBRACK = asciiByte("]");
const B_COLON = asciiByte(":");
const B_DOT = asciiByte(".");
const B_EQ = asciiByte("=");
const B_CARET = asciiByte("^");
const B_I = asciiByte("I");
const B_M = asciiByte("M");
const B_s = asciiByte("s");
const B_y = asciiByte("y");
const B_q = asciiByte("q");
const B_Q = asciiByte("Q");
const B_l = asciiByte("l");
const B_b = asciiByte("b");
const B_t = asciiByte("t");
const B_T = asciiByte("T");
const B_a = asciiByte("a");
const B_i = asciiByte("i");
const B_c = asciiByte("c");

const SED_TRIGGER_CMD_BYTES = byteSet("ewrWR");
const SED_SUBST_TRIGGER_FLAG_BYTES = byteSet("ewrW");
const SED_SUBST_SAFE_FLAG_BYTES = byteSet("gpIiMm0123456789");
const SED_ZERO_ARG_CMD_BYTES = byteSet("dDgGhHnNpPx=zF");

/**
 * Linear UTF-8 byte-level `sed` script inspector mirroring
 * `crates/atrium-core/src/policy/v4/tools/sed.rs::inspect_sed_script` 1-to-1
 * (`e`, `w`, `r`, `W`, `R`, `s///e|w|W`, safe commands including `z`, `F`, `l`, `q`, `Q`).
 *
 * Scans UTF-8 bytes (`TextEncoder`) exactly like core so non-ASCII `s`/`y`/`\`
 * delimiters fail closed to `false` while non-ASCII bytes inside regex/replacement
 * text under an ASCII delimiter (`s/é/x/`) stay accepted.
 */
function isSedScriptSafe(script: string): boolean {
  const bytes = UTF8_ENCODER.encode(script);
  const n = bytes.length;
  let i = 0;

  const isAsciiDigit = (byte: number): boolean => byte >= B_0 && byte <= B_9;

  const scanRegexToDelim = (delim: number): boolean => {
    let inClass = false;
    let classFirst = false;
    while (i < n) {
      const byte = bytes[i]!;
      if (inClass) {
        if (
          byte === B_LBRACK &&
          i + 1 < n &&
          (bytes[i + 1] === B_COLON || bytes[i + 1] === B_DOT || bytes[i + 1] === B_EQ)
        ) {
          const marker = bytes[i + 1]!;
          i += 2;
          let closed = false;
          while (i + 1 < n) {
            if (bytes[i] === marker && bytes[i + 1] === B_RBRACK) {
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
        if (byte === B_RBRACK && !classFirst) {
          inClass = false;
          i++;
          continue;
        }
        if (byte === B_CARET && classFirst) {
          i++;
          continue;
        }
        classFirst = false;
        i++;
        continue;
      }
      if (byte === B_BSLASH) {
        if (i + 1 >= n) return false;
        i += 2;
        continue;
      }
      if (byte === delim) {
        i++;
        return true;
      }
      if (byte === B_LF) return false;
      if (byte === B_LBRACK) {
        inClass = true;
        classFirst = true;
        i++;
        continue;
      }
      i++;
    }
    return false;
  };

  const scanReplacementToDelim = (delim: number): boolean => {
    while (i < n) {
      const byte = bytes[i]!;
      if (byte === B_BSLASH) {
        if (i + 1 >= n) return false;
        i += 2;
        continue;
      }
      if (byte === delim) {
        i++;
        return true;
      }
      if (byte === B_LF) return false;
      i++;
    }
    return false;
  };

  const skipAddr = (): "yes" | "no" | "bad" => {
    if (i >= n) return "no";
    const byte = bytes[i]!;
    if (byte === B_DOLLAR) { i++; return "yes"; }
    if (isAsciiDigit(byte)) {
      while (i < n && isAsciiDigit(bytes[i]!)) i++;
      if (i < n && (bytes[i] === B_TILDE || bytes[i] === B_PLUS)) {
        i++;
        const start = i;
        while (i < n && isAsciiDigit(bytes[i]!)) i++;
        if (i === start) return "bad";
      }
      return "yes";
    }
    if (byte === B_PLUS || byte === B_TILDE) {
      i++;
      const start = i;
      while (i < n && isAsciiDigit(bytes[i]!)) i++;
      return i === start ? "bad" : "yes";
    }
    if (byte === B_SLASH) {
      i++;
      if (!scanRegexToDelim(B_SLASH)) return "bad";
      while (i < n && (bytes[i] === B_I || bytes[i] === B_M)) i++;
      return "yes";
    }
    if (byte === B_BSLASH) {
      if (i + 1 >= n) return "bad";
      const d = bytes[i + 1]!;
      if (d === B_LF || d === B_BSLASH) return "bad";
      i += 2;
      if (!scanRegexToDelim(d)) return "bad";
      while (i < n && (bytes[i] === B_I || bytes[i] === B_M)) i++;
      return "yes";
    }
    return "no";
  };

  let blockDepth = 0;
  while (i < n) {
    while (
      i < n &&
      (bytes[i] === B_SPACE || bytes[i] === B_TAB || bytes[i] === B_CR || bytes[i] === B_LF || bytes[i] === B_SEMI)
    ) {
      i++;
    }
    if (i >= n) break;
    if (bytes[i] === B_HASH) {
      while (i < n && bytes[i] !== B_LF) i++;
      continue;
    }
    if (bytes[i] === B_RBRACE) {
      if (blockDepth === 0) return false;
      blockDepth--;
      i++;
      continue;
    }
    const a1 = skipAddr();
    if (a1 === "bad") return false;
    if (a1 === "yes") {
      while (i < n && (bytes[i] === B_SPACE || bytes[i] === B_TAB)) i++;
      if (i < n && bytes[i] === B_COMMA) {
        i++;
        while (i < n && (bytes[i] === B_SPACE || bytes[i] === B_TAB)) i++;
        if (skipAddr() !== "yes") return false;
      }
    }
    while (i < n && (bytes[i] === B_SPACE || bytes[i] === B_TAB || bytes[i] === B_BANG)) i++;
    if (i >= n) return false;
    const cmd = bytes[i]!;
    i++;
    if (cmd === B_LBRACE) { blockDepth++; continue; }
    if (SED_TRIGGER_CMD_BYTES.has(cmd)) return false;
    if (cmd === B_s) {
      if (i >= n) return false;
      const delim = bytes[i]!;
      if (delim === B_LF || delim === B_BSLASH) return false;
      i++;
      if (!scanRegexToDelim(delim)) return false;
      if (!scanReplacementToDelim(delim)) return false;
      while (i < n) {
        const f = bytes[i]!;
        if (f === B_SEMI || f === B_LF || f === B_RBRACE || f === B_HASH) break;
        if (f === B_SPACE || f === B_TAB || f === B_CR) { i++; continue; }
        if (SED_SUBST_TRIGGER_FLAG_BYTES.has(f)) return false;
        if (SED_SUBST_SAFE_FLAG_BYTES.has(f)) { i++; continue; }
        return false;
      }
      continue;
    }
    if (cmd === B_y) {
      if (i >= n) return false;
      const delim = bytes[i]!;
      if (delim === B_LF || delim === B_BSLASH) return false;
      i++;
      if (!scanReplacementToDelim(delim)) return false;
      if (!scanReplacementToDelim(delim)) return false;
      continue;
    }
    if (SED_ZERO_ARG_CMD_BYTES.has(cmd)) {
      while (i < n && (bytes[i] === B_SPACE || bytes[i] === B_TAB || bytes[i] === B_CR)) i++;
      if (i < n && bytes[i] !== B_SEMI && bytes[i] !== B_LF && bytes[i] !== B_RBRACE && bytes[i] !== B_HASH) return false;
      continue;
    }
    if (cmd === B_q || cmd === B_Q || cmd === B_l) {
      while (i < n && (bytes[i] === B_SPACE || bytes[i] === B_TAB || bytes[i] === B_CR)) i++;
      while (i < n && isAsciiDigit(bytes[i]!)) i++;
      while (i < n && (bytes[i] === B_SPACE || bytes[i] === B_TAB || bytes[i] === B_CR)) i++;
      if (i < n && bytes[i] !== B_SEMI && bytes[i] !== B_LF && bytes[i] !== B_RBRACE && bytes[i] !== B_HASH) return false;
      continue;
    }
    if (cmd === B_COLON || cmd === B_b || cmd === B_t || cmd === B_T) {
      while (i < n && bytes[i] !== B_SEMI && bytes[i] !== B_LF && bytes[i] !== B_RBRACE && bytes[i] !== B_HASH) i++;
      continue;
    }
    if (cmd === B_a || cmd === B_i || cmd === B_c) {
      while (i < n && (bytes[i] === B_SPACE || bytes[i] === B_TAB)) i++;
      if (i < n && bytes[i] === B_BSLASH) {
        i++;
        if (i < n && bytes[i] === B_LF) i++;
      }
      while (i < n) {
        if (bytes[i] === B_BSLASH && i + 1 < n) { i += 2; continue; }
        if (bytes[i] === B_LF) { i++; break; }
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
        if (val !== null) scripts.push(val);
        else if (i + 1 < args.length) scripts.push(args[i + 1]!);
        else return false;
      } else if (val !== null || !SED_SAFE_BOOL_LONG.has(name)) {
        return false;
      }
    } else {
      for (let k = 1; k < tok.length; k++) {
        const ch = tok[k]!;
        const atEnd = k + 1 >= tok.length;
        if (ch === "e") {
          if (atEnd && i + 1 < args.length) scripts.push(args[i + 1]!);
          else return false;
        } else if (!SED_SAFE_BOOL_SHORT.has(ch)) {
          return false;
        }
      }
    }
    i++;
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
        !args[1]!.startsWith("-") &&
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
        const first = rest[0];
        const pinnedFirst = first !== undefined && first !== "" && !first.startsWith("-") && first !== triggerSub;
        if (!pinnedFirst || hasGitSubcommandSubargTrigger(rest, triggerSub)) {
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
        const hit = findGitSubcommandTriggerFlag(rest, flags);
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
          !args[1]!.startsWith("-") &&
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
