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
| Editor AI | Optional and off until you pick a provider: Copilot-style ghost text, next-edit prediction, ⌘K inline edit. Backends: Copilot LSP, your own local server (never bundled or auto-started), remote APIs, free hosted models (`untrusted`: manual only, current file only). Context engine in `atriumd`; the one place it calls models directly (3.21) |
| Databases | SQLite/libSQL + PostgreSQL: query editor, drawDB-style schema diagram with design → migration, Drizzle Studio-style table view with staged edits; agents get read-only `db_*` tools, writes go through approvals (3.19) |
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
│  db         SQLite/libSQL + Postgres: introspection, queries, staged edits │
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
│  ├─ atrium-db/             DB drivers, introspection, schema model, diff → migrations
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
  - **Approvals (found in the M0-00 smoke test)**: headless `agy` can't prompt and ignores hook `allow` grants, but enforces hook `deny`. So Atrium runs `agy` with `--dangerously-skip-permissions` plus a `PreToolUse` hook (matcher `"*"`) that calls back into `atriumd`'s policy engine over the socket: `allow`/`deny` per call, and `ask` becomes "deny + inbox approval + retry". Re-test with each `agy` release; a native host-approval channel would replace this.
  - Event stream: NDJSON with `event: init | step_update | result`. `step_update` carries `tool_name`, `state` (ACTIVE/DONE/ERROR), parameters and errors; `result` carries the response, usage and `denied_actions`.
  - **Spike (M0)**: per-session MCP injection (`agy mcp` manages servers globally, so per-session scoping may need a generated config), and the `--input-format stream-json` multi-turn mode.
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
  - Import, export, retention and backup of chat histories: see 3.18.
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
| `db_*` | every thread with a configured connection | `db_connections`, `db_schema`, `db_query` (read-only), `db_explain`, `db_execute` (approval required; never for production): see 3.19 |
| `sandbox_*` | when the sandbox is available | `sandbox_run` (approval required; read-only copy of the worktree unless writes are allowed), `status`, `logs` |
| `workspace_*` / `task_*` | Home only | list workspaces, start or queue threads, read results and handoffs, open diffs, manage the backlog: Home is the orchestrator |

### 3.16 Frontend (SolidJS)

- **Layout**:
  - A left rail: Inbox (badge), Home, Workspaces → Threads/Backlog.
  - A main thread view: messages, tool calls, inline approvals and question forms.
  - A right panel with tabs: Review card, Diff, Browser, Terminal, Files, Editor, Checks, Database.
  - Split panes, saved layouts and theming: see 3.20.
