// Scripted scenes for the documentation screenshots (ATRIUM_BENCH=shot:<scene>). The app stays
// open afterwards so `scripts/screenshots.sh` can capture the window.

import type { StartInfo } from "./api";
import type { PaneController } from "./pane/controller";
import type { KeyLike } from "./pane/input";

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const noMods = { altKey: false, ctrlKey: false, metaKey: false, shiftKey: false };

function typeText(pane: PaneController, text: string): void {
  for (const ch of text) {
    const e: KeyLike = {
      key: ch,
      code: /[a-z]/i.test(ch) ? `Key${ch.toUpperCase()}` : "",
      keyCode: ch.toUpperCase().charCodeAt(0),
      location: 0,
      repeat: false,
      isComposing: false,
      ...noMods,
      shiftKey: ch !== ch.toLowerCase(),
    };
    pane.key("keyDown", e);
    pane.key("keyUp", e);
  }
}

export async function runShot(
  scene: string,
  pane: PaneController,
  info: StartInfo,
  takeOver: (on: boolean) => Promise<void>,
  show: (context: number) => Promise<void>,
): Promise<void> {
  const base = `http://127.0.0.1:${info.fixturePort}`;
  await sleep(600);
  switch (scene) {
    case "agent": {
      // The agent is driving a page: no take over, input ignored, chip shown.
      await takeOver(true);
      await pane.navigate(`${base}/animation.html`);
      await takeOver(false);
      break;
    }
    case "form": {
      await takeOver(true);
      await pane.navigate(`${base}/form.html`);
      await sleep(600);
      // Click into the first field through the real pane handlers, then type and compose.
      const r = pane.canvas.getBoundingClientRect();
      const at = { clientX: r.left + 200, clientY: r.top + 70, button: 0, detail: 1, ...noMods };
      pane.mouseDown({ ...at, buttons: 1 });
      pane.mouseUp({ ...at, buttons: 0 });
      await sleep(200);
      typeText(pane, "Hello Atrium ");
      pane.ime("compose", "に");
      await sleep(80);
      pane.ime("compose", "日本");
      await sleep(80);
      pane.ime("commit", "日本語");
      break;
    }
    case "contexts-a": {
      // Context A writes its marker into cookies and storage.
      await takeOver(true);
      await pane.navigate(`${base}/state.html?set=A`);
      break;
    }
    case "contexts-b": {
      // Context B opens the same origin and sees none of it.
      await takeOver(true);
      await show(1);
      await pane.navigate(`${base}/state.html`);
      break;
    }
    case "isolation": {
      // One launch, one browser process: A writes its marker, then B opens the same origin and
      // sees nothing, then A is shown again and still has its own state. Each phase lasts 10 s
      // so scripts/screenshots.sh can capture all three.
      await takeOver(true);
      await pane.navigate(`${base}/state.html?set=A`);
      await sleep(10_000);
      await show(1);
      await pane.navigate(`${base}/state.html`);
      await sleep(10_000);
      await show(0);
      await pane.navigate(`${base}/state.html`);
      break;
    }
    default:
      throw new Error(`unknown scene ${scene}`);
  }
}
