// Right panel (ui-direction §2.2): a card whose tabs depend on the main card, minimizable to
// a 46 px strip of icons with attention dots.

import { ChevronDown, ChevronRight, ChevronsLeft, ChevronsRight, CircleCheck, Database, File, Folder, GitCompare, Globe, MessageCircle, Terminal } from "lucide-solid";
import { For, Match, Show, Switch } from "solid-js";
import { CHECKS, DIFF, FILE_TREE, INBOX, TERMINAL_LINES } from "../data/dummy";
import { type PanelTab, panelTabs } from "../layout";
import { inboxPick, panelTab, screen, setPanel, setScreen, showToast } from "../state";
import { Dot, IconButton, Spacer } from "./controls";
import { MiniChat } from "./ChatView";
import { openChat } from "./Sidebar";

const TAB_LABEL: Record<PanelTab, string> = { changes: "Changes", checks: "Checks", files: "Files", terminal: "Terminal", chat: "Chat", preview: "Preview" };

function TabLabel(props: { tab: PanelTab }) {
  return (
    <Switch fallback={TAB_LABEL[props.tab]}>
      <Match when={props.tab === "changes"}>Changes <span class="add-t">+5</span><span class="del-t">−3</span></Match>
      <Match when={props.tab === "checks"}>Checks <Dot state="needs-you" /></Match>
    </Switch>
  );
}

function Changes() {
  return (
    <>
      <div class="panel-sub"><span class="mono">{DIFF.path}</span><Spacer /><button type="button" class="btn ghost sm" onClick={() => setScreen("editor")}>Open in editor</button></div>
      <div class="diff raised">
        <For each={DIFF.lines}>
          {(l) => (
            <div class="dl" data-kind={l.kind}>
              <span class="n">{l.kind === "del" || l.kind === "add" || l.kind === "ctx" ? l.no : ""}</span>
              <span class="s">{l.kind === "add" ? "+" : l.kind === "del" ? "−" : ""}</span>
              <span class="c" data-indent={l.indent}>{l.code}</span>
            </div>
          )}
        </For>
      </div>
      <div class="panel-foot"><span class="muted">Click a line to comment for the agent</span><Spacer /><button type="button" class="btn sm" onClick={() => showToast("Changes requested (dummy)")}>Request changes</button></div>
    </>
  );
}

function Checks() {
  return (
    <div class="checks">
      <For each={CHECKS}>{(c) => <div class="ck"><Dot state={c.state} label /><span classList={{ code: c.mono }}>{c.label}</span><Spacer /><span class="muted">{c.result}</span></div>}</For>
    </div>
  );
}

function Files() {
  return (
    <div class="tree">
      <For each={FILE_TREE}>
        {(f) => (
          <button type="button" class="tr" classList={{ on: f.selected }} data-depth={f.depth} onClick={() => f.selected && setScreen("editor")}>
            <Show when={f.dir} fallback={<File class="i sm" aria-hidden="true" />}>
              <Show when={f.open} fallback={<ChevronRight class="i sm" aria-hidden="true" />}><ChevronDown class="i sm" aria-hidden="true" /></Show>
            </Show>
            {f.name}
            <Show when={f.mark}><span class="m add-t">{f.mark}</span></Show>
          </button>
        )}
      </For>
    </div>
  );
}

function TerminalTab() {
  return (
    <div class="term raised">
      <For each={TERMINAL_LINES}>{(l) => <div classList={{ strong: l.strong }}>{l.text}</div>}</For>
      <div class="strong">$ <span class="caret" /></div>
    </div>
  );
}

