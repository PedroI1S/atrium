import { For, Match, Show, Switch, createSignal } from "solid-js";
import Resizable from "corvu/resizable";
import { ALL_THREADS, DB_ROWS, WORKSPACES } from "../data/dummy";
import { outerSizes, setOuterSizes, setPaletteOpen, setRightTab } from "../state";
import { accessibilityOpaque, bringBack, model, nativeError, nativeSupported, patchSettings, patchView, popOut, viewId, windowLabel } from "../windows";
import type { ViewId } from "../window-model";
import { ChatPane } from "./ChatPane";
import { RightPanel } from "./RightPanel";

const [screen, setScreen] = createSignal<ViewId | "settings" | "inbox" | "home">("chat");
const [topic, setTopic] = createSignal(0);
const [sidebar, setSidebar] = createSignal(true);
const [panel, setPanel] = createSignal(true);
const [advanced, setAdvanced] = createSignal(false);
function stripShare() {
  return Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--panel-strip-width")) / window.innerWidth;
}
let savedSidebar = 0.18;
let savedPanel = 0.31;
export const openFloatingInbox = () => { setScreen("inbox"); setAdvanced(false); };
export const toggleFloatingSidebar = () => {
  const [left, main, right] = outerSizes() as [number, number, number];
  if (sidebar()) savedSidebar = left;
  const next = sidebar() ? stripShare() : savedSidebar;
  setOuterSizes([next, main + left - next, right]); setSidebar(v => !v);
};
export const toggleFloatingPanel = () => {
  const [left, main, right] = outerSizes() as [number, number, number];
  if (panel()) savedPanel = right;
  const next = panel() ? stripShare() : savedPanel;
  setOuterSizes([left, main + right - next, next]); setPanel(v => !v);
};