- **Libraries**:
  - CodeMirror 6 for the editor and for the diff/merge view (inline comments as decorations; they're sent back to the agent as a review message).
  - xterm.js with the WebGL addon for terminals.
  - Markdown rendered incrementally while it streams.
  - Kobalte + Corvu primitives, solid-dnd, solid-virtual, Shiki, lucide (3.20).
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

### 3.18 Chat history: import, export, retention

Every provider already keeps its own transcripts:
- Claude Code: `~/.claude/projects/<project>/<session>.jsonl`, plus `subagents/`
- Codex: `~/.codex/sessions/…` rollouts
- `agy`: `~/.gemini/antigravity-cli/conversations/*.db` + `brain/<id>/…/transcript_full.jsonl`
- MonoCode: `monocode.db`

Atrium doesn't replace these. It keeps a **normalized copy** in its own store, plus the native session ID so any thread can be resumed in its own CLI.

- **Source of truth**: the per-thread append-only `events` table (3.3). Each thread records `{provider, native_session_id, native_path}`, so Atrium can always point back to the original transcript.
- **Import** (read-only, never modifies the source):
  - Importers for Claude Code, Codex, `agy` and MonoCode histories. They map native records into the same event enum as the live adapters (3.2) and reuse their parsers and recorded fixtures.
  - Imported threads appear under Home or under the workspace that matches their cwd, marked "imported". Re-imports are idempotent (keyed by native session ID and record offset).
  - First-run offer: "Import your existing conversations?", per provider, with a preview of the count and size.
  - The Atrium **build archive** (8.7) uses the same format, so Atrium's own construction history imports at graduation.
- **Export**: a thread or workspace to Markdown (readable, with tool calls collapsed) or JSONL (lossless events + manifest). Redaction runs first (below).
- **Search**: FTS5 over messages, tool calls, notes and handoffs, across live and imported threads (the full UI comes in M7).
- **Retention**: per-workspace policy, keep forever (default) / N days / archive only.
  - Archived threads keep their handoff and summary but drop bulky tool output: large tool results go into a content-addressed blob store (zstd), which can be pruned.
  - **Delete** removes Atrium's copy and, optionally, the provider's native copy, with confirmation and the paths shown.
- **Redaction**: a secret scanner (API-key formats, tokens, `.env` values, private keys) runs on export, before any remote sync, and on demand. Matches are replaced with typed placeholders, and the originals are never written to export files.
- **Backup**:
  - The SQLite DB is snapshotted with `VACUUM INTO` on a schedule (default daily, keep 7) into Atrium's data dir. It's covered by Time Machine or a backup tool of your choice.
  - "Restore from snapshot" is in settings.
- **Privacy**: everything stays local. Nothing is uploaded unless you explicitly export or enable remote access.

### 3.19 Databases: query editor, schema diagram, table view (`atrium-db`)

Built-in database tooling for **SQLite/libSQL and PostgreSQL**. It's for you (a query editor, a drawDB-style schema diagram, a Drizzle Studio-style table view) and for agents (`db_*` MCP tools: read-only by default, writes through approvals).

**Connections**
- **Discovery per workspace**: Atrium proposes connections it finds, and you confirm them before it connects. It looks in:
  - `.env*` (`DATABASE_URL`, `POSTGRES_URL`, `TURSO_DATABASE_URL`, …),
  - `drizzle.config.*`, the `prisma/schema.prisma` datasource,
  - `docker-compose.yml` Postgres services (mapped to localhost),
  - `*.db` / `*.sqlite` files in the repo.
- **Config**: `atrium.toml` references connections by name and environment variable. It never stores secrets:

  ```toml
  [[databases]]
  name = "dev"
  url_env = "DATABASE_URL"   # or url_keychain = "atrium/dev-db"
  env = "development"        # development | staging | production
  ```

  Credentials typed into the UI go to the OS keychain (macOS Keychain / Secret Service via `keyring`), never into the store.
- **Environment tags**:
  - `production` connections get a red banner and typed confirmation for any human write, and are always read-only for agents.
  - `development` is the default for local files and Docker.
- **Drivers**:
  - SQLite through **`rusqlite`**, the same library as Atrium's own store, run on a blocking pool. Using `sqlx` here as well would bring a second `libsqlite3-sys` link and conflict.
  - PostgreSQL through **`tokio-postgres`** + rustls.
  - Remote libSQL/Turso through the `libsql` crate's remote-only (HTTP) feature, which avoids a second native SQLite.
  - SSH tunnels come later.
- **Introspection** into one normalized schema model, shared by every feature below:
  - Model: schemas → tables → columns (type, nullability, default, generated), primary keys, foreign keys, indexes, unique/check constraints, comments; plus views and enums.
  - SQLite: `sqlite_schema` + `pragma_table_info/foreign_key_list/index_list`.
  - Postgres: `pg_catalog` + `information_schema`.
- **Per-thread databases (later)**: worktree setup (3.6) can give a thread its own copy, so agents mutate a copy instead of the shared dev DB. SQLite: a `clonefile` copy. Postgres: `CREATE DATABASE … TEMPLATE …`, or provider branching such as Neon.

**Query editor**
- CodeMirror 6 + `@codemirror/lang-sql`, with **schema-aware autocomplete** from introspection. Run the statement under the cursor or the selection. Multiple result tabs. Cancel (`pg_cancel_backend`, or SQLite interrupt), statement timeout, row limit with streamed pagination.
- **Statement classification** with `sqlparser-rs` (read / write / DDL / destructive):
  - `DROP`, `TRUNCATE`, and `UPDATE`/`DELETE` without `WHERE` need confirmation.
  - On production connections, every write needs typed confirmation.
  - There's an optional manual-transaction mode (BEGIN … COMMIT/ROLLBACK buttons).
- `EXPLAIN` / `EXPLAIN ANALYZE` (Postgres) shown as a plan tree with cost hotspots.
- **History** per connection in Atrium's store, plus **saved queries** as `.sql` files in the workspace (committable). Export results to CSV/JSON.
- **"Ask the agent"**: sends the query, its error or plan, and the relevant schema slice to the current thread. Atrium has no built-in text-to-SQL model; the thread's own agent does it.

**Schema diagram (drawDB-style)**
- **View**:
  - Each table is a card showing columns, types and PK/FK/nullable markers. Foreign keys are drawn as edges with crow's-foot cardinality.
  - Tables are color-grouped by schema.
  - **Auto layout** with ELK (`elkjs`, layered, in a web worker). Manual positions persist per workspace in `.atrium/diagrams/<connection>.json`.
  - Search, **focus mode** (a table plus its n-hop neighborhood), minimap, zoom/pan, export to SVG/PNG.
  - Export the schema as **DBML**, SQL DDL or a **Mermaid `erDiagram`**. Mermaid is handy for docs and as compact context for agents.
- **Design → migration** (nothing is ever applied directly):
  1. Edit on the canvas: add or rename tables and columns, change types/nullability/defaults, add foreign keys, indexes and uniques. This builds a *target* schema next to the live one, with changes highlighted.
  2. A diff engine (current → target, over the normalized model) generates **dialect-correct migration SQL**. For SQLite that includes the table-rebuild procedure for changes `ALTER TABLE` can't do.
     - v1 covers tables, columns, foreign keys, indexes and uniques. Views, enums, triggers and policies are shown read-only.
  3. Choose an output:
     - **(a)** write a migration file into the project's migrations folder, or
     - **(b) hand off to an agent**: Atrium opens a thread whose brief is the schema diff, e.g. *"apply this to the Drizzle schema in `src/db/schema.ts` and generate the migration with `drizzle-kit`"*. ORM-specific code stays with agents that know the project.
  4. Applying a migration is a reviewed, explicit action: through the query editor, or the project's own migration command run by the agent or the checks.
- **Correctness check**: every generated migration is tested by applying it to a scratch copy (a SQLite file copy, or a Postgres template DB), re-introspecting, and comparing with the target model.

**Table view (Drizzle Studio-style)**
- **Grid**:
  - Virtualized, with **keyset pagination** on the primary key, so large tables stay fast.
  - Type-aware cells: a JSON viewer, booleans, timestamps with timezone, bytes shown as size/hex, enums as dropdowns.
  - Per-column filters (operator + value), sort, hide/reorder columns, quick search.
- **Relations**: FK cells link to the referenced row. A side panel shows *reverse* relations (rows that reference this one), with counts.
- **Staged edits**:
  - Edit cells, add rows (pre-filled with defaults), and delete or duplicate rows. Pending changes are highlighted and shown as a diff.
  - **Review** shows the generated parameterized SQL. **Apply** runs everything in one transaction; **Discard** drops it.
  - Each `UPDATE`/`DELETE` checks the row's original values, so if the row changed in the meantime, the conflict is reported instead of overwritten.
  - Applied batches store best-effort inverse statements for **undo**.
  - Tables without a primary key are read-only (SQLite falls back to `rowid` where it's safe).

**Agent tools (`db_*` MCP)**

| Tool | Access |
|---|---|
| `db_connections` | Lists connection *names* and environments. Agents never see credentials. |
| `db_schema(conn, tables?)` | Compact schema (DBML-like text) to save tokens; optional table filter and n-hop neighborhood |
| `db_query(conn, sql, limit)` | **Read-only**, enforced three ways: (1) `sqlparser` classification; (2) execution inside a `READ ONLY` transaction (Postgres) or a read-only connection with `query_only` (SQLite); (3) row limit + statement timeout |
| `db_explain(conn, sql)` | Query plan |
| `db_execute(conn, sql)` | Writes/DDL. Always goes to the **approval queue**, showing the SQL and a dry-run estimate (affected rows, computed in a transaction that is rolled back). Never offered for `production` connections. Approval rules (3.12) can allow e.g. "writes to the dev SQLite in this workspace". |

Recommended (optional): give agents a least-privilege Postgres role for their connections. Note that the tools don't stop an agent from reading `.env` inside its worktree. Real isolation for risky database work is the sandbox or a per-thread database copy.

**UI**: a **Database** tab in the workspace right panel, with sub-tabs Query / Tables / Diagram. Home lists connections across workspaces.

### 3.20 Look & feel: theming, layout, customization

Atrium has **its own design language**. Other tools (e.g. MonoCode) serve only as references for density and polish; nothing is copied. Everything visual is driven by tokens, so the look can change without code changes.

**Window chrome (native feel)**
- macOS:
  - Tauri `titleBarStyle: "Overlay"` + `hiddenTitle` + custom `trafficLightPosition`, so the traffic lights sit inline with the tab strip. Drag regions use `data-tauri-drag-region`.
  - Native blur via the `window-vibrancy` crate (`NSVisualEffectView`), with CSS `backdrop-filter` on panels.
- Linux: client-side decorations with the same tab-strip header. Blur only where the compositor supports it; otherwise a solid-color fallback (decided by the M0 UI shell spike).

**Design tokens**
- Every color, radius, spacing step, shadow, font, font size, density and motion duration is a CSS custom property. Components never hard-code values.
- Three layers:
  1. *primitive* (palette, scales),
  2. *semantic* (`--surface-1`, `--text-muted`, `--accent`, `--danger`, `--diff-add`, …),
  3. *component* (`--tab-active-bg`, `--composer-radius`, …).
- Contrast is checked against WCAG AA for every shipped theme (a CI check on the token files).

**Themes**
- A theme is a token file (`themes/<name>.toml`), loaded at runtime. Switching is instant, with no rebuild or reload. Atrium ships dark and light defaults plus a high-contrast variant, and can follow the system appearance.
- **VS Code theme import**:
  - UI colors are mapped from `colors` onto semantic tokens.
  - Syntax colors come straight from `tokenColors` through **Shiki** (chat, markdown) and a CodeMirror theme generated from the same data (editor, diffs, query editor).
  - Terminal colors come from `terminal.ansi*`.
- **User overrides**: per-token overrides in settings, an accent color picker, and an escape hatch `~/.config/atrium/theme.css` (like Firefox's `userChrome.css`) for anything tokens don't cover. Atrium documents a set of stable `data-atrium-*` attributes and part names, so user CSS keeps working across updates.

**Layout**
- A panel system: the left sidebar, the tab strip with draggable tabs, **split panes** (e.g. chat next to a file preview, or two threads side by side), and a right panel (Explorer / Changes / History / Review / Browser / Terminal / Database). Any panel can be moved, resized, collapsed or popped out to its own window.
- Layouts are saved per workspace, and there are **named presets**: *Default*, *Review* (diff large, chat narrow), *Focus* (chat only), *Browser* (pane large), *Data* (database tabs). Switch from the command palette.
- **Density**: compact / comfortable. UI font, code font and size are configurable separately. Any spacing token can be scaled.

**Keybindings**: `~/.config/atrium/keybindings.json`, with VS Code-style `when` contexts. Everything in the command palette can be bound.

**Building blocks (SolidJS)**
- Headless, accessible primitives: **Kobalte** (dialogs, menus, combobox/command palette, select, popover, tabs) and **Corvu** (resizable panels, drawers). Styling is entirely ours via tokens.
- Other libraries:
  - `@thisbeyond/solid-dnd` for tab and panel dragging
  - `@tanstack/solid-virtual` for transcripts, file trees and grids
  - `lucide-solid` icons + `material-icon-theme` file icons
  - Shiki for highlighting
  - incremental markdown with path → file-chip rendering (file-type icons on inline paths)
- **Motion**: short, purposeful transitions (panel resize, streaming caret, inbox state changes), all disabled under `prefers-reduced-motion`.

**Visual direction**: decided in the M0 UI shell spike, where 2–3 directions are built as live token sets on the same shell and Pedro picks one. The others ship as alternative themes.

### 3.21 AI in the editor: completion, next-edit prediction, inline edit (`completion`)

Goal: editing should feel like **VS Code + Copilot or Zed**. Ghost text appears as you type, `Tab` accepts it, suggestions can point at the *next edit* nearby, and `⌘K` edits a selection in place. But it's **entirely optional and always the user's choice**.

**Principles**
1. **Off by default; nothing runs until you choose.** No provider is preselected, no model is bundled, and Atrium never installs or starts a local model server. On first use (status bar or command palette), a setup sheet explains each option and its trade-offs (table below), and you pick one or keep it off.
2. **Local models are your call.** They're private but heavy (RAM/VRAM, battery, heat). Atrium only *connects* to a server you run yourself (Ollama, llama.cpp, LM Studio). It can detect running ones, but never launches them.
3. **Every provider has a trust level**, and the level decides what's allowed (below). Free hosted models (e.g. OpenCode's free models) are **`untrusted` by default**: free tiers commonly allow the provider to log or train on submitted code, so Atrium treats them that way unless you change it after reading their terms.
4. **Never in the way.** Typing is never blocked. Every request is cancellable and hard-capped by a timeout. A late answer is dropped.
5. **The one exception to "Atrium doesn't call models itself."** Agents always run through their native CLIs (3.2). Editor AI needs answers in a few hundred milliseconds, so `atriumd` has one contained subsystem that talks to completion backends directly. Nothing else uses it.

**Features**
- **Inline completion (ghost text)**:
  - *Automatic*: appears after a short pause in typing.
  - *Manual*: only on `⌥\`.
  - `Tab` accepts all, `⌥→` accepts a word, `⌥⇧→` a line, `Esc` dismisses.
  - Multi-line suggestions render as a block.
  - Several candidates can be cycled with `⌥]` / `⌥[`.
- **Next-edit prediction** (like Copilot's next edit suggestions or Zed's edit prediction):
  - From your recent edits, the model predicts the next change *anywhere nearby*, e.g. the other call sites after a rename, or the matching field after adding a struct member.
  - Shown as a gutter arrow plus an inline diff preview. `Tab` jumps to it, `Tab` again applies it.
  - Needs a provider that supports edit prediction (an endpoint or a local Zeta-style model).
- **Inline edit (`⌘K`)**:
  - Select code (or just place the cursor), type an instruction, and see the result as an **inline diff** in the editor. Accept with `⌘↵`, reject with `Esc`, refine by typing a follow-up.
  - Uses the configured chat/fast provider.
  - **"Send to agent"** turns the same instruction into a thread task when it's bigger than a local rewrite.

**Backends** (one `CompletionBackend` trait in the daemon, with capabilities `inline` / `next_edit` / `edit`):

| Backend | What it is | Trust (default) | Notes |
|---|---|---|---|
| **Copilot (LSP)** | GitHub's official Copilot language server, spoken to over LSP `textDocument/inlineCompletion` | `trusted` (your GitHub account and its data settings) | Official sign-in, no API keys. The daemon includes a minimal LSP client for this even if full LSP support for the editor comes later. Closest to the "VS Code Copilot" feel. |
| **Local server** | Ollama / llama.cpp / LM Studio on `localhost` (FIM, chat, edit-prediction models) | `local` | You run it; Atrium only connects. Optional `keep_alive` so the model unloads when idle, and "pause on battery". |
| **Remote API (FIM)** | OpenAI-style `/completions` with `suffix` (e.g. DeepSeek, Codestral, vLLM) | `trusted` if it's your paid account, otherwise `untrusted` | API key in the OS keychain |
| **Free hosted models** | e.g. OpenCode's free models | **`untrusted`** | Strict rules below; a warning on setup and in the status bar |
| **Chat models** | Any chat API (Claude/GPT/Gemini via API key) | per provider | Used for `⌘K` inline edit and as a prompt-based completion fallback (too slow and costly for automatic ghost text) |

Model IDs are never hard-coded. Atrium lists what each backend reports, and you choose.

**Trust levels and what they allow**

| | `local` | `trusted` | `untrusted` |
|---|---|---|---|
| Automatic mode | ✅ | ✅ | ❌ manual only (unless explicitly overridden per workspace) |
| Consent | none | once per workspace | once per workspace + repeated warning in the status bar |
| Context sent | full context engine | full context engine | **current file only**, no neighbouring files or edit history |
| Secret redaction on the payload (3.18 scanner) | – | ✅ | ✅ + skip the request entirely if anything secret-like is in range |
| Workspaces tagged `private` | ✅ | ✅ | ❌ never |

Always excluded, for every trust level: `exclude` globs (default `.env*`, `*.pem`, `*.key`, `secrets/**`), git-ignored files, files outside the workspace, and anything open from a `production` database connection (3.19).

**Context engine** (in the daemon; this decides quality more than the model does)
- **Recent edits**: a ring buffer of your last N edits across open files. This is the strongest signal for both completion and next-edit prediction.
- **Syntax-aware windows**: tree-sitter parses the file. Prefix and suffix are cut at statement/function boundaries, the enclosing scope is always included, and multi-line completions stop at the end of the block.
- **Related code**: definitions of symbols near the cursor (tree-sitter tags across open files and imported modules, or LSP when available), plus snippets from open tabs ranked by similarity to the cursor region.
- **Token budget** per backend, with priority: cursor scope → recent edits → definitions → neighbouring files.
- **Post-processing**: trim overlap with the existing suffix, balance brackets, fix indentation, drop suggestions that only repeat existing code, de-duplicate candidates.
- The context build budget is < 10 ms. It runs on daemon threads, never the UI thread.

**Configuration** (global settings, overridable per workspace in `atrium.toml`):

```toml
[editor.ai]
enabled  = false            # off until you run setup
provider = ""               # chosen in setup, e.g. "copilot" | "local" | "deepseek" | "opencode-free"
mode     = "automatic"      # "automatic" | "manual"; forced to "manual" for untrusted providers
next_edit = true            # only if the provider supports it
debounce_ms = 300
timeout_ms  = 800           # completion; inline edit uses its own, longer timeout
exclude = [".env*", "**/*.pem", "**/*.key", "secrets/**"]

[[editor.ai.providers]]
name  = "local"
kind  = "ollama"            # copilot-lsp | ollama | openai-compatible | fim-openai | chat
url   = "http://localhost:11434"
model = "<your FIM-capable coder model>"
keep_alive = "5m"           # let the model unload when idle
trust = "local"

[[editor.ai.providers]]
name  = "opencode-free"
kind  = "openai-compatible"
url   = "<endpoint>"
model = "<model id>"
trust = "untrusted"         # default for free tiers; changing it requires confirmation
```

**UI**
- **Status bar badge**: provider + trust icon + mode, e.g. `● Copilot · Auto`, `⌂ local · Auto`, `⚠ opencode-free · Manual`.
  - Clicking it opens a popover: Off / Manual / Automatic, provider switcher, "pause for 1 hour", "exclude this file".
  - One click always turns AI off.
- **Command palette**: `AI: Set Up Editor AI…`, `AI: Toggle`, `AI: Switch Provider…`, `AI: Trigger Suggestion`, `AI: Inline Edit`.
- Ghost text and diff previews use theme tokens (`--ghost-text`, `--diff-add`, `--diff-del`).
- The CodeMirror ghost-text layer starts from existing extensions (e.g. `codemirror-copilot`) as a reference. Next-edit previews and inline-edit diffs are our own CodeMirror decorations.

**Keybindings** (defaults; rebindable in `keybindings.json`, 3.20):

| Key | Command | When |
|---|---|---|
| `⌥\` | `ai.trigger` | `editorTextFocus` |
| `Tab` | `ai.accept` / `ai.nextEdit.jumpOrApply` | `hasGhostText \|\| hasNextEdit`, and `!completionPopupOpen` |
| `⌥→` / `⌥⇧→` | `ai.acceptWord` / `ai.acceptLine` | `hasGhostText` |
| `⌥]` / `⌥[` | `ai.nextCandidate` / `ai.prevCandidate` | `hasGhostText` |
| `Esc` | `ai.dismiss` | `hasGhostText \|\| hasNextEdit \|\| inlineEditOpen` |
| `⌘K` | `ai.inlineEdit` | `editorTextFocus` |
| `⌘↵` | `ai.inlineEdit.accept` | `inlineEditOpen` |

- Precedence: CodeMirror's own completion popup (e.g. SQL autocomplete, 3.19) wins `Tab`/`Enter`.
- With nothing pending, `Tab` indents and `Esc` behaves normally.
- On macOS, `⌥\` types `«` on many keyboard layouts, so bindings match the physical key (`event.code`). This gets tested across layouts, with `Ctrl+\` as the fallback.

**Measurement (local only, never sent anywhere)**: accept rate, partial-accept rate, latency p50/p95 per provider and per feature. Shown in the usage dashboard (3.11) along with tokens and cost for paid providers, with optional daily caps.

## 4. Performance budgets (targets)

- Cold start to interactive: < 400 ms (macOS, M-series); reconnecting to a running daemon with full state: < 100 ms.
- App idle RSS (excluding agents and Chromium): < 150 MB; daemon idle RSS: < 50 MB.
- Token streaming: 60 fps, no dropped input with 10 active threads.
- Browser pane: ≥ 30 fps screencast, < 80 ms input-to-frame locally.
- Conflict radar: an incremental update in < 200 ms per thread change.
- Database: first page of any table (keyset) in < 150 ms locally; schema diagram layout for 300 tables in < 1 s (in a worker); autocomplete in < 30 ms.
- Sandbox: < 1 s from "start" to guest ready, excluding agent CLI startup.

## 5. Milestones

**M0: Spikes (de-risk, ~1–2 weeks)**
- **First**: orchestration setup (8.7): `git init`, `docs/agents/` scaffolding, and a smoke test of each headless launcher (Claude subagent, `codex exec`, `agy -p`) in a scratch worktree, including permission modes and network for builds.

1. From Rust: Claude stream-json with permission control requests, Codex app-server, `agy` stream-json (permissions + per-session MCP), and one ACP agent. Normalize all four into the event enum.
2. Daemon ↔ Tauri over a unix socket: streaming, reconnect with event replay, an agent surviving an app restart.
3. CDP screencast into a Tauri canvas with input forwarding. Measure fps and latency on macOS *and* WebKitGTK/Linux.
4. Tauri + Solid: stream 10k tokens per second through the client bridge without jank.
5. **UI shell**: a clickable mock with dummy data. Overlay title bar + traffic lights, vibrancy, tab strip, resizable split panes, sidebar, right panel, composer, command palette.
   - The token pipeline with runtime theme switching + one imported VS Code theme.
   - **2–3 visual directions** as token sets on the same shell, for Pedro to choose.
   - Measured on macOS and WebKitGTK/Linux: blur fallback, scroll/resize smoothness at 120 Hz, idle memory.

**M1: Daemon + Home + attention**
- Cargo workspace, protocol + TS generation, SQLite store, `atriumd` with its lifecycle (detached start, single instance, crash reaping, launchd/systemd option), `atrium-client`, Tauri shell, tray.
- Home chat with Claude Code and Codex, the read-only + approval policy, resume.
- Thread state machine, inbox v1, approval queue, question forms, native notifications, `⌘J`.
- Approval rules ("always…") and the hard-deny list.
- History basics: native session IDs on every thread, export to Markdown/JSONL, daily DB snapshots.

**M2: Workspaces + definition of done**
- Add folder(s) and threads, a worktree per thread, checkpoints and revert.
- Worktree environments: copy, setup/teardown, port leases, preview processes.
- Checks: `atrium.toml`, the automatic runner, the auto-fix loop, `checks_*` MCP tools, review cards with risk flags, "unverified" labelling.
- Diff view with inline comments, terminal, editor.
- Commit/push/PR through `gh`. ACP adapter.
- History import: Claude Code, Codex, `agy` and MonoCode importers, and the build archive (8.7) at graduation.

**M3: Browser**
- Discovery and settings, per-thread contexts, the screencast pane with take-over, the browser MCP tools.
- Preview opened in the thread's browser, screenshot evidence on review cards.

**M3.5: Databases (3.19)**
- **a**: connection discovery + keychain, SQLite/libSQL + Postgres drivers, introspection → schema model, query editor (autocomplete, classification, history, EXPLAIN), table view (keyset grid, filters, relations, staged edits), `db_*` MCP tools with read-only enforcement and approvals, view-only schema diagram.
- **b**: design mode, diff engine → migration SQL (SQLite rebuild procedure included), round-trip verification on scratch copies, hand-off-to-agent flow, DBML/Mermaid export.

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
- History retention policies, blob pruning, the secret scanner/redaction, delete-with-native-copy.

**M7: Extras**
- Search across threads (FTS5), timeline/replay of tool calls per thread.
- Provider handoff: continue a thread with another provider, seeded with the summary, the handoff note and the diff.
- Fan-out & compare: the same prompt to N agents/worktrees, side-by-side diffs and checks, pick a winner.
- Design mode, stacked threads, worktree pool.
- Editor AI (3.21), can be pulled forward right after the M2 editor:
  - **a**: setup sheet, trust levels + consent + exclusions, backend trait with local-server/remote FIM/chat backends, context engine (recent edits, tree-sitter windows, related code, post-processing), ghost-text completion, ⌘K inline edit.
  - **b**: Copilot LSP backend (minimal LSP client).
  - **c**: next-edit prediction.

**M8: Polish → open source**
- Settings, keybindings, themes, packaging (notarized dmg with the hypervisor entitlement; AppImage/deb), docs, license, contributor guide.

## 6. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Claude CLI's stream-json control protocol is not a stable public API | Pin versions, add contract tests, fall back to `claude-code-acp` via ACP |
| `agy` stream-json format and per-session MCP are undocumented; approvals depend on skip-permissions + a deny-enforcing hook | Recorded transcripts as fixtures; pin the `agy` version; contract test that hook `deny` is still enforced under skip-permissions (if a release breaks it, `agy` threads only run inside the sandbox) |
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
| Editor AI leaks code or secrets to a third party | Off by default; trust levels with `untrusted` restricted to manual + current file; consent per workspace; exclude globs + git-ignored files; secret scanner on payloads; status-bar trust indicator |
| Local models drain memory/battery | Never bundled or auto-started; connect-only; `keep_alive` unload; pause on battery; a single request in flight |
| Agent read-only DB access bypassed (writable CTEs, side-effecting functions, multi-statements) | Three layers: parser classification + `READ ONLY` transaction / `query_only` + timeouts; Argon adversarial suite; optional least-privilege role; production connections are never writable by agents |
| Migration diff engine produces wrong SQL (especially SQLite rebuilds) | Round-trip verification on a scratch copy before any migration is offered; v1 limited to tables/columns/FKs/indexes/uniques |
| Two SQLite builds linked (`rusqlite` + `sqlx`) | Use `rusqlite` everywhere, `tokio-postgres` for Postgres, `libsql` remote-only |
| Chrome blocks remote debugging on the default profile | Always use a dedicated `--user-data-dir` (we do anyway) |
| Credentials inside sandboxes | Forward only the needed credential, scoped per VM; destroyed with the VM |

## 7. Open questions (for later)

- License (MIT vs Apache-2.0)?
- How deep should the editor go: quick edits only, or LSP? (Editor AI, 3.21, is planned either way; its minimal LSP client for Copilot could grow into full LSP support.)
- Default network policy for sandboxes.
- Which ACP agents to test first (OpenCode, Goose)?
- Workspace notes: private by default (current plan) or committed?
- Merge queue default: local fast-forward or PR + auto-merge?
- Linear import in M6, or GitHub only at first?
- Split panes / multiple threads visible at once: M2 or later?
- Databases: MySQL and DuckDB after v1? SSH tunnels? Per-thread database copies by default?
- Visual direction: chosen from the 2–3 directions built in the M0 UI shell spike.

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
| **M0 spikes** | Event enum and normalization; picks the winner of each spike; token architecture for the UI shell | Codex app-server client; daemon socket + replay spike | Claude stream-json + control protocol; CDP screencast → canvas; streaming perf; **UI shell + visual directions** | `agy` stream-json client + ACP client | Recorded protocol transcripts as fixtures; spike reports | Review of the M0 findings and architecture |
| **M1 daemon + Home + attention** | Protocol, store schema, thread state machine, policy model | `atriumd`, store, `atrium-client`, lifecycle, policy engine | Tauri shell, tray, Home chat UI, inbox, approval queue, question forms | Adversarial tests for the hard-deny list and approval rules; socket permissions | Unit tests, docs, protocol TS generation checks | Gate review |
| **M2 workspaces + done** | `atrium.toml` schema, checks/env contracts | `atrium-git` (worktrees, checkpoints), `atrium-env` (ports, setup), `atrium-checks` | Diff + inline comments, terminal, editor, review cards | ACP adapter for production | Risk-flag heuristics, check-output trimming, fixtures | n/a |
| **M3 browser** | Browser MCP tool schemas | CDP backend: discovery, contexts, input, a11y snapshot | Screencast pane, take-over, preview/evidence UI | Isolation review (contexts, profiles, pipe) | Tool tests against local fixture pages | n/a |
| **M3.5 databases** | Normalized schema model + `db_*` tool contracts | Drivers, introspection, statement classifier, staged-edit SQL generation, diff → migration engine | Query editor, table grid, ER diagram (ELK), design mode | Adversarial tests of read-only enforcement (side-effecting functions, `COPY`, writable CTEs, multi-statement tricks) | Fixture databases (SQLite + Postgres in Docker), round-trip migration tests, docs | Review of the migration engine's correctness |
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

| Model | Launch (verified 2026-10-22 smoke test, task `M0-00`) |
|---|---|
| Sonnet 5.5 / Opus 5.5 | Claude Code subagent (`Agent`, model `sonnet`/`opus`, background), told to work only in its worktree path |
| GPT-6.1 Sol / GPT-6 Astra | `scripts/agent-run codex <model> <id> <prompt-file>` runs `codex exec -m <model> -C <worktree> -s workspace-write -c sandbox_workspace_write.network_access=true --json -o <last.md>` |
| Gemini 4 Argon / 3.8 Flash | `scripts/agent-run agy <model> <id> <prompt-file>` runs `agy -p --model <model> --mode accept-edits --dangerously-skip-permissions --output-format stream-json` with cwd = worktree, **guarded by the policy hook** |

**Smoke-test findings:**
- **Claude subagent:** works as-is.
- **Codex:** `workspace-write` blocks network by default, so `cargo search` failed until `network_access=true` was set. Codex's sandbox still confines writes to the worktree. Pass `< /dev/null`, or Codex waits for extra stdin.
- **`agy` 1.4.5:**
  - Headless mode auto-denies every command that would need a prompt.
  - A `PreToolUse` hook returning `allow`, even with `permissionOverrides`, does **not** grant it.
  - Project `.agents/settings.json` permissions aren't read; only the user-level `~/.gemini/config/config.json` is.
  - A hook **`deny` is enforced even under `--dangerously-skip-permissions`**. So `agy` workers skip permissions, and `scripts/agy-policy.py` (matcher `"*"`) is the policy. `scripts/agent-run` installs it **only into each worker's worktree** (`.agents/hooks.json`, git-ignored, using absolute paths), never into the main checkout, so Pedro's own `agy` sessions are unaffected (D-006). It fails closed:
    - shell commands on an allowlist,
    - git only on the worker's own branch,
    - file tools only inside the worktree,
    - browser/MCP/subagent/scheduling tools denied.
  - Some tool names used at runtime (`write_file`) aren't in the advertised tool list. The policy logs denials, so new names surface quickly.
- **`.agents/` belongs to `agy`** (its project customization root). Orchestration logs live in `.orchestration/` instead.

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
   - The orchestrator runs the merge queue: rebase in dependency order, full checks on `main`, merge, archive the task's transcripts (below), `git worktree remove`.
   - Workers never merge or push.
   - **Push policy (D-005, D-007)**: all work happens on the `dev` branch. Plan, docs and orchestration tooling are pushed to `dev` right away. Merged Atrium code is pushed to `dev` at every handoff (8.9). `main` moves only at milestone gates, after sign-off (a merge of `dev`).
9. **Status board**: the orchestrator keeps `docs/agents/status.md` up to date (task, model, state, worktree, log). Pedro can check it any time, or open a worktree in MonoCode to watch.

**Chat histories during the build** (two tiers):

| Tier | Where | What | Who reads it |
|---|---|---|---|
| **1. Curated, in git** | `docs/agents/` | briefs, handoffs, reviews, decisions, **session logs** | Everyone; the orchestrator starts each session here |
| **2. Raw archive, local only** | `.orchestration/archive/<task-id>/` (git-ignored) | every transcript for a task + manifest | Debugging, audits, resuming, and the Atrium import at graduation |

- **Session logs**: at the end of every orchestrator session (and at each milestone gate), the orchestrator writes `docs/agents/sessions/<date>-<n>.md`. It covers what was dispatched, decided, escalated and left open, with links to the briefs and handoffs. A new orchestrator session reads the latest log, `status.md` and `decisions.md` before anything else.
- **Archiving a task** (step 8 of the task loop, after merge or abandonment): collect every transcript the task produced from its native location:

  | Source | Native location |
  |---|---|
  | Claude subagent | `~/.claude/projects/<project>/<session>/subagents/` |
  | Codex | `.orchestration/logs/<task>.jsonl` and its `~/.codex/sessions` rollout |
  | `agy` | `.orchestration/logs/<task>.jsonl` and `~/.gemini/antigravity-cli/brain/<id>/…/transcript_full.jsonl` |

  This includes reviewer and escalation sessions as well as the worker.
  1. Compress them with zstd into `.orchestration/archive/<task-id>/`.
  2. Write `manifest.json` with one entry per session (role, model, provider, native session ID for resuming, start/end, tokens, cost, exit status), plus the check outputs and the merge commit.
- **Orchestrator sessions** are archived the same way under `.orchestration/archive/_orchestrator/<session-id>/` at rotation.
- **Never in git**: raw transcripts can contain env values, file contents or tokens, and the repo may go public. Only the curated tier 1 is committed. Anything leaving the machine goes through redaction first (3.18).
- **Backup**: `~/Documents` is covered by Time Machine, which includes the archive. The provider-native stores stay where they are; the archive is our copy, so tool cleanups or format changes don't lose history.
- **Graduation**: at M2, Atrium's importer reads `.orchestration/archive/` (same manifest format), so Atrium's own build history becomes searchable inside Atrium.

**Guardrails:**
- Concurrency limits from 8.4 (≤ 6 workers at once).
- Astra launches and security-sensitive tasks need Pedro's OK first.
- `.orchestration/logs/` is git-ignored.
- **Context hygiene**: the orchestrator reads handoffs and check results, not full transcripts. It opens logs only when debugging a failure.
- **Session rotation**: at milestone boundaries, a fresh orchestrator session starts from `PLAN.md` + `docs/agents/`, so nothing depends on chat history.
- **Multi-agent workflows (optional)**: Claude Code's Workflow tool can run fixed fan-out pipelines, like the M0 calibration bake-off. It only runs when Pedro explicitly asks for a workflow.

**MonoCode's role**:
- A viewer and an interactive fallback. Open the repo there to watch or take over any worktree.
- If an interactive, visible orchestration is ever preferred, the `/operator` + `app` CLI flow (`worktrees.create`, `sessions.start` with provider/model/effort/`worktreeCwd`, `sessions.send`, `sessions.archive`) maps onto the same task loop.

**Graduation**: once Atrium M2 works (workspaces, checks, inbox), the project moves into Atrium, which then builds itself. Every friction point from this phase becomes an Atrium backlog task. One example: reading only the last few exchanges of a worker is why Atrium's `thread_status` returns the handoff + checks + diff summary.

### 8.9 Two drivers: handoffs and checkpoints

Pedro and a partner build Atrium **in turns, never at the same time**. Whoever is working is the *driver*: they run their own orchestrator (Claude Code on their machine), dispatch workers and merge. The full protocol is in `docs/agents/HANDOFF.md`. In short:

- **Branches**: `dev` is the shared working branch. Everything a driver does lands there and is pushed by the end of the shift. `main` only moves at milestone gates. Worker branches (`agent/*`) are pushed at handoff if their task is still in flight.
- **The baton**: `docs/agents/DRIVER.md` on `dev` records who is driving. `scripts/handoff start` claims it after syncing, and refuses if the other person is still active. It then prints:
  - the last session log's `## Next`,
  - all commits since the last handoff,
  - the status board,
  - the in-flight branches.

  `scripts/handoff end` checks that the shift is safe to hand over, then frees the baton:
  - no headless workers running,
  - every worktree committed and its branch pushed,
  - a session log committed this shift.

  It then tags `handoff/<date>-<driver>` and pushes. The tags are the **checkpoints**: `git log handoff/<tag>..dev` shows exactly what the other person did.
- **Never hand off with a worker running.** Native agent sessions can't be resumed on the other machine. An interrupted task gets a `partial` handoff note and a WIP commit on its branch, and the next driver dispatches a fresh worker from there.
- **Shared rules live in the repo**: `CLAUDE.md` (orchestrator sessions) and `AGENTS.md` (Codex/`agy` workers), not in Claude Code memory, which is per machine.
- **Machine-local things**: worktrees, raw transcripts, agent CLI logins, and the generated `agy` hook. These are per machine and are recreated or kept locally. `scripts/doctor` checks a machine's tools, GitHub access and the `agy` policy self-test.
- **Authority**: either driver can approve contracts, give a phase go and sign off a gate. Each decision is recorded with who made it. Changes to security/approval defaults need both people.
- **Records carry the driver's name**: status board owner column, session logs, decisions, and archive manifests (with the machine that holds the raw transcripts).

### 8.8 Your role (Pedro)

- Approve contracts at the start of each milestone.
- Sign off at each gate (demo + review). Sign-off is what moves `main` (a merge of `dev`). The same applies to the partner when they're the driver (8.9).
- Decide open questions and anything that changes security or approval defaults.
- Everything else reaches you through the handoffs/inbox, and only when it needs you.
