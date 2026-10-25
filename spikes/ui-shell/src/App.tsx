import { model, patchSettings } from "./windows";
import { onCleanup, onMount } from "solid-js";
import { Show } from "solid-js";
import { FloatingShell, openFloatingInbox, toggleFloatingPanel, toggleFloatingSidebar } from "./ui/FloatingShell";
import { Palette } from "./ui/Palette";

import { benchOverlay, paletteOpen, setPaletteOpen, toast } from "./state";

export function App() {
  const onKey = (e: KeyboardEvent) => {
    if (!(e.metaKey || e.ctrlKey)) return;
    const k = e.key.toLowerCase();
    if (k === "k") {
      e.preventDefault();
      setPaletteOpen(!paletteOpen());
    } else if (k === "j") {
      e.preventDefault();
      openFloatingInbox();
    } else if (k === "l" && e.shiftKey) {
      e.preventDefault();
      void patchSettings({ theme: model().settings.theme.endsWith("dark") ? "atrium-calm-light" : "atrium-calm-dark" });
    } else if (k === "b") {
      e.preventDefault();
      if (e.altKey) toggleFloatingPanel(); else toggleFloatingSidebar();
    }
  };
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));
  return (
    <div class="app" data-atrium-part="window">
      <FloatingShell />
      <Palette />
      <div class="toast-region" role="status" aria-live="polite">
        <Show when={toast()}>{(t) => <div class="toast">{t().text}</div>}</Show>
      </div>
      <Show when={benchOverlay()}>{(t) => <pre class="bench-overlay">{t()}</pre>}</Show>
    </div>
  );
}
