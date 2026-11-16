# Handoff: M1-23-ui-gaps

Branch: `agent/opus/M1-23-ui-gaps`, cut from `dev` at `4bf9874e`. Worker: Claude Opus 5.5.

## Commits

| Commit | What |
|---|---|
| `789ae35a` | ⌘⇧J jumps to the previous item that needs you |
| `a029eb5b` | Thread actions (rename, export, end session, archive) and enforcement details; `ask_save_path` and `safety_info` in src-tauri |
| `c7ead224` | Rules editor, Settings → Security disclosures, enforcement details from Settings → Agents |
| `a8d385a2` | Batch approve and deny in the inbox's approval queue |
| `4c348f88` | Demo covers the new UI; `docs/demos/M1-demo.md` updated |
| `fc7d875d` | Screenshots of the new surfaces (light and dark) |
| (this commit) | This handoff |

## What was added, item by item

### 1. Thread actions
- **Logic:** `app/src/chat/thread-actions.ts`, tested in `thread-actions.test.ts`:
  - `MAX_TITLE_CHARS` = 120, the same as the daemon;
  - `cleanTitle`;
  - `exportFileName`, which sanitizes the title and falls back to "chat";
  - `hasLiveSession`;
  - the actions `renameThread`, `exportThread`, `endSession` and `archiveThread`. Errors show as toasts.
- **UI:** `app/src/ui/ThreadActions.tsx`:
  - `threadActions(t)` is one list shared by the header ⋯ menu (`ThreadMenu`) and the palette.
  - The menu has Rename, Export as Markdown…, Export as JSONL…, End session (disabled when no session is live) and Archive.
  - It is a small hand-written popover. It handles Escape, the arrow keys and outside clicks.
- **Confirmations:** `app/src/ui/Confirm.tsx` provides `askConfirm()` on the existing Kobalte Dialog. Archive and End session ask first.
- **Where the menu appears:**
  - The thread header (`ThreadCard.tsx`). Rename edits the title inline: Enter saves and Escape cancels.
  - The pop-out window bar (`Shell.tsx`).
  - The palette, as a "This chat" section (`Palette.tsx`).
- **Native save dialog:** `app/src-tauri/src/save.rs` adds the command `ask_save_path`.
  - On macOS it uses `NSSavePanel` through the `objc2-app-kit` crate that was already a dependency. Only features were added; there are no new crates and `Cargo.lock` is unchanged.
  - On Linux it uses a GTK `FileChooserDialog` in Save mode.
  - It runs on the main thread and contains no `unsafe` code.
  - `export_thread` then writes the file and the UI toasts "Exported … to <path>".

