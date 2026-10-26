// Main-card views besides chat: Inbox, Editor, Database, and the placeholder shown where a
// popped-out view used to be.

import { AppWindow, ExternalLink } from "lucide-solid";
import { For, Show, createMemo, type JSX } from "solid-js";
import { DB_ROWS, INBOX, INBOX_GROUPS, needsYouCount, topicOf } from "../data/dummy";
import { inboxFilter, inboxPick, setInboxFilter, setInboxPick, setPanel, showToast } from "../state";
import type { ViewId } from "../window-model";
import { bringBack, canEdit, focusPopout, model, patchView, popOut } from "../windows";
import { Dot, IconButton, Seg, Spacer } from "./controls";

export function InboxView() {
  const visible = createMemo(() => INBOX.filter((i) => inboxFilter() === "all" || (inboxFilter() === "approvals" ? i.group === "ask" : i.group === "ready")));
  const topics = new Set(INBOX.map((i) => topicOf(i.threadId).id)).size;
  return (
    <>
      <header class="head" data-tauri-drag-region>
        <span class="title lg">Inbox</span>
        <span class="muted small">{needsYouCount()} need you, across {topics} topics</span>
        <Spacer />
        <Seg label="Filter" value={inboxFilter()} onChange={setInboxFilter} options={[{ value: "all", label: "All" }, { value: "approvals", label: "Approvals" }, { value: "reviews", label: "Reviews" }]} />
      </header>
      <div class="inbox-list">
        <For each={INBOX_GROUPS}>
          {(g) => (
            <Show when={visible().some((i) => i.group === g.id)}>
              <div class="igrp"><Dot state={g.state} />{g.label}</div>
              <For each={visible().filter((i) => i.group === g.id)}>
                {(it) => (
                  <div class="item" classList={{ on: inboxPick() === it.id }} role="button" tabindex="0" aria-pressed={inboxPick() === it.id}
                    onClick={() => { setInboxPick(it.id); setPanel({ open: true }); }}
                    onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); setInboxPick(it.id); setPanel({ open: true }); } }}>
                    <Dot state={g.state} label />
                    <span class="body">
                      <span class="ttl">{it.ask}<Show when={it.code}> <span class="code">{it.code}</span></Show></span>
                      <span class="src">{it.source}</span>
                      <Show when={it.actions.length}>
                        <span class="acts">
                          <For each={it.actions}>{(a) => <button type="button" class="btn sm" classList={{ pri: a.primary, ghost: a.ghost }} onClick={(e) => { e.stopPropagation(); showToast(`${a.label} (dummy)`); }}>{a.label}</button>}</For>
                        </span>
                      </Show>
                    </span>
                    <span class="tm">{it.age}</span>
                  </div>
                )}
              </For>
            </Show>
          )}
        </For>
      </div>
      <footer class="statusb"><span>J / K to move</span><span>↵ open</span><span>A allow · D deny</span><Spacer /><span>⌘J jumps here from anywhere</span></footer>
    </>
  );
}

/** Dummy editor: a plain textarea stands in for CodeMirror (out of scope); its text is view state. */
export function EditorBody() {
  const content = () => model().views.editor.content;
  const lines = createMemo(() => content().split("\n").length);
  return (
    <>
      <div class="ed">
        <div class="ed-gutter" aria-hidden="true"><For each={Array.from({ length: lines() }, (_, i) => i + 36)}>{(n) => <div>{n}</div>}</For></div>
        <textarea class="ed-text" aria-label="Editor content" spellcheck={false} readOnly={!canEdit("editor")} value={content()} onInput={(e) => void patchView("editor", { content: e.currentTarget.value })} />
      </div>
      <footer class="statusb"><span>Rust</span><span>{lines()} lines</span><span>Spaces 4</span><Spacer /><span>Tab accepts the grey suggestion</span><span class="ai-badge">● Copilot · Auto</span></footer>
    </>
  );
}

export function EditorHeader(props: { popout?: boolean }) {
  return (
    <header class="head ed-head" data-tauri-drag-region>
      <div class="ftabs" role="tablist" aria-label="Open files">
        <button type="button" class="ftab on" role="tab" aria-selected="true"><span class="mod" aria-label="modified" />session.rs</button>
        <button type="button" class="ftab" role="tab" aria-selected="false">retry.rs</button>
        <Show when={!props.popout}><button type="button" class="ftab" role="tab" aria-selected="false">Cargo.toml</button></Show>
      </div>
      <span class="crumbs">token-refresh<span>/</span>crates/atrium-core/src<span>/</span><b>session.rs</b></span>
      <div class="ed-right">
        <span class="chip">⌘K edit · ⌘L ask</span>
        <Show when={!props.popout}><IconButton label="Open editor in new window" onClick={() => void popOut("editor")}><ExternalLink class="i" aria-hidden="true" /></IconButton></Show>
      </div>
    </header>
  );
}

export function DatabaseBody() {
  return (
    <div class="db">
      <label class="db-label" for="db-query">Query</label>
      <textarea id="db-query" class="db-query raised" spellcheck={false} readOnly={!canEdit("database")} value={model().views.database.content} onInput={(e) => void patchView("database", { content: e.currentTarget.value })} />
      <table class="grid">
        <thead><tr><th>id</th><th>thread</th><th>state</th><th>agent</th><th class="num">tokens</th></tr></thead>
        <tbody><For each={DB_ROWS}>{(r) => <tr><td class="num">{r.id}</td><td class="mono">{r.thread}</td><td>{r.state}</td><td>{r.agent}</td><td class="num mono">{r.tokens.toLocaleString()}</td></tr>}</For></tbody>
      </table>
    </div>
  );
}

export function DatabaseHeader(props: { popout?: boolean }) {
  return (
    <header class="head" data-tauri-drag-region>
      <span class="title">sessions</span><span class="chip">atrium.db · read only</span>
      <Spacer />
      <Show when={!props.popout}><IconButton label="Open database in new window" onClick={() => void popOut("database")}><ExternalLink class="i" aria-hidden="true" /></IconButton></Show>
    </header>
  );
}

const VIEW_NAME: Record<ViewId, string> = { chat: "This chat", editor: "The editor", database: "The database view" };

/** Where a popped-out view used to be: one place at a time (ui-direction §3 Popped-out windows). */
export function Placeholder(props: { view: ViewId }) {
  return (
    <div class="placeholder">
      <AppWindow class="i lg" aria-hidden="true" />
      <p>{VIEW_NAME[props.view]} is open in its own window.</p>
      <div class="row-actions">
        <button type="button" class="btn sm" onClick={() => void focusPopout(props.view)}>Show window</button>
        <button type="button" class="btn sm pri" onClick={() => void bringBack(props.view)}>Bring back</button>
      </div>
    </div>
  );
}

/** Renders `children` when this window owns the view, else the placeholder. */
export function Owned(props: { view: ViewId; children: JSX.Element }) {
  return <Show when={model().views[props.view].owner === "main"} fallback={<Placeholder view={props.view} />}>{props.children}</Show>;
}
