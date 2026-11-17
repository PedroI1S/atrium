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

// What a command allow rule must never be about (rounds 2 and 3, blocker 1). The policy engine
// (`atrium-core` `policy/shell.rs`) reads the code a shell, `eval` or a wrapper runs as derived
// units and drops their reasons, on the assumption that no rule allows a prefix of `sh`; an allow
// rule matches the outer argv only. So a rule on any of these would allow whatever they run.
// The check fails closed: names are folded the way the disk folds them, an option word counts as
// inline code if it holds a code flag letter anywhere, and what it can't read is refused.

/** `shell.rs` `SHELLS`. */
const SHELLS = new Set(["sh", "bash", "zsh", "dash", "ksh", "fish", "ash", "csh", "tcsh", "mksh", "yash", "rbash", "posh", "elvish", "xonsh"]);
/** Run their text in this shell (`shell.rs` `runs_here`). */
const RUNS_HERE = new Set(["eval", "source", "."]);
/**
 * The wrappers `shell.rs` `unwrap_reading` strips to find the command they run, and the ones
 * that run it as another user or in another root (`deny.rs` `PRIV`, `chroot`, …).
 */
const WRAPPERS = new Set([
  "time", "nohup", "builtin", "command", "setsid", "unbuffer", "caffeinate", "sandbox-exec", "exec", "nice", "ionice", "stdbuf", "xargs", "busybox", "toybox",
  "timeout", "chrt", "taskset", "arch", "chroot", "flock", "script", "env",
  "sudo", "sudoedit", "doas", "su", "runuser", "setpriv", "pkexec", "run0",
]);
/** `shell.rs` `COMMAND_RUNNERS`: run a command somewhere else (a host, a container). */
const REMOTE_RUNNERS = new Set([
  "ssh", "mosh", "autossh", "nsenter", "unshare", "docker", "podman", "nerdctl", "kubectl", "oc", "lxc", "incus", "machinectl", "systemd-run", "systemd-nspawn",
  "firejail", "bwrap", "flatpak-spawn", "toolbox", "distrobox", "distrobox-enter", "multipass", "vagrant", "limactl", "colima", "parallel",
]);

interface Interpreter {
  /** Short flags that take code (compared case-insensitively, anywhere in a `-…` word). */
  short: string;
  /** Long flags that take code, with or without `=…`. */
  long: readonly string[];
  /** Subcommands that take code (`deno eval`). */
  sub: readonly string[];
  /** Subcommands that come before the script, so they don't name it (`deno run`). */
  verbs: readonly string[];
}
/** `deny.rs` `is_interpreter`, and how each is given code inline. Keys are folded names. */
const INTERPRETERS = new Map<string, Interpreter>([
  ["python", { short: "c", long: [], sub: [], verbs: [] }],
  ["node", { short: "ep", long: ["--eval", "--print"], sub: [], verbs: [] }],
  ["deno", { short: "", long: ["--eval", "--eval-file"], sub: ["eval", "repl"], verbs: ["run", "test", "task", "bench", "serve", "x", "install", "compile", "jupyter"] }],
  ["bun", { short: "ep", long: ["--eval", "--print"], sub: ["eval", "repl", "exec"], verbs: ["run", "test", "x", "create", "build"] }],
  ["ruby", { short: "e", long: [], sub: [], verbs: [] }],
  ["perl", { short: "e", long: [], sub: [], verbs: [] }],
  ["php", { short: "rbef", long: ["--run", "--process-code", "--process-begin", "--process-end", "--process-file"], sub: [], verbs: [] }],
  ["lua", { short: "e", long: [], sub: [], verbs: [] }],
  ["osascript", { short: "e", long: [], sub: [], verbs: [] }],
]);
/** PowerShell's options that take code; it accepts any unambiguous prefix, with `-`, `--` or `/`. */
const PWSH_CODE = ["command", "encodedcommand", "commandwithargs"];
const PWSH_SHORT = new Set(["c", "e", "ec", "cwa"]);
/** Run a package or tool by name: the rule must name it. */
const PACKAGE_RUNNERS = new Set(["npx", "pnpx", "bunx", "uvx", "pipx"]);
/** `npx -c` / `--call` runs a shell command. */
const RUNNER_CODE = new Set(["-c", "--call"]);
/** Script names that mean standard input (`shell.rs` `reads_stdin_as_script`, `deny.rs`). Lower-cased. */
const STDIN = new Set(["-", "/dev/stdin", "/dev/fd/0", "/proc/self/fd/0"]);
/** `find` options that run a command for each file (`shell.rs` `find_exec_units`). */
const FIND_EXEC = new Set(["-exec", "-execdir", "-ok", "-okdir"]);

