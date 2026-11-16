# Review: `M1-23-ui-gaps` (`agent/opus/M1-23-ui-gaps`, `789ae35a..eb5997ec` on `4bf9874e`)

> Saved by the orchestrator from the reviewer's report (cross-provider: Argon reviewing Opus work, D-109). Links are relative to the worktree. The orchestrator spot-checked blockers 1 and 2 against `engine.rs` and `shell.rs` and confirmed both.


- **Reviewer:** Gemini 4 Argon (High), read-only
- **Worker:** Claude Opus 5.5
- **Verdict:** **Rework** (2 blockers in the rules editor's interaction with the policy engine, 1 major in the Linux GTK save dialog, 4 minors, 2 nits)

---

## Findings (Ordered by Severity; Safety Blockers First)

### 1. [BLOCKER] Rules editor allows `CommandPrefix` allow rules on shells/runners (`bash -c`, `sh -c`, `bash`, `env`, …) that bypass simple-command checks on payloads — and skips the runner warning whenever `argv.length > 1`

- **Location:** [rule-form.ts](app/src/chat/rule-form.ts#L59) and [rule-form.ts](app/src/chat/rule-form.ts#L191-L194) (interacting with [shell.rs](crates/atrium-core/src/policy/shell.rs#L1153-L1155) and [engine.rs](crates/atrium-core/src/policy/engine.rs#L79-L87), [engine.rs](crates/atrium-core/src/policy/engine.rs#L139-L149))
- **Evidence:**
  1. In `atrium-core`, [shell.rs](crates/atrium-core/src/policy/shell.rs#L1143-L1156) lexes `sh -c` / `bash -c` / `eval` payloads into `payload_units` marked `u.derived = true` and **discards `l.reasons`** from the payload under the explicit M1-03 assumption that prefix rules never match `sh`:
     ```rust
     // reasons of a payload don't matter: the call is `sh -c …`, which
     // rules never match as a prefix of `sh`, and the hard-deny list
     // reads the units.
     ```
     And in [engine.rs](crates/atrium-core/src/policy/engine.rs#L79-L87) and [engine.rs](crates/atrium-core/src/policy/engine.rs#L139-L149), `allow_covers` for `CommandPrefix` checks only `part_ids` (`!u.derived`), i.e. the outer unit `["bash", "-c", "<payload>"]` whose top-level `a.reasons` is empty.
  2. In [RuleEditor.tsx](app/src/ui/RuleEditor.tsx#L106), the UI tells the user: *"Matches a simple command whose words start with these, in order."* And [rule-form.ts](app/src/chat/rule-form.ts#L66-L67) states: *"Anything that makes a command not simple (operators, redirects, `$`, backquotes, unquoted globs, brace expansion, a leading `NAME=value`) is refused: no rule would ever match it."*
  3. However, [rule-form.ts](app/src/chat/rule-form.ts#L192-L194) allows creating a `command_prefix` Allow rule on `sh`, `bash`, `bash -c`, `sh -c`, `eval`, `env`, `python3 -c`, `node -e`, etc., and only warns when `split.argv.length === 1`:
     ```ts
     if (form.decision === "allow" && form.matcher === "command_prefix" && split.argv.length === 1 && RUNNERS.has(split.argv[0]!.split("/").pop()!)) {
       warnings.push(`This allows every command run through ${split.argv[0]}, whatever follows it.`);
     }
     ```
     - If the user enters `bash -c`, `sh -c`, `zsh -c`, `python3 -c`, `node -e`, `env -i`, `nice -n 10`, or `npx -y`, `split.argv.length` is `>= 2`, so **no warning at all is shown**.
     - Once a `CommandPrefix` Allow rule for `bash` or `bash -c` (or `sh`, `sh -c`, `eval`, `source`, `.`, `env`, `command`, `exec`, `nohup`, `time`, `nice`) exists, `DefaultPolicy` auto-allows `bash -c "cmd1 && cmd2 > /tmp/out; echo $VAR *.txt"` (compound commands, file redirects, `$` expansions, and globs inside the payload, as long as not in `HARD_DENY_DEFAULTS`) without checking the inner payload commands against rules or `reasons`.
- **Smallest fix:**
  - In [rule-form.ts](app/src/chat/rule-form.ts#L59) and [rule-form.ts](app/src/chat/rule-form.ts#L188-L196): refuse `command_prefix` Allow rules whose leading command (after stripping wrapper tokens in `RUNNERS`) is a shell/eval/wrapper (`sh`, `bash`, `zsh`, `dash`, `ksh`, `fish`, `eval`, `source`, `.`, `env`, `exec`, `command`, `nohup`, `time`, `nice`, `xargs`, `sudo`, `doas`) or an interpreter with a code flag (`python* -c`, `node -e`/`--eval`, `ruby -e`, `perl -e`, `osascript -e`, `find … -exec`), so the UI never creates a rule that violates [shell.rs](crates/atrium-core/src/policy/shell.rs#L1153-L1155)'s invariant. For remaining `RUNNERS` (e.g. `python3`, `node`, `npx`), warn regardless of `split.argv.length` when no script path is pinned.

---

### 2. [BLOCKER] `Deny` rules (and `Allow` rules) on commands with `pathScope` (`"Only for paths inside"`) — and `PathGlob` rules with `Kind of call = Commands` — fail to match in Home chats

- **Location:** [rule-form.ts](app/src/chat/rule-form.ts#L47), [rule-form.ts](app/src/chat/rule-form.ts#L220-L225), [RuleEditor.tsx](app/src/ui/RuleEditor.tsx#L96-L98), [RuleEditor.tsx](app/src/ui/RuleEditor.tsx#L113), [rule-form.test.ts](app/src/chat/rule-form.test.ts#L52-L55), [gap-shots.ts](app/src/gap-shots.ts#L135-L136) (vs [engine.rs](crates/atrium-core/src/policy/engine.rs#L34-L46), [engine.rs](crates/atrium-core/src/policy/engine.rs#L64-L69), [engine.rs](crates/atrium-core/src/policy/engine.rs#L90-L113), [engine.rs](crates/atrium-core/src/policy/engine.rs#L163-L168), and [words.rs](crates/atrium-core/src/policy/words.rs#L508-L552))
- **Evidence:**
  1. Look at the exact rule tested in [rule-form.test.ts](app/src/chat/rule-form.test.ts#L52-L55):
     ```ts
     validateRule(form({ decision: "deny", matcher: "command_exact", command: "git push", scope: "home", pathScope: "~/work/**" }), ctx)
     // preview: "Deny this exact command git push · In every Home chat, only for paths inside /Users/me/work/**"
     ```
     And the rule showcased in [gap-shots.ts](app/src/gap-shots.ts#L135-L136) / [settings-rule-editor-light.png](docs/agents/handoffs/M1-23-ui-gaps/settings-rule-editor-light.png):
     ```
     Allow commands that start with cargo test … · In every chat, only for paths inside /home/you/projects/**
     ```
  2. In `atrium-core`, [engine.rs](crates/atrium-core/src/policy/engine.rs#L34-L46) computes `all_paths(input, a)` for every command by appending `a.start.clone()` (line 43) plus every non-flag word resolved against `cwd` ([words.rs](crates/atrium-core/src/policy/words.rs#L540), so `"push"` in `git push` becomes `<cwd>/push` and `"test"` in `cargo test` becomes `<cwd>/test`).
  3. In M1, every thread is in Home (`input.cwd = ~`), and Claude Code's `Bash` subject always has `cwd: None` ([claude/tools.rs](crates/atrium-agents/src/claude/tools.rs#L67)), so `a.start` is always `/Users/me`.
  4. Both `deny_matches` ([engine.rs](crates/atrium-core/src/policy/engine.rs#L90-L113)) and `allow_covers` ([engine.rs](crates/atrium-core/src/policy/engine.rs#L134-L136), [engine.rs](crates/atrium-core/src/policy/engine.rs#L163-L168)) require `paths.iter().all(|p| glob_matches(g, p))`. Because `paths` always includes `a.start` (`/Users/me`), and `/Users/me` does not match `/Users/me/work/**` or `/home/you/projects/**`:
     - The **Deny** rule `Deny git push` with `pathScope: "~/work/**"` **never denies** `cd ~/work && git push` or `git -C ~/work push` in Home (and a `Deny` rule on file tools with `pathScope: "~/work/**"` also fails to deny a multi-path call if one path is in `~/work/**` and another is outside, because `deny_matches` uses `path_scope_ok` with `.all()`).
     - The **Allow** rule `Allow cargo test` with `pathScope: "~/projects/**"` (and any `PathGlob` rule with `toolKind = "execute"`, which [rule-form.ts](app/src/chat/rule-form.ts#L47) includes in `PATH_KINDS`) **never matches any command in Home**.
- **Smallest fix:**
  - In `app/src/` (owned paths):
    1. Remove `"execute"` from `PATH_KINDS` in [rule-form.ts](app/src/chat/rule-form.ts#L47).
    2. In [RuleEditor.tsx](app/src/ui/RuleEditor.tsx#L113) and [rule-form.ts](app/src/chat/rule-form.ts#L220-L225), do not offer `pathScope` on command rules (`command_prefix` / `command_exact`) or `toolKind === "execute"`, and do not offer `pathScope` on `decision === "deny"` rules (for a deny on a path glob, `matcher: "path_glob"` uses `paths.iter().any(...)` in [engine.rs](crates/atrium-core/src/policy/engine.rs#L110) and works as expected when `path_scope` is `None`).
    3. Escalate the `engine.rs` `all_paths` / `deny_matches` `path_scope_ok` limitation in `docs/agents/handoffs/M1-23-ui-gaps.md` per the brief's escalation rule.

---

### 3. [MAJOR] Linux GTK save dialog calls `dialog.close()` after `dialog.run()`, which leaves the `GtkFileChooserDialog` visible and leaked

- **Location:** [save.rs](app/src-tauri/src/save.rs#L80-L103)
- **Evidence:**
  ```rust
  let response = dialog.run();
  let path = if response == gtk::ResponseType::Accept {
      dialog.filename()
  } else {
      None
  };
  dialog.close();
  ```
  In GTK 3 (`gtk = "0.18.2"`):
  - `GtkDialog` installs `gtk_dialog_delete_event_handler` on `"delete-event"`, which emits `GTK_RESPONSE_DELETE_EVENT` and returns `TRUE` (`GDK_EVENT_STOP` — *"Do not destroy"*).
  - While `dialog.run()` (`gtk_dialog_run`) is active, it connects temporary `"response"` and `"delete-event"` handlers to exit the nested main loop, then **disconnects** them before returning — without hiding or destroying the dialog widget.
  - Calling `dialog.close()` (`gtk_window_close`) after `dialog.run()` has returned synthesizes a `delete-event` that hits `gtk_dialog_delete_event_handler` and stops (`return TRUE`) before `GtkWindow` can hide or destroy the window. Dropping the Rust `dialog` wrapper only decrements the GObject refcount while GTK's global toplevel list still owns the visible window.
- **Smallest fix:**
  Use `gtk::FileChooserNative::new(Some(title), None::<&gtk::Window>, gtk::FileChooserAction::Save, Some("_Save"), Some("_Cancel"))` (whose `run()` hides the dialog automatically and needs no `unsafe { dialog.destroy() }` or toplevel cleanup):
  ```rust
  let dialog = gtk::FileChooserNative::new(
      Some(title),
      None::<&gtk::Window>,
      gtk::FileChooserAction::Save,
      Some("_Save"),
      Some("_Cancel"),
  );
  dialog.set_current_name(name);
  if dialog.run() == gtk::ResponseType::Accept {
      dialog.filename().map(|path| path.to_string_lossy().into_owned())
  } else {
      None
  }
  ```

---

### 4. [MINOR] `splitCommand` and `checkGlob` accept inputs that `atrium-core` never matches (including on `Deny` rules), and `validateRule` misses warnings on `/**/*`, `/*`, and `any` + `pathScope`

- **Location:** [rule-form.ts](app/src/chat/rule-form.ts#L69-L175), [rule-form.ts](app/src/chat/rule-form.ts#L204-L214)
- **Evidence:**
  1. **Unquoted `#`, control chars, reserved words, and `""` in `splitCommand`:**
     - In [shell.rs](crates/atrium-core/src/policy/shell.rs#L650-L655), an unquoted `#` at the start of a word begins a comment (`cargo test # note` → `argv = ["cargo", "test"]`). `splitCommand` treats `#` as a word character (`argv = ["cargo", "test", "#", "note"]`), creating a rule (including a `Deny` rule) that never matches.
     - [shell.rs](crates/atrium-core/src/policy/shell.rs#L409-L413) rejects control characters and `\u{2028}`/`\u{2029}`, and [shell.rs](crates/atrium-core/src/policy/shell.rs#L1086-L1090) rejects first words in `RESERVED` (`!`, `{`, `[[`, `if`, `for`, `while`, …). `splitCommand` accepts them, as well as `""` / `''` (`argv: [""]`).
  2. **Invalid `globset` bracket syntax in `checkGlob`:**
     - [rule-form.ts](app/src/chat/rule-form.ts#L173) only checks `(abs.match(/\[/g) ?? []).length !== (abs.match(/\]/g) ?? []).length`. Patterns like `/Users/me/][`, `/Users/me/[]`, or a trailing `\` pass `checkGlob` and `create_rule`, then fail `GlobBuilder::new(glob).build()` in [engine.rs](crates/atrium-core/src/policy/engine.rs#L25-L30) and silently evaluate to `false` (even on a `Deny` rule).
  3. **Broad-glob and `any` warnings:**
     - [rule-form.ts](app/src/chat/rule-form.ts#L204) only checks `/^\/(\*\*)?\/?$/`, missing `/**/*` and `/*` (and `~/**`).
     - [rule-form.ts](app/src/chat/rule-form.ts#L211) suppresses the `matcher: "any"` warning whenever `form.pathScope.trim()` is non-empty (even `pathScope: "/**"`).
- **Smallest fix:**
  - In `splitCommand`, reject word-initial `#`, control characters / `\u2028` / `\u2029`, `argv[0] === ""`, and `RESERVED` first words.
  - In `checkGlob`, validate that every `[` is closed by a later `]` with at least one character inside and no trailing unescaped `\`.
  - In `validateRule`, warn on `/^\/(\*\*(\/\*)?|\*)?\/?$/` (and on `pathScope` matching root), and keep the `any` warning when `pathScope` is set (`"This allows every command touching that folder without asking."`).

---

### 5. [MINOR] Native save dialog asks "Replace?" on existing files, then `export_thread` fails with `AlreadyExists`

- **Location:** [save.rs](app/src-tauri/src/save.rs#L67-L77), [save.rs](app/src-tauri/src/save.rs#L93), [thread-actions.ts](app/src/chat/thread-actions.ts#L89-L106)
- **Evidence:**
  - Contract §13 specifies that `history::create_export_file` creates new files only (`create_new(true)`).
  - When a user picks an existing file in `NSSavePanel` (macOS) or `gtk::FileChooserDialog` (Linux, where line 93 explicitly sets `dialog.set_do_overwrite_confirmation(true)`), the OS dialog prompts *"Do you want to replace it?"*. After the user clicks *"Replace"*, the daemon rejects the export with `"Couldn't export the chat: something already exists at the export path"`.
- **Smallest fix:**
  - Remove `dialog.set_do_overwrite_confirmation(true)` in [save.rs](app/src-tauri/src/save.rs#L93), and in [save.rs](app/src-tauri/src/save.rs#L52-L55) (or [thread-actions.ts](app/src/chat/thread-actions.ts#L102-L105)), if the chosen path already exists (`std::fs::symlink_metadata(&path).is_ok()`), either re-open the save dialog with a message or toast `"Export never overwrites an existing file: pick a new file name"`.

---

### 6. [MINOR] Archiving a popped-out chat leaves the pop-out window open on a dead view (and from a pop-out, does not clear `mainThread`)

- **Location:** [ThreadActions.tsx](app/src/ui/ThreadActions.tsx#L37-L51), [Shell.tsx](app/src/ui/Shell.tsx#L103), [windows.ts](app/src/windows.ts#L261-L269)
- **Evidence:**
  ```ts
  if (await archiveThread(threadId)) {
    // The main card doesn't keep showing a chat that left the list.
    if (model().mainThread === threadId) openThread(null);
  }
  ```
  - `PopoutWindow` ([Shell.tsx](app/src/ui/Shell.tsx#L103)) and `Palette` ([Palette.tsx](app/src/ui/Palette.tsx#L37)) offer `Archive…` in pop-out windows, and `ThreadCard` offers it in the main window while a chat is popped out.
  - When archived from a pop-out window, `setMainThread(null)` in [windows.ts](app/src/windows.ts#L262) returns early (`if (isPopout() || ...) return;`), and `bringBack(threadId)` is never called, leaving the pop-out window open showing `"This chat is no longer here"` with a Home composer inside the pop-out.
- **Smallest fix:**
  In [ThreadActions.tsx](app/src/ui/ThreadActions.tsx#L47-L50), call `if (!ownsView(threadId) || isPopout()) await bringBack(threadId);` before `if (model().mainThread === threadId) openThread(null);`.

---

### 7. [MINOR] Clicking any batch checkbox in the Inbox disables `J`/`K`/`A`/`D`/`X` keyboard shortcuts, and batch state is stored at module scope

- **Location:** [InboxCard.tsx](app/src/ui/InboxCard.tsx#L48-L55), [InboxCard.tsx](app/src/ui/InboxCard.tsx#L180)
- **Evidence:**
  1. [InboxCard.tsx](app/src/ui/InboxCard.tsx#L180) ignores keydowns when `e.target instanceof HTMLInputElement`:
     ```ts
     if (e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
     ```
     Because the batch checkboxes (`[data-atrium-part="batch-all"]` and `[data-atrium-part="batch-pick"]`) are `<input type="checkbox">` (`HTMLInputElement`), clicking a checkbox focuses it and stops `J`, `K`, `A`, `D`, `X`, and `Enter` from working until focus moves off the checkbox.
  2. `selection`, `batchBusy`, and `batchResult` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L49-L51)) are module-level signals that are not cleared when `InboxCard` unmounts, so a stale `batchResult` banner remains visible when navigating away from Inbox and returning later.
- **Smallest fix:**
  Change the `HTMLInputElement` guard on line 180 to `(e.target instanceof HTMLInputElement && e.target.type !== "checkbox")`, and clear `setBatchResult(null)` and `setSelection(new Set())` in `onCleanup` inside `InboxCard`.

---

### 8. [NIT] `quoteArg` single-quotes `~` / `~/…` in the rule preview and rules list

- **Location:** [rules.ts](app/src/chat/rules.ts#L20-L23)
- **Evidence:**
  `splitCommand("ls ~/src")` produces `argv: ["ls", "~/src"]` (unquoted tilde, expanded by the policy engine). Because `~` is not in `/^[\w@%+=:,./-]+$/`, `quoteArg("~/src")` formats it in the preview and rules list as `'~/src'` (which in shell syntax means a literal directory named `~`, not home).
- **Smallest fix:**
  In `quoteArg`, allow a leading `~` or `~/` before `[\w@%+=:,./-]+` (`if (arg !== "" && /^(~(\/|$))?[\w@%+=:,./-]*$/.test(arg)) return arg;`).

---

### 9. [NIT] `X` shortcut in the Inbox only toggles the first approval of a thread when a thread has 2 pending approval cards

- **Location:** [InboxCard.tsx](app/src/ui/InboxCard.tsx#L189)
- **Evidence:**
  `const t = targets().find((x) => x.threadId === id);` always picks the first `BatchTarget` for `id`, even when `MAX_CARDS` (`2`) approval cards are shown for that thread.
- **Smallest fix:**
  If `e.target` is inside a `[data-atrium-part="approval"]` card, toggle that card's `batchKey`; otherwise toggle all offered targets of the picked thread (or the first).

---

## Checklist Verification & Evidence

### 1. Brief Items & M1-20 Gate Audit Gaps (`majors 1–5`, `minor 1`, `missing` UI rows)
| Item / Audit Gap | Status | Evidence |
|---|---|---|
| **1 / Major 1 & 3:** Thread actions — Rename, Archive, Export (MD & JSONL via native save dialog), End session (`Shutdown`) in thread header & Palette | Delivered (with Minor 5 & 6) | [thread-actions.ts](app/src/chat/thread-actions.ts#L69-L128), [ThreadActions.tsx](app/src/ui/ThreadActions.tsx#L25-L122), [ThreadCard.tsx](app/src/ui/ThreadCard.tsx#L33-L62), [Shell.tsx](app/src/ui/Shell.tsx#L103), [Palette.tsx](app/src/ui/Palette.tsx#L44). Errors toast via `showToast`. |
| **2 / Major 5:** Approval queue batch approve & deny | Delivered (with Minor 7) | [batch.ts](app/src/chat/batch.ts#L1-L69), [InboxCard.tsx](app/src/ui/InboxCard.tsx#L118-L157), [InboxCard.tsx](app/src/ui/InboxCard.tsx#L200-L223), [RequestCard.tsx](app/src/ui/RequestCard.tsx#L74-L78). |
| **3 / Minor 1:** `⌘⇧J` jumps to previous item | Delivered | [App.tsx](app/src/App.tsx#L20), [jump.ts](app/src/ui/jump.ts#L34-L36), [inbox.ts](app/src/chat/inbox.ts#L58-L75), [Palette.tsx](app/src/ui/Palette.tsx#L51). |
| **4 / Major 2a:** Rules editor in Settings → Approvals (`CreateRule`) | Needs fixes (Blockers 1–2, Minor 4) | [RuleEditor.tsx](app/src/ui/RuleEditor.tsx#L43-L140), [rule-form.ts](app/src/chat/rule-form.ts#L1-L237). |
| **5 / Major 2b & §1.1 row 5:** Settings → Security disclosures (`HARD_DENY_DEFAULTS`, `git config` limit, same-UID peer boundary, CR-12 spot) | Delivered | [Security.tsx](app/src/ui/Security.tsx#L1-L107), [Settings.tsx](app/src/ui/Settings.tsx#L146-L153), [save.rs](app/src-tauri/src/save.rs#L28-L41). |
| **6 / Major 4:** Enforcement "details" from thread header, Context panel, Settings → Agents | Delivered | [Enforcement.tsx](app/src/ui/Enforcement.tsx#L24-L70), [EnforcementDetails.tsx](app/src/ui/EnforcementDetails.tsx#L13-L74), [enforcement.ts](app/src/chat/enforcement.ts#L1-L96), [ContextPanel.tsx](app/src/ui/ContextPanel.tsx#L104), [Settings.tsx](app/src/ui/Settings.tsx#L180). |
| **7:** Demo & docs (`m1-demo.ts`, `m1_demo.py`, `M1-demo.md`, screenshots) | Delivered | [m1-demo.ts](app/src/m1-demo.ts#L390-L681), [m1_demo.py](app/scripts/m1_demo.py#L13-L25), [M1-demo.md](docs/demos/M1-demo.md#L42-L72), 22 light/dark PNGs in `docs/agents/handoffs/M1-23-ui-gaps/`. |

### 2. Safety Checks
- **Batch approve/deny:**
  - *Can it allow something the user didn't see or select?* **No.** `targets()` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L119-L134)) only includes items currently in `flat()` (respecting the active filter), inside `withCards()` (the first `MAX_HELD = 12` rendered items), within `pending().slice(0, MAX_CARDS)` (the `MAX_CARDS = 2` rendered cards per thread), with `r.kind === "approval"` and `canAct(cardState(r, localOf(id, r))) === true` (`fresh` or `failed`, never `stale`, `sending`, `sent`, or `resolved`). `batchKey` binds `threadId:incarnation:requestId`, and a `createEffect` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L138-L143)) prunes any key that leaves `offered()`.
  - *Does each go through `ResolveApproval` separately, and are failures shown?* **Yes.** `runBatch` ([InboxCard.tsx](app/src/ui/InboxCard.tsx#L151-L152)) calls `resolveApproval(t.threadId, t.request, action)` per item via `Promise.all` and renders per-item failures (`{chat}: {subject}: {reason}`) in `[data-atrium-part="batch-result"]` as well as on each `RequestCard`.
  - *Is there any batch "Always"?* **No.** `resolveApproval` is called with default `remember = null`, and only `Allow once` and `Deny` buttons exist in the batch bar.
- **Rules editor:**
  - Cannot create an `Any` rule by accident (requires selecting `"Every call of one kind"` in the dropdown; empty command or empty glob fails validation).
  - However, see **Blocker 1** (`CommandPrefix` allow rules on `bash`/`bash -c`/`sh -c`/`eval`/`env` bypass simple-command checks on payloads), **Blocker 2** (`pathScope` on commands and on `Deny` rules fails to match in Home), and **Minor 4** (`#` comments, reserved words, and malformed `[...]` globs).
- **Settings → Security:**
  - `safety_info()` ([save.rs](app/src-tauri/src/save.rs#L28-L41)) reads directly from `atrium_protocol::policy::HARD_DENY_DEFAULTS` (verified by unit test `save::tests::safety_info_lists_every_hard_deny_default_in_order`).
  - Heading is `"Always blocked (for calls Atrium sees)"` ([Settings.tsx](app/src/ui/Settings.tsx#L149)), and both the row hint and the footer note in [Security.tsx](app/src/ui/Security.tsx#L41) explicitly state that it applies only to calls Atrium sees and is not a machine-wide ban or containment of allowed code.
  - Known limits cover `git config` ([Security.tsx](app/src/ui/Security.tsx#L58-L61)), the same-UID peer boundary ([Security.tsx](app/src/ui/Security.tsx#L63-L66)), and lack of isolation ([Security.tsx](app/src/ui/Security.tsx#L68-L71)), with `CredentialStoresThatAsk` ([Security.tsx](app/src/ui/Security.tsx#L46-L54)) marking the spot for CR-12.
- **End session and Archive confirm first:**
  - Verified in [ThreadActions.tsx](app/src/ui/ThreadActions.tsx#L25-L51) (`confirmEndSession` and `confirmArchive` both await `askConfirm` from [Confirm.tsx](app/src/ui/Confirm.tsx#L19-L22), shared by both the header `ThreadMenu` and `Palette`).

### 3. Native Code in `app/src-tauri`
- Zero `unsafe` blocks; `#![deny(unsafe_code)]` via workspace lints holds.
- On macOS ([save.rs](app/src-tauri/src/save.rs#L52-L78)), `ask_save_path` runs `run_dialog` on the main thread via `app.run_on_main_thread`, acquires `MainThreadMarker::new()?`, and runs `NSSavePanel::savePanel(mtm)` modally.
- On Linux ([save.rs](app/src-tauri/src/save.rs#L80-L103)), see **Major 3** (`dialog.close()` after `dialog.run()` does not hide or destroy `GtkFileChooserDialog`) and **Minor 5** (`set_do_overwrite_confirmation(true)` vs `create_export_file` new-file-only).

### 4. Performance
- `RuleEditor` (11.14 kB), `EnforcementDetails` (5.06 kB), `Security` (2.99 kB), `m1-demo` (22.68 kB), and `gap-shots` (5.55 kB) all split into lazy chunks and do not load at startup; `safety_info` IPC is deferred until `Security` or `RuleEditor` is opened.
- Nothing was added to the chat transcript or per-frame streaming path (`Transcript.tsx`, `chat/transcript.ts`, `chat/threads.ts`, and `daemon/reduce.ts` are untouched).

### 5. Tests & Demo
- The 53 new Vitest assertions across 6 test files (`batch.test.ts`, `enforcement.test.ts`, `inbox.test.ts`, `jump.test.ts`, `rule-form.test.ts`, `thread-actions.test.ts`), the new Rust unit test in `save.rs`, and the new steps in `m1-demo.ts` / `m1_demo.py` (5b, 7b, 9a, 9b/c/d, 10, 11, 12a, 12b, 13) exercise real reducer/DOM/daemon behavior rather than mocks (except `jump.test.ts`, which unit-tests `jump.ts` routing against a mocked state store while `inbox.test.ts` and `m1-demo.ts` step 5b test the pure function and end-to-end keybinding).

---

## Commands Run

| Command | Result |
|---|---|
| `pnpm --dir app typecheck` | Passed (`tsc --noEmit`, 0 errors) |
| `pnpm --dir app test` | Passed (`19` test files, `317` tests passed in `1.68s`) |
| `pnpm --dir app build` | Passed (`vite v8.3.5`, built in `1.86s`; lazy chunks verified) |
| `cargo clippy -p atrium-app --all-targets -- -D warnings` | Passed (0 warnings) |
| `cargo test -p atrium-app --no-fail-fast` | Passed (`46` unit tests + `2` `delivery_shapes` integration tests passed) |
| `cargo fmt --all --check` | Passed (clean) |
| `git status --porcelain` | Clean (no untracked or modified files) |
