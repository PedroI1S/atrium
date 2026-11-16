# Review (Round 2 / D-113): `M1-23-ui-gaps` (`agent/opus/M1-23-ui-gaps`, `eb5997ec..8ba5d3f8`)

> Saved by the orchestrator from the reviewer's report (D-113). Links are relative to the worktree. The orchestrator confirmed blocker 2 (`InboxCard.tsx:180-186`: `decide` falls back to the picked thread's first approval when the focused card belongs to another thread) and that `shell.rs` matches shell names case-sensitively (no lowercasing), which makes blocker 1's case/APFS part an engine gap too.


- **Reviewer:** Gemini 4 Argon (High), read-only
- **Worker:** Claude Opus 5.5
- **Verdict:** **Rework** (2 blockers — 1 in `commandRisk` inline-code/case/APFS-lookalike checks, 1 in `InboxCard` keyboard approval targeting when focus is on a batch checkbox or card — plus 1 major in `pathScopeOffered`, 2 minors)

---

## Findings (Ordered by Severity; Safety Blockers First)

### 1. [BLOCKER] `commandRisk` lets allow rules through on inline-code interpreters with attached arguments (`python3 -c'...'`), digit option clusters (`perl -0e`), `deno -q eval` / `bun eval`, and uppercase / APFS-folded shells & wrappers (`/bin/BASH`, `ENV`, `SUDO`, `/bin/baſh`) — and crashes on `Object.prototype` keys

