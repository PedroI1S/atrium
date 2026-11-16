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
- `settings-rule-editor`, `settings-rule-editor-refused`, `settings-security`;
- `palette-this-chat`.

To regenerate them: `python3 app/scripts/screenshots.py gaps`, under the GUI lock. They come from `app/src/gap-shots.ts`.

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
