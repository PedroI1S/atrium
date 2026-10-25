import { For, Show, createSignal, onCleanup } from "solid-js";
import { Composer } from "./Composer";
import { TranscriptView } from "./Transcript";
import type { Pipeline } from "./pipeline";
import { bridgeStop } from "./platform";
import { DEFAULT_RUN, type RunConfig, startSession } from "./session";
import type { StartupInfo } from "./platform";
import { ENCODINGS, type Encoding } from "./wire";

export const [activePipeline, setActivePipeline] = createSignal<Pipeline | null>(null);

export function App(props: { info: StartupInfo }) {
  const [cfg, setCfg] = createSignal<RunConfig>({ ...DEFAULT_RUN });
  const [status, setStatus] = createSignal("idle");
  const [stats, setStats] = createSignal("");
  const set = <K extends keyof RunConfig>(k: K, v: RunConfig[K]) => setCfg((c) => ({ ...c, [k]: v }));

  const start = async () => {
    setStatus("starting");
    try {
      const { pipeline } = await startSession(cfg(), props.info);
      setActivePipeline(pipeline);
      setStatus("streaming");
    } catch (e) {
      setStatus(`error: ${String(e)}`);
    }
  };
  const stop = async () => {
    await bridgeStop();
    setStatus("stopped");
  };

  // Light readout twice a second.
  let lastEvents = 0;
  let lastAt = performance.now();
  const timer = setInterval(() => {
    const p = activePipeline();
    if (!p) return;
    const now = performance.now();
    const rate = ((p.eventsApplied - lastEvents) * 1000) / (now - lastAt);
    lastEvents = p.eventsApplied;
    lastAt = now;
    const rows = p.threads[p.visible[0]()]?.rows.length ?? 0;
    setStats(`${rate.toFixed(0)} ev/s  ${p.threads.reduce((s, t) => s + t.chars, 0) >> 2} tok  ${rows} rows  e2e p50 ${p.latency.pct(0.5)} ms`);
  }, 500);
  onCleanup(() => clearInterval(timer));

  const num = (k: keyof RunConfig) => (e: Event & { currentTarget: HTMLInputElement }) => set(k, Number(e.currentTarget.value) as never);
  const visibleT = () => {
    const p = activePipeline();
    return p ? p.threads[p.visible[0]()] : undefined;
  };

  return (
    <div class="app">
      <div class="bar">
        <label>
          encoding
          <select value={cfg().encoding} onChange={(e) => set("encoding", e.currentTarget.value as Encoding)}>
            <For each={[...ENCODINGS]}>{(e) => <option value={e}>{e}</option>}</For>
          </select>
        </label>
        <label>
          batch ms
          <input type="number" value={cfg().batchMs} onInput={num("batchMs")} />
        </label>
        <label>
          threads
          <input type="number" value={cfg().threads} onInput={num("threads")} />
        </label>
        <label>
          tok/s each
          <input type="number" value={cfg().tokensPerSecond} onInput={num("tokensPerSecond")} />
        </label>
        <label>
          chunk chars
          <input type="number" value={cfg().chunkChars} onInput={num("chunkChars")} />
        </label>
        <label>
          seconds
          <input type="number" value={cfg().durationMs / 1000} onInput={(e) => set("durationMs", Number(e.currentTarget.value) * 1000)} />
        </label>
        <button class="btn" onClick={start} disabled={status() === "starting" || status() === "streaming"}>
          Start
        </button>
        <button class="btn secondary" onClick={stop}>
          Stop
        </button>
        <span>{status()}</span>
        <span class="stats">{stats()}</span>
      </div>
      <div class="tabs">
        <Show when={activePipeline()}>
          {(p) => (
            <For each={p().threads}>
              {(t, i) => (
                <button class={p().visible[0]() === i() ? "tab active" : "tab"} onClick={() => p().visible[1](i())}>
                  <span class={t.running ? "dot running" : "dot"} />
                  thread {i() + 1}
                </button>
              )}
            </For>
          )}
        </Show>
      </div>
      <Show when={visibleT()} keyed fallback={<div class="transcript" />}>
        {(t) => <TranscriptView t={t} tick={activePipeline()!.tick[0]} settings={{ overscan: 6 }} />}
      </Show>
      <Composer />
    </div>
  );
}
