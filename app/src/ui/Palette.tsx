import { Dialog } from "@kobalte/core/dialog";
import { CornerDownLeft } from "lucide-solid";
import { For, createEffect, createMemo, createSignal } from "solid-js";
import { command, daemon, reconnect } from "../daemon/client";
import { isCommandFailure } from "../daemon/types";
import { paletteOpen, panelOpen, setPanel, setPaletteOpen, setScreen, setSidebarOpen, showToast, sidebarOpen } from "../state";
import { MAIN } from "../window-model";
import { bringBack, debugBuild, isPopout, model, ownsView, patchSettings, popOut } from "../windows";
import { openThread } from "./Sidebar";

interface Command {
  id: string;
  label: string;
  hint?: string;
  run: () => void;
}
interface Section {
  label: string;
  options: Command[];
}

const titleOf = (threadId: string) => daemon().threads[threadId]?.title || "Untitled chat";

/**
 * Development builds only: a thread run by the daemon's synthetic agent, to exercise the list,
 * the states and the windows without a provider (D-059).
 */
function startFakeThread(): void {
  command({ type: "start_thread", agent: { type: "fake", tokens_per_second: 200, chunk_chars: 16, duration_ms: 30_000, tool_call_every_ms: 5_000 }, message: "A synthetic thread" })
    .catch((e) => showToast(isCommandFailure(e) ? e.message : String(e)));
}

function sections(): Section[] {
  const current = model().mainThread;
  const windows: Command[] = isPopout() ? [] : [
    ...(current && ownsView(current) && daemon().threads[current] ? [{ id: "pop-current", label: "Move this chat to a new window", run: () => void popOut(current) }] : []),
    ...Object.entries(model().popouts).filter(([label]) => label !== MAIN).map(([label, popout]) => ({ id: `back-${label}`, label: `Bring back: ${titleOf(popout.threadId)}`, run: () => void bringBack(popout.threadId) })),
  ];
  const go: Command[] = isPopout() ? [] : [
    { id: "inbox", label: "Go to Inbox", hint: "⌘J", run: () => setScreen("inbox") },
    { id: "new-chat", label: "New chat", run: () => openThread(null) },
    { id: "settings", label: "Open Settings", hint: "⌘,", run: () => setScreen("settings") },
    { id: "toggle-sidebar", label: "View: toggle sidebar", hint: "⌘B", run: () => setSidebarOpen(!sidebarOpen()) },
    { id: "toggle-panel", label: "View: minimize or expand the panel", hint: "⌘⌥B", run: () => setPanel({ open: !panelOpen() }) },
  ];
  const chats: Command[] = isPopout() ? [] : daemon().order.map((id) => ({ id: `chat-${id}`, label: titleOf(id), run: () => openThread(id) }));
  const look: Command[] = isPopout() ? [] : [
    { id: "theme-light", label: "Theme: Light", run: () => void patchSettings({ theme: "light" }) },
    { id: "theme-dark", label: "Theme: Dark", run: () => void patchSettings({ theme: "dark" }) },
    { id: "theme-system", label: "Theme: System", run: () => void patchSettings({ theme: "system" }) },
  ];
  const developer: Command[] = debugBuild() && !isPopout() ? [
    { id: "dev-fake", label: "Dev: start a synthetic thread", hint: "30 s of streaming", run: startFakeThread },
    { id: "dev-reconnect", label: "Dev: reconnect to the daemon", run: reconnect },
  ] : [];
  return [
    { label: "Go", options: go },
    { label: "Chats", options: chats },
    { label: "Windows", options: windows },
    { label: "Look", options: look },
    { label: "Developer", options: developer },
  ].filter((x) => x.options.length);
}

/** Case-insensitive subsequence-friendly match: every word of the query must appear in the label. */
function matches(c: Command, q: string): boolean {
  const hay = c.label.toLowerCase();
  return q
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((w) => hay.includes(w));
}

/**
 * Command palette: Kobalte Dialog (focus trap, scrim, escape, aria) around an inline ARIA combobox.
 * Kobalte's own Combobox renders its listbox in a popper-positioned portal, which is the wrong
 * shape for a palette whose list is part of the dialog; the combobox pattern is built by hand here.
 */
function PaletteBody() {
  const [query, setQuery] = createSignal("");
  const [active, setActive] = createSignal(0);
  const groups = createMemo(() =>
    sections()
      .map((s) => ({ label: s.label, options: s.options.filter((c) => matches(c, query())) }))
      .filter((s) => s.options.length),
  );
  const flat = createMemo(() => groups().flatMap((g) => g.options));
  createEffect(() => {
    query();
    setActive(0);
  });
  let list!: HTMLDivElement;
  const run = (c: Command | undefined) => {
    if (!c) return;
    setPaletteOpen(false);
    // Let the dialog close (and focus return) before the action moves layout around.
    queueMicrotask(c.run);
  };
  const move = (delta: number) => {
    const n = flat().length;
    if (!n) return;
    const next = (active() + delta + n) % n;
    setActive(next);
    list.querySelector(`#pal-opt-${next}`)?.scrollIntoView({ block: "nearest" });
  };
  return (
    <>
      <div class="pal-control">
        <input
          class="pal-input"
          role="combobox"
          aria-expanded="true"
          aria-controls="pal-list"
          aria-activedescendant={flat().length ? `pal-opt-${active()}` : undefined}
          aria-label="Command"
          placeholder="Type a command..."
          autocomplete="off"
          spellcheck={false}
          autofocus
          value={query()}
          onInput={(e) => setQuery(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") (e.preventDefault(), move(1));
            else if (e.key === "ArrowUp") (e.preventDefault(), move(-1));
            else if (e.key === "Enter") (e.preventDefault(), run(flat()[active()]));
          }}
        />
      </div>
      <div class="pal-list" id="pal-list" role="listbox" aria-label="Commands" ref={list}>
        <For each={groups()} fallback={<div class="pal-empty">No matching command</div>}>
          {(g) => {
            return (
              <div role="group" aria-label={g.label}>
                <div class="pal-section" aria-hidden="true">{g.label}</div>
                <For each={g.options}>
                  {(c) => {
                    const n = () => flat().indexOf(c);
                    return (
                      <div
                        id={`pal-opt-${n()}`}
                        role="option"
                        class="pal-item"
                        aria-selected={active() === n()}
                        data-highlighted={active() === n() ? "" : undefined}
                        onPointerMove={() => active() !== n() && setActive(n())}
                        onClick={() => run(c)}
                      >
                        <span>{c.label}</span>
                        <span class="pal-hint">{c.hint}</span>
                      </div>
                    );
                  }}
                </For>
              </div>
            );
          }}
        </For>
      </div>
    </>
  );
}

export function Palette() {
  return (
    <Dialog open={paletteOpen()} onOpenChange={setPaletteOpen}>
      <Dialog.Portal>
        <Dialog.Overlay class="overlay" />
        <div class="palette-pos">
          <Dialog.Content class="palette" aria-label="Command palette" data-atrium-part="palette">
            <Dialog.Title class="sr-only">Command palette</Dialog.Title>
            <PaletteBody />
            <div class="pal-foot"><CornerDownLeft class="icon-xs" /> run <span class="pal-sep" /> up/down move <span class="pal-sep" /> esc close</div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