function Appearance() {
  const s = () => model().settings;
  return <section class="settings-content">
    <h2>Appearance</h2>
    <label class="setting-row">Theme <select value={s().theme} onChange={e => void patchSettings({ theme: e.currentTarget.value })}>
      <option value="atrium-calm-light">Light</option><option value="atrium-calm-dark">Dark</option>
    </select></label>
    <label class="setting-row">Window opacity <span>{Math.round(s().alpha * 100)}%</span><input aria-label="Window opacity" type="range" min="30" max="100" value={s().alpha * 100} disabled={!nativeSupported()} onInput={e => void patchSettings({ alpha: Number(e.currentTarget.value) / 100 })} /></label>
    <label class="setting-row">Blur behind window <input type="checkbox" checked={s().blur} disabled={!nativeSupported()} onChange={e => void patchSettings({ blur: e.currentTarget.checked })} /></label>
    <button class="setting-row" onClick={() => setAdvanced(!advanced())}>Advanced appearance ›</button>
    <Show when={!nativeSupported()}><p class="muted">This platform uses opaque windows. Native opacity and blur require the macOS app.</p></Show>
    <Show when={accessibilityOpaque()}><p class="muted">Accessibility preferences keep surfaces opaque.</p></Show>
    <Show when={nativeError()}><p role="alert">{nativeError()}</p></Show>
  </section>;
}
function Advanced() {
  return <aside class="floating-card advanced"><header class="pane-head">Appearance / Advanced <button class="btn" onClick={() => setAdvanced(false)}>Close</button></header>
    <div class="settings-content">
      <label class="setting-row">Opacity applies to <select value={model().settings.mode} onChange={e => void patchSettings({ mode: e.currentTarget.value as "background" | "whole" })}><option value="background">Background</option><option value="whole">Whole window</option></select></label>
      <label class="setting-row">Keep reading areas solid <input type="checkbox" checked={model().settings.solid} onChange={e => void patchSettings({ solid: e.currentTarget.checked })} /></label>
    </div>
  </aside>;
}
function View(props: { id: ViewId }) {
  const owned = () => model().views[props.id].owner === windowLabel;
  return <Show when={owned()} fallback={<div class="placeholder">Open in its own window <button class="btn" onClick={() => void bringBack(props.id)}>Bring back</button></div>}>
    <Switch>
      <Match when={props.id === "chat"}><ChatPane threadId={model().views.chat.threadId} primary /></Match>
      <Match when={props.id === "editor"}><div class="editor-view"><header class="pane-head centered-path">token-refresh / crates/atrium-core/src / <strong>session.rs</strong></header><textarea class="dummy-editor" aria-label="Editor content" spellcheck={false} value={model().views.editor.content} onInput={e => void patchView("editor", { content: e.currentTarget.value })} /></div></Match>
      <Match when={props.id === "database"}><div class="settings-content"><label>Query<textarea class="dummy-query" value={model().views.database.content} onInput={e => void patchView("database", { content: e.currentTarget.value })} /></label><table class="grid"><thead><tr><th>Thread</th><th>State</th></tr></thead><tbody><For each={DB_ROWS}>{r => <tr><td>{r.thread}</td><td>{r.state}</td></tr>}</For></tbody></table></div></Match>
    </Switch>
  </Show>;
}
export function FloatingShell() {
  const currentView = () => viewId ?? (screen() === "editor" || screen() === "database" ? screen() as ViewId : "chat");
  const navigate = (next: typeof screen extends () => infer T ? T : never) => { setScreen(next); setAdvanced(false); };
  return <>
    <header class="floating-chrome" data-tauri-drag-region><span class="strip-traffic" /><span class="side-grow" data-tauri-drag-region>Atrium · Floating spaces</span><Show when={viewId}><button class="btn" onClick={() => void bringBack(viewId!)}>Move back</button></Show></header>
    <main class="workspace floating-workspace">
      <Show when={!viewId} fallback={<section class="floating-card detached"><View id={viewId!} /></section>}>
        <Resizable class="rz" orientation="horizontal" sizes={outerSizes()} onSizesChange={setOuterSizes}>
          <Resizable.Panel class="rz-panel" minSize={stripShare()} maxSize={0.32}>
            <nav class="topic-sidebar" classList={{ minimized: !sidebar() }} aria-label="Topics">
              <button class="side-row" onClick={toggleFloatingSidebar}>{sidebar() ? "Hide sidebar" : "☰"}</button>
              <Show when={sidebar()}><button class="side-row" onClick={() => setPaletteOpen(true)}>⌘K　Ask, search or jump</button></Show>
              <button class="side-row" onClick={() => navigate("inbox")}>Inbox <span class="badge">3</span></button>
              <Show when={sidebar()}><button class="side-row" onClick={() => navigate("home")}>Home</button><div class="side-label">{WORKSPACES[topic()]!.name}</div><button class="side-row" onClick={() => navigate("chat")}>+ New chat in this topic</button><For each={WORKSPACES[topic()]!.threads}>{t => <button class="side-row" onClick={() => { navigate("chat"); if (model().views.chat.owner === "main") void patchView("chat", { threadId: t.id }); }}><span class="state-dot" data-state={t.state} /><span class="side-grow">{t.title}</span><span class="muted">{t.meta}</span></button>}</For><div class="view-links"><button class="side-row" onClick={() => navigate("editor")}>Editor</button><button class="side-row" onClick={() => navigate("database")}>Database</button></div></Show>
              <div class="topic-switcher"><For each={WORKSPACES}>{(t, i) => <button class="topic-circle" aria-label={`Topic ${t.name}`} aria-pressed={topic() === i()} onClick={() => setTopic(i())}>{t.name.slice(0, 2)}</button>}</For><button class="side-row" onClick={() => navigate("settings")}>⚙ <Show when={sidebar()}>Settings</Show></button></div>
            </nav>
          </Resizable.Panel>
          <Resizable.Handle class="handle" aria-label="Resize sidebar" />
          <Resizable.Panel class="rz-panel floating-card" minSize={0.3}>
            <header class="pane-head"><span class="pane-title">{screen() === "settings" ? "Settings · General · Appearance" : screen() === "chat" ? "Rework session token refresh" : screen()}</span><Show when={["chat", "editor", "database"].includes(screen())}><button class="btn" onClick={() => void popOut(currentView()).catch(e => alert(String(e)))}>⧉ Open in new window</button></Show></header>
            <Switch>
              <Match when={screen() === "settings"}><Appearance /></Match>
              <Match when={screen() === "inbox"}><div class="settings-content"><h2>Inbox</h2><For each={ALL_THREADS.filter(t => ["needs-you", "failed", "ready"].includes(t.state)).sort((a, b) => ["needs-you", "failed", "ready"].indexOf(a.state) - ["needs-you", "failed", "ready"].indexOf(b.state))}>{t => <button class="setting-row" onClick={() => { navigate("chat"); if (model().views.chat.owner === "main") void patchView("chat", { threadId: t.id }); }}><span class="state-dot" data-state={t.state} />{t.title}<span class="muted">{t.state} · {t.agent}</span></button>}</For></div></Match>
              <Match when={screen() === "home"}><div class="settings-content"><h2>Home</h2><button class="btn" onClick={() => navigate("chat")}>Continue current chat</button></div></Match>
              <Match when={true}><View id={currentView()} /></Match>
            </Switch>
          </Resizable.Panel>
          <Resizable.Handle class="handle" aria-label="Resize right panel" />
          <Resizable.Panel class="rz-panel" minSize={stripShare()} maxSize={0.5}>
            <Show when={screen() === "settings"} fallback={<aside class="floating-card context-panel"><button class="side-row" onClick={toggleFloatingPanel}>{panel() ? "» Minimize" : "« Expand"}</button><Show when={panel()} fallback={<div class="panel-icons"><For each={[{ name: "Changes", id: "diff" }, { name: "Checks", id: "review" }, { name: "Files", id: "browser" }, { name: "Terminal", id: "terminal" }] as const}>{tab => <button class="side-row" aria-label={tab.name} onClick={() => { setRightTab(tab.id); if (!panel()) toggleFloatingPanel(); }}>{tab.name.slice(0, 1)}</button>}</For></div>}><RightPanel /></Show></aside>}><Show when={advanced()}><Advanced /></Show></Show>
          </Resizable.Panel>
        </Resizable>
      </Show>
    </main>
  </>;
}