function normalizePosixPath(raw: string): string {
  const parts: string[] = [];
  for (const seg of raw.split("/")) {
    if (seg === "" || seg === ".") continue;
    if (seg === "..") parts.pop();
    else parts.push(seg);
  }
  return `/${parts.join("/")}`.toLowerCase().replaceAll("ſ", "s");
}

function isStdinPath(raw: string): boolean {
  if (raw === "-") return true;
  if (!raw.startsWith("/")) return false;
  const norm = normalizePosixPath(raw);
  return STDIN.has(norm) || norm === "/proc/thread-self/fd/0";
}

function isPs1Script(w: string): boolean {
  return w !== "" && !isStdinPath(w) && /\.ps1$/i.test(w);
}

function pwshOptionBody(w: string): string | null {
  if (w === "-" || w === "--") return null;
  if (w.startsWith("--")) return w.slice(2);
  if (w.startsWith("-")) return w.slice(1);
  if (w.startsWith("/")) {
    const rest = w.slice(1);
    if (!rest.includes("/") && !/\.ps1$/i.test(rest)) return rest;
  }
  return null;
}

const pwshIsFileOpt = (bare: string): boolean => bare === "f" || (bare.length >= 2 && "file".startsWith(bare));

function pwshIsValueOpt(bare: string): boolean {
  if (bare === "wd" || bare === "ep" || bare === "w" || bare === "if" || bare === "of" || bare === "ea") return true;
  if (bare.length >= 2 && (
    "workingdirectory".startsWith(bare) ||
    "executionpolicy".startsWith(bare) ||
    "windowstyle".startsWith(bare) ||
    "inputformat".startsWith(bare) ||
    "outputformat".startsWith(bare) ||
    "encodedarguments".startsWith(bare) ||
    "settingsfile".startsWith(bare) ||
    "custompipename".startsWith(bare)
  )) return true;
  if (bare.length >= 3 && "configurationname".startsWith(bare)) return true;
  if (bare.length >= 14 && "configurationfile".startsWith(bare)) return true;
  return false;
}

function pwshIsBoolOpt(bare: string): boolean {
  if (bare === "?" || bare === "h" || bare === "v" || bare === "l" || bare === "i" || bare === "sta" || bare === "mta" || bare === "wa") return true;
  if (bare.length >= 2 && ("help".startsWith(bare) || "version".startsWith(bare) || "wait".startsWith(bare) || "login".startsWith(bare) || "interactive".startsWith(bare))) return true;
  if (bare.length >= 3 && ("nologo".startsWith(bare) || "noexit".startsWith(bare) || "noprofile".startsWith(bare) || "noninteractive".startsWith(bare))) return true;
  if (bare.length >= 4 && "noprofileloadtime".startsWith(bare)) return true;
  return false;
}

