import {
  DragDropProvider,
  DragDropSensors,
  DragOverlay,
  SortableProvider,
  closestCenter,
  createSortable,
  type DragEventHandler,
} from "@thisbeyond/solid-dnd";
import { DropdownMenu } from "@kobalte/core/dropdown-menu";
import { Check, Columns2, House, MessagesSquare, PanelLeft, PanelRight, Plus, Palette, X } from "lucide-solid";
import { For, Show } from "solid-js";
import type { TabSpec } from "../data/dummy";
import { DIRECTIONS } from "../theme/registry";
import {
  activateTheme,
  activeTabId,
  counterpartTheme,
  outerSizes,
  setActiveTabId,
  setTabs,
  showToast,
  tabs,
  themeId,
  themes,
  toggleAppearance,
  toggleRightPanel,
  toggleSidebar,
} from "../state";

function TabIcon(props: { kind: TabSpec["kind"] }) {
  return props.kind === "home" ? <House size={13} /> : props.kind === "split" ? <Columns2 size={13} /> : <MessagesSquare size={13} />;
}

function SortableTab(props: { tab: TabSpec }) {
  const sortable = createSortable(props.tab.id);
  const close = (e: MouseEvent) => {
    e.stopPropagation();
    const list = tabs();
    if (list.length === 1) return showToast("Last tab stays open");
    const idx = list.findIndex((t) => t.id === props.tab.id);
    const next = list.filter((t) => t.id !== props.tab.id);
    setTabs(next);
    if (activeTabId() === props.tab.id) setActiveTabId(next[Math.min(idx, next.length - 1)]!.id);
  };
  return (
    <div
      ref={sortable.ref}
      role="tab"
      tabindex="0"
      aria-selected={activeTabId() === props.tab.id}
      class="tab"
      classList={{ "is-active": activeTabId() === props.tab.id, "is-dragging": sortable.isActiveDraggable }}
      data-atrium-tab
      style={{ transform: sortable.transform ? `translate3d(${sortable.transform.x}px, 0, 0)` : undefined }}
      onClick={() => setActiveTabId(props.tab.id)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") setActiveTabId(props.tab.id);
      }}
      {...sortable.dragActivators}
    >
      <TabIcon kind={props.tab.kind} />
      <span class="tab-title">{props.tab.title}</span>
      <button class="tab-close" aria-label={`Close ${props.tab.title}`} onClick={close} onPointerDown={(e) => e.stopPropagation()}>
        <X size={12} />
      </button>
    </div>
  );
}

function LookMenu() {
  const entriesOf = (dir: string) => themes().filter((t) => t.direction === dir);
  return (
    <DropdownMenu placement="bottom-end" gutter={6}>
      <DropdownMenu.Trigger class="icon-btn" aria-label="Look: theme and direction" title="Look">
        <Palette size={15} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content class="menu" data-atrium-menu>
          <DropdownMenu.RadioGroup value={themeId()} onChange={(v) => activateTheme(v)}>
            <For each={DIRECTIONS}>
              {(d) => (
                <Show when={entriesOf(d.id).length}>
                  <DropdownMenu.Group>
                    <DropdownMenu.GroupLabel class="menu-label">
                      {d.name} <span class="menu-blurb">{d.blurb}</span>
                    </DropdownMenu.GroupLabel>
                    <For each={entriesOf(d.id)}>
                      {(e) => (
                        <DropdownMenu.RadioItem class="menu-item" value={e.id}>
                          <DropdownMenu.ItemIndicator class="menu-indicator">
                            <Check size={13} />
                          </DropdownMenu.ItemIndicator>
                          <span class="swatch" style={{ background: e.theme.semantic.accent }} />
                          {e.theme.meta.name}
                          <span class="menu-hint">{e.theme.meta.appearance}</span>
                        </DropdownMenu.RadioItem>
                      )}
                    </For>
                  </DropdownMenu.Group>
                </Show>
              )}
            </For>
          </DropdownMenu.RadioGroup>
          <DropdownMenu.Separator class="menu-sep" />
          <DropdownMenu.Item class="menu-item" onSelect={toggleAppearance} disabled={!counterpartTheme(themeId())}>
            Toggle dark / light
            <span class="menu-hint">⌘⇧L</span>
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu>
  );
}

export function TabStrip() {
  const onDragEnd: DragEventHandler = ({ draggable, droppable }) => {
    if (!draggable || !droppable) return;
    const list = tabs();
    const from = list.findIndex((t) => t.id === draggable.id);
    const to = list.findIndex((t) => t.id === droppable.id);
    if (from === to || from < 0 || to < 0) return;
    const next = list.slice();
    next.splice(to, 0, next.splice(from, 1)[0]!);
    setTabs(next);
  };
  let n = 1;
  return (
    <header class="strip" data-tauri-drag-region data-atrium-part="tab-strip">
      {/* Room for the native traffic lights (titleBarStyle Overlay). */}
      <div class="strip-traffic" data-tauri-drag-region />
      <button class="icon-btn" aria-label="Toggle sidebar" title="Toggle sidebar" onClick={toggleSidebar} aria-pressed={outerSizes()[0]! > 0.01}>
        <PanelLeft size={15} />
      </button>
      <DragDropProvider onDragEnd={onDragEnd} collisionDetector={closestCenter}>
        <DragDropSensors />
        <div class="tabs" role="tablist" aria-label="Open threads">
          <SortableProvider ids={tabs().map((t) => t.id)}>
            <For each={tabs()}>{(t) => <SortableTab tab={t} />}</For>
          </SortableProvider>
        </div>
        <DragOverlay>
          {(d) => {
            const t = tabs().find((x) => x.id === d?.id);
            return <div class="tab is-active is-overlay">{t?.title}</div>;
          }}
        </DragOverlay>
      </DragDropProvider>
      <button
        class="icon-btn"
        aria-label="New tab"
        title="New tab"
        onClick={() => {
          const id = `tab-new-${n++}`;
          setTabs([...tabs(), { id, title: "New thread", kind: "thread", threads: ["t-perf"], file: "stream.ts" }]);
          setActiveTabId(id);
        }}
      >
        <Plus size={15} />
      </button>
      <div class="strip-fill" data-tauri-drag-region />
      <LookMenu />
      <button class="icon-btn" aria-label="Toggle right panel" title="Toggle right panel" onClick={toggleRightPanel} aria-pressed={outerSizes()[2]! > 0.01}>
        <PanelRight size={15} />
      </button>
    </header>
  );
}
