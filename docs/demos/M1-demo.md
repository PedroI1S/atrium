# M1 Gate Demo Script

A one-page walkthrough for demonstrating Milestone 1 (Daemon + Home + attention) with real **Claude Code** and **Codex**, followed by the one-command automated synthetic run.

## Automated run (synthetic agent, no real provider)

Run under the GUI lock so desktop measurements do not overlap (keep `lockf` around the whole script):

```fish
/usr/bin/lockf -k /tmp/atrium-gui-measure.lock python3 app/scripts/m1_demo.py
```

- **Desktop banners & build time:** the automated run opens app windows and posts real macOS Notification Center banners (`CommandNotifier` in `crates/atrium-daemon/src/upkeep.rs` invokes `/usr/bin/osascript` on macOS). After the app section finishes, `m1_demo.py` runs `cargo test` (daemon recording notifier + scripted-adapter N1 resume test) with no timeout; a cold Rust test build can take several minutes.
- **Fake-agent limit:** step `7c` asserts that `resume_thread` on a synthetic (`atrium-fake:`) thread is refused (`invalid`), while real resume is checked by the scripted-adapter daemon test (`7d`) and the live demo below.
- **What the app section covers (M1-23 added 5b, 7b, 9a/9b, 10, 11, 12a/12b, 13):** streaming, Allow/Deny/Always, Answer/Decline, the inbox and mark-read, `⌘J` and `⌘⇧J`, the notification switches, Stop, **End session from the thread menu**, daemon kill and reconnect, **Rename and Export (Markdown and JSONL) from the thread menu** (in bench mode the save dialog is bypassed and files go to the run's `exports/` folder), **Archive from the thread menu** (Cancel keeps it, Archive removes it), **batch Allow once / Deny** in the inbox (`X` on a focused checkbox toggles that approval; the result clears when you leave the inbox), the **rules editor** (a compound command and an allow rule on `bash -c` are refused, a command rule offers no path limit, then `cargo test` is added as a prefix rule), **Settings → Security**, and the **enforcement details**.
- Pass `--keep` to inspect the throwaway `ATRIUM_HOME` directory, `app.log`, and exported `.md` / `.jsonl` files afterwards. Related live checks: `python3 app/scripts/live_check.py` (pop-out windows, drafts, app crash recovery) and `python3 app/scripts/chat_check.py` (pop-out mid-stream + daemon kill).

---

## Live demo (Claude Code & Codex)

Launch the app (`cargo run -p atrium-app --features custom-protocol` or the built binary). Confirm the **Context panel → Details** and **Settings (`⌘,`) → Agents** list Claude Code and Codex with their enforcement level (`Full` or `Partial`) and CLI versions.

1. **Home chat streaming (Claude Code & Codex)**
   - Click **New chat** (or **Home**) in the sidebar.
   - In the composer's **Agent** selector, pick **Claude Code**, ask a read-only question (e.g. `List the files in ~ and summarize what is here`), and press `↵`.
   - **Expect:** the user bubble appears, markdown and tool calls stream smoothly, and **Details** populates `Session` (native ID) and `Usage`. Repeat in a second chat with **Codex**.

2. **Approvals: Allow once, Deny, Always…**
   - In Home, ask Claude Code to run a write/mutating command (e.g. `Run echo hello > /tmp/atrium-live-demo.txt`).
   - **Expect:** the thread pill changes to **Needs approval** and an approval card docks above the composer showing the exact command.
   - Click **Allow once** (`↵`) — the tool call finishes **Done**.
   - Send a follow-up that runs a different command (`Run touch /tmp/atrium-live-demo-2.txt`) and click **Deny** (`esc`) — the tool call finishes **Denied** and the agent adapts.
   - Send `Run echo hello > /tmp/atrium-live-demo.txt` again, click **Always… → Always in Home**, then repeat the same request once more — **Expect:** a rule appears under **Settings → Approvals** and the next turn runs without asking.

3. **Question forms**
   - Trigger a question from the agent (or ask it to confirm one of two options before continuing).
   - **Expect:** the thread pill shows **Has a question** and a question card docks above the composer. Pick an option and click **Answer** (`↵`) — streaming resumes. On a second question, click **Decline** — the turn continues without stopping.

4. **Inbox & Mark read**
   - Start a chat, switch away to **New chat** before it finishes so it collects an unread reply, while keeping an approval or question thread waiting.
   - Click **Inbox** in the sidebar.
   - **Expect:** items are grouped and ranked (**Approvals and questions** → **Failed** → **Ready for you**). Open the unread reply item — once focused, the unread state clears and its **New reply** item leaves the Inbox.

5. **Quick jump (`⌘J`, `⌘⇧J`) and batch approvals**
   - From **Inbox** or another chat, press `⌘J`.
   - **Expect:** Atrium opens the top-ranked chat that needs you; pressing `⌘J` again cycles to the next Inbox item (or focuses its pop-out window). `⌘⇧J` goes back to the previous one (from the Inbox, to the last).
   - With two or three approvals waiting, open **Inbox**: tick their checkboxes (or **Select all approvals**, or `X` on the picked item) and click **Allow once (N)** or **Deny (N)**. **Expect:** each one is resolved on its own; the bar says how many went through and lists any that failed (e.g. one that expired meanwhile). There is no batch **Always…**.

6. **Notifications**
   - Open **Settings (`⌘,`) → Notifications** and confirm the three switches (**An agent needs your approval**, **An agent has a question**, **A chat failed**) are on.
   - Start an approval-triggering turn, then switch to **Inbox** or unfocus the Atrium window (notifications are suppressed for the focused chat: `ThreadCard.tsx` calls `reportFocus` → `set_focus`, and `crates/atrium-daemon/src/hub.rs` `thread_changed` / `crates/atrium-core/src/attention/mod.rs` `should_notify` return `false` when `focused` is true).
   - **Expect:** a macOS notification banner appears. Turn off **An agent needs your approval** and repeat — no approval notification is posted. Turn it back on.

7. **Stop & Resume**
   - Start a long turn and click the square **Stop the turn** button in the composer — **Expect:** the turn ends (`Stopped`, thread returns to **Idle**).
   - Open the thread header's **⋯** menu → **End session…** and confirm — **Expect:** the agent's process stops and the thread shows **Ended**; the menu item is then off.
   - In a real provider thread (**Claude Code** or **Codex**) whose session was interrupted or ended, click **Resume** in the thread header (visible when **Interrupted**) or send a new message in the composer — **Expect:** a new incarnation starts from the stored native session ID and continues the conversation. *(Fake `atrium-fake:` threads cannot be resumed; `atriumd` refuses `resume_thread` and `ThreadCard.tsx` hides the **Resume** button for them.)*

8. **Daemon kill & recovery**
   - Start a streaming turn with a real provider thread, then kill the daemon (`pkill -9 atriumd`).
   - **Expect:** the connection banner appears, Atrium launches a fresh `atriumd` and reconnects, and the cut-off thread transitions to **Interrupted** with its partial transcript intact. Click **Resume** in the header to continue it *(available for real provider threads; hidden for synthetic `fake` threads)*.

9. **Rename, Export & Archive**
   - In a thread, open **⋯ → Rename** (or `⌘K` → **This chat: Rename**), type a name and press `↵` (`esc` cancels). **Expect:** the header and the sidebar show the new name.
   - **⋯ → Export as Markdown…** then **Export as JSONL…**: a native save dialog asks where (the name is the chat's title). **Expect:** a toast names the file and its size; the file is mode `0600`, with the home folder as `~` and secrets redacted. Exporting over an existing file is refused (the daemon only creates new files): pick a new name.
   - **⋯ → Archive…** and confirm. **Expect:** the chat leaves the sidebar and the Inbox, and the main card goes back to Home.

10. **Rules, Security and enforcement details**
   - **Settings → Approvals → Add a rule…**: pick **Commands that start with**, type `cargo test`, **Add allow rule**. **Expect:** it is listed as *Allow commands that start with `cargo test …`*. Typing `cargo test && rm -rf target` is refused before anything is sent. Path globs take `~/…` and full paths only.
   - **Settings → Security** lists **Always blocked (for calls Atrium sees)** (the seven `HARD_DENY_DEFAULTS`, read-only, with the note that it isn't a machine-wide ban) and the known limits: git config can run programs during auto-allowed git reads; any program running as you can talk to `atriumd`; allowed code isn't contained.
   - Click the enforcement badge in a thread header (or **Details** in the Context panel or in **Settings → Agents**). **Expect:** the level, CLI version, reasons, coverage per tool class, the configuration sources, then **Policy** and **Isolation** as separate sections.