function checkPwshRisk(name: string, args: readonly string[]): CommandRisk {
  for (const w of args) {
    const opt = pwshOptionBody(w);
    if (opt === null) continue;
    const bare = opt.toLowerCase().split(/[:=]/)[0]!;
    if (PWSH_SHORT.has(bare) || (bare !== "" && PWSH_CODE.some((c) => c.startsWith(bare)))) {
      return { refuse: `A rule can't allow ${name} ${w}: it runs code written on the command line, and Atrium wouldn't check it.` };
    }
  }
  const refuseNoScript = (): CommandRisk => ({ refuse: `A rule can allow ${name} only with the script it runs, like ${name} -File build.ps1.` });
  let foundScript = false;
  let i = 0;
  while (i < args.length) {
    const w = args[i]!;
    if (w === "--") {
      if (args[i + 1] !== undefined && isPs1Script(args[i + 1]!)) foundScript = true;
      break;
    }
    const opt = pwshOptionBody(w);
    if (opt === null) {
      if (isPs1Script(w)) {
        foundScript = true;
        break;
      }
      return refuseNoScript();
    }
    const lw = opt.toLowerCase();
    const sep = lw.search(/[:=]/);
    const bare = sep >= 0 ? lw.slice(0, sep) : lw;
    const attached = sep >= 0 ? opt.slice(sep + 1) : null;
    if (pwshIsFileOpt(bare)) {
      const script = attached ?? args[i + 1];
      if (script !== undefined && isPs1Script(script)) {
        foundScript = true;
        break;
      }
      return refuseNoScript();
    }
    if (pwshIsValueOpt(bare)) {
      if (attached !== null) i += 1;
      else if (i + 1 < args.length) i += 2;
      else return refuseNoScript();
      continue;
    }
    if (pwshIsBoolOpt(bare)) {
      i += 1;
      continue;
    }
    return refuseNoScript();
  }
  if (!foundScript) return refuseNoScript();
  return { warn: `This allows the script ${name} runs, whatever the script does.` };
}

function interpreterValueFlag(key: string, raw: string, lw: string): boolean {
  switch (key) {
    case "node":
      return raw === "-r" || raw === "-C" || [
        "--require", "--import", "--loader", "--experimental-loader",
        "--experimental-specifier-resolution", "--input-type", "--conditions",
        "--title", "--max-old-space-size", "--env-file", "--watch-path",
        "--redirect-warnings", "--diagnostic-dir", "--cpu-prof-dir",
        "--heap-prof-dir", "--test-name-pattern",
      ].includes(lw);
    case "deno":
      return raw === "-c" || raw === "-L" || [
        "--config", "--import-map", "--lock", "--env", "--env-file",
        "--ext", "--log-level", "--location", "--cert", "--v8-flags",
      ].includes(lw);
    case "bun":
      return raw === "-c" || raw === "-r" || [
        "--config", "--cwd", "--env-file", "--preload", "--port", "--define", "--loader",
      ].includes(lw);
    case "ruby":
      return ["-I", "-r", "-C", "-E", "-F", "-K"].includes(raw) || [
        "--encoding", "--external-encoding", "--internal-encoding",
      ].includes(lw);
    case "perl":
      return ["-I", "-M", "-m", "-F", "-D", "-x"].includes(raw);
    case "php":
      return ["-c", "-d", "-z", "-S", "-t"].includes(raw);
    case "lua":
      return raw === "-l";
    case "osascript":
      return raw === "-l" || raw === "-s";
    default:
      return false;
  }
}

function interpreterBoolOrAttached(key: string, raw: string, lw: string): boolean {
  if (lw.startsWith("--") && lw.includes("=")) return true;
  switch (key) {
    case "node":
      return ["-v", "-h", "-c"].includes(raw) || [
        "--version", "--help", "--check", "--no-deprecation", "--throw-deprecation",
        "--trace-deprecation", "--trace-warnings", "--no-warnings", "--zero-fill-buffers",
        "--preserve-symlinks", "--preserve-symlinks-main", "--watch", "--test",
        "--enable-source-maps", "--experimental-vm-modules", "--no-experimental-fetch",
      ].includes(lw);
    case "deno":
      return ["-A", "-q", "-r", "-h", "-V"].includes(raw) || [
        "--allow-all", "--allow-read", "--allow-write", "--allow-net", "--allow-env",
        "--allow-run", "--allow-ffi", "--allow-sys", "--allow-hrtime", "--quiet",
        "--reload", "--no-check", "--watch", "--cached-only", "--no-lock",
        "--no-remote", "--unstable", "--help", "--version",
      ].includes(lw) || lw.startsWith("--allow-") || lw.startsWith("--unstable-");
    case "bun":
      return ["-b", "-h", "-v"].includes(raw) || [
        "--hot", "--watch", "--bun", "--smol", "--silent", "--no-install",
        "--prefer-offline", "--production", "--help", "--version",
      ].includes(lw);
    case "ruby":
      return lw === "--verbose" || lw === "--version"
        || ((raw.startsWith("-I") || raw.startsWith("-r") || raw.startsWith("-E") || raw.startsWith("-W") || raw.startsWith("-0") || raw.startsWith("-i")) && raw.length > 2)
        || (!raw.startsWith("--") && [...raw.slice(1)].every((c) => "wvVTsnapld0y".includes(c)));
    case "perl":
      return ((raw.startsWith("-I") || raw.startsWith("-M") || raw.startsWith("-m") || raw.startsWith("-0") || raw.startsWith("-i")) && raw.length > 2)
        || (!raw.startsWith("--") && [...raw.slice(1)].every((c) => "wWvVTtsnpald0cuU".includes(c)));
    case "php":
      return !raw.startsWith("--") && [...raw.slice(1)].every((c) => "nqwlsmvih".includes(c));
    case "lua":
      return raw === "-i" || raw === "-v" || raw === "-W";
    case "osascript":
      return raw === "-i" || raw === "-o";
    default:
      return false;
  }
}

