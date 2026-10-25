import { Dialog } from "@kobalte/core/dialog";
import { CornerDownLeft } from "lucide-solid";
import { For, createEffect, createMemo, createSignal } from "solid-js";
import { runFrameBenchesInteractive, benchResize } from "../bench";
import {
  PRESETS,
  type PresetId,
  activateTheme,
  applyPreset,
  nextAttention,
  paletteOpen,
  setPaletteOpen,
  showToast,
  themes,
  toggleAppearance,
  toggleRightPanel,
  toggleSidebar,
} from "../state";

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
  const presets: Command[] = (Object.keys(PRESETS) as PresetId[]).map((id) => ({
    id: `layout-${id}`,
    label: `Layout: ${PRESETS[id].label}`,
    hint: id === "default" ? "chat + preview + review" : id === "review" ? "diff large, chat narrow" : "chat only",
    run: () => applyPreset(id),
  }));
  const look: Command[] = [
    ...themes().map((t) => ({ id: `theme-${t.id}`, label: `Theme: ${t.theme.meta.name}`, hint: t.theme.meta.appearance, run: () => activateTheme(t.id) })),
    { id: "toggle-appearance", label: "Theme: toggle dark / light", hint: "⌘⇧L", run: toggleAppearance },
  ];
  const actions: Command[] = [
    { id: "next-attention", label: "Go to next item needing attention", hint: "⌘J", run: nextAttention },
    { id: "new-thread", label: "New thread", hint: "⌘N", run: () => showToast("New thread (dummy)") },
    { id: "quick-capture", label: "Quick capture a task", hint: "⌃⌥Space", run: () => showToast("Quick capture (dummy)") },
    { id: "toggle-sidebar", label: "View: toggle sidebar", hint: "⌘B", run: toggleSidebar },
    { id: "toggle-right", label: "View: toggle right panel", run: toggleRightPanel },
  ];
  const bench: Command[] = [
    { id: "bench-frames", label: "Dev: run frame-time benchmarks", hint: "scroll 5,000 messages + resize", run: () => void runFrameBenchesInteractive() },
    { id: "bench-resize", label: "Dev: resize panes for 3 s", run: () => void benchResize() },
  ];
  return [
    { label: "Actions", options: actions },
    { label: "Layout", options: presets },
    { label: "Look", options: look },
    { label: "Developer", options: bench },
  ];
}

/** Case-insensitive subsequence-friendly match: every word of the query must appear in the label or hint. */
function matches(c: Command, q: string): boolean {
  const hay = `${c.label} ${c.hint ?? ""}`.toLowerCase();
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
            <div class="pal-foot"><CornerDownLeft size={12} /> run <span class="pal-sep" /> up/down move <span class="pal-sep" /> esc close</div>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
