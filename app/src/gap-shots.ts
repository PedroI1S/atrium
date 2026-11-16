// `ATRIUM_BENCH=shots-gaps` with `ATRIUM_SHOT_DIR`: pictures of the M1-23 surfaces for review,
// run by app/scripts/screenshots.py (`gaps`) in a throwaway ATRIUM_HOME, light and dark.
//
// As in chat-shots.ts, the chats are SEEDED IN THIS WINDOW (the synthetic agent can't raise
// approvals in a picture-friendly way). Nothing seeded is the daemon's, except:
//   - the batch result: a real refusal from the daemon for each seeded approval;
//   - Settings → Security: the real `HARD_DENY_DEFAULTS` from the app;
//   - the enforcement details show an example Partial descriptor (seeded), since the synthetic
//     agent reports no coverage at all.

import { MINUTE_MS, Seed, UNKNOWN_THREAD_WAIT_MS, both, click, seeds } from "./chat-shots";
import { setRenaming } from "./chat/thread-actions";
import { holdThread } from "./chat/threads";
import { apply, daemon } from "./daemon/client";
import type { EnforcementDescriptor, InboxItem } from "./daemon/types";
import { writeReport } from "./platform";
import { sleep, until } from "./roundtrip";
import { setInboxPick, setPaletteOpen, setPanel, setScreen, setSettingsSection } from "./state";
import { openEnforcementDetails } from "./ui/Enforcement";
import { openThread } from "./ui/Sidebar";

const SETTLE_MS = 150;

/** What a Claude session with a plugin and an untested CLI could report (seeded). */
const PARTIAL_EXAMPLE: EnforcementDescriptor = {
  level: "partial",
  reasons: [
    { code: "uninspected_config", message: "A plugin's hooks are not inspected: ~/.claude/plugins/formatter" },
    { code: "untested_version", message: "Claude Code 2.3.0 isn't in Atrium's tested versions" },
    { code: "class_not_intercepted", message: "MCP tool calls run without Atrium seeing them first" },
  ],
  provider: "claude",
  cli_version: "2.3.0",
  version_tested: false,
  coverage: [
    { class: "read", coverage: "intercepted", evidence: "fixtures/agents/claude/gate-read" },
    { class: "search", coverage: "intercepted", evidence: null },
    { class: "edit", coverage: "intercepted", evidence: "fixtures/agents/claude/gate-edit" },
    { class: "move", coverage: "intercepted", evidence: null },
    { class: "delete", coverage: "intercepted", evidence: null },
    { class: "execute", coverage: "intercepted", evidence: "fixtures/agents/claude/gate-deny" },
    { class: "fetch", coverage: "provider_gated", evidence: null },
    { class: "mcp", coverage: "not_intercepted", evidence: null },
    { class: "subagent", coverage: "unknown", evidence: null },
    { class: "provider_specific", coverage: "not_applicable", evidence: null },
  ],
  config_sources: [
    { kind: "user_settings", path: "~/.claude/settings.json", sha256: "9f2c1e4b7a0d3c58e6b1f0a2d4c6e8b0a1c3e5f7", inspected: true },
    { kind: "plugin", path: "~/.claude/plugins/formatter", sha256: null, inspected: false },
  ],
};

function approvalSeed(id: string, title: string, command: string, at: number): Seed {
  const s = new Seed(id, title, at).start().user(1, title).tool("c1", "Bash", "execute", command, null);
  s.add({ type: "approval_requested", request_id: "q1", call_id: "c1", name: "Bash", kind: "execute", title: command, input: { command }, reason: "Home asks before anything is changed.", rememberable: true });
  return s;
}

const typeInto = (selector: string, text: string): void => {
  const el = document.querySelector<HTMLInputElement>(selector);
  if (!el) throw new Error(`no field ${selector}`);
  el.value = text;
  el.dispatchEvent(new InputEvent("input", { bubbles: true, data: text, inputType: "insertText" }));
};