function gitConfigIsExec(kv: string): boolean {
  const eq = kv.indexOf("=");
  const key = (eq >= 0 ? kv.slice(0, eq) : kv).toLowerCase();
  const val = eq >= 0 ? kv.slice(eq + 1) : "";
  return (
    val.trimStart().startsWith("!")
    || key.startsWith("alias.")
    || (key.startsWith("merge.") && key.endsWith(".driver"))
    || (key.startsWith("filter.") && (key.endsWith(".clean") || key.endsWith(".smudge") || key.endsWith(".process")))
    || (key.startsWith("diff.") && (key.endsWith(".command") || key.endsWith(".textconv") || key === "diff.external"))
    || key.startsWith("credential.")
    || key.startsWith("uploadpack.")
    || key.startsWith("receive.")
    || key.startsWith("gpg.")
    || key.startsWith("remote.")
    || key.startsWith("protocol.")
    || [
      "core.fsmonitor",
      "core.sshcommand",
      "core.askpass",
      "core.editor",
      "core.pager",
      "core.gitproxy",
      "sequence.editor",
      "browser",
    ].includes(key)
  );
}

function gitCliExecConfig(args: readonly string[]): { hasAny: boolean; hasDangerous: boolean } {
  let hasAny = false;
  let hasDangerous = false;
  let i = 0;
  while (i < args.length) {
    const w = args[i]!;
    if (w === "--") break;
    if (w === "-c") {
      hasAny = true;
      const kv = args[i + 1];
      if (kv === undefined || gitConfigIsExec(kv)) hasDangerous = true;
      i += 2;
      continue;
    }
    if (w.startsWith("-c") && w.length > 2) {
      hasAny = true;
      if (gitConfigIsExec(w.slice(2))) hasDangerous = true;
      i += 1;
      continue;
    }
    if (w === "--config-env" || w === "--exec-path") {
      hasAny = true;
      hasDangerous = true;
      i += 2;
      continue;
    }
    if (w.startsWith("--config-env=") || w.startsWith("--exec-path=")) {
      hasAny = true;
      hasDangerous = true;
      i += 1;
      continue;
    }
    if (["-C", "--git-dir", "--work-tree", "--namespace", "--super-prefix", "--list-cmds"].includes(w)) {
      i += 2;
      continue;
    }
    if (w.startsWith("-")) {
      i += 1;
      continue;
    }
    break;
  }
  for (const w of args) {
    if (w === "--") break;
    if (
      ["--ext-diff", "--config-env", "--exec-path", "--upload-pack", "--receive-pack"].includes(w)
      || w.startsWith("--config-env=")
      || w.startsWith("--exec-path=")
      || w.startsWith("--upload-pack=")
      || w.startsWith("--receive-pack=")
      || w.startsWith("--exec=")
    ) {
      hasAny = true;
      hasDangerous = true;
    }
  }
  return { hasAny, hasDangerous };
}

