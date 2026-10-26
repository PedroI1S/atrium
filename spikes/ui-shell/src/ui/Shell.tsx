// Window anatomy of layout B (ui-direction §2): the sidebar on the frame, the main card and
// the right column as floating cards. Pop-out windows get minimal chrome (§3 Popped-out windows).

import { Pin, Undo2 } from "lucide-solid";
import { Match, Show, Switch } from "solid-js";
import { threadById } from "../data/dummy";
import { clampWidth, rightColumn } from "../layout";
import { advancedOpen, keepOnTop, panelOpen, panelWidth, screen, setKeepOnTopSignal, setPanelWidth, setSidebarWidth, sidebarOpen, sidebarWidth } from "../state";
import { bringBack, model, setKeepOnTop, viewId } from "../windows";
import { ChatCard, ChatThread } from "./ChatView";
import { ContextPanel, PanelStrip } from "./ContextPanel";
import { IconButton, Pill, Spacer } from "./controls";
import { AdvancedCard, SettingsCard } from "./Settings";
import { Sidebar, SidebarStrip } from "./Sidebar";
import { DatabaseBody, DatabaseHeader, EditorBody, EditorHeader, InboxView, Owned } from "./Views";

const px = (name: string) => Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue(name));

/** Drag handle in the gap between two columns; widths stay inside their token bounds. */
function Handle(props: { label: string; edge: "left" | "right"; target: () => HTMLElement | undefined; set: (w: number) => void; min: string; max: string }) {
  let startX = 0;
  let startW = 0;
  let bounds: [number, number] = [0, 0];
  return (
    <div
      class="handle"
      role="separator"
      aria-orientation="vertical"
      aria-label={props.label}
      onPointerDown={(e) => {
        const el = props.target();
        if (!el) return;
        e.currentTarget.setPointerCapture(e.pointerId);
        startX = e.clientX;
        startW = el.getBoundingClientRect().width;
        bounds = [px(props.min), px(props.max)];
        e.currentTarget.dataset.dragging = "";
      }}
      onPointerMove={(e) => {
        if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
        const dx = e.clientX - startX;
        props.set(clampWidth(startW + (props.edge === "left" ? dx : -dx), bounds[0], bounds[1]));
      }}
      onPointerUp={(e) => { e.currentTarget.releasePointerCapture(e.pointerId); delete e.currentTarget.dataset.dragging; }}
    />
  );
}

export function MainWindow() {
  let side: HTMLDivElement | undefined;
  let panel: HTMLDivElement | undefined;
  const right = () => rightColumn(screen(), panelOpen(), advancedOpen());
  const widths = () => ({
    ...(sidebarWidth() === null ? {} : { "--sidebar-w": `${sidebarWidth()}px` }),
    ...(panelWidth() === null ? {} : { "--panel-w": `${panelWidth()}px` }),
  });
  return (
    <div class="frame" style={widths()} data-screen={screen()}>
      <Show when={sidebarOpen()} fallback={<><SidebarStrip /><span class="gap" /></>}>
        <div class="side-col" ref={side}><Sidebar /></div>
        <Handle label="Resize sidebar" edge="left" target={() => side} set={setSidebarWidth} min="--sidebar-min-width" max="--sidebar-max-width" />
      </Show>
      <section class="card main" aria-label="Main card">
        <Switch>
          <Match when={screen() === "settings"}><SettingsCard /></Match>
          <Match when={screen() === "inbox"}><InboxView /></Match>
          <Match when={screen() === "editor"}><Owned view="editor"><EditorHeader /><EditorBody /></Owned></Match>
          <Match when={screen() === "database"}><Owned view="database"><DatabaseHeader /><DatabaseBody /></Owned></Match>
          <Match when={true}><Owned view="chat"><ChatCard /></Owned></Match>
        </Switch>
      </section>
      <Switch>
        <Match when={right().kind === "panel"}>
          <Handle label="Resize panel" edge="right" target={() => panel} set={setPanelWidth} min="--panel-min-width" max="--panel-max-width" />
          <div class="panel-col" classList={{ editor: screen() === "editor" }} ref={panel}><ContextPanel /></div>
        </Match>
        <Match when={right().kind === "strip"}><span class="gap" /><PanelStrip /></Match>
        <Match when={right().kind === "advanced"}><span class="gap" /><AdvancedCard /></Match>
      </Switch>
    </div>
  );
}

/** A popped-out view: traffic lights, title, status pill, keep-on-top, move back (↩). */
export function PopoutWindow() {
  const view = viewId!;
  const thread = () => threadById(model().views.chat.threadId);
  const title = () => (view === "chat" ? thread().title : view === "editor" ? "session.rs" : "sessions");
  return (
    <div class="pop-frame">
      <header class="pop-bar" data-tauri-drag-region>
        <span class="title sm">{title()}</span>
        <Switch>
          <Match when={view === "database"}><span class="chip">atrium.db · read only</span></Match>
          <Match when={true}><Pill state={thread().state} /></Match>
        </Switch>
        <Spacer />
        <IconButton small label="Keep on top" on={keepOnTop()} onClick={() => { const next = !keepOnTop(); setKeepOnTopSignal(next); void setKeepOnTop(next); }}><Pin class="i sm" aria-hidden="true" /></IconButton>
        <IconButton small label="Move back into the main window" onClick={() => void bringBack(view)}><Undo2 class="i sm" aria-hidden="true" /></IconButton>
      </header>
      <section class="card pop-card">
        <Show when={model().views[view].owner === `view-${view}`} fallback={<p class="placeholder muted">Returning to the main window…</p>}>
          <Switch>
            <Match when={view === "chat"}><ChatThread primary compact /></Match>
            <Match when={view === "editor"}><EditorHeader popout /><EditorBody /></Match>
            <Match when={view === "database"}><DatabaseHeader popout /><DatabaseBody /></Match>
          </Switch>
        </Show>
      </section>
    </div>
  );
}