function Preview() {
  const item = () => INBOX.find((i) => i.id === inboxPick()) ?? INBOX[0]!;
  return (
    <div class="mini">
      <p class="msg muted">{item().summary}</p>
      <div class="checks flush">
        <div class="ck"><Dot state="ready" label /><span>Unit tests</span><Spacer /><span class="muted">passed</span></div>
        <div class="ck"><Dot state="ready" label /><span>Clippy</span><Spacer /><span class="muted">clean</span></div>
        <div class="ck"><Dot state="needs-you" label /><span>Evidence</span><Spacer /><span class="muted">waiting</span></div>
      </div>
      <div class="row-actions"><button type="button" class="btn sm" onClick={() => openChat(item().threadId)}>Open chat</button><button type="button" class="btn ghost sm" onClick={() => { openChat(item().threadId); setPanel({ tab: "changes" }); }}>See changes</button></div>
    </div>
  );
}

export function ContextPanel() {
  const tabs = () => panelTabs(screen()).tabs;
  const current = () => (tabs().includes(panelTab()) ? panelTab() : panelTabs(screen()).initial);
  const item = () => INBOX.find((i) => i.id === inboxPick()) ?? INBOX[0]!;
  return (
    <section class="card panel" aria-label="Context panel">
      <header class="head panel-head">
        <Show when={screen() !== "inbox"} fallback={<><span class="title sm">Preview</span><span class="muted ellipsis">{item().ask}</span></>}>
          <div class="stabs" role="tablist" aria-label="Panel tabs">
            <For each={tabs()}>
              {(t) => <button type="button" class="stab" role="tab" aria-selected={current() === t} classList={{ on: current() === t }} onClick={() => setPanel({ tab: t })}><TabLabel tab={t} /></button>}
            </For>
          </div>
        </Show>
        <Spacer />
        <IconButton small label="Minimize panel (⌘⌥B)" onClick={() => setPanel({ open: false })}><ChevronsRight class="i sm" aria-hidden="true" /></IconButton>
      </header>
      <div class="panel-body">
        <Switch>
          <Match when={screen() === "inbox"}><Preview /></Match>
          <Match when={current() === "changes"}><Changes /></Match>
          <Match when={current() === "checks"}><Checks /></Match>
          <Match when={current() === "files"}><Files /></Match>
          <Match when={current() === "terminal"}><TerminalTab /></Match>
          <Match when={current() === "chat"}><MiniChat /></Match>
        </Switch>
      </div>
    </section>
  );
}

/** Minimized panel: expand, then one icon per tab; a dot means something is waiting there. */
export function PanelStrip() {
  const open = (tab?: PanelTab) => setPanel({ open: true, ...(tab ? { tab } : {}) });
  const icons = () =>
    screen() === "editor"
      ? [{ tab: "chat" as const, Icon: MessageCircle, dot: "needs-you" }, { tab: "files" as const, Icon: Folder }, { tab: "changes" as const, Icon: GitCompare }]
      : screen() === "inbox"
        ? []
        : [
            { tab: "changes" as const, Icon: GitCompare, dot: "ready" },
            { tab: "checks" as const, Icon: CircleCheck, dot: "needs-you" },
            { tab: "files" as const, Icon: Folder },
            { tab: "terminal" as const, Icon: Terminal },
          ];
  return (
    <nav class="card strip" aria-label="Panel">
      <button type="button" class="sb" aria-label="Expand panel" title="Expand panel" onClick={() => open()}><ChevronsLeft class="i sm" aria-hidden="true" /></button>
      <Show when={icons().length}><span class="sdiv" /></Show>
      <For each={icons()}>
        {(x) => (
          <button type="button" class="sb" aria-label={TAB_LABEL[x.tab] + ("dot" in x ? ", something is waiting" : "")} title={TAB_LABEL[x.tab]} onClick={() => open(x.tab)}>
            <x.Icon class="i" aria-hidden="true" />
            <Show when={"dot" in x}><span class="bd" data-state={(x as { dot: string }).dot} /></Show>
          </button>
        )}
      </For>
      <Show when={screen() !== "editor" && screen() !== "inbox"}>
        <button type="button" class="sb" aria-label="Browser (M1)" title="Browser (M1)" disabled><Globe class="i" aria-hidden="true" /></button>
        <button type="button" class="sb" aria-label="Database" title="Database" onClick={() => setScreen("database")}><Database class="i" aria-hidden="true" /></button>
      </Show>
    </nav>
  );
}
