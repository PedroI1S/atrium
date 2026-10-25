import { onCleanup, onMount } from "solid-js";
import { Show } from "solid-js";
import { Layout } from "./ui/Layout";
import { Palette } from "./ui/Palette";
import { TabStrip } from "./ui/TabStrip";
import { benchOverlay, nextAttention, paletteOpen, setPaletteOpen, toast, toggleAppearance, toggleSidebar } from "./state";

export function App() {
  const onKey = (e: KeyboardEvent) => {
    if (!(e.metaKey || e.ctrlKey)) return;
    const k = e.key.toLowerCase();
    if (k === "k") {
      e.preventDefault();
      setPaletteOpen(!paletteOpen());
    } else if (k === "j") {
      e.preventDefault();
      nextAttention();
    } else if (k === "l" && e.shiftKey) {
      e.preventDefault();
      toggleAppearance();
    } else if (k === "b") {
      e.preventDefault();
      toggleSidebar();
    }
  };
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));
  return (
    <div class="app" data-atrium-part="window">
      <TabStrip />
      <Layout />
      <Palette />
      <div class="toast-region" role="status" aria-live="polite">
        <Show when={toast()}>{(t) => <div class="toast">{t().text}</div>}</Show>
      </div>
      <Show when={benchOverlay()}>{(t) => <pre class="bench-overlay">{t()}</pre>}</Show>
    </div>
  );
}
