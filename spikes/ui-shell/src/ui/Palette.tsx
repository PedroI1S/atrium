import { Dialog } from "@kobalte/core/dialog";
import { CornerDownLeft } from "lucide-solid";
import { For, createEffect, createMemo, createSignal } from "solid-js";
import { benchResize, runFrameBenchesInteractive } from "../bench";
import { paletteOpen, panelOpen, setPanel, setPaletteOpen, setScreen, setSidebarOpen, sidebarOpen } from "../state";
import { VIEW_IDS } from "../window-model";
import { bringBack, model, patchSettings, popOut, viewId } from "../windows";

/** The palette's quick "see-through" preset (the harness pins the same value). */
const SEE_THROUGH = 0.6;

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

function sections(): Section[] {
  const windows: Command[] = VIEW_IDS.map((view) =>
    model().views[view].owner === "main"
      ? { id: `pop-${view}`, label: `Move ${view} to New Window`, run: () => void popOut(view) }
      : { id: `back-${view}`, label: `Bring ${view} back`, run: () => void bringBack(view) },
  );
  const go: Command[] = viewId ? [] : [
    { id: "inbox", label: "Go to Inbox", hint: "⌘J", run: () => setScreen("inbox") },
    { id: "chat", label: "Go to chat", run: () => setScreen("chat") },
    { id: "editor", label: "Go to editor", run: () => setScreen("editor") },
    { id: "settings", label: "Open Settings", hint: "⌘,", run: () => setScreen("settings") },
    { id: "toggle-sidebar", label: "View: toggle sidebar", hint: "⌘B", run: () => setSidebarOpen(!sidebarOpen()) },
    { id: "toggle-panel", label: "View: minimize or expand the panel", hint: "⌘⌥B", run: () => setPanel({ open: !panelOpen() }) },
  ];
  const look: Command[] = [
    { id: "theme-light", label: "Theme: Light", run: () => void patchSettings({ theme: "atrium-calm-light" }) },
    { id: "theme-dark", label: "Theme: Dark", run: () => void patchSettings({ theme: "atrium-calm-dark" }) },
    { id: "theme-system", label: "Theme: System", run: () => void patchSettings({ theme: "system" }) },
    { id: "opacity-60", label: "Opacity: 60 %", run: () => void patchSettings({ alpha: SEE_THROUGH }) },
    { id: "opacity-100", label: "Opacity: 100 %", run: () => void patchSettings({ alpha: 1 }) },
  ];
  const bench: Command[] = viewId ? [] : [
    { id: "bench-frames", label: "Dev: run frame-time benchmarks", hint: "scroll 5,000 messages + resize", run: () => void runFrameBenchesInteractive() },
    { id: "bench-resize", label: "Dev: resize panes for 3 s", run: () => void benchResize() },
  ];
  return [
    { label: "Go", options: go },
    { label: "Windows", options: windows },
    { label: "Look", options: look },
    { label: "Developer", options: bench },
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
