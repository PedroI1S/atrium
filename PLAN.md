# Atrium: Plan

A fast, lightweight agent development environment written in Rust. It wraps the native coding agents (Claude Code, Codex, any ACP agent) so each model keeps its own tools, skills and subscription. Around them it adds:

- **Home**: a default conversation for questions about the whole machine, which also orchestrates workspaces.
- **Workspaces**: MonoCode/T3-style project workspaces with a worktree per thread, a terminal, an editor and diff review.
- **Browser**: an embedded Chromium pane per thread, driven by the agent, using a Chromium-based browser you already have installed.
- **Sandbox (optional)**: disposable Linux microVMs (libkrun), used only when a task needs isolation. Everything else works without it.
- **Multi-agent workflow**: an attention inbox, automatic checks, conflict detection, a merge queue, cross-agent review and delegation, and a task backlog.

## 0. Design principles

1. **The human is the bottleneck, not the agents.** Every feature is judged by whether it reduces the attention you need to spend per finished task.
2. **Attention routing.** Atrium always shows what needs you next, and everything else runs without you.
3. **Evidence instead of claims.** "Done" means checks passed and there's proof (test output, screenshots), not that the agent said so.
4. **Cheap, safe parallelism.** Starting a task is instant. Every task gets isolated files, ports and dependencies. Conflicts show up early, not at merge time.
5. **Durability.** Agents keep running when you close the window, sleep the laptop or walk away.
6. **Parallel independent tasks before agent swarms.** Agents coordinate through explicit handoffs, notes and reviews, not open-ended conversations with each other.

## 1. Decisions so far

| Area | Decision |
|---|---|
| Agent core | Wrap native agents. We don't write our own agent loop. |
| Providers v1 | Claude Code, Codex, Antigravity (`agy`, for Gemini models), generic ACP adapter (OpenCode, Goose, …) |
| Process model | **The core runs as a background daemon (`atriumd`) from v1.** The Tauri app is a client over a local socket. Agents keep running after the window closes. Remote/mobile later means adding a transport, not rewriting the core. |
| UI | Tauri 2 + SolidJS |
| Platforms | macOS (Apple Silicon) + Linux |
| Browser | Detect an installed Chromium, drive it over CDP, show it in the app through a screencast pane, one isolated browser context per thread |
| Sandbox | **Optional, on demand.** Threads run locally by default. A libkrun microVM is created only when you ask for one or an agent requests one (with your approval). It is thrown away after the task. The app installs and runs fine without libkrun. |
| Home chat | Reads freely; any change needs approval; orchestrates workspaces through app MCP tools |
| Workspace | Worktree per thread with a ready-to-use environment (env files, setup, ports, preview), checkpoints/revert, checks, terminal, editor, diff review with inline comments |
| Multi-agent | Attention inbox, definition of done + automatic checks, conflict radar + merge queue, delegation + cross-provider review, shared notes + handoffs, backlog/queue/schedules, concurrency and rate-limit management, approval rules |
| Audience | Personal first, kept clean enough to open-source later |
| Build team | Opus 5.5 orchestrates; Sol 6.1 (backend), Sonnet 5.5 (frontend), Argon (systems/security), 3.8 Flash (utility), Astra (gates/escalation). Cross-provider review, contract-first parallel fan-out. See section 8. |

## 2. Landscape (what we borrow / where we differ)

- **T3 Code** (Electron + Node, Effect, SQLite): adapters that use each provider's native protocol, an event-sourced thread model, per-turn git checkpoints, message-level revert, `delegate_task`. *We borrow the adapter + event model, the checkpoints and delegation.*
- **MonoCode** (Tauri + Rust): multi-root workspaces, split panes, `/operator` meta-agent, notes, issue inbox, cross-project scheduled "Monos". *We borrow the workspace UX, the operator idea (our Home chat), notes and scheduled runs.*
- **Orca**: a worktree per task, an embedded Chromium per worktree, design mode, a CLI that agents use to control the app, fan-out/compare. *We borrow per-thread browsers, agent→app control (via MCP rather than a CLI) and, later, fan-out.*
- **Where Atrium differs:**
  - A Rust daemon core and a lighter footprint.
  - A built-in optional VM sandbox.
  - A Home chat that covers the whole machine.
  - It uses the browser you already have instead of bundling one.
  - **The workflow is designed around your attention:** an inbox, evidence-based "done", conflict radar, a merge queue and cross-provider review.

## 3. Architecture

```
┌──────────── Tauri app (atrium) — thin client ────────────┐
│ SolidJS UI: Inbox · Home · Workspaces · Thread · Diff ·   │
│   Terminal · Editor · Browser pane · Backlog · Usage      │
│      ▲ tauri-specta types, Channels (binary streams)      │
│      ▼                                                    │
│ atrium-client (Rust): socket ↔ webview bridge, replay     │
└──────────────▲────────────────────────────────────────────┘
               │ unix socket: framed JSON control + binary streams
┌──────────────▼──────── atriumd (background daemon) ────────────────────────┐
│  store      SQLite (WAL): append-only events + projections, FTS            │
│  attention  thread state machine, inbox ranking, notifications             │
│  agents     AgentAdapter → claude | codex | acp                            │
│  policy     approval rules, presets, hard-deny list                        │
│  git        worktrees, checkpoints, conflict radar, merge queue, PRs       │
│  env        worktree setup/teardown, port leases, preview processes        │
│  checks     definition-of-done runner, auto-fix loop, evidence, risk flags │
│  scheduler  backlog queue, concurrency limits, rate-limit pauses, cron     │
│  pty        portable-pty terminals                                         │
│  browser    Chromium discovery, CDP (chromiumoxide), screencast, input     │
│  sandbox    (optional) libkrun VMs via atrium-vmm child processes          │
│  mcp        rmcp server: browser_* sandbox_* thread_* notes_* checks_* …   │
└────────────────────────────────────────────────────────────────────────────┘
      │ stdio (local)                         │ vsock ↔ unix socket
      ▼                                       ▼
  claude / codex / ACP agent        ┌─ libkrun microVM (optional) ───────────┐
  processes on host                 │ atrium-guest: exec, PTY, agent stdio   │
                                    │ agent CLI in full-access mode          │
                                    │ /work = worktree via virtio-fs         │
                                    │ atrium-mcp-shim (stdio ↔ vsock)        │
                                    └────────────────────────────────────────┘
```

### Cargo workspace layout