/**
 * A command name as the disk resolves it (macOS APFS is case-insensitive): the basename,
 * lower-cased (which also folds K, U+212A, to k), with ſ (U+017F) folded to s, as `deny.rs` `lower`.
 */
export function foldName(word: string): string {
  const b = word.split("/").pop() ?? word;
  return b.toLowerCase().replaceAll("ſ", "s");
}
const isOption = (w: string): boolean => w.length > 1 && w.startsWith("-");

function interpreterOf(name: string): { key: string; spec: Interpreter } | "pwsh" | null {
  if (name === "pwsh" || name === "powershell" || name.startsWith("pwsh") || name.startsWith("powershell")) return "pwsh";
  // `deny.rs` reads any name starting with `python` as Python (`python3.12`, `python3-dbg`).
  const key = name.startsWith("python") ? "python" : name;
  const spec = INTERPRETERS.get(key);
  return spec ? { key, spec } : null;
}

export type CommandRisk = { refuse: string } | { warn: string } | null;

const NAME_A_SCRIPT = "Name the script right after it, like python3 scripts/build.py, with no options before it: otherwise the rule would also allow code written on the command line.";

/**
 * Whether an allow rule on this argv would allow more than the command it names. Fails closed.
 * Refused, prefix or exact: a shell, `eval`/`source`/`.`, a wrapper, an interpreter or runner
 * given code inline, PowerShell given code, `find -exec`. Refused for a prefix rule only: an
 * interpreter, package runner or `find` with nothing after it that names the script, package or
 * file (a prefix rule on `find` at all: it would also allow `-exec` added later). Warned: what
 * the rule then allows still runs code it doesn't pin. Deny rules on any of them are fine.
 */