- **Location:** [rule-form.ts](app/src/chat/rule-form.ts#L75-L138) (interacting with [shell.rs](crates/atrium-core/src/policy/shell.rs#L1153-L1155), [shell.rs](crates/atrium-core/src/policy/shell.rs#L2231-L2243), and [deny.rs](crates/atrium-core/src/policy/deny.rs#L231-L233), [deny.rs](crates/atrium-core/src/policy/deny.rs#L530-L540))
- **Evidence:**
  1. **Attached inline-code arguments (`-c'...'`, `-e"..."`) and digit option clusters (`perl -0e`, `ruby -0e`) bypass `/^-[A-Za-z]+$/` ([rule-form.ts](app/src/chat/rule-form.ts#L128)):**
     ```ts
     const inline = long.some((l) => w === l || w.startsWith(`${l}=`))
       || (short !== "" && /^-[A-Za-z]+$/.test(w) && [...w.slice(1)].some((c) => short.includes(c)))
       || (i === 1 && sub?.includes(w) === true);
     ```
     - `splitCommand` strips quotes before `commandRisk` runs. When inline code is attached to the short option in standard `getopt` style (`python3 -c'import os; os.system("sh")'`, `python3 -c"print(1)"`, `node -e"1"`, `perl -e'system("sh")'`, `ruby -e'system("sh")'`, `php -r'system("sh");'`, `lua -e'os.execute("sh")'`), `w` in `split.argv` is `"-cimport os; os.system(\"sh\")"`, `"-cprint(1)"`, `"-e1"`, etc. Because `w` contains characters outside `[A-Za-z]` (spaces, parens, quotes, digits, semicolons), `/^-[A-Za-z]+$/.test(w)` is `false` and `validateRule` returns `ok: true`.
     - When a short option cluster before `-e` contains digits (`perl -0e 'system("sh")'`, `perl -0777ne 'system("sh")'`, `ruby -0e 'system("sh")'`), `/^-[A-Za-z]+$/.test("-0e")` is also `false` and `validateRule` returns `ok: true`.
  2. **`deno` with flags before `eval`, `bun eval`, and `pwsh` ([rule-form.ts](app/src/chat/rule-form.ts#L86-L96), [rule-form.ts](app/src/chat/rule-form.ts#L129)):**
     - `i === 1 && sub?.includes(w) === true` only checks `argv[1]`. `deno -q eval "..."` or `deno --quiet eval "..."` has `eval` at `i === 2` and passes with `ok: true`.
     - `bun` also supports `bun eval "..."` (`sub: ["eval"]` is missing on `bun`), and [deny.rs](crates/atrium-core/src/policy/deny.rs#L538) includes `pwsh` in `is_interpreter` (`pwsh -c` / `-Command` / `-EncodedCommand`), which is missing from `INLINE_CODE`.
  3. **Case sensitivity and APFS lookalikes on `name` ([rule-form.ts](app/src/chat/rule-form.ts#L102), [rule-form.ts](app/src/chat/rule-form.ts#L117)):**
     - `const name = base(first);` is case-sensitive and does not fold `ſ` (U+017F) → `s` or `K` (U+212A) → `k` as [deny.rs](crates/atrium-core/src/policy/deny.rs#L231-L233) does for APFS.
     - On macOS (case-insensitive APFS), `/bin/BASH -c 'ls'`, `Bash -c 'ls'`, `/usr/bin/ENV`, `SUDO`, `Python3 -c 'x'`, `FIND . -exec rm {} +`, `/bin/baſh`, and `/bin/Ksh` resolve to the same binaries on disk, pass `validateRule` with `ok: true` and no warnings, and in `shell.rs` (where `SHELLS` is lowercase) `/bin/BASH -c "..."` does not even have its `-c` payload extracted.
     - Also, `PRIV` in [deny.rs](crates/atrium-core/src/policy/deny.rs#L530) includes `run0` and `sudoedit`, which are missing from `WRAPPERS`.
  4. **`TypeError` crash on `Object.prototype` keys in `interpreterOf` ([rule-form.ts](app/src/chat/rule-form.ts#L104)):**
     - `name in INLINE_CODE` evaluates to `true` for `"constructor"`, `"toString"`, `"valueOf"`, etc. Typing `constructor x` in the command input sets `interp = "constructor"`, where `INLINE_CODE[interp]` is `Object.prototype.constructor` (`long` is `undefined`), throwing `TypeError: Cannot read properties of undefined (reading 'some')`.
- **Smallest fix:**
  - Normalize `const name = base(first).toLowerCase().replaceAll("ſ", "s");` (and do the same in `RESERVED.has(argv[0]!.toLowerCase())`), add `"run0"` and `"sudoedit"` to `WRAPPERS`, add `sub: ["eval"]` to `bun`, and add `pwsh: { short: "ce", long: ["--command", "-command", "-c", "-encodedcommand", "-e", "-ec"] }` to `INLINE_CODE`.
  - In `interpreterOf`, replace `name in INLINE_CODE` with `Object.hasOwn(INLINE_CODE, name)`.
  - In `commandRisk`'s interpreter loop:
    - Match short inline flags even when preceded by digit flags or followed by an attached argument: `short !== "" && w.startsWith("-") && !w.startsWith("--") && [...w.slice(1)].some((c, idx) => short.includes(c) && /^[A-Za-z0-9]*$/.test(w.slice(1, 1 + idx)))`.
    - Check `sub?.includes(w)` on the first non-option argument (`!w.startsWith("-")`), not only `i === 1`.
    - For `find`, check `FIND_EXEC.has(w.toLowerCase())`.

---

### 2. [BLOCKER] `InboxCard` `A`/`D` and `X` shortcuts can approve, deny, or toggle the wrong thread's approval when focus is on a batch checkbox or another row's card

- **Location:** [InboxCard.tsx](app/src/ui/InboxCard.tsx#L93-L95), [InboxCard.tsx](app/src/ui/InboxCard.tsx#L180-L201)
- **Evidence:**
  1. **`decide` falls back to `picked()`'s first approval when `focused` is on a different row's card ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L180-L186)):**
     ```ts
     const decide = (action: "allow" | "deny", focused: string | null) => {
       const item = picked();
       if (!item?.thread_id) return;
       const pending = held.get(item.thread_id)?.transcript.pending() ?? [];
       const request = pending.find((r) => r.kind === "approval" && batchKey(item.thread_id!, r) === focused) ?? pending.find((r) => r.kind === "approval");
       if (request) void resolveApproval(item.thread_id, request, action);
     };
     ```
     - `div.item` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L93)) only calls `setInboxPick(key())` on `onPointerDown` (and `button.open` on `onFocus`), not when keyboard focus moves (`Tab` / `Shift+Tab`) into a `RequestCard`'s checkbox or buttons, nor when `createEffect` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L167-L170)) resets `inboxPick` to `keys()[0]`.
     - If `inboxPick()` is Thread 1 (`t1`) while DOM focus (`e.target`) is on Thread 2's approval card (`focused = "t2:0:q1"`), pressing `X` toggles Thread 2 (`xKeys` searches all `targets()`), but pressing `A` calls `decide("allow", "t2:0:q1")`, which searches only `t1`'s `pending`, fails to find `"t2:0:q1"`, and via `?? pending.find(...)` **approves Thread 1's approval instead of Thread 2's focused card**.
  2. **Pressing `J`/`K` while a batch checkbox has focus moves `inboxPick()` to a new row while leaving DOM focus (`e.target`) on the old row's checkbox ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L190-L200)):**
     - When a user clicks Thread 1's batch checkbox (`e.target` is Thread 1's `<input type="checkbox">`) and presses `J` to move the active inbox row (`div.item.on`) to Thread 2, `setInboxPick` updates `inboxPick()` to Thread 2, but `document.activeElement` stays on Thread 1's checkbox (`focusedCardKey(e.target) === "t1:0:q1"`).
     - Pressing `X` then calls `xKeys(targets(), "t1:0:q1", "t2")`, which toggles **Thread 1** instead of the newly selected Thread 2.
- **Smallest fix:**
  1. On `div.item` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L93)), add `onFocusIn={() => setInboxPick(key())}`.
  2. In `onKey` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L194-L195)), when `J`, `K`, `ArrowDown`, or `ArrowUp` moves the selection, call `list.focus({ preventScroll: true })` so DOM focus does not stay inside the previous row's card.
  3. In `decide` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L180-L186)), look up the target in `targets()` without falling back across keys when `focused !== null`:
     ```ts
     const decide = (action: "allow" | "deny", focused: string | null) => {
       const t = focused !== null
         ? targets().find((x) => x.key === focused)
         : targets().find((x) => x.threadId === picked()?.thread_id);
       if (t) void resolveApproval(t.threadId, t.request, action);
     };
     ```

---

### 3. [MAJOR] `pathScopeOffered` still offers `"Only for paths inside"` on `matcher: "any"` for non-file kinds (`fetch`, `mcp`, `other`), producing rules that never match

- **Location:** [rule-form.ts](app/src/chat/rule-form.ts#L314-L318) (vs [deny.rs](crates/atrium-core/src/policy/deny.rs#L143-L160) and [engine.rs](crates/atrium-core/src/policy/engine.rs#L64-L69))
- **Evidence:**
  ```ts
  export function pathScopeOffered(form: Pick<RuleForm, "decision" | "matcher" | "toolKind">): boolean {
    if (form.decision !== "allow") return false;
    if (form.matcher === "path_glob") return true;
    return form.matcher === "any" && form.toolKind !== "execute" && ANY_KINDS.includes(form.toolKind);
  }
  ```
  - The doc comment on line 310 states: *"So it is offered only on allow rules about file calls"*.
  - However, line 317 checks `form.toolKind !== "execute" && ANY_KINDS.includes(form.toolKind)`, which is `true` for `"fetch"` (`Web requests`), `"mcp"` (`MCP tool calls`), and `"other"` (`Other tool calls`).
  - In [deny.rs](crates/atrium-core/src/policy/deny.rs#L143-L160) and [engine.rs](crates/atrium-core/src/policy/engine.rs#L64-L69), `path_scope_ok` requires `!paths.is_empty() && paths.iter().all(|p| glob_matches(g, p))`:
    - For `fetch` (`https://...`), `paths` is empty (`[]`), so `path_scope_ok` is always `false`.
    - For `other` (`ToolSubject::None` or `ToolSubject::Unknown`), `paths` is empty or `allow_covers` returns `none`, so `path_scope_ok` is always `false`.
    - For `mcp`, `paths` is empty unless a JSON string looks `path_like`, and any URL or MIME type containing `/` is resolved as `<cwd>/https://...`, failing the glob.
- **Smallest fix:**
  In [rule-form.ts](app/src/chat/rule-form.ts#L317), replace `form.toolKind !== "execute" && ANY_KINDS.includes(form.toolKind)` with `(PATH_KINDS as readonly ToolKind[]).includes(form.toolKind)`.

---

### 4. [MINOR] `const SAVE_MESSAGE` in `save.rs` is not gated by `#[cfg(target_os = "macos")]`, which will fail `cargo clippy -- -D warnings` on Linux

- **Location:** [save.rs](app/src-tauri/src/save.rs#L62)
- **Evidence:**
  - `const SAVE_MESSAGE: &str = "Atrium never replaces a file: pick a new name.";` is defined unconditionally at module scope, but is only used inside `#[cfg(target_os = "macos")] fn run_dialog` ([save.rs](app/src-tauri/src/save.rs#L94)).
  - On Linux (`#[cfg(target_os = "linux")]`), `SAVE_MESSAGE` is dead code and will fail `cargo clippy --workspace --all-targets -- -D warnings`.
- **Smallest fix:**
  Add `#[cfg(target_os = "macos")]` to `const SAVE_MESSAGE` (or move it inside the macOS `run_dialog`).

---

### 5. [MINOR] Edge-case parser mismatches in `splitCommand` (`{{a},b}` brace expansion, quoted `\n`) and `globSyntaxError` (`[a-z-_]` false refusal, `[\]` inside `[...]`)

- **Location:** [rule-form.ts](app/src/chat/rule-form.ts#L160-L163), [rule-form.ts](app/src/chat/rule-form.ts#L218-L222), [rule-form.ts](app/src/chat/rule-form.ts#L258-L275)
- **Evidence:**
  1. **False refusal on `[a-z-_]` and false acceptance of `/a/[\]` in `globSyntaxError` ([rule-form.ts](app/src/chat/rule-form.ts#L258-L275)):**
     - After a range `[a-z]` completes (`inRange = false`), `last` remains `"a"`. In a class with a literal dash or second range after a range like `/a/[a-z-_]` or `/a/[a-z-0]`, `d === "-"` enters `inRange = true` again and compares `"_"` or `"0"` against `last = "a"`, falsely refusing `/a/[a-z-_]` (`"The range a-_ goes down"`) even though `globset` accepts it.
     - Inside `[...]`, `\` escapes the next character in `globset` (`backslash_escape: true`). `globSyntaxError` does not skip `\` inside `[...]`, so `/a/[\]` passes `globSyntaxError` (`]` closes `[`), whereas `globset` fails to build the glob.
  2. **Nested brace expansion `echo {{a},b}` and quoted `\n` in `splitCommand` ([rule-form.ts](app/src/chat/rule-form.ts#L178-L222)):**
     - `const close = line.indexOf("}", i)` stops at the first `}` instead of tracking brace depth as [shell.rs](crates/atrium-core/src/policy/shell.rs#L865-L890) does, so `echo {{a},b}` passes `splitCommand` while `shell.rs` flags `"brace expansion"`.
     - `\n` or `\r` inside quotes (`echo 'a\nb'`) is not caught by line 178, whereas [engine.rs](crates/atrium-core/src/policy/engine.rs#L606) never allows a command with `\n`.
- **Smallest fix:**
  - In `globSyntaxError`'s `[` loop: when `inRange` finishes, set `last = null` (and only set `inRange = true` when `d === "-" && last !== null && cs[k] !== "]"`), and handle `d === "\\"` inside `[...]` by consuming `cs[k++]` (returning the trailing-`\` error if `undefined`).
  - In `splitCommand`: check `if (/[\n\r]/.test(line)) return { ok: false, error: "A rule matches one line: remove the line break" };` up front, and scan `{...}` with a depth counter matching [shell.rs](crates/atrium-core/src/policy/shell.rs#L867-L890).

---

## Round-2 Verification Checklist

### 1. Round-1 Findings Status
| Round-1 Finding | Status | Notes |
|---|---|---|
| **Blocker 1:** Allow rules on shells, `eval`/`source`/`.`, wrappers, inline code, `find -exec` | **Partially fixed (see Blocker 1)** | `SHELLS` and `WRAPPERS` match `shell.rs` `SHELLS` and `unwrap_reading`, and `command_exact` is covered. Needs fixes for attached `-c'...'`/`-e'...'`, digit clusters (`perl -0e`), `deno -q eval`/`bun eval`, `pwsh`, case/APFS lookalikes (`Bash`, `/bin/baſh`), and `Object.prototype` keys. |
| **Blocker 2:** Path scope on commands and deny rules | **Partially fixed (see Major 3)** | `execute` removed from `PATH_KINDS`; `pathScope` dropped on command and deny rules. Still offered on `matcher: "any"` with `fetch`, `mcp`, and `other`. |
| **Major 3:** Linux GTK save dialog | **Fixed (with Minor 4)** | Uses `gtk::FileChooserNative::new(Some(title), None::<&gtk::Window>, gtk::FileChooserAction::Save, Some("_Save"), Some("_Cancel"))`, `set_do_overwrite_confirmation(false)`, `set_current_name(name)`, `dialog.run() == gtk::ResponseType::Accept`, `dialog.filename()`. All types and trait methods (`NativeDialogExt::run`, `FileChooserExt`) match `gtk = "0.18.2"` and free cleanly on drop. Only `const SAVE_MESSAGE` needs `#[cfg(target_os = "macos")]`. |
| **Minor 4:** `splitCommand` / `checkGlob` / broad warnings | **Fixed (with Minor 5)** | Word-initial `#`, control chars + U+2028/U+2029, empty command name, `RESERVED` first words, `broadGlob` (`/*`, `/**/*`, `~/**`), and `any` + `pathScope` warnings all work and have tests. See Minor 5 for `[a-z-_]` and `[\]`. |
| **Minor 5:** Export over an existing file | **Fixed** | `SavePick::at` uses `std::fs::symlink_metadata(&path).is_ok()` (catching regular files, dirs, and live or dangling symlinks). `pickNewPath` retries up to 5 times (`MAX_ASKS = 5`) with `: pick a new name` and toasts `EXISTS_TEXT`; TOCTOU races and bench mode hit `create_export_file`'s `create_new(true)` and map `AlreadyExists` to the same `EXISTS_TEXT` via `exportFailureText`. |
| **Minor 6:** Archive from a pop-out | **Fixed** | `confirmArchive` calls `if (archiveClosesPopout(threadId)) await bringBack(threadId);` (closing the pop-out from either window), and `MainWindow`'s `mainChatLeft` effect clears `mainThread` when a listed thread leaves `daemon().threads` without changing `screen()`. |
| **Minor 7:** Checkbox focus vs Inbox keys; module-level batch state | **Partially fixed (see Blocker 2)** | `selection`, `batchBusy`, and `batchResult` are now scoped to `InboxCard`, and `.batch-bar` + checkboxes forward `onKeyDown={onKey}`. However, `J`/`K` from a focused checkbox leaves DOM focus on the old checkbox, and `decide` falls back to `picked()` when `focused` is on another row. |
| **Nit 8:** `quoteArg` and `~` | **Fixed** | `~` and `~/…` stay unquoted; `~/a b` formats as `~/'a b'`; `~bob` and `a~b` stay single-quoted. |
| **Nit 9:** `X` with two approval cards | **Fixed (with Blocker 2)** | `xKeys` and `toggledGroup` toggle the focused card via `data-batch-key`, or all selectable approvals of the picked thread as a group. |

### 2. Assessment of the Handoff's Escalations (`docs/agents/handoffs/M1-23-ui-gaps.md` lines 176–179)
1. **Engine `path_scope` semantics (`crates/atrium-core/src/policy/engine.rs`):**
   - **Agree.** `all_paths` ([engine.rs](crates/atrium-core/src/policy/engine.rs#L34-L46)) pushes `a.start` (`~` in Home chats) and every non-flag word resolved against `cwd`, and `deny_matches` ([engine.rs](crates/atrium-core/src/policy/engine.rs#L91-L113)) uses `path_scope_ok` (`.all()`), which fails open on multi-path calls when one path is outside the scope.
   - **Severity for the M1 gate:** **Low (not a gate blocker once Major 3 above is fixed).** `"Always…"` on approval cards ([ledger.rs](crates/atrium-core/src/policy/ledger.rs#L200)) always sets `path_scope: None`, and once `pathScopeOffered` restricts `pathScope` to `PATH_KINDS` allow rules, no M1 UI path can create a broken `path_scope`. Fixing `engine.rs` (scoping commands by named paths only, and using `.any()` for `deny_matches` `path_scope`) can be done in M2 when folder/project workspaces land (or in a separate `atrium-core` task).
2. **Daemon-side `CreateRule` guard (`crates/atrium-daemon/src/commands.rs`):**
   - **Agree.** `create_rule` ([commands.rs](crates/atrium-daemon/src/commands.rs#L609-L632)) only checks `!argv.is_empty()`.
   - **Severity for the M1 gate:** **Low for the socket boundary, Medium for defense-in-depth.** Contract §4 already treats any same-UID process on the socket as a trusted peer (and `Settings → Security` discloses this). However, enforcing the shell/wrapper refusal in the daemon or `engine.rs` is good defense-in-depth so `shell.rs`'s invariant (*"rules never match as a prefix of `sh`"*) is upheld inside `atrium-core` itself.
3. **Engine `CommandPrefix` allow rules covering `find … -exec` / `-delete` (and `python3 -c` / `node -e`) (`crates/atrium-core/src/policy/engine.rs`):**
   - **Agree — this is the most significant of the three escalations.** Because `parts(a)` ([engine.rs](crates/atrium-core/src/policy/engine.rs#L79-L87)) only checks `!u.derived` units against `allow_covers`, a `CommandPrefix` allow rule on `find` or `find .` (which the UI permits with a warning) covers `find . -exec <cmd> \;` without checking `<cmd>` against allow rules, and a `CommandPrefix` allow rule on `python3` or `node` (which the UI also permits with a warning) covers `python3 -c <code_string>` because `["python3"]` is a prefix of `["python3", "-c", "..."]`.
   - **Severity for the M1 gate:** **Medium.** In `rule-form.ts`, the `find` warning explicitly mentions `-exec` and `-delete`, whereas the interpreter warning ([rule-form.ts](app/src/chat/rule-form.ts#L132)) says *"This allows every script python3 is given…"* without mentioning `-c`/`-e` inline code (or, even safer, `rule-form.ts` can refuse `command_prefix` allow rules on interpreters/runners unless a script/package argument is included, or update the warning to mention inline code until `engine.rs` checks derived `-exec` / inline-code units).

---

## Commands Run

| Command | Result |
|---|---|
| `pnpm --dir app typecheck` | Passed (`tsc --noEmit`, 0 errors) |
| `pnpm --dir app test` | Passed (`19` test files, `338` tests passed in `1.69s`) |
| `pnpm --dir app build` | Passed (`vite v8.3.5`, built in `1.81s`; lazy chunks verified) |
| `cargo clippy -p atrium-app --all-targets -- -D warnings` | Passed (0 warnings on macOS) |
| `cargo test -p atrium-app --no-fail-fast` | Passed (`47` unit tests + `2` `delivery_shapes` integration tests passed) |
| `cargo fmt --all --check` | Passed (clean) |
| `git status --porcelain` | Clean (no untracked or modified files) |