```
atrium/
├─ Cargo.toml                (workspace)
├─ crates/
│  ├─ atrium-protocol/       serde types for commands/events; specta → TS
│  ├─ atrium-core/           domain, store, attention, policy, scheduler
│  ├─ atrium-daemon/  (bin)  atriumd: socket server, lifecycle, single instance
│  ├─ atrium-client/         socket client + reconnect/replay (used by Tauri)
│  ├─ atrium-agents/         AgentAdapter + claude / codex / acp adapters
│  ├─ atrium-git/            worktrees, checkpoints, conflict radar, merge queue, PR (gh)
│  ├─ atrium-env/            worktree setup/teardown, port allocator, previews
│  ├─ atrium-checks/         check runner, evidence, risk-flag heuristics
│  ├─ atrium-browser/        discovery, CDP, screencast, input, a11y snapshot
│  ├─ atrium-sandbox/        (feature) image builder, VM lifecycle, vsock plumbing
│  ├─ atrium-mcp/            MCP server (rmcp) + per-thread scoping
│  ├─ atrium-vmm/   (bin)    one process per VM, calls krun_start_enter
│  ├─ atrium-guest/ (bin)    static musl helper inside the VM
│  └─ atrium-mcp-shim/ (bin) stdio↔socket bridge so any agent can reach MCP
├─ app/
│  ├─ src-tauri/             Tauri shell + atrium-client
│  └─ src/                   SolidJS + Vite, CodeMirror 6, xterm.js
└─ images/                   sandbox base image definitions
```

### 3.1 Daemon and client (`atriumd`, `atrium-client`)

- **Owns everything**: the store, agent processes, PTYs, the browser, VMs and the MCP server. The app only renders and sends commands.
- **Lifecycle**:
  - The app connects to the socket and starts `atriumd` detached if it isn't running.
  - Optional "keep agents running after quitting" installs a launchd LaunchAgent (macOS) or a systemd user unit (Linux).
  - A lock file plus a socket at `~/Library/Application Support/Atrium/` or `$XDG_RUNTIME_DIR/atrium/` guarantee a single instance.
- **Protocol**:
  - Length-prefixed frames over a unix socket: JSON for control, binary frames for PTY and screencast streams.
  - A version handshake on connect.
  - Clients subscribe with their last seen event id and the daemon replays the gap, so reconnects are seamless (and remote clients later behave the same way).
- **Crash safety**:
  - Agent/browser/VM PIDs are recorded in the store.
  - On daemon restart, orphans are reaped and their threads marked `Interrupted`, with one-click resume using the agent's native session id.
- **Upgrades**: the app and daemon ship together. A new daemon takes over when no turns are running, or after you confirm.
- **Presence**: a menu bar / tray item shows running and needs-attention counts while the window is closed.

### 3.2 Agent adapters (`atrium-agents`)

All adapters map into one internal event stream: `MessageDelta`, `ToolCall{Started,Updated,Finished}`, `ApprovalRequested`, `QuestionAsked`, `PlanUpdated`, `TurnCompleted`, `Usage`, `RateLimited`, `Error`.

```rust
trait AgentAdapter {
    async fn start(&self, cfg: SessionConfig, io: Transport) -> Result<Session>;
}
// Transport = local child stdio | vsock stream into a sandbox (same code path)
// SessionConfig: cwd, model, permission policy, injected MCP servers, resume id
```