export function commandRisk(argv: readonly string[], matcher: "command_prefix" | "command_exact" = "command_prefix"): CommandRisk {
  const first = argv[0];
  if (first === undefined) return null;
  const name = foldName(first);
  const args = argv.slice(1);
  const prefix = matcher === "command_prefix";
  if (SHELLS.has(name)) return { refuse: `A rule can't allow ${name}: it would allow every command run through it. Write the command itself.` };
  if (RUNS_HERE.has(name)) return { refuse: `A rule can't allow ${name}: it runs whatever text follows it. Write the command itself.` };
  if (WRAPPERS.has(name)) return { refuse: `A rule can't allow ${name}: it runs the command that follows it, and Atrium wouldn't check that one. Write the command without ${name}.` };

  const interp = interpreterOf(name);
  if (interp === "pwsh") return checkPwshRisk(name, args);
  if (interp !== null) {
    const { key, spec } = interp;
    const { short, long, sub, verbs } = spec;
    for (const w of args) {
      const lw = w.toLowerCase();
      if (lw.startsWith("--")) {
        if (long.some((l) => lw === l || lw.startsWith(`${l}=`))) return { refuse: `A rule can't allow ${name} ${w}: it runs code written on the command line, and Atrium wouldn't check it.` };
      } else if (isOption(w)) {
        // A flag letter anywhere in the word: `-c'code'` attached, `-0777ne`, `-Ic`.
        if (short !== "" && [...lw.slice(1)].some((c) => short.includes(c))) return { refuse: `A rule can't allow ${name} ${w}: it runs code written on the command line (${w} holds a code flag), and Atrium wouldn't check it.` };
      }
    }
    const refuseNoScript = (): CommandRisk => ({ refuse: `A rule can't allow ${argv.slice(0, 1).join(" ")} without the script it runs: without one, ${name} reads code from its input. ${NAME_A_SCRIPT}` });
    const isPython = key === "python";
    let verbSeen = false;
    let foundTarget = false;
    let i = 0;
    while (i < args.length) {
      const w = args[i]!;
      const lw = w.toLowerCase();
      if (w === "--") {
        const next = args[i + 1];
        if (next !== undefined) {
          if (isStdinPath(next)) return { refuse: `A rule can't allow ${name} ${next}: it runs code read from its input, and Atrium wouldn't check it.` };
          if (!verbSeen && sub.includes(next.toLowerCase())) return { refuse: `A rule can't allow ${name} ${next}: it runs code written on the command line, and Atrium wouldn't check it.` };
          foundTarget = true;
        }
        break;
      }
      if (!isOption(w)) {
        if (isStdinPath(w)) return { refuse: `A rule can't allow ${name} ${w}: it runs code read from its input, and Atrium wouldn't check it.` };
        if (!verbSeen && sub.includes(lw)) return { refuse: `A rule can't allow ${name} ${w}: it runs code written on the command line, and Atrium wouldn't check it.` };
        if (!verbSeen && verbs.length > 0 && verbs.includes(lw)) {
          verbSeen = true;
          i += 1;
          continue;
        }
        foundTarget = true;
        break;
      }
      if (isPython) {
        if (w === "-m") {
          const mod = args[i + 1];
          if (mod !== undefined && mod !== "" && !mod.startsWith("-")) {
            foundTarget = true;
            break;
          }
          return refuseNoScript();
        }
        if (w.startsWith("-m") && w.length > 2) {
          foundTarget = true;
          break;
        }
        if (w === "-W" || w === "-X" || w === "-Q" || w === "--check-hash-based-pycs") {
          if (i + 1 >= args.length) return refuseNoScript();
          i += 2;
          continue;
        }
        if (w.startsWith("--check-hash-based-pycs=") || ((w.startsWith("-W") || w.startsWith("-X") || w.startsWith("-Q")) && w.length > 2)) {
          i += 1;
          continue;
        }
        if ([
          "--version", "--help", "--help-all", "--help-env", "--help-xoptions", "--verbose",
          "--quiet", "--isolated", "--ignore-environment", "--no-site", "--no-user-site",
          "--skip-source-first-line", "--bytes-warning", "--dont-write-bytecode", "--optimize",
          "--inspect", "--unbuffered",
        ].includes(lw) || (!w.startsWith("--") && [...w.slice(1)].every((c) => "bBdEhiIOqsSuvVx".includes(c)))) {
          i += 1;
          continue;
        }
        return refuseNoScript();
      }
      if (interpreterValueFlag(key, w, lw)) {
        if (i + 1 >= args.length) return refuseNoScript();
        i += 2;
        continue;
      }
      if (interpreterBoolOrAttached(key, w, lw)) {
        i += 1;
        continue;
      }
      return refuseNoScript();
    }
    if (!foundTarget) return refuseNoScript();
    if (prefix) {
      // The script must come right after the command (after a verb such as `deno run`, right
      // after the verb): an interpreter reads its own options only before the script, so words
      // added after it go to the script. An option before it could take code added later.
      const at = verbs.length && args[0] !== undefined && verbs.includes(args[0].toLowerCase()) ? 1 : 0;
      const named = args[at];
      if (named === undefined || named.startsWith("-")) return { refuse: `A prefix rule can't allow ${argv.slice(0, at + 1).join(" ")}${named === undefined ? " alone" : ` ${named}`}. ${NAME_A_SCRIPT}` };
    }
    return { warn: `This allows the ${name} script it names${prefix ? ", with whatever arguments follow," : ""} whatever the script does.` };
  }
  if (PACKAGE_RUNNERS.has(name)) {
    // `-c` also in a cluster or with its value attached (`-yc 'sh'`, `-c'sh'`), as for interpreters.
    const code = args.find((w) => {
      const lw = w.toLowerCase();
      return RUNNER_CODE.has(lw) || lw.startsWith("--call=") || (isOption(w) && !lw.startsWith("--") && lw.slice(1).includes("c"));
    });
    if (code) return { refuse: `A rule can't allow ${name} ${code}: it runs a shell command, and Atrium wouldn't check it.` };
    // The package right after the runner, as for interpreters: an option before it could be `-c` added later.
    if (prefix && (args[0] === undefined || args[0].startsWith("-"))) return { refuse: `A prefix rule on ${name} must name the package right after it, like ${name} prettier, with no options before it.` };
    let i = 0;
    let foundPkg = false;
    while (i < args.length) {
      const w = args[i]!;
      const lw = w.toLowerCase();
      if (w === "--") {
        const next = args[i + 1];
        foundPkg = next !== undefined && next !== "" && next !== "-";
        break;
      }
      if (!isOption(w)) {
        if (w !== "-") {
          foundPkg = true;
          break;
        }
        i += 1;
        continue;
      }
      if (lw.startsWith("--") && lw.includes("=")) {
        i += 1;
        continue;
      }
      if ([
        "-p", "--package", "--from", "--with", "--with-editable", "--with-requirements",
        "--python", "--index-url", "--extra-index-url", "--node-options", "-n", "--cache", "--userconfig",
      ].includes(lw)) {
        if (i + 1 >= args.length) return { refuse: `A rule on ${name} must name the package it runs, like ${name} prettier.` };
        i += 2;
        continue;
      }
      if ([
        "-y", "--yes", "-q", "--quiet", "--no", "--no-install", "--offline", "--prefer-offline",
        "--ignore-existing", "--isolated", "--no-cache", "--no-build-isolation", "--system",
        "--verbose", "-v", "-h", "--help", "--version", "--shell-auto-fallback",
      ].includes(lw) || (!lw.startsWith("--") && [...lw.slice(1)].every((c) => "yqnhv".includes(c)))) {
        i += 1;
        continue;
      }
      return { refuse: `A rule on ${name} must name the package it runs, like ${name} prettier.` };
    }
    if (!foundPkg) return { refuse: `A rule on ${name} must name the package it runs, like ${name} prettier.` };
    return { warn: `This allows the package ${name} runs, whatever it does.` };
  }
  if (name === "git") {
    const { hasAny, hasDangerous } = gitCliExecConfig(args);
    if ((prefix && hasAny) || hasDangerous) {
      return { refuse: "A rule can't allow git with command-line configuration or exec options: it can run arbitrary commands." };
    }
  }
  if (name === "find") {
    const exec = args.find((w) => FIND_EXEC.has(w.toLowerCase()));
    if (exec) return { refuse: `A rule can't allow find ${exec}: it runs another command for each file, and Atrium wouldn't check that one.` };
    if (prefix) return { refuse: "A prefix rule on find would also allow find … -exec or -delete added after it: use “This exact command” instead." };
    if (args.some((w) => w.toLowerCase() === "-delete")) return { warn: "This allows find to delete what it matches." };
    return null;
  }
  if (REMOTE_RUNNERS.has(name)) return { warn: `This allows whatever ${name} runs (on a host, in a container), whatever follows these words.` };
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
/**
 * Whether the `{` at `at` starts a brace expansion as `shell.rs` reads it: a `,` or `..` at depth
 * 1 before its `}`, counting nested braces, stopping at a blank or an operator; too far to tell
 * (256 characters) counts as one.
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
  // The engine never allows a command holding a line break, quoted or not.
  if (/[\n\r]/.test(line)) return { ok: false, error: "A rule matches one line: remove the line break" };
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
      // globset 0.4.21 `parse_class`, checked by building globs with it (round 3): no escapes
      // inside a class (`[\]` is a class holding a backslash); a leading `]` or `-` is literal; a `-`
      // after a range extends that range from its start, so `[a-c-e]` builds and `[a-z-_]`
      // doesn't (`_` sorts before `a`).
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
 * Whether "Only for paths inside" can be set (round 2 blocker 2, round 3 major 3). The engine checks a path scope
 * against every path of the call, a command's working folder included, and with `all` for deny
 * rules too. So it is offered only on allow rules about file calls: on a command rule it would
 * never match in a Home chat (cwd = ~), and on a deny rule it would let a call through as soon as
 * one of its paths is outside. Escalated in the M1-23 handoff.
 */
export function pathScopeOffered(form: Pick<RuleForm, "decision" | "matcher" | "toolKind">): boolean {
  if (form.decision !== "allow") return false;
  // File calls only (round 3, major 3): web requests, MCP and other calls have no paths the
  // engine reads, so a limit there would never match.
  // A path rule's kind is always a file kind (validateRule makes it one).
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
