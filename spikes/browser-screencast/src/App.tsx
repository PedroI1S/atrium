import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { listen } from "@tauri-apps/api/event";
import { type StartInfo, type StartupInfo, agentAction, browserStart, setUserControl, startupInfo } from "./api";
import type { PaneController } from "./pane/controller";
import { Pane } from "./pane/Pane";
import { normalizeUrl } from "./pane/url";

export function App() {
  const [info, setInfo] = createSignal<StartInfo | null>(null);
  const [cfg, setCfg] = createSignal<StartupInfo | null>(null);
  const [context, setContext] = createSignal(0);
  const [urls, setUrls] = createSignal<string[]>(["", ""]);
  const [urlDraft, setUrlDraft] = createSignal("");
  const [userControl, setControl] = createSignal(false);
  const [dsf, setDsf] = createSignal(1);
  const [status, setStatus] = createSignal<{ text: string; kind: "info" | "error" } | null>(null);
  const [fps, setFps] = createSignal(0);
  const [stats, setStats] = createSignal({ kib: 0, decode: 0, size: "" });
  let pane: PaneController | undefined;

  const say = (text: string, kind: "info" | "error" = "info") => {
    setStatus({ text, kind });
    if (kind === "info") window.setTimeout(() => setStatus((s) => (s?.text === text ? null : s)), 4000);
  };

  async function show(ctx: number) {
    if (!pane) return;
    setContext(ctx);
    setUrlDraft(urls()[ctx] ?? "");
    await pane.attach(ctx);
  }

  async function changeScale(next: number) {
    if (!pane) return;
    setDsf(next);
    pane.options.deviceScaleFactor = next;
    const r = pane.canvas.parentElement!.getBoundingClientRect();
    await pane.resize(r.width, r.height);
  }

  async function takeOver(on: boolean) {
    // Handing back: release any button the pane still holds while the backend still accepts it.
    if (!on) await pane?.releaseAll();
    await setUserControl(on);
    setControl(on);
    pane?.setUserControl(on);
    say(on ? "You have control. The agent is paused." : "Control handed back to the agent.");
  }

  async function agent(kind: "click" | "navigate") {
    try {
      const out = await agentAction(context(), kind, kind === "click" ? { x: 100, y: 100 } : { url: `http://127.0.0.1:${info()!.fixturePort}/click.html` });
      say(out);
    } catch (e) {
      say(String(e), "error");
    }
  }

  onMount(async () => {
    const config = await startupInfo();
    // The pane reads its options once at mount, so the scale must be set before it mounts.
    setDsf(config.deviceScaleFactor ?? 1);
    setCfg(config);
    const unlisten = await listen<{ context: number; url: string }>("browser://nav", (ev) => {
      setUrls((u) => u.map((x, i) => (i === ev.payload.context ? ev.payload.url : x)));
      if (ev.payload.context === context()) setUrlDraft(ev.payload.url);
    });
    onCleanup(unlisten);
    try {
      setInfo(await browserStart());
    } catch (e) {
      say(`Could not start the browser: ${e}`, "error");
      return;
    }
    // Pane is mounted once the info is set; wait one tick for `ready`.
    await new Promise((r) => setTimeout(r, 0));
    if (!pane) return;
    pane.ackAfterDraw = !config.eagerAck;
    pane.options.quality = config.quality;
    pane.options.format = config.format;
    const r = pane.canvas.parentElement!.getBoundingClientRect();
    await pane.resize(r.width, r.height);
    await show(0);
    if (config.bench?.startsWith("shot:")) {
      const { runShot } = await import("./shot");
      try {
        await runShot(config.bench.slice(5), pane, info()!, takeOver, show);
      } catch (e) {
        say(`scene failed: ${e}`, "error");
      }
    } else if (config.bench) {
      const { runBench } = await import("./bench");
      await runBench(pane, config, info()!, takeOver);
    }
  });

  // Frame rate and size, sampled twice a second.
  onMount(() => {
    let last = { frames: 0, at: performance.now() };
    const timer = window.setInterval(() => {
      if (!pane) return;
      const now = performance.now();
      const frames = pane.stats.framesDrawn;
      setFps(((frames - last.frames) * 1000) / (now - last.at));
      last = { frames, at: now };
      setStats({
        kib: pane.stats.lastFrameBytes / 1024,
        decode: pane.stats.lastDecodeMs,
        // Decoded frame size (what Chromium sent) next to the canvas size (what we draw into).
        size: `frame ${pane.stats.lastFrameWidth}x${pane.stats.lastFrameHeight}, canvas ${pane.canvas.width}x${pane.canvas.height}`,
      });
    }, 500);
    onCleanup(() => window.clearInterval(timer));
  });

  return (
    <div class="app">
      <div class="bar" role="toolbar" aria-label="Browser pane">
        <div role="tablist" aria-label="Browser contexts" style={{ display: "flex", gap: "6px" }}>
          <For each={info()?.contexts ?? ["Context A", "Context B"]}>
            {(label, i) => (
              <button class="tab" role="tab" aria-selected={context() === i()} onClick={() => void show(i())}>
                {label}
              </button>
            )}
          </For>
        </div>
        <span class="spacer" />
        <span class="label">Scale</span>
        <span class="seg" role="group" aria-label="Device scale factor">
          <button aria-pressed={dsf() === 1} onClick={() => void changeScale(1)}>1x</button>
          <button
            aria-pressed={dsf() === 2}
            disabled={(info()?.launchDeviceScaleFactor ?? 1) < 2}
            title="Needs a browser launched with --force-device-scale-factor=2 (start the app with ATRIUM_LAUNCH_DSF=2). Without it Chromium keeps sending 1x frames."
            onClick={() => void changeScale(2)}
          >
            2x
          </button>
        </span>
        <button class="btn" onClick={() => void agent("click")}>Agent: click</button>
        <button class="btn" onClick={() => void agent("navigate")}>Agent: open click.html</button>
        <button class="btn takeover" aria-pressed={userControl()} onClick={() => void takeOver(!userControl())}>
          {userControl() ? "Hand back to agent" : "Take over"}
        </button>
      </div>
      <form
        class="navbar"
        onSubmit={(e) => {
          e.preventDefault();
          if (!pane || !info()) return;
          const base = `http://127.0.0.1:${info()!.fixturePort}`;
          pane.navigate(normalizeUrl(urlDraft(), base)).catch((err) => say(String(err), "error"));
        }}
      >
        <button type="button" class="btn" aria-label="Back" disabled={!userControl()} onClick={() => pane?.history("back").catch((e) => say(String(e), "error"))}>&lt;</button>
        <button type="button" class="btn" aria-label="Forward" disabled={!userControl()} onClick={() => pane?.history("forward").catch((e) => say(String(e), "error"))}>&gt;</button>
        <button type="button" class="btn" aria-label="Reload" disabled={!userControl()} onClick={() => pane?.history("reload").catch((e) => say(String(e), "error"))}>Reload</button>
        <input
          class="url"
          aria-label="Address"
          spellcheck={false}
          disabled={!userControl()}
          value={urlDraft()}
          onInput={(e) => setUrlDraft(e.currentTarget.value)}
          placeholder={userControl() ? "URL, or a fixture path like /form.html" : "The agent is driving. Take over to navigate."}
        />
      </form>
      <Show when={cfg()}>
        <div style={{ display: "flex", "flex-direction": "column", flex: 1, "min-height": 0 }}>
          <Pane
            options={{ quality: cfg()!.quality, format: cfg()!.format, deviceScaleFactor: dsf() }}
            userControl={userControl()}
            ready={(c) => (pane = c)}
            onError={(m) => say(m, "error")}
          />
        </div>
      </Show>
      <Show when={!info()}>
        <div class="starting">Starting the browser...</div>
      </Show>
      <Show when={status()}>
        {(s) => <div class={`chip ${s().kind === "error" ? "error" : "user"}`} role="status" style={{ bottom: "44px" }}>{s().text}</div>}
      </Show>
      <Show when={info() && !userControl()}>
        <div class="chip" style={{ bottom: "44px", right: "12px", left: "auto" }}>Agent is driving</div>
      </Show>
      <div class="status">
        <span>{fps().toFixed(0)} fps</span>
        <span>{stats().kib.toFixed(0)} KiB/frame</span>
        <span>decode {stats().decode.toFixed(1)} ms</span>
        <span>{stats().size}</span>
        <span class="sep" />
        <span>{info() ? `${info()!.browser.name} ${info()!.product} (pid ${info()!.pid})` : ""}</span>
      </div>
    </div>
  );
}