- **Claude Code**: spawn `claude -p --input-format stream-json --output-format stream-json --verbose` with `--mcp-config` pointing at Atrium's MCP. Handle permission prompts through the control protocol (`can_use_tool`) so approvals show up in our UI and the policy engine. Session resume via `--resume`.
- **Codex**: `codex app-server` (JSON-RPC over stdio). It has native `sandbox` (`read-only` / `workspace-write`) and `approval_policy`, which fit the Home chat policy directly. Its rate-limit notifications feed the scheduler.
- **Antigravity (`agy`)**: Google's agent CLI, which runs the Gemini models (Argon, 3.8 Flash).
  - Spawn `agy -p --input-format stream-json --output-format stream-json --model <id> --effort <level>`.
  - Permission modes: `--mode accept-edits|auto|plan`. Resume: `--conversation <id>`.
  - **Spike (M0)**: map its stream-json events (possibly close to Claude's), find out whether permission requests can be answered from the host, and how to inject MCP servers per session. `agy mcp` manages servers globally, so per-session scoping may need a generated config.
- **ACP**: the `agent-client-protocol` crate (Zed). One adapter covers OpenCode, Goose, etc. MCP servers are passed in `session/new`.
- **Skills / native config**: we never rewrite prompts. Agents load their own `CLAUDE.md` / `AGENTS.md` / skills from the cwd and home dir as usual. Atrium only *adds* MCP tools and sends ordinary user messages (review comments, check failures, handoff requests).
- **Fallback if Claude's stream-json control protocol proves brittle**: use `claude-code-acp` through the ACP adapter (costs a Node dependency).

### 3.3 Domain model and storage (`atrium-core`)

- **Home**: a built-in pseudo-workspace with cwd `~`. Threads here use the *read freely, approve changes* policy:
  - Claude: auto-allow Read/Grep/Glob plus an allowlist of read-only Bash commands; everything else → approval.
  - Codex: `sandbox=read-only`, `approval_policy=on-request`.
- **Workspace**: one or more root folders (multi-repo, like MonoCode), plus an `atrium.toml` (see 3.6–3.7) holding checks, worktree setup, preview, review and schedule settings.
- **Thread**: a provider/model plus a location, either `Local{checkout|worktree}` or `Sandbox{vm}`. It also has a browser context, a port lease, an attention state, a parent thread (if delegated), a group (fan-out), a handoff note and links (branch, PR, backlog task).
- **Turn**: one user→agent cycle. Before and after each turn a checkpoint is written to hidden refs at `refs/atrium/cp/<thread>/<turn>`. Revert restores both the files and the agent session (resume from the matching session point). Check results and evidence attach to the turn.
- **Task** (backlog): a title and description, a source (manual / GitHub / Linear / schedule), a priority, the preferred agent and location, and the thread it became.
- **Storage**:
  - SQLite (rusqlite, WAL). There is an append-only `events` table per thread, and projections (threads, turns, approvals, tasks, rules, leases) are updated in the same transaction.
  - Commands go through a single writer task.
  - FTS5 indexes messages, notes and branches.
- **Protocol**: `atrium-protocol` types are exported to TS with `tauri-specta`. The daemon socket carries them; inside the app, `atrium-client` forwards streams to the webview via `tauri::ipc::Channel` with binary payloads.

### 3.4 Attention inbox and notifications (`attention`)

- **Thread state machine**: `Running` → `NeedsApproval` | `NeedsInput` (agent asked a question) | `ChecksRunning` → `ChecksFailed` | `ReadyForReview` → `Queued(merge)` → `Merged`/`Settled`. Also `Failed`, `Interrupted`, `Paused(rate limit)`.
- **Inbox**: one cross-workspace list, ranked by state priority (approval > question > failed > ready for review), then by age.
  - Each card shows the thread, the ask, and the handoff/review summary.
  - `⌘J` jumps to the next item that needs you, and `⌘⇧J` to the previous one.
- **Approval queue**: every pending approval from all agents in one place, with batch approve/deny. Each approval offers "allow once / always…" (see 3.12).
- **Questions**: agent questions (Claude's ask-user tool, Codex/ACP elicitation) render as small forms rather than plain text.
- **Notifications**: the daemon posts native notifications (`notify-rust`) for transitions into states that need you. They're grouped and rate-limited, and suppressed when you're already looking at that thread. The tray badge shows the needs-you count.

### 3.5 Definition of done, checks and evidence (`atrium-checks`)

- **Workspace checks** in `atrium.toml`:

  ```toml
  [checks]
  test  = "cargo test"
  lint  = "cargo clippy -- -D warnings"
  types = "pnpm tsc --noEmit"

  [checks.policy]
  run_after = "turn"        # turn | done | manual
  auto_fix_attempts = 2     # failures are sent back to the agent up to N times
  ```

- **Automatic loop**:
  1. After a turn (or when the agent says it's done), the daemon runs the checks in the thread's worktree and environment.
  2. A failure is sent to the agent as a message with the trimmed output, up to `auto_fix_attempts` times.
  3. After that the thread goes to `ChecksFailed`.
- **Agents can self-verify** with the `checks_run` MCP tool, so they don't need to guess the commands.
- **Evidence**: check results, `browser_screenshot(evidence=true)` captures (for UI work) and preview URLs attach to the turn and show on the review card.
- **Review card** for each `ReadyForReview` thread:
  - What changed and why (from the handoff note), check status and evidence.
  - **Risk flags** from cheap heuristics: tests deleted or skipped, new dependencies, files touched outside the task's apparent scope, lockfile churn, very large diffs, secrets-like strings.
- **No silent success**: without checks configured, a thread is labelled "unverified" rather than shown as passing.

### 3.6 Worktree environments (`atrium-env`)

```toml
[worktree]
copy     = [".env", ".env.local"]        # copied from the main checkout
setup    = "pnpm install --frozen-lockfile"
teardown = ""
ports    = 2                             # leases $PORT, $PORT_1

[preview]
command = "pnpm dev --port $PORT"
url     = "http://localhost:$PORT"
```

- **Setup on creation**: files are copied and `setup` runs in a PTY you can watch. The agent's first turn waits for setup to finish (or starts immediately, if configured).
- **Port allocator**:
  - The daemon leases non-overlapping port ranges per thread and checks the OS that each port is actually free.
  - The ports are exported as env vars to the agent, terminals, checks and the preview.
  - Leases are released when the thread settles.
- **Preview**: a per-thread dev server managed by the daemon, opened in that thread's browser context. The agent can reach it through the `browser_*` tools.
- **Shared caches**: rely on global package stores (pnpm store, cargo registry, uv cache). Optional `sccache`/`RUSTC_WRAPPER` and shared build-cache env vars per workspace.
- **Later**: a worktree pool. Pre-created, pre-set-up worktrees on the latest main for instant thread start.

### 3.7 Integration of parallel work (`atrium-git`)

- **Conflict radar**:
  - The daemon tracks each thread's changed files against its base.
  - Overlapping files show a warning in both threads.
  - A real conflict check runs with `git merge-tree --write-tree` between branches (no checkout), so true conflicts are flagged early in the inbox.
- **Base drift**: each thread shows "N commits behind main".
  - One-click rebase.
  - On conflict, choose between handing it to the agent ("rebase onto main and resolve conflicts") and resolving it yourself in the merge view.
- **Merge queue** per workspace. Finished threads land in order. For each one:
  1. Rebase onto the latest target.
  2. Re-run checks.
  3. Merge (local fast-forward, or a PR with auto-merge via `gh`).
  4. Move to the next thread.

  A failure sends the thread back to the inbox with the reason.
- **Git flow**: commit, push and open a PR through `gh`. Threads auto-settle after merge.
- **Later**: stacked threads, where a thread is based on another thread's branch and is restacked automatically when the parent changes.

### 3.8 Delegation and cross-provider review

- **Delegation** (any thread, not just Home): `thread_delegate(prompt, provider?, location?, wait?)` creates a child thread linked to its parent.
  - When the child finishes, its handoff note, check status and diff reference come back to the parent as a message.
  - Depth limit (default 2) and per-thread budgets prevent runaway trees.
- **Cross-provider review**: when a thread reaches `ReadyForReview`, a reviewer agent from a *different* provider can review it automatically:

  ```toml
  [review]
  auto = true
  reviewer = "codex"      # reviews Claude threads; Claude reviews Codex threads
  ```

  - The reviewer runs read-only on the worktree and emits line comments in the same format as human inline comments.
  - You accept or dismiss them; accepted comments go to the author agent.
  - Atrium tracks the acceptance rate per reviewer so you can tell whether it's worth the tokens.

### 3.9 Shared knowledge: notes and handoffs

- **Workspace notes**: Markdown files per workspace, stored privately in Atrium's data dir by default, or in a committed `docs/atrium/` if you prefer. They hold decisions, conventions, gotchas and a decision log.
  - The MCP tools `notes_list/read/search/write` let any agent read and write them.
  - The Home chat can read notes across workspaces.
- **Handoff notes**: when a thread finishes a task, Atrium asks the agent for a short structured handoff: done / not done / gotchas / follow-ups. It powers:
  - review cards,
  - delegation results,
  - Home's view of a workspace,
  - a "spawn follow-up tasks" action that turns follow-ups into backlog items.
- Agents' own native memory (CLAUDE.md, etc.) is left untouched.

### 3.10 Task intake and background work (`scheduler`)

- **Backlog per workspace**:
  - Quick capture (global shortcut), manual tasks, and import from GitHub issues (`gh`) and Linear (API token).
  - Each task carries a preferred agent, location (local/sandbox) and priority.
- **Queue**: "start when a slot is free" respects concurrency limits (3.11). Tasks become threads automatically.
- **Schedules**: recurring tasks defined in `atrium.toml` or the UI, run by the daemon, e.g. a nightly dependency bump or a weekly flaky-test hunt. Results land in the inbox.

  ```toml
  [[schedules]]
  name   = "deps"
  cron   = "0 3 * * 1-5"
  prompt = "Update dependencies, run checks, open a PR if green."
  agent  = "codex"
  location = "sandbox"
  ```

### 3.11 Resources and usage

- **Concurrency limits**: global, per provider and per workspace. Excess work waits in the queue.
- **Rate-limit awareness**: the adapters' `RateLimited` events and usage-window data pause affected threads (`Paused`, showing a resume ETA) and requeue work instead of failing it.
- **Usage dashboard**: tokens and cost per thread, workspace and provider, plus subscription windows and reset times for Claude and Codex.
- **Budgets**: per-thread caps cover auto-fix loops, delegation trees and cross-reviews.
- **Sandbox caps**: vCPU/RAM per VM (`krun_set_vm_config`) and a maximum number of concurrent VMs.

### 3.12 Approval policy (`policy`)

- **Rules from approvals**: every approval offers *allow once / always in this thread / always in this workspace / always*.
  - A rule matches a tool plus a command prefix or glob plus a path scope.
  - Rules can be listed and edited in settings.
- **Presets per workspace**:
  - *Careful*: ask for everything except reads.
  - *Normal*: edits auto-allowed, commands ask unless a rule matches.
  - *Trusted*: everything except the deny list.
- **Hard-deny list** that no rule overrides, e.g. `rm -rf` outside the worktree, force-push to protected branches, reading credential stores, `curl | sh` outside a sandbox. A hard-denied action can instead be offered as "run it in a sandbox" when the sandbox is available.
- **Enforcement per provider**: Claude via `can_use_tool` responses, Codex via app-server approval responses, ACP via permission requests.

### 3.13 Browser (`atrium-browser`)

- **Discovery**:
  - macOS: Chrome, Chrome Canary, Brave, Edge, Chromium, Vivaldi, Arc, Helium in `/Applications` and `~/Applications`.
  - Linux: `$PATH` (`google-chrome`, `chromium`, `brave-browser`, `microsoft-edge`), plus flatpak and snap paths.
  - The user picks a default in settings.
- **Process model**: one Chromium process owned by the daemon (`--headless=new`, dedicated `--user-data-dir` under Atrium's data dir, `--remote-debugging-pipe`). Each thread gets its own `Target.createBrowserContext`, so cookies and storage are isolated and lighter than a process per thread. A workspace can opt into a persistent, logged-in context later.
- **Pane**:
  - `Page.startScreencast` sends JPEG frames over a binary stream; they're drawn with `createImageBitmap` onto a `<canvas>`.
  - Mouse, keyboard, wheel and IME input go back through `Input.dispatch*`. The pane resizes via `Emulation.setDeviceMetricsOverride`.
  - Navigation bar, tabs per context.
  - "Take over" mode lets you handle logins and captchas, then hand control back.
- **Agent tools (via MCP)**: `browser_navigate`, `browser_snapshot` (accessibility tree with element refs, Playwright-MCP style), `browser_click/type/select/scroll(ref)`, `browser_screenshot(evidence?)`, `browser_eval`, `browser_console`, `browser_network`, `browser_wait`.
- **Later**: design mode, where you click an element and its HTML, CSS and a cropped screenshot go into the composer.

### 3.14 Sandbox (`atrium-sandbox`), optional

**Optional by design.** Local is the default for every thread. The sandbox is extra capability, not a dependency:

- **Lazy availability**:
  - libkrun is loaded at runtime (`dlopen`), not linked.
  - If it's missing, sandbox options are hidden or show "Set up sandbox…", and nothing else changes.
  - The base image is built or downloaded on first use, not at install.
  - Linux builds can also ship with the `sandbox` cargo feature off.
- **Three ways to use it**:
  1. **Thread toggle**: you start a thread "in sandbox" (or move a thread into one before its next turn). The whole agent runs inside the VM, as described below.
  2. **Agent request**: a local agent (including Home) calls `sandbox_run` to try something risky, such as an unknown install script, a dependency upgrade, or running untrusted code. You approve it, then a VM boots, runs the command, returns its output and is destroyed. The agent itself stays on the host.
  3. **Manual**: open a scratch sandbox with a terminal from the command palette.
- **Not open-ended**: there are no long-lived VMs to manage. Every VM is tied to a thread or a single request.

**When a thread runs in a sandbox:**

- **VMM**: libkrun (Hypervisor.framework on macOS arm64, KVM on Linux). `krun_start_enter` takes over the calling process, so each VM runs in its own `atrium-vmm` child process, and the daemon supervises them.
  - macOS: the binary must be signed with `com.apple.security.hypervisor`.
- **Base image**:
  - Built from an OCI image (Debian slim + git, node, python, build-essential, `claude`, `codex`, `agy`, ACP agents, `atrium-guest`, `atrium-mcp-shim`).
  - Unpacked to a directory rootfs and versioned. Atrium can rebuild it.
- **Per task (disposable)**:
  1. Clone the rootfs copy-on-write (APFS `clonefile` on macOS; reflink or overlay on Linux, *needs a spike*).
  2. Create a git worktree for the thread on the host and run its environment setup (3.6).
  3. Boot the VM with the rootfs, plus the worktree mounted at `/work` via virtio-fs. Changes land directly on the host branch, so there's no sync step.
  4. vsock ports: control (`atrium-guest`: exec, PTY, process stdio) and MCP (the shim bridges to the host MCP socket, scoped to the thread).
  5. The agent CLI runs inside the VM in full-access mode. Its stdio is tunnelled over vsock into the same `AgentAdapter`.
  6. When the task ends, the VM is destroyed and the rootfs clone deleted. The worktree and branch stay for review/PR. Optionally, the VM can be snapshotted for debugging.
- **Credentials**: forwarded only for the agent in use (Claude: OAuth token / API key env; Codex: `auth.json`). Never mount the host home dir.
- **Network**: on by default (agents need their APIs). Later: an egress allowlist through a host-side proxy.
- **Terminal into the sandbox**: a PTY through `atrium-guest`, shown in the same terminal component.

### 3.15 MCP server (`atrium-mcp`)

- One rmcp server inside the daemon, listening on a unix socket. Each agent session is launched with an `atrium-mcp-shim --thread <id> --token <t>` stdio server, which works for Claude, Codex and ACP alike. Tools are scoped to that thread.
- Tool groups:

| Group | Who gets it | Tools |
|---|---|---|
| `browser_*` | every thread | see 3.13 |
| `checks_*` | workspace threads | `checks_run`, `checks_status` |
| `notes_*` | every thread | `notes_list/read/search/write` |
| `thread_*` | every thread | `thread_delegate`, `thread_status`, `thread_handoff` |
| `sandbox_*` | when the sandbox is available | `sandbox_run` (approval required; read-only copy of the worktree unless writes are allowed), `status`, `logs` |
| `workspace_*` / `task_*` | Home only | list workspaces, start or queue threads, read results and handoffs, open diffs, manage the backlog: Home is the orchestrator |

### 3.16 Frontend (SolidJS)

- **Layout**:
  - A left rail: Inbox (badge), Home, Workspaces → Threads/Backlog.
  - A main thread view: messages, tool calls, inline approvals and question forms.
  - A right panel with tabs: Review card, Diff, Browser, Terminal, Files, Editor, Checks.
  - Split panes come later.
- **Libraries**:
  - CodeMirror 6 for the editor and for the diff/merge view (inline comments as decorations; they're sent back to the agent as a review message).
  - xterm.js with the WebGL addon for terminals.
  - Markdown rendered incrementally while it streams.
- **Keyboard-first**: a command palette, `⌘J` for the next item needing attention, quick thread switcher, global quick capture.
- **Performance rules**: token streams are batched per animation frame, long transcripts are virtualized, and nothing heavy runs on the UI thread.

### 3.17 Model routing (`scheduler`)

Atrium turns the build-orchestration rules from section 8 into a feature, so delegation, reviews, schedules and backlog tasks pick models automatically:

```toml
[routing]
default    = "claude:sonnet-5.5"
backend    = "codex:gpt-6.1-sol"
frontend   = "claude:sonnet-5.5"
systems    = "agy:gemini-4-argon-high"
utility    = "agy:gemini-3.8-flash-medium" # tests, docs, triage, summaries
escalation = ["claude:opus-5.5", "codex:gpt-6-astra", "agy:gemini-4-argon-high"]

[routing.rules]
review_cross_provider = true   # reviewer is never the author's provider
escalate_after_failed_fix_rounds = 2
escalate_across_providers = true

[routing.limits]                # per model, on top of 3.11 limits
"codex:gpt-6-astra" = 1
"agy:gemini-3.8-flash-medium" = 4
```

- A task kind comes from the backlog label, the delegating agent's hint, or a cheap classification by the utility model. You can always override it.
- The routing scoreboard (pass rate, review findings, cost per model × task kind) is collected automatically from thread outcomes. It powers suggestions like "Sol has passed 9 of 10 backend tasks; Sonnet 6 of 10".

## 4. Performance budgets (targets)

- Cold start to interactive: < 400 ms (macOS, M-series); reconnecting to a running daemon with full state: < 100 ms.
- App idle RSS (excluding agents and Chromium): < 150 MB; daemon idle RSS: < 50 MB.
- Token streaming: 60 fps, no dropped input with 10 active threads.
- Browser pane: ≥ 30 fps screencast, < 80 ms input-to-frame locally.
- Conflict radar: an incremental update in < 200 ms per thread change.
- Sandbox: < 1 s from "start" to guest ready, excluding agent CLI startup.

## 5. Milestones

**M0: Spikes (de-risk, ~1–2 weeks)**
- **First**: orchestration setup (8.7): `git init`, `docs/agents/` scaffolding, and a smoke test of each headless launcher (Claude subagent, `codex exec`, `agy -p`) in a scratch worktree, including permission modes and network for builds.

1. From Rust: Claude stream-json with permission control requests, Codex app-server, `agy` stream-json (permissions + per-session MCP), and one ACP agent. Normalize all four into the event enum.
2. Daemon ↔ Tauri over a unix socket: streaming, reconnect with event replay, an agent surviving an app restart.
3. CDP screencast into a Tauri canvas with input forwarding. Measure fps and latency on macOS *and* WebKitGTK/Linux.
4. Tauri + Solid: stream 10k tokens per second through the client bridge without jank.

**M1: Daemon + Home + attention**
- Cargo workspace, protocol + TS generation, SQLite store, `atriumd` with its lifecycle (detached start, single instance, crash reaping, launchd/systemd option), `atrium-client`, Tauri shell, tray.
- Home chat with Claude Code and Codex, the read-only + approval policy, resume.
- Thread state machine, inbox v1, approval queue, question forms, native notifications, `⌘J`.
- Approval rules ("always…") and the hard-deny list.

**M2: Workspaces + definition of done**
- Add folder(s) and threads, a worktree per thread, checkpoints and revert.
- Worktree environments: copy, setup/teardown, port leases, preview processes.
- Checks: `atrium.toml`, the automatic runner, the auto-fix loop, `checks_*` MCP tools, review cards with risk flags, "unverified" labelling.
- Diff view with inline comments, terminal, editor.
- Commit/push/PR through `gh`. ACP adapter.

**M3: Browser**
- Discovery and settings, per-thread contexts, the screencast pane with take-over, the browser MCP tools.
- Preview opened in the thread's browser, screenshot evidence on review cards.

**M4: Sandbox (optional feature)**
- **Spike**: boot libkrun on macOS from a directory rootfs, then a virtio-fs share, a vsock round-trip and hypervisor-entitlement signing.
- **Runtime**: detection and `dlopen`, the "Set up sandbox" flow, and the image builder on first use.
- **4a**: `sandbox_run` one-shot VMs that agents can request with approval, including "run it in a sandbox" for hard-denied actions.
- **4b**: "Run this thread in a sandbox": the agent runs inside the VM over vsock, with the MCP shim in the guest and a sandbox terminal.

**M5: Multi-agent**
- Home orchestrator tools (`workspace_*`, `task_*`).
- `thread_delegate` for every thread, with depth limits and budgets.
- Cross-provider auto-review with accept/dismiss and acceptance tracking.
- Conflict radar, base drift + rebase (agent- or human-resolved), merge queue.
- Workspace notes + handoff notes + "spawn follow-ups".

**M6: Scale**
- Backlog with quick capture and GitHub/Linear import; queue-to-thread.
- Concurrency scheduler, rate-limit-aware pausing, usage dashboard, per-thread budgets, sandbox caps.
- Scheduled runs.
- Approval presets per workspace.
- Model routing (3.17): `[routing]` config, task-kind classification, escalation ladder, automatic routing scoreboard.

**M7: Extras**
- Search across threads (FTS5), timeline/replay of tool calls per thread.
- Provider handoff: continue a thread with another provider, seeded with the summary, the handoff note and the diff.
- Fan-out & compare: the same prompt to N agents/worktrees, side-by-side diffs and checks, pick a winner.
- Design mode, stacked threads, worktree pool.

**M8: Polish → open source**
- Settings, keybindings, themes, packaging (notarized dmg with the hypervisor entitlement; AppImage/deb), docs, license, contributor guide.

## 6. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Claude CLI's stream-json control protocol is not a stable public API | Pin versions, add contract tests, fall back to `claude-code-acp` via ACP |
| `agy` stream-json format, permission handling and per-session MCP are undocumented | Spike in M0 with recorded transcripts as fixtures; pin the `agy` version; if host-side approvals aren't possible, run `agy` threads in `accept-edits`/`plan` modes or inside the sandbox |
| Provider terms for third-party clients using subscription logins | Check each provider's current terms before relying on subscription auth; API keys are supported as a fallback |
| Daemon lifecycle: orphaned agents/browsers/VMs, version skew between app and daemon | PID tracking + reaping, single-instance lock, protocol version handshake, hand over to the new daemon only when idle |
| Runaway cost from auto-fix loops, delegation trees and auto-review | Attempt caps, a delegation depth limit, per-thread budgets, reviewer acceptance-rate tracking |
| Noisy cross-review comments | Reviewer emits only high-confidence comments; easy dismiss; turn it off per workspace if the acceptance rate is low |
| Slow per-worktree setup (dependency installs) | Global package caches, optional build cache, worktree pool (M7) |
| Port collisions with non-Atrium processes | The allocator checks the OS before leasing; leases are re-validated on preview start |
| Approval rules too permissive | A hard-deny list that rules can't override; rules visible and editable; scoped by default to thread/workspace |
| libkrun on macOS is Apple Silicon only; needs a hypervisor entitlement + signing | The sandbox is optional, so Intel Macs just don't get it; handle signing in the M4 spike |
| CoW rootfs on Linux (no clonefile) | Spike reflink, overlayfs (fuse-overlayfs) and a qcow2 disk overlay; pick one |
| WebKitGTK performance on Linux (canvas, xterm WebGL) | Measure in M0; tune frame size/quality; fall back to the xterm canvas renderer |
| Chrome blocks remote debugging on the default profile | Always use a dedicated `--user-data-dir` (we do anyway) |
| Credentials inside sandboxes | Forward only the needed credential, scoped per VM; destroyed with the VM |

## 7. Open questions (for later)

- License (MIT vs Apache-2.0)?
- How deep should the editor go: quick edits only, or LSP?
- Default network policy for sandboxes.
- Which ACP agents to test first (OpenCode, Goose)?
- Workspace notes: private by default (current plan) or committed?
- Merge queue default: local fast-forward or PR + auto-merge?
- Linear import in M6, or GitHub only at first?
- Split panes / multiple threads visible at once: M2 or later?

## 8. Build orchestration (how we build Atrium with agents)

This section covers building Atrium with a team of agents. The same rules later become Atrium's own default routing (3.17).

### 8.1 The roster

| Model | Provider / runs through | Profile | Role on this project |
|---|---|---|---|
| **Claude Opus 5.5** | Anthropic / Claude Code | Strongest Claude; best long-context reasoning and judgment | **Orchestrator / tech lead.** Plan, contracts, task breakdown, integration, final review of contract-touching changes. Exactly one instance. |
| **GPT-6 Astra** | OpenAI / Codex | OpenAI flagship for coding, computer use and security; expensive (~$10/$50 per M tokens) | **Second-opinion architect and escalation.** Milestone gate reviews, the hardest debugging, security review. Used sparingly. |
| **Gemini 4 Argon** | Google / Antigravity (`agy`) | Google's new frontier model, pitched at coding, migrations and cyber defense (~$2/$10 intro) | **Systems and security specialist.** Sandbox/FFI/unsafe work, adversarial testing of policy and isolation, the Antigravity and ACP adapters. |
| **GPT-6.1 Sol** | OpenAI / Codex | Close to Astra on agentic coding at ~1/5 the price; slower output (~$2/$10) | **Backend workhorse.** Most Rust crates. Long, well-specified background tasks. |
| **Claude Sonnet 5.5** | Anthropic / Claude Code | Fast, strong coder, good at UI iteration | **Frontend + Claude-integration workhorse.** SolidJS UI, Tauri bridge, Claude adapter, mid-tier reviews. |
| **Gemini 3.8 Flash** | Google / Antigravity (`agy`) | Cheap, fast, 1M context (~$0.75/$3.75) | **High-volume utility.** Tests, fixtures, docs, boilerplate, codebase exploration, check-failure triage, handoff/commit summaries. |

Prices and positioning come from vendor announcements and early reviews (Sept–Oct 2026), and benchmark claims aren't independently verified. **The routing below is a starting hypothesis that gets calibrated in M0 (8.6).**

### 8.2 Routing by task type

| Task type | Primary | Fallback | Reviewer (always a different provider) |
|---|---|---|---|
| Architecture, crate boundaries, protocol types, traits, task breakdown | Opus 5.5 | Astra | Astra (gate review) |
| Rust core/backend (daemon, store, git, env, checks, scheduler) | Sol 6.1 | Sonnet 5.5 | Sonnet 5.5; Opus if a contract is touched |
| Systems / FFI / `unsafe` / sandbox / vsock / guest | Argon | Astra | Astra + Opus |
| Frontend (SolidJS, CodeMirror, xterm, canvas, Tauri bridge) | Sonnet 5.5 | Sol 6.1 | Sol 6.1, plus screenshots as evidence |
| Claude adapter | Sonnet 5.5 | Opus 5.5 | Sol 6.1 |
| Codex adapter | Sol 6.1 | Astra | Sonnet 5.5 |
| Antigravity adapter (`agy` stream-json) and ACP adapter | Argon | 3.8 Flash | Sonnet 5.5 |
| Tests, fixtures, recorded protocol transcripts, docs, boilerplate | 3.8 Flash | Sonnet 5.5 | Spot-check by the owner of the code under test |
| Codebase exploration, summaries, triage of failing checks | 3.8 Flash | n/a | n/a |
| Security review (policy, hard-deny, MCP tokens, socket permissions, sandbox, browser isolation) | Argon **and** Astra | n/a | Opus integrates the findings |
| Performance profiling and optimization | Astra | Opus 5.5 | Sol or Sonnet |
| Hard bugs (after escalation) | Opus or Astra (whichever is from the other provider than the author) | Argon | n/a |

**Pairing rule:** the reviewer is never from the author's provider (Anthropic ↔ OpenAI ↔ Google). Different models catch different bugs.

### 8.3 Escalation ladder

```
3.8 Flash  →  Sonnet 5.5 / Sol 6.1  →  Opus 5.5 / Argon / Astra
 (cheap)        (workhorses)              (frontier, scarce)
```

- A task escalates when:
  - it fails its checks after **2 automatic fix rounds**,
  - the agent reports low confidence or a blocker, or
  - it needs to change a contract.
- **Escalate across providers**: when a Sol task fails, the next attempt goes to Opus or Argon, not Astra. This gives a fresh perspective instead of the same blind spots.
- The escalated agent gets the original brief, the handoff note and the failing output. It doesn't get the previous transcript.

### 8.4 How parallel work runs (each milestone)

```
 ┌─ 1. Contract (serial) ─┐  ┌─ 2. Fan-out (parallel) ─┐  ┌─ 3. Integrate (serial) ─┐  ┌─ 4. Gate ─┐
 │ Opus: types, traits,   │→ │ N implementers, each in │→ │ merge queue in          │→ │ demo +    │
 │ schemas, test stubs,   │  │ its own worktree, owning│  │ dependency order, cross-│  │ Astra/    │
 │ task briefs; Astra     │  │ disjoint paths; Flash   │  │ provider review, Opus   │  │ Argon gate│
 │ reviews; Pedro approves│  │ writes tests alongside  │  │ resolves integration    │  │ review;   │
 └────────────────────────┘  └─────────────────────────┘  └─────────────────────────┘  │ Pedro OK  │
                                                                                       └───────────┘
```

1. **Contract phase (serial)**:
   - Opus writes the milestone's interfaces first: `atrium-protocol` types, trait signatures, SQLite schema, MCP tool schemas, and failing test stubs.
   - Then one brief per task.
   - Astra reviews the contracts and Pedro approves them. After that the contracts are frozen for the milestone.
2. **Fan-out (parallel)**:
   - One worktree and branch per task (`agent/<model>/<task-id>`).
   - Each brief lists the **paths the agent owns**. Touching anything outside them, or changing a contract, means stopping and filing a *change request* to Opus rather than editing.
   - Flash writes tests and fixtures in parallel against the frozen contracts.
3. **Integration (serial)**:
   - Finished tasks enter a merge queue in dependency order. Each one is rebased, re-checked, cross-reviewed and merged.
   - Opus handles integration fixes and contract change requests. A contract change means bumping the contract and notifying the affected tasks.
4. **Gate**:
   - The milestone demo runs end to end. All checks are green.
   - Argon + Astra do a security and architecture review where relevant.
   - Pedro signs off, and open questions get decided.

**Concurrency limits** (human review bandwidth is the real limit):

| Model | Max parallel |
|---|---|
| Opus 5.5 | 1 (orchestrator) |
| Astra | 1, gates and escalations only |
| Argon | 2 |
| Sol 6.1 | 3 |
| Sonnet 5.5 | 3 |
| 3.8 Flash | 4 |
| **Total implementers at once** | **≤ 6** |

**No agent-to-agent chat.** Coordination goes only through files in the repo:

```
docs/agents/
├─ briefs/<task-id>.md      goal, context, owned paths, contract, DoD, out of scope
├─ handoffs/<task-id>.md    done / not done / gotchas / follow-ups (written at the end)
├─ decisions.md             ADR-style log; only Opus writes, others propose
├─ change-requests.md       contract changes requested by implementers
└─ routing.md               scoreboard per model × task type (8.6)
```

**Definition of done for every task**:
- `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test -p <owned crates>`
- `pnpm typecheck && pnpm test` for frontend tasks
- Screenshots for UI tasks, and benchmark numbers for performance-sensitive tasks (section 4 budgets)
- A handoff note
- No changes outside owned paths

**Task brief template**:

```markdown
# <task-id>: <title>
Model: <primary> · Reviewer: <other provider> · Depends on: <ids>
## Goal
## Context (links to PLAN sections, contracts, related handoffs)
## Owned paths
## Contract to implement (types/traits/schemas: do not modify)
## Definition of done (exact commands + evidence)
## Out of scope
## Escalate if
```

### 8.5 Assignments by milestone

| Milestone | Opus 5.5 | Sol 6.1 | Sonnet 5.5 | Argon | 3.8 Flash | Astra |
|---|---|---|---|---|---|---|
| **M0 spikes** | Event enum and normalization; picks the winner of each spike | Codex app-server client; daemon socket + replay spike | Claude stream-json + control protocol; CDP screencast → canvas; streaming perf | `agy` stream-json client + ACP client | Recorded protocol transcripts as fixtures; spike reports | Review of the M0 findings and architecture |
| **M1 daemon + Home + attention** | Protocol, store schema, thread state machine, policy model | `atriumd`, store, `atrium-client`, lifecycle, policy engine | Tauri shell, tray, Home chat UI, inbox, approval queue, question forms | Adversarial tests for the hard-deny list and approval rules; socket permissions | Unit tests, docs, protocol TS generation checks | Gate review |
| **M2 workspaces + done** | `atrium.toml` schema, checks/env contracts | `atrium-git` (worktrees, checkpoints), `atrium-env` (ports, setup), `atrium-checks` | Diff + inline comments, terminal, editor, review cards | ACP adapter for production | Risk-flag heuristics, check-output trimming, fixtures | n/a |
| **M3 browser** | Browser MCP tool schemas | CDP backend: discovery, contexts, input, a11y snapshot | Screencast pane, take-over, preview/evidence UI | Isolation review (contexts, profiles, pipe) | Tool tests against local fixture pages | n/a |
| **M4 sandbox** | Sandbox contracts, integration | Image builder, rootfs CoW | Sandbox UI, terminal | **libkrun FFI, `atrium-vmm`, `atrium-guest`, vsock, MCP shim** | Guest tests, docs | **Security review of sandbox + credentials** |
| **M5 multi-agent** | Delegation/orchestrator design, budgets | Conflict radar, merge queue, rebase flow | Review/merge-queue UI, notes UI | Cross-review prompt + reviewer harness | Handoff templates, follow-up extraction | Gate review |
| **M6 scale** | Scheduler contracts | Scheduler, rate-limit pausing, budgets, cron | Usage dashboard, backlog UI | Approval presets review | GitHub/Linear import, quick capture | n/a |
| **M7–M8** | Prioritization, release plan | Search (FTS), provider handoff | Fan-out compare UI, design mode, polish | Packaging/signing (hypervisor entitlement) | Docs site, contributor guide | Final security pass |

### 8.6 Calibration (during M0)

- Run 3 representative tasks on 2–3 candidate models each (same brief, separate worktrees):
  - a Rust protocol client,
  - a SolidJS streaming component,
  - a Rust/FFI-adjacent task.
- Record pass rate after N rounds, review findings, tokens/cost and wall-clock time in `docs/agents/routing.md`.
- Re-route anything where the scoreboard disagrees with 8.2. Re-check after each milestone. New models (they ship every few weeks) enter through the same bake-off.

### 8.7 Running the orchestration from Claude Code

Until Atrium can build itself, the orchestrator is a **Claude Code session on Opus 5.5** (this one). Workers run headless in the background, each in its own worktree. Claude Code notifies the orchestrator when each worker finishes, so it never has to poll. MonoCode stays available for watching workers and as a fallback.

**Why Claude Code rather than MonoCode's `/operator`:**
- **Reliable wake-ups**: background agents and background shell commands notify the orchestrator when they exit. MonoCode's operator CLI only works during an active turn, and it was unclear whether completion reports wake it.
- **Full logs**: every worker writes a JSONL transcript to disk, instead of the last 3 exchanges.
- **Full control**: git, the merge queue, notes, `docs/agents/` and the checks are all scriptable directly.
- **The trade-off**: no live panes for Codex/agy workers, and stepping into a running worker is harder. Mitigated by resumable sessions (`codex exec resume`, `agy --conversation`) and by opening any worktree in MonoCode to watch or take over.

**How each model is launched** (all in the background, all in their own worktree under `../atrium-wt/<id>`):

| Model | Launch |
|---|---|
| Sonnet 5.5 / Opus 5.5 | Claude Code subagent (`Agent`, model `sonnet`/`opus`, background), told to work only in its worktree path |
| GPT-6.1 Sol / GPT-6 Astra | `codex exec -m <model> -C <worktree> -s workspace-write --json -o <last.md> "<prompt>" > .agents/logs/<id>.jsonl` |
| Gemini 4 Argon / 3.8 Flash | `agy -p "<prompt>" --model gemini-4-argon-high --mode <mode> --output-format stream-json > .agents/logs/<id>.jsonl`, run with cwd = worktree |

Calibrate in M0, before the first real dispatch:
- Which `agy --mode` lets a worker run `cargo test` headless without `--dangerously-skip-permissions`.
- Whether Codex's `workspace-write` sandbox needs network enabled (`-c sandbox_workspace_write.network_access=true`) for crate downloads.

**The task loop:**

1. **Brief**: write `docs/agents/briefs/<id>.md` and commit it on `main`.
2. **Worktree**: `git worktree add ../atrium-wt/<id> -b agent/<model>/<id>`.
3. **Dispatch** in the background with: *"Read `docs/agents/briefs/<id>.md` and do it. Finish by running the Definition of Done and writing `docs/agents/handoffs/<id>.md`."*
4. **Wait** for the completion notification (no polling). A long-interval fallback check catches hung workers.
5. **Verify, don't trust**: read the handoff file, re-run the DoD commands in the worktree, and check that no edits landed outside the owned paths (`git diff --stat` against the brief).
6. **Fix loop**: resume the same worker session with the trimmed failing output, at most 2 rounds. After that, escalate to the next-tier model from a different provider in the same worktree (8.3).
7. **Cross-provider review**:
   - A reviewer from another provider runs read-only in the worktree (Codex `-s read-only`, `agy --mode plan`, or a Claude subagent told not to edit) and writes `docs/agents/reviews/<id>.md`.
   - The orchestrator filters its comments. The accepted ones go back to the author.
8. **Merge**:
   - The orchestrator runs the merge queue: rebase in dependency order, full checks on `main`, merge, `git worktree remove`.
   - Workers never merge or push. Pushing to a remote needs Pedro.
9. **Status board**: the orchestrator keeps `docs/agents/status.md` up to date (task, model, state, worktree, log). Pedro can check it any time, or open a worktree in MonoCode to watch.

**Guardrails:**
- Concurrency limits from 8.4 (≤ 6 workers at once).
- Astra launches and security-sensitive tasks need Pedro's OK first.
- `.agents/logs/` is git-ignored.
- **Context hygiene**: the orchestrator reads handoffs and check results, not full transcripts. It opens logs only when debugging a failure.
- **Session rotation**: at milestone boundaries, a fresh orchestrator session starts from `PLAN.md` + `docs/agents/`, so nothing depends on chat history.
- **Multi-agent workflows (optional)**: Claude Code's Workflow tool can run fixed fan-out pipelines, like the M0 calibration bake-off. It only runs when Pedro explicitly asks for a workflow.

**MonoCode's role**:
- A viewer and an interactive fallback. Open the repo there to watch or take over any worktree.
- If an interactive, visible orchestration is ever preferred, the `/operator` + `app` CLI flow (`worktrees.create`, `sessions.start` with provider/model/effort/`worktreeCwd`, `sessions.send`, `sessions.archive`) maps onto the same task loop.

**Graduation**: once Atrium M2 works (workspaces, checks, inbox), the project moves into Atrium, which then builds itself. Every friction point from this phase becomes an Atrium backlog task. One example: reading only the last few exchanges of a worker is why Atrium's `thread_status` returns the handoff + checks + diff summary.

### 8.8 Your role (Pedro)

- Approve contracts at the start of each milestone.
- Sign off at each gate (demo + review).
- Decide open questions and anything that changes security or approval defaults.
- Everything else reaches you through the handoffs/inbox, and only when it needs you.
