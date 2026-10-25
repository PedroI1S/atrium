import { Tabs } from "@kobalte/core/tabs";
import { ClipboardCheck, Database, Globe, SquareTerminal, GitCompare, type LucideIcon } from "lucide-solid";
import { For } from "solid-js";
import { DB_ROWS, DIFF_LINES, TERMINAL_LINES } from "../data/dummy";
import { RIGHT_TABS, type RightTabId, rightTab, setRightTab, showToast } from "../state";

const ICONS: Record<RightTabId, LucideIcon> = { review: ClipboardCheck, diff: GitCompare, browser: Globe, terminal: SquareTerminal, database: Database };

export function RightPanel() {
  return (
    <Tabs value={rightTab()} onChange={(v) => setRightTab(v as RightTabId)} class="right" data-atrium-part="right-panel">
      <Tabs.List class="right-tabs" aria-label="Right panel">
        <For each={RIGHT_TABS}>
          {(t) => {
            const Icon = ICONS[t.id];
            return (
              <Tabs.Trigger class="right-tab" value={t.id}>
                <Icon class="icon-sm" /> {t.label}
              </Tabs.Trigger>
            );
          }}
        </For>
        <Tabs.Indicator class="right-indicator" />
      </Tabs.List>

      <Tabs.Content class="right-body" value="review">
        <div class="card">
          <div class="card-title">Review card <span class="pill" data-tone="ready">Ready</span></div>
          <p class="card-text">Rework session token refresh: 2 files changed, 41 additions, 9 deletions.</p>
          <ul class="checks">
            <li data-ok="true">cargo test -p atrium-core <span class="muted">14 passed</span></li>
            <li data-ok="true">cargo clippy -- -D warnings</li>
            <li data-ok="false">Definition of done: evidence missing for "retry once"</li>
          </ul>
          <div class="approval-actions">
            <button class="btn" onClick={() => showToast("Changes requested (dummy)")}>Request changes</button>
            <button class="btn btn-primary" onClick={() => showToast("Approved (dummy)")}>Approve</button>
          </div>
        </div>
      </Tabs.Content>

      <Tabs.Content class="right-body flush" value="diff">
        <pre class="diff">
          <For each={DIFF_LINES}>{(l) => <div class="diff-line" data-kind={l.kind}>{l.text}</div>}</For>
        </pre>
      </Tabs.Content>

      <Tabs.Content class="right-body" value="browser">
        <div class="browser-bar"><span class="url">localhost:5173/settings</span></div>
        <div class="placeholder">Browser pane: a CDP screencast lands here in M0-08.</div>
      </Tabs.Content>

      <Tabs.Content class="right-body flush" value="terminal">
        <pre class="terminal">{TERMINAL_LINES.join("\n")}<span class="caret" /></pre>
      </Tabs.Content>

      <Tabs.Content class="right-body" value="database">
        <table class="grid">
          <thead><tr><th>id</th><th>thread</th><th>state</th><th>agent</th><th class="num">tokens</th></tr></thead>
          <tbody>
            <For each={DB_ROWS}>
              {(r) => (
                <tr><td>{r.id}</td><td class="mono">{r.thread}</td><td>{r.state}</td><td>{r.agent}</td><td class="num mono">{r.tokens.toLocaleString()}</td></tr>
              )}
            </For>
          </tbody>
        </table>
      </Tabs.Content>
    </Tabs>
  );
}
