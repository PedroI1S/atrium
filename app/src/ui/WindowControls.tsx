// Linux draws its own window controls on the frame (D-059): minimize, maximize, close, on the
// right. macOS keeps its traffic lights, so this renders nothing there.

import { Minus, Square, X } from "lucide-solid";
import { Show } from "solid-js";
import { inTauri } from "../platform";
import { platform } from "../windows";

async function current() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

export function WindowControls() {
  const act = (what: "minimize" | "toggleMaximize" | "close") => {
    if (inTauri) void current().then((w) => w[what]());
  };
  return (
    <Show when={platform() === "linux"}>
      <div class="wincontrols" role="group" aria-label="Window">
        <button type="button" class="wc" aria-label="Minimize" title="Minimize" onClick={() => act("minimize")}><Minus class="i sm" aria-hidden="true" /></button>
        <button type="button" class="wc" aria-label="Maximize" title="Maximize" onClick={() => act("toggleMaximize")}><Square class="i xs" aria-hidden="true" /></button>
        <button type="button" class="wc close" aria-label="Close" title="Close" onClick={() => act("close")}><X class="i sm" aria-hidden="true" /></button>
      </div>
    </Show>
  );
}