### 2. Batch approve and deny
- **Logic:** `app/src/chat/batch.ts`, tested in `batch.test.ts`. It handles selection, select all, pruning answered requests, and an outcome per item.
- **Card:** `RequestCard.tsx` shows a checkbox on each approval card that can act.
- **Inbox:** `InboxCard.tsx` has a batch bar with Select all (with an indeterminate state), Allow and Deny.
  - Each approval still goes alone through `resolve_approval`, sent in parallel.
  - The result reads like "Allowed 2. 1 wasn't allowed:" and lists each failure with its chat, its command and the reason (expired, unknown outcome, or the daemon's message).
  - The X key toggles the picked card.
- There is no batch "Always": rules are made one at a time.

### 3. ⌘⇧J
- `prevJump` is in `app/src/chat/inbox.ts`, sharing `jump()` with `nextJump`, with tests.
- `jumpToPrev` is in `app/src/ui/jump.ts`, with tests in the new `jump.test.ts`.
- It is bound in `App.tsx`, listed in the palette and hinted in the inbox footer.

### 4. Rules editor (CreateRule)
- **Validation:** `app/src/chat/rule-form.ts`, with 13 tests in `rule-form.test.ts`.
  - **Matcher types:** command prefix, exact command, path glob (by kind), or every call of one kind.
  - **Options:** allow or deny; every chat, Home or one chat; an optional tool name and path scope.
  - **Refused, with a message:**
    - shell operators, `$` and backquotes;
    - unquoted globs and brace expansion;
    - a leading `NAME=`;
    - `~name`;
    - relative paths, `.` and `..`;
    - unbalanced brackets.
  - **Expanded:** `~/` is expanded with the user's home, because the policy engine matches absolute paths and does no `~` expansion.
  - **Warnings:** runner programs (for example `bash`, `python`, `npx`), allow-any rules and root globs.
- **UI:** `app/src/ui/RuleEditor.tsx`, lazy-loaded.
  - It opens from "Add a rule…" in Settings → Approvals.
  - It shows a live preview in the same wording as the rules list (`ruleText`).
  - It sends one `create_rule`.

### 5. Safety disclosures
- `app/src/ui/Security.tsx`, lazy-loaded, is a new **Settings → Security** section. Contract §4 names "Settings → Security", so the disclosures go there rather than under Approvals.
- **"Always blocked (for calls Atrium sees)"** is built from the real `HARD_DENY_DEFAULTS`. The new command `safety_info` in `save.rs` exposes them along with the home folder, which the rule editor needs.
- **Known limits:**
  - git config can run programs;
  - processes running as the same user are trusted peers;
  - Full enforcement is not isolation.
- **CR-12 marked spot:** `CredentialStoresThatAsk` in `Security.tsx`. It renders nothing yet, and a comment says where the CR-12 list goes.

### 6. Enforcement details
- **Logic:** `app/src/chat/enforcement.ts`, with tests. It covers coverage per tool class (the weakest entry wins and a missing class is "Not verified"), the level and version lines, and config sources.
- **Opening it:** `app/src/ui/Enforcement.tsx` provides the badge (`chip.enf`) in the thread header and a "Details" link in the Context panel and in Settings → Agents.
- **Dialog:** `app/src/ui/EnforcementDetails.tsx`, lazy-loaded.
  - It shows the level, the CLI version (tested or not), the reasons and the coverage grid with evidence, then config sources.
  - Policy and Isolation are separate sections, never one score.
  - It links to Settings → Security.

### 7. Demo
- **`app/src/m1-demo.ts`** gains these steps:
  - 5b: ⌘⇧J;
  - 7b: End session through the menu, confirmed;
  - 9a: rename;
  - 9b: export both formats through the menu;
  - 10: archive (cancel, then confirm);
  - 11: batch;
  - 12a: rules editor;
  - 12b: Security;
  - 13: enforcement details.
- **Export in the demo:** it goes through the real menu and the real `export_thread`. Only the native dialog is replaced, by a fixed folder in the demo home (`setSaveDirOverride`).
- **`app/scripts/m1_demo.py`:** the export file checks are renamed 9c/9d and the N1 daemon test is 7d.
- **`docs/demos/M1-demo.md`:** updated for every new surface.

## Definition of done

- **Front end (`app/`):**
  - `pnpm typecheck`: clean.
  - `pnpm test`: 19 files, 317 tests passed.
  - `pnpm build`: ok.
- **Rust:**
  - `cargo fmt --all --check`: clean.
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean.
  - `cargo test --workspace --no-fail-fast`: 719 passed, 0 failed.
- **Demo:** `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 app/scripts/m1_demo.py` ran 3 times in a row and each run passed **27/27**.
- **Startup and idle memory:** `startup_bench.py` with the `perf` profile, run under the GUI lock. The method is from `docs/spikes/M1-macos-checks.md`; the baseline is that doc's run at `a688f5c`.
  - **Idle memory (one window, 9 launches): pass.**
    - Per-launch medians were 86.8–93.8 MiB and the worst sample was 97.0 MiB, against a gate of 150 MB.
    - The baseline was 84.6–91.5 MiB, worst 92.9 MiB.
  - **TTI, first full run: over the gate**, at median 808 / p90 982 ms. This run is not valid: the 1-minute load average was **72.8** at the start, because M1-18 was compiling and testing in another worktree. Almost all of the time went to native phases this branch doesn't touch (main → setupStart, bridgeStarted → windowBuilt).
  - **TTI, rerun on its own** (`--only tti --app target/perf/atrium`, load average 4.0): **pass**.
    - Median 373 ms and p90 392 ms, against a gate of < 400 / < 450.
    - The baseline was 330.9 / 341.8 at load 2.3–3.6.
    - First JS → mounted is 71 ms against the baseline's 65 ms. That is about +6 ms, consistent with the startup JS growing about 14 kB.
    - The rest of the gap is in the native phases and follows the machine's load.
  - The raw JSON is in my scratchpad and is not kept in the repository.

### Bundle size

- **Before** (`4bf9874e`): the main chunk was 273.5 kB (90.5 kB gzip).
- **Now, loaded at startup:** `index` is 217.8 kB (70.0 kB gzip), plus the shared `controls` chunk at 70.1 kB (25.9 kB gzip, modulepreloaded). That is about 288 kB (96 kB gzip) in all.
- **Loaded on first use only:** RuleEditor 11.1 kB, EnforcementDetails 5.1 kB and Security 3.0 kB.

## Screenshots

`docs/agents/handoffs/M1-23-ui-gaps/`, each in `-light.png` and `-dark.png`:
- `thread-menu`, `thread-rename`, `confirm-archive`;
- `enforcement-badge`, `enforcement-details`;
- `inbox-batch-selected`, `inbox-batch-result`;
- `settings-rule-editor`, `settings-rule-editor-refused`, `settings-rule-editor-shell-refused` (round 2), `settings-security`;
- `palette-this-chat`.

To regenerate them: `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 app/scripts/screenshots.py docs/agents/handoffs/M1-23-ui-gaps gaps` (the first argument is the output folder; corrected in round 2). They come from `app/src/gap-shots.ts`.

How the pictures were made:
- The chats are seeded in the window.
- The batch result shows the daemon's real refusals, because it has none of the seeded approvals.
- The enforcement details use an example Partial descriptor. The fake agent reports no coverage at all, so in the demo every class reads "Not verified".
- The rule path is `/home/you/projects/**`, so that no real home path appears in a picture.

## Notes and anything left

- **The Linux save dialog (GTK) has not been compiled.** There is no Linux toolchain on this Mac. It needs a build on the Linux box.
- **Export onto an existing file is refused by the daemon**, which creates new files only. The macOS panel asks before replacing an existing file, and then the daemon refuses it with an error toast. Changing that is a daemon change, which this brief excludes.
- **Batch acts only on approvals shown in the queue.** That is the first 12 held threads, with up to 2 cards each.
- **The brief's context list points at PLAN §3.2 and §3.13.** Those sections are adapters and browser, not threads and rules. I worked from §3.4, §3.12 and contract §3, §4 and §9.
- **CR-12** is a marked spot in code only, with no visible placeholder.
- No daemon, protocol, policy, store or agent crate changed. `app/src-tauri` changed only for `ask_save_path` and `safety_info`.

## Round 2 (D-113): the Argon review

Branch `agent/opus/M1-23-ui-gaps`, on top of `eb5997ec`. Commits: `e534d7bb` (rules editor), `03a620df` (inbox, archive, quoting), `ced96fa2` (save dialog, export), and the commit with this section.

| # | Finding | Result | What changed |
|---|---|---|---|
| 1 | Blocker: allow rules on shells, wrappers, inline code | Fixed | `commandRisk` in `app/src/chat/rule-form.ts`. Allow rules (prefix **and** exact) are refused when the first word (by basename) is one of these: a shell (`shell.rs` `SHELLS`); `eval`, `source` or `.`; a wrapper `unwrap_reading` strips (`time nohup builtin command setsid unbuffer caffeinate sandbox-exec exec nice ionice stdbuf xargs busybox toybox timeout chrt taskset arch chroot flock script env`), plus `sudo doas su runuser setpriv pkexec`. They are also refused for an interpreter given inline code: `python*` `-c`, `node`/`bun` `-e -p --eval --print`, `ruby -e`, `perl -e/-E`, `php -r…`, `lua -e`, `osascript -e`, `deno eval`. The flag also counts inside a cluster such as `perl -lne` or `python3 -Ic`. `find` with `-exec`, `-execdir`, `-ok` or `-okdir` is refused too. Every wrapper is refused, rather than stripped and the rest checked. That is stricter than the brief needs, and needs no option tables in TS. Warned, at any argv length: interpreters with no inline code, `npx pnpx bunx uvx pipx`, `shell.rs` `COMMAND_RUNNERS` (`ssh`, `docker`, …), and plain `find`. Deny rules on all of these are still allowed and get no warning. The command field's hint says what can't be allowed. |
| 2 | Blocker: path limit on command and deny rules | Fixed in the UI; engine limit **escalated** | `PATH_KINDS` no longer has `execute`. "Only for paths inside" now shows only when `pathScopeOffered` is true: an allow rule that is a path glob, or `any` of a non-`execute` kind. `validateRule` drops a leftover limit from any other rule, so `path_scope` is `null`, and an invalid leftover can't block it. The test and the `gap-shots.ts` example that showed scoped command rules were changed. |
| 3 | Major: Linux GTK save dialog left on screen | Fixed, **not compiled here** | `save.rs` uses `gtk::FileChooserNative::new(Some(title), None::<&gtk::Window>, Save, Some("_Save"), Some("_Cancel"))`. Its `run()` hides the dialog before it returns, and dropping the last reference frees it. There is no `close()`. I checked the signature, `NativeDialogExt::run -> ResponseType` and the `FileChooserExt` methods against docs.rs for gtk 0.18.2. The Linux `cfg` doesn't build on this Mac, so this code is checked by reading only. |
| 4 | Minor: input the engine never matches | Fixed | `splitCommand` refuses these: a `#` that starts a word (a quoted `#`, or `a#b`, is fine); control characters (C0 except tab, DEL, C1) and U+2028/U+2029; an empty command name (`''`, `""`); and a first word from `shell.rs` `RESERVED`, even quoted. Empty arguments such as `grep -e '' x` are still allowed. New `globSyntaxError` mirrors the `globset` 0.4.21 parser. It catches a dangling `\`, an unclosed class (`[`, `[]`, `[!]`, `][`), a range that goes down (`[z-a]`, `[z--]`), and unpaired braces. A leading `]` or `-` in a class and nested braces are accepted, as globset accepts them. `broadGlob` warns on `/`, `/*`, `/**`, `/**/*`, and on the home folder (`~`, `~/*`, `~/**`, `~/**/*`). The `any` warning now shows with a path limit too ("…inside that folder…", or "…in your home folder…"; a whole-disk limit gets the plain warning). |
| 5 | Minor: export over an existing file | Fixed | `ask_save_path` returns `{ path, exists }` (`SavePick`, which uses `symlink_metadata`, so a dangling symlink also counts as there). `pickNewPath` in `thread-actions.ts` toasts *"That file already exists, and an export never replaces a file: pick a new name."* and opens the dialog again, with the title ending in ": pick a new name". It gives up after 5 taken picks. Linux turns off GTK's overwrite confirmation. NSSavePanel has no public switch for its "Replace?" prompt, so the panel's message now reads "Atrium never replaces a file: pick a new name." If the daemon still refuses with AlreadyExists (a race, or bench mode), the toast uses the same clear wording. |
| 6 | Minor: archive from a pop-out | Fixed | `confirmArchive` calls `bringBack(threadId)` when this window is a pop-out or another window owns the view (`archiveClosesPopout`). From the pop-out itself, that closes the window. In the main window, a new effect (`mainChatLeft`) clears `mainThread` when the chat it shows leaves the list after being listed: archived in a pop-out, or by another client. The screen stays as it is (Settings stays Settings). |
| 7 | Minor: checkbox focus vs Inbox keys; module-level batch state | Fixed | The key guard skips only text inputs, so J, K, A, D, X and ↵ work from a batch checkbox. The batch bar also takes the keys, so "Select all" keeps them. `selection`, `batchBusy` and `batchResult` now live inside `InboxCard` and reach the cards as a `Batch` prop, so leaving the inbox drops them. A and D act on the approval card that holds focus, when there is one. |
| 8 | Nit: `quoteArg` and `~` | Fixed | `~` and `~/…` stay unquoted. `~/a b` reads `~/'a b'`. `~bob` and `a~b` are still quoted. |
| 9 | Nit: `X` with two approval cards | Fixed | Each approval card carries `data-batch-key`. `xKeys` makes X toggle the focused card. Without one, X toggles every selectable approval of the picked chat as a group (`toggledGroup`). |

### Escalated (not done: daemon, policy or protocol crates)
- **Engine path-scope limit (`crates/atrium-core/src/policy/engine.rs`).** `all_paths` adds `a.start`, the command's cwd, to every command's paths. Every non-flag word is also resolved against the cwd as a path. `path_scope_ok` requires **all** of them to match. So a `path_scope` on a command rule never matches in a Home chat (cwd = `~`, and Claude's `Bash` has no cwd), and `PathGlob` rules of kind `execute` never match there either. `deny_matches` also uses `path_scope_ok`, which is `all`, so a scoped deny rule lets a call through as soon as one of its paths is outside the scope. The UI no longer offers these rules. Rules made before this, or by other clients, still can be. Two fixes are possible: in the engine, scope commands by their named paths only and use `any` for deny; or have the daemon refuse such drafts in `CreateRule`.
- **Daemon-side `CreateRule` guard.** The daemon accepts any `RuleDraft`, including an allow `CommandPrefix` on `sh`, `bash -c`, `env` or `sudo`. Such a rule breaks the `shell.rs` assumption that no rule allows a prefix of `sh` ("reasons of a payload don't matter"). The UI now refuses these drafts, but another client on the socket can still create one (and contract §4 lets any same-UID process talk to the socket). Recommended: the same refusal list in `create_rule`, or `allow_covers` refusing a part whose `eff[0]` is a shell, `eval`, a wrapper or a runner. Prefix allow rules on `find` also cover `find … -exec/-delete`, because derived `-exec` units aren't checked against allow rules. The UI warns about this; the engine may want to stop such a rule from covering `-exec`.

### Definition of done (round 2)
| Command | Result |
|---|---|
| `pnpm typecheck` (app/) | pass |
| `pnpm test` (app/) | pass: 19 files, 338 tests (317 before; new tests in `rule-form`, `rules`, `batch`, `thread-actions`) |
| `pnpm build` (app/) | pass; `RuleEditor` is still a lazy chunk (15.1 kB) |
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass (macOS: the Linux `run_dialog` isn't compiled) |
| `cargo test -p atrium-app --no-fail-fast` | pass: 47 unit tests (new: `a_save_pick_says_whether_something_is_already_there`) + 2 `delivery_shapes` |
| `/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 app/scripts/m1_demo.py`, 3 runs in a row | **27/27, 27/27, 27/27** |

Only `atrium-app` changed among the Rust crates, so the workspace tests weren't rerun.

New demo checks:
- 9b: exporting to the same name again is refused, with the clear message.
- 11: X on a focused checkbox unselects that approval, and the batch result clears when you leave the inbox and come back.
- 12a: an allow rule on `bash -c 'cargo test'` is refused, a command rule shows no path field, and the rule that gets created has `path_scope: null`.

`m1_demo.py`'s docstring and `docs/demos/M1-demo.md` were updated.

### Screenshots: **not refreshed**
Every capture failed with `screencapture exit status: 1` ("could not create image from window"). The session's screen was locked (`CGSSessionScreenIsLocked=Yes`; a full-screen capture came out black). In this round, `gap-shots.ts` no longer types a path limit, and it adds the shot `settings-rule-editor-shell-refused`; `screenshots.py` now expects 22 gap shots. So these pictures in `docs/agents/handoffs/M1-23-ui-gaps/` are **out of date**:
- `settings-rule-editor-{light,dark}.png` still shows the old field and a scoped command rule;
- `settings-rule-editor-shell-refused-{light,dark}.png` doesn't exist yet.

The batch, archive and export surfaces look the same as before. Once the screen is unlocked, regenerate with:
`/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 app/scripts/screenshots.py docs/agents/handoffs/M1-23-ui-gaps gaps`
That run also writes a `gaps.log` there, which should be deleted.

### Left open
- The screenshots above.
- Linux build of `save.rs` (CI or a Linux machine).
- The two escalations.

## Round 3 (D-116): fail-closed rule check, Inbox targeting

Review: `docs/agents/reviews/M1-23-ui-gaps-round2.md`. Commits:
- `1761ff6d`: the code;
- the commit with this section: the screenshots, refreshed.

| # | Finding | Result | What changed |
|---|---|---|---|
| 1 | Blocker: `commandRisk` spellings | Fixed, fail closed | See "How the check reads a command" below. |
| 2 | Blocker: A/D/X on the wrong thread | Fixed | New `keyTargets` in `batch.ts`, shared by A, D and X. With focus in an approval card, they act on that card only, and only if it can still be answered; otherwise nothing happens, with no fallback to another card or another chat. With no card focused, A and D act on the picked chat's first approval, and X on all of that chat's approvals. Each row's `onFocusIn` sets `inboxPick`, so focus and pick are always in the same row. J and K move the pick and put focus back on the list. |
| 3 | Major: path limit on non-file kinds | Fixed | `pathScopeOffered`: allow rules only, and only for path globs (always a file kind) or `any` of a `PATH_KINDS` kind. Web requests, MCP and other calls get no limit. |
| 4 | Minor: `SAVE_MESSAGE` on Linux | Fixed | It now has `#[cfg(target_os = "macos")]`. |
| 5a | Minor: glob classes | **Checked against globset; kept** | A temporary probe test built each glob with globset 0.4.21 (the engine's version), then was deleted and not committed. Results: `[a-z-_]` and `[a-z-0]` fail ("invalid range; 'a' > '_'"); `[\]`, `[\]]`, `[a-c-e]`, `[a-c-b]` and `[a\-z]` build; `[\` fails ("unclosed character class"). The round-2 checker already matched all of that. The review's fix would refuse `[\]`, which globset accepts, and accept `[a-z-_]`, which globset refuses. Tests now pin the probed results, and the code comment cites them. |
| 5b | Minor: line breaks, brace depth | Fixed | `splitCommand` refuses `\n` and `\r` anywhere, quoted or not. New `expandsBraces` follows `shell.rs`'s scan: depth counting, a `,` or `..` at depth 1, stop at a blank or an operator, 256-character cap. So `echo {{a},b}` is refused, and `{}`, `{a}` and `{{a}}` stay allowed. |

### How the check reads a command (`rule-form.ts` `commandRisk`, allow rules only)
- **Names are folded** with `foldName`: the basename, lowercased (which also turns U+212A into `k`), then `ſ` to `s`, as `deny.rs` `lower` does. Every table is a `Set` or a `Map`, so names such as `constructor` and `__proto__` no longer crash it.
- **Refused for both prefix and exact rules:**
  - Shells, `eval`/`source`/`.`, and every wrapper, now including `sudoedit` and `run0` from `deny.rs` `PRIV`.
  - For interpreters (`deny.rs` `is_interpreter`: `python*`, perl, ruby, node, deno, bun, php, lua, osascript, pwsh), any single-dash option word that holds a code-flag letter anywhere, compared case-insensitively. That covers attached code (`-c'…'`), digit clusters (`-0777ne`) and `-Ic`.
  - Long code flags, with or without `=`, in any case.
  - `eval` or `repl` as the first word that isn't an option (`deno -q eval`, `bun eval`).
  - `-` as an operand (code from stdin).
  - PowerShell (`pwsh`, `powershell`, also with `.exe`) given `-c`, `-e`, `-ec`, `-cwa`, or any prefix of `command`, `encodedcommand` or `commandwithargs`, written with `-`, `--` or `/`. Without a named `.ps1`, PowerShell is refused too, because Windows PowerShell reads bare words as a command.
  - `npx`-style runners given `-c` or `--call`, and `find` with `-exec`, `-execdir`, `-ok` or `-okdir` (folded).
- **Refused for prefix rules only:**
  - An interpreter or package runner unless the script or package comes **right after the command**, with no option before it. For deno and bun, "right after" means after a verb such as `run`. An interpreter reads its own options only before the script, so words added after it go to the script. An option before the script could take code added later: `python3 -W ignore` would cover `python3 -W ignore -c …`. So `python3 -u build.py` and `npx -y prettier` work as exact rules only.
  - Any prefix rule on `find`, because it would also cover `-exec` or `-delete` added later. The error points to "This exact command".
- **Fail closed:** a code-flag letter after the script also refuses the rule (`perl tool.pl -e`).
- **Warned:** an allowed interpreter or runner rule, remote runners, and an exact `find … -delete`.
- The error texts say what to write instead, and the field's hint mentions putting the script right after the command.

### Escalated (not changed)
- **`shell.rs` matches shell and wrapper names case-sensitively.** `/bin/BASH -c '…'`, `Bash`, `ENV` and `/bin/baſh` aren't read as a shell or wrapper, so their payloads are never lexed. This holds even without any rule, and APFS resolves those names to the same binaries. The engine should fold names the way `deny.rs` `lower` does.
- **Daemon-side `CreateRule` guard.** This is unchanged from round 2: the daemon accepts any draft, so other clients on the socket can create the rules the UI refuses.
- **Round 2's engine escalations still stand:** the path-scope `all`/cwd behaviour, and derived `find -exec` units not being checked against allow rules.

### Definition of done (round 3)
| Command | Result |
|---|---|
| `pnpm typecheck`, `pnpm build` | pass |
| `pnpm test` | pass: 19 files, 350 tests (338 before). New tests cover every spelling in the review plus case and APFS variants, `Object.prototype` names, PowerShell prefixes, the prefix-needs-a-script rule, `keyTargets` (focus on another row's card, a focused card that can't be answered, no focus), line breaks, nested braces, and the probed globset results. |
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `cargo test -p atrium-app --no-fail-fast` | pass: 47 + 2 |
| `m1_demo.py` under the GUI lock, 3 runs in a row | **27/27, 27/27, 27/27**. Step 11 now also checks that focusing a row's checkbox picks that row, and that J from a checkbox moves the pick and focus to the list; X then toggles only the newly picked row. |
| Screenshots | **Refreshed** (the screen was unlocked): all 22, light and dark, under `docs/agents/handoffs/M1-23-ui-gaps/`, including the new `settings-rule-editor-shell-refused-*`. `settings-rule-editor-*` no longer has the path field. `gaps.log` deleted. This replaces round 2's "not refreshed" note. |

### Left open
- The Linux build of `save.rs` (CI or a Linux machine).
- The escalations above.

## Round 3 follow-up (review 3, finding 1)
Argon approved with changes (`.orchestration/review-input/M1-23-review3.md`). It confirmed the globset class parsing, and I kept it as is. Changes in `rule-form.ts` `commandRisk`, covering both prefix and exact allow rules:
- **`bun exec`:** `exec` moved from bun's verbs to its inline-code subcommands, because it runs a shell string. `bun exec …` and `bun --silent exec …` are now refused.
- **Package runners** (`npx`, `pnpx`, `bunx`, `uvx`, `pipx`): a `c` anywhere in a single-dash option word is refused, as for interpreters. This catches `-yc 'sh'` and `-c'sh'`. Exact rules must also name a package, so `npx`, `npx -y` and `uvx` alone are refused.
- **Standard input:** `-`, `/dev/stdin`, `/dev/fd/0` and `/proc/self/fd/0` are refused as the script, in any case.
- **Interpreters with no script operand** (or only a verb, as in `deno run`) are refused for exact rules too. Without a script the interpreter reads code from its input, so an exact rule on `python3` would cover `echo … | python3` in Home. Exact rules that name a script or module still pass: `python3 -m pytest`, `python3 -u build.py`, `npx -y prettier`.

Checks:
- `pnpm typecheck` and `pnpm build` pass.
- `pnpm test`: 354 tests pass (4 new tests covering these cases).
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check` pass.
- `m1_demo.py` under the GUI lock: 27/27, one run. The demo itself is unchanged.
