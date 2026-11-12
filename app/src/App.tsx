import { Show, onCleanup, onMount } from "solid-js";
import { PROBE_KEY, markOnce } from "./harness";
import { advancedOpen, paletteOpen, panelOpen, setAdvancedOpen, setPanel, setPaletteOpen, setScreen, setSidebarOpen, sidebarOpen, toast } from "./state";
import { Palette } from "./ui/Palette";
import { MainWindow, PopoutWindow } from "./ui/Shell";
import { isPopout } from "./windows";

export function App() {
  const onKey = (e: KeyboardEvent) => {
    if (e.key === PROBE_KEY) { markOnce("firstInput"); return; }
    if (e.key === "Escape" && advancedOpen()) { setAdvancedOpen(false); return; }
    if (!(e.metaKey || e.ctrlKey)) return;
    const k = e.key.toLowerCase();
    const take = (f: () => void) => { e.preventDefault(); f(); };
    if (k === "k") take(() => setPaletteOpen(!paletteOpen()));
    else if (isPopout()) return;
    else if (k === "j") take(() => setScreen("inbox"));
    else if (k === ",") take(() => setScreen("settings"));
    else if (k === "b" && e.altKey) take(() => setPanel({ open: !panelOpen() }));
    else if (k === "b") take(() => setSidebarOpen(!sidebarOpen()));
  };
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));
  return (
    <div class="app" data-atrium-part="window" data-window={isPopout() ? "popout" : "main"}>
      <Show when={isPopout()} fallback={<MainWindow />}><PopoutWindow /></Show>
      <Palette />
      <div class="toast-region" role="status" aria-live="polite">
        <Show when={toast()}>{(t) => <div class="toast">{t().text}</div>}</Show>
      </div>
    </div>
  );
}