export async function runGapShots(): Promise<void> {
  await until("the daemon", () => daemon().connection.type === "connected" && daemon().loaded);
  const notes: Record<string, unknown> = {};
  const now = Date.now();
  const seeded = seeds(now);
  const more = [
    approvalSeed("seed-approval-2", "Update the lockfile", "pnpm install --lockfile-only", now - 2 * MINUTE_MS),
    approvalSeed("seed-approval-3", "Format the workspace", "cargo fmt --all", now - MINUTE_MS),
  ];
  const all = [...seeded.all, ...more];
  for (const summary of seeded.summaries) apply({ type: "thread_updated", summary });
  for (const s of more) apply({ type: "thread_updated", summary: s.summary({ type: "needs_approval" }, { pending_approvals: 1, attention_since_ms: now - MINUTE_MS }) });
  for (const s of all) holdThread(s.id);
  await sleep(UNKNOWN_THREAD_WAIT_MS);
  for (const s of all) apply({ type: "snapshot", thread_id: s.id, events: s.events, has_older: false, truncated: false, last: s.events[s.events.length - 1]!.id });
  const inbox: InboxItem[] = [
    ...more.map((s, i) => ({ thread_id: s.id, reason: "approval" as const, since_ms: now - (i + 1) * MINUTE_MS, title: s.title, detail: s.title })),
    ...seeded.inbox,
  ];
  apply({ type: "inbox_updated", items: inbox });

  // 1. The thread header's ⋯ menu.
  setPanel({ open: true });
  openThread(seeded.stream.id);
  await sleep(SETTLE_MS);
  notes.menu = click('.card.main [data-atrium-part="thread-menu-button"]');
  await both("thread-menu");
  click('.card.main [data-atrium-part="thread-menu-button"]');

  // 2. Renaming in the header.
  setRenaming(seeded.stream.id);
  await both("thread-rename");
  setRenaming(null);

  // 3. The archive confirmation.
  click('.card.main [data-atrium-part="thread-menu-button"]');
  await sleep(SETTLE_MS);
  notes.archive = click('[data-atrium-part="thread-menu"] [data-action="archive"]');
  await until("the confirmation", () => document.querySelector('[data-atrium-part="confirm"]') !== null);
  await both("confirm-archive");
  click('[data-atrium-part="confirm"] .btn:not(.pri)');
  await until("the confirmation to close", () => document.querySelector('[data-atrium-part="confirm"]') === null);

  // 4. The enforcement badge in the header, and the details (an example Partial descriptor).
  openThread(seeded.approval.id);
  await both("enforcement-badge");
  openEnforcementDetails(PARTIAL_EXAMPLE, "Clean the build folder and rebuild");
  await until("the details", () => document.querySelectorAll('[data-atrium-part="enforcement-coverage"] .enf-row').length > 0);
  await both("enforcement-details");
  document.querySelector('[data-atrium-part="enforcement-details"]')?.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
  await until("the details to close", () => document.querySelector('[data-atrium-part="enforcement-details"]') === null);

  // 5. Batch approvals: all three selected, then the daemon's real refusals (it has none of them).
  setScreen("inbox");
  setInboxPick(more[0]!.id);
  await until("three approval checkboxes", () => document.querySelectorAll('[data-atrium-part="batch-pick"]').length === 3);
  notes.selectAll = click('[data-atrium-part="batch-all"]');
  await both("inbox-batch-selected");
  notes.batchAllow = click('[data-atrium-part="batch-allow"]');
  await until("the batch result", () => document.querySelector('[data-atrium-part="batch-result"]') !== null);
  notes.batchResult = document.querySelector('[data-atrium-part="batch-result"]')?.textContent;
  await both("inbox-batch-result");

  // 6. The rules editor: a valid rule with its preview, then a refused command.
  setScreen("settings");
  setSettingsSection("approvals");
  await sleep(SETTLE_MS);
  click('[data-atrium-part="setting-action-add-rule"]');
  await until("the rules editor", () => document.querySelector('[data-atrium-part="rule-command"]') !== null);
  typeInto('[data-atrium-part="rule-command"]', "cargo test");
  typeInto('[data-atrium-part="rule-path-scope"]', "/home/you/projects/**");
  await until("the preview", () => document.querySelector('[data-atrium-part="rule-preview"]') !== null);
  document.querySelector('[data-atrium-part="rule-editor"]')?.scrollIntoView({ block: "end" });
  await both("settings-rule-editor");
  typeInto('[data-atrium-part="rule-command"]', "cargo test && rm -rf target");
  await until("the error", () => document.querySelector('[data-atrium-part="rule-editor"] .err') !== null);
  await both("settings-rule-editor-refused");
  click('[data-atrium-part="rule-editor"] .btn.ghost');

  // 7. Settings → Security.
  setSettingsSection("security");
  await until("the hard-deny list", () => document.querySelectorAll('[data-atrium-part="always-blocked"] .s-item').length > 0);
  notes.blocked = document.querySelectorAll('[data-atrium-part="always-blocked"] .s-item').length;
  await both("settings-security");

  // 8. The palette's "This chat" commands.
  openThread(seeded.stream.id);
  setPaletteOpen(true);
  await until("the palette", () => document.querySelector(".pal-input") !== null);
  typeInto(".pal-input", "this chat");
  await both("palette-this-chat");
  setPaletteOpen(false);

  await writeReport({ kind: "shots", pass: true, notes });
}
