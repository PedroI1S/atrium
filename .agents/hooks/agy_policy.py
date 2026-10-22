#!/usr/bin/env python3
"""PreToolUse policy for headless agy workers (Atrium build orchestration).

Headless agy (1.4.5) auto-denies every command that needs a permission
prompt, and a hook "allow" cannot override that. So workers run with
--dangerously-skip-permissions and this hook is the policy: it runs before
every tool call and denies anything outside the allowlist (a hook "deny" is
enforced even under skip-permissions).

This is a guardrail against mistakes, not a security boundary: cargo build
scripts and package.json scripts can still run arbitrary code.

Contract: JSON on stdin ({"toolCall": {"name", "args": {"CommandLine"}}, ...}),
JSON on stdout ({"decision": "allow"|"deny", "reason": ...}).
"""
import json
import os
import re
import shlex
import sys

# First word of each command segment that is always allowed.
ALLOWED = {
    "pwd", "ls", "cat", "head", "tail", "wc", "echo", "printf", "true", "false",
    "which", "test", "[", "diff", "sort", "uniq", "cut", "tr", "jq", "rg", "grep",
    "find", "sed", "awk", "mkdir", "touch", "cp", "mv", "basename", "dirname",
    "date", "tee",
    "cargo", "rustc", "rustfmt", "pnpm", "npm", "node", "tsc",
}
# git subcommands a worker may use on its own branch.
GIT_ALLOWED = {
    "status", "diff", "log", "show", "add", "commit", "rev-parse", "ls-files",
    "blame", "grep", "branch", "restore", "mv", "rm",
}
GIT_DENIED_FLAGS = {"-D", "-d", "-m", "-M", "--delete", "--move", "-f", "--force"}
# Patterns that are never allowed anywhere in the command line.
DENY_PATTERNS = [
    (r"(^|\s)~|\$HOME", "home-directory paths are not allowed"),
    (r"\bsudo\b", "sudo is not allowed"),
    (r"\s-(exec|execdir|delete|ok)\b", "find -exec/-delete is not allowed"),
    (r"\b(pnpm|npm)\s+(dlx|exec)\b", "running arbitrary packages is not allowed"),
    (r"\$\(|`", "command substitution is not allowed"),
    (r"\.\./", "paths outside the worktree are not allowed"),
]


def inside(path: str, roots: list[str]) -> bool:
    real = os.path.realpath(path)
    return path == "/dev/null" or any(real == r or real.startswith(r + os.sep) for r in roots)


def decide(cmdline: str, roots: list[str]) -> tuple[str, str]:
    for pattern, reason in DENY_PATTERNS:
        if re.search(pattern, cmdline):
            return "deny", reason
    # Split compound commands into segments and check each one.
    for segment in re.split(r"&&|\|\||;|\|", cmdline):
        segment = segment.strip()
        if not segment:
            continue
        # Strip redirections (2>&1, > file) before parsing; files must be relative.
        for target in re.findall(r"\d?>>?\s*(&?\S+)", segment):
            if target.startswith("/") and not inside(target, roots):
                return "deny", f"redirect outside the worktree: {target}"
        segment = re.sub(r"\d?>>?\s*&?\S+", "", segment)
        try:
            words = shlex.split(segment)
        except ValueError:
            return "deny", "could not parse command"
        while words and re.fullmatch(r"[A-Z_][A-Z0-9_]*=\S*", words[0]):
            words = words[1:]  # leading VAR=value assignments
        if not words:
            continue
        cmd = words[0]
        outside = [w for w in words[1:] if w.startswith("/") and not inside(w, roots)]
        if outside:
            return "deny", f"absolute paths outside the worktree are not allowed: {outside[0]}"
        if cmd == "git":
            sub = words[1] if len(words) > 1 else ""
            if sub not in GIT_ALLOWED:
                return "deny", f"git {sub} is not allowed (workers never push, merge, rebase or switch branches)"
            if sub == "branch" and GIT_DENIED_FLAGS & set(words[2:]):
                return "deny", "git branch may only be used to inspect branches"
            continue
        if cmd == "rm":
            targets = [w for w in words[1:] if not w.startswith("-")]
            if any(t in (".", "./", "*", "./*", ".git") for t in targets):
                return "deny", "rm of the worktree root or .git is not allowed"
            continue
        if cmd in ("curl", "wget", "ssh", "scp", "nc", "sh", "bash", "zsh"):
            return "deny", f"'{cmd}' is not allowed for workers (no network tools or nested shells)"
        if cmd not in ALLOWED:
            return "deny", (
                f"'{cmd}' is not on the worker allowlist. Do without it, or record it "
                "as a blocker in your handoff."
            )
    return "allow", "allowed by Atrium worker policy"


# Non-shell tools a worker may use. Anything else (browser, MCP, subagents,
# scheduling, notebooks, tools added by future agy versions) is denied.
TOOLS_ALLOWED = {
    "finish", "wait", "wait_5_seconds", "command_status", "send_command_input",
    "ask_question", "ask_permission", "ask_custom_permission", "list_permissions",
    "manage_task", "search_web", "read_url_content", "list_resources", "read_resource",
}
# File tools: allowed only when every absolute path stays inside the workspace.
FILE_TOOLS = {
    "view_file", "list_dir", "find_by_name", "grep_search", "write_to_file",
    "replace_file_content", "multi_replace_file_content", "sed_file", "notebook_edit",
    # Names seen at runtime that are not in the advertised tool list.
    "write_file", "read_file", "edit_file", "create_file",
}


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for v in value.values():
            yield from strings(v)
    elif isinstance(value, list):
        for v in value:
            yield from strings(v)


def check_paths(args: dict, roots: list[str]) -> tuple[str, str]:
    for value in strings(args):
        if value.startswith("~") or "../" in value:
            return "deny", f"path outside the worktree: {value}"
        if value.startswith("/") and not inside(value, roots):
            return "deny", f"path outside the worktree: {value}"
    return "allow", "allowed by Atrium worker policy"


def main() -> None:
    event = json.load(sys.stdin)
    call = event.get("toolCall", {})
    name = call.get("name", "")
    roots = [os.path.realpath(w) for w in event.get("workspacePaths", [])]
    if name in TOOLS_ALLOWED:
        print(json.dumps({"decision": "allow"}))
        return
    if name in FILE_TOOLS:
        decision, reason = check_paths(call.get("args", {}), roots)
        print(json.dumps({"decision": decision, "reason": reason}))
        return
    if name != "run_command":
        print(json.dumps({"decision": "deny", "reason": f"tool '{name}' is not allowed for workers"}))
        return
    cmdline = call.get("args", {}).get("CommandLine", "")
    decision, reason = decide(cmdline, roots)
    print(json.dumps({"decision": decision, "reason": reason}))


if __name__ == "__main__":
    main()
