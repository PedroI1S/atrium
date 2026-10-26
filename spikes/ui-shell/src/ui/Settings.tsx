// Settings card (in place of the main card) and the Advanced card (in place of the right
// panel). Sections come from the registry; Appearance is real, the other sections are inert
// stand-ins that show the essentials / Advanced pattern.

import { ChevronLeft, ChevronRight, Search, X } from "lucide-solid";
import { For, Match, Show, Switch, createSignal } from "solid-js";
import { type SettingRow, type SettingsSectionSpec, registerSection, searchSettings } from "../settings-registry";
import { advancedOpen, highlightRow, setAdvancedOpen, setHighlightRow, setScreen, setSettingsQuery, setSettingsSection, settingsQuery, settingsSection, type SettingsSection } from "../state";
import { MIN_ALPHA, type AppearanceSettings } from "../window-model";
import { accessibilityOpaque, effectiveSettings, nativeError, nativeSupported, patchSettings } from "../windows";
import { IconButton, Range, Seg, Select, Spacer, Toggle } from "./controls";

type Opt = { value: string; label: string };
type Control =
  | { kind: "seg"; value: () => string; options: Opt[]; set: (v: string) => void; disabled?: () => boolean }
  | { kind: "toggle"; value: () => boolean; set: (v: boolean) => void; disabled?: () => boolean }
  | { kind: "range"; value: () => number; min: number; max: number; format: (v: number) => string; set: (v: number) => void; disabled?: () => boolean }
  | { kind: "select"; value: () => string; options: Opt[]; set: (v: string) => void }
  | { kind: "button"; text: string; run: () => void };
type Row = SettingRow<Control>;

/** Visual-only controls keep their value locally (not persisted, no effect). */
const [local, setLocal] = createSignal<Record<string, string | boolean>>({});
const localSeg = (id: string, initial: string, options: Opt[]): Control => ({ kind: "seg", options, value: () => String(local()[id] ?? initial), set: (v) => setLocal((l) => ({ ...l, [id]: v })) });
const localToggle = (id: string, initial: boolean): Control => ({ kind: "toggle", value: () => Boolean(local()[id] ?? initial), set: (v) => setLocal((l) => ({ ...l, [id]: v })) });
const localSelect = (id: string, initial: string, options: Opt[]): Control => ({ kind: "select", options, value: () => String(local()[id] ?? initial), set: (v) => setLocal((l) => ({ ...l, [id]: v })) });
const opts = (...labels: string[]): Opt[] => labels.map((l) => ({ value: l.toLowerCase(), label: l }));

const s = effectiveSettings;
const set = (patch: Partial<AppearanceSettings>) => void patchSettings(patch);
const PERCENT = 100;
const opacityLocked = () => !nativeSupported() || accessibilityOpaque();

const SECTIONS: SettingsSectionSpec<Control>[] = [
  registerSection<Control>({
    id: "general", label: "General",
    essentials: [
      { id: "login", label: "Open at login", hint: "Start Atrium when you log in", control: localToggle("login", false) },
      { id: "workspace", label: "Default workspace", hint: "Where new chats start", control: localSelect("workspace", "atrium", opts("atrium", "harness-docs")) },
      { id: "updates", label: "Check for updates", control: localToggle("updates", true) },
    ],
    advanced: [{ id: "logs", label: "Log level", hint: "For bug reports", control: localSeg("logs", "info", opts("Info", "Debug")) }],
  }),
  registerSection<Control>({
    id: "appearance", label: "Appearance",
    essentials: [
      { id: "theme", label: "Theme", hint: "Atrium Calm", control: { kind: "seg", value: () => s().theme, set: (v) => set({ theme: v as AppearanceSettings["theme"] }), options: [{ value: "atrium-calm-light", label: "Light" }, { value: "atrium-calm-dark", label: "Dark" }, { value: "system", label: "System" }] } },
      { id: "opacity", label: "Window opacity", hint: "Lets the desktop show through. Text stays solid.",
        control: { kind: "range", min: MIN_ALPHA * PERCENT, max: PERCENT, value: () => Math.round(s().alpha * PERCENT), format: (v) => `${v}%`, set: (v) => set({ alpha: v / PERCENT }), disabled: opacityLocked } },
      { id: "blur", label: "Blur behind the window", hint: "Softens what shows through", control: { kind: "toggle", value: () => s().blur, set: (v) => set({ blur: v }), disabled: opacityLocked } },
      { id: "text-size", label: "Text size", control: localSeg("text-size", "default", opts("Small", "Default", "Large")) },
    ],
    advanced: [
      { id: "mode", group: "Opacity", label: "Applies to", hint: "Background fades surfaces; whole window fades everything", control: { kind: "seg", value: () => s().mode, set: (v) => set({ mode: v as AppearanceSettings["mode"] }), options: [{ value: "background", label: "Background" }, { value: "whole", label: "Whole window" }] } },
      { id: "solid", group: "Opacity", label: "Keep reading areas solid", hint: "Cards stay opaque, the frame fades", control: { kind: "toggle", value: () => s().solid, set: (v) => set({ solid: v }), disabled: () => s().mode === "whole" } },
      { id: "blur-strength", group: "Opacity", label: "Blur strength", hint: "macOS sets the blur radius; not adjustable", control: { kind: "range", min: 0, max: 48, value: () => 24, format: (v) => `${v} px`, set: () => {}, disabled: () => true } },
      { id: "popouts", group: "Opacity", label: "Popped-out windows", control: { kind: "select", value: () => s().popouts, set: (v) => set({ popouts: v as AppearanceSettings["popouts"] }), options: [{ value: "same", label: "Same as main" }, { value: "opaque", label: "Always opaque" }] } },
      { id: "tint", group: "Look", label: "Tint", hint: "Warm and Cool ship as sibling theme files (M1)", control: localSeg("tint", "neutral", opts("Neutral", "Warm", "Cool")) },
      { id: "gap", group: "Look", label: "Card gap", hint: "Space around the floating cards", control: localSeg("gap", "8", [{ value: "0", label: "None" }, { value: "8", label: "8" }, { value: "12", label: "12" }]) },
      { id: "fonts", group: "Look", label: "Fonts", control: localSelect("fonts", "instrument", [{ value: "instrument", label: "Instrument Sans · Geist Mono" }, { value: "system", label: "SF Pro · SF Mono" }]) },
      { id: "theme-file", group: "Look", label: "Theme file", hint: "Import VS Code, edit tokens, custom CSS", control: { kind: "button", text: "Open…", run: () => {} } },
    ],
  }),
  registerSection<Control>({
    id: "agents", label: "Agents",
    essentials: [
      { id: "default-agent", label: "Default agent", hint: "For new chats", control: localSelect("default-agent", "claude", [{ value: "claude", label: "Claude · Sonnet 5.5" }, { value: "codex", label: "Codex · GPT-6.1" }, { value: "agy", label: "Gemini · agy" }]) },
      { id: "reasoning", label: "Reasoning", control: localSeg("reasoning", "auto", opts("Low", "Auto", "High")) },
      { id: "delegation", label: "Let agents start sub-chats", hint: "They land in the parent chat's topic", control: localToggle("delegation", true) },
    ],
    advanced: [{ id: "binaries", label: "Agent binaries", hint: "Paths and versions", control: { kind: "button", text: "Show…", run: () => {} } }],
  }),
  registerSection<Control>({
    id: "approvals", label: "Approvals",
    essentials: [
      { id: "approval-mode", label: "Approval mode", hint: "What an agent may do without asking", control: localSeg("approval-mode", "ask first", opts("Ask first", "Auto in worktree")) },
      { id: "network", label: "Allow network", hint: "Commands that reach the internet always ask", control: localToggle("network", false) },
    ],
    advanced: [{ id: "rules", label: "Per-worktree rules", control: { kind: "button", text: "Edit…", run: () => {} } }],
  }),
  registerSection<Control>({
    id: "notifications", label: "Notifications",
    essentials: [
      { id: "notify-needs", label: "When something needs you", control: localToggle("notify-needs", true) },
      { id: "notify-ready", label: "When a chat is ready for review", control: localToggle("notify-ready", true) },
      { id: "sound", label: "Sound", control: localToggle("sound", false) },
    ],
    advanced: [{ id: "quiet", label: "Quiet hours", control: localSeg("quiet", "off", opts("Off", "Evenings")) }],
  }),
  registerSection<Control>({
    id: "more", label: "More",
    essentials: [
      { id: "editor-ai", label: "Editor AI", hint: "Grey suggestions while you type", control: localToggle("editor-ai", true) },
      { id: "databases", label: "Databases", hint: "Read-only connections", control: localToggle("databases", true) },
      { id: "sandbox", label: "Sandbox", hint: "Run agents in a VM", control: localToggle("sandbox", false) },
    ],
    advanced: [{ id: "experiments", label: "Experiments", control: localToggle("experiments", false) }],
  }),
];

const sectionById = (id: string) => SECTIONS.find((x) => x.id === id) ?? SECTIONS[1]!;

function ControlView(props: { row: Row }) {
  const c = props.row.control;
  return (
    <Switch>
      <Match when={c.kind === "seg" && c}>{(x) => <Seg label={props.row.label} value={x().value()} options={x().options} onChange={x().set} disabled={x().disabled?.()} />}</Match>
      <Match when={c.kind === "toggle" && c}>{(x) => <Toggle label={props.row.label} on={x().value()} onChange={x().set} disabled={x().disabled?.()} />}</Match>
      <Match when={c.kind === "range" && c}>{(x) => <Range label={props.row.label} value={x().value()} min={x().min} max={x().max} format={x().format} onInput={x().set} disabled={x().disabled?.()} wide={props.row.id === "opacity"} />}</Match>
      <Match when={c.kind === "select" && c}>{(x) => <Select label={props.row.label} value={x().value()} options={x().options} onChange={x().set} />}</Match>
      <Match when={c.kind === "button" && c}>{(x) => <button type="button" class="btn" onClick={x().run}>{x().text}</button>}</Match>
    </Switch>
  );
}

function RowView(props: { row: Row }) {
  return (
    <div class="row" id={`setting-${props.row.id}`} classList={{ flash: highlightRow() === props.row.id }}>
      <div class="rl"><b>{props.row.label}</b><Show when={props.row.hint}><span>{props.row.hint}</span></Show></div>
      <ControlView row={props.row} />
    </div>
  );
}

/** How long a search hit stays highlighted. */
const FLASH_MS = 1200;
function jumpTo(section: string, row: string, advanced: boolean) {
  setSettingsQuery(null);
  setSettingsSection(section as SettingsSection);
  setAdvancedOpen(advanced);
  setHighlightRow(row);
  setTimeout(() => setHighlightRow((r) => (r === row ? null : r)), FLASH_MS);
  queueMicrotask(() => document.getElementById(`setting-${row}`)?.scrollIntoView({ block: "nearest" }));
}

export function SettingsCard() {
  const section = () => sectionById(settingsSection());
  const results = () => searchSettings(SECTIONS, settingsQuery() ?? "");
  return (
    <>
      <header class="head settings-head" data-tauri-drag-region>
        <IconButton label="Back to chat" onClick={() => setScreen("chat")}><ChevronLeft class="i" aria-hidden="true" /></IconButton>
        <span class="title">Settings</span>
        <Spacer />
        <Show when={settingsQuery() === null} fallback={
          <input class="settings-search" type="search" placeholder="Search settings, Advanced included" aria-label="Search settings" autofocus value={settingsQuery() ?? ""} onInput={(e) => setSettingsQuery(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Escape") setSettingsQuery(null); if (e.key === "Enter" && results()[0]) { const r = results()[0]!; jumpTo(r.section, r.row, r.advanced); } }} />
        }>
          <div class="stabs" role="tablist" aria-label="Settings sections">
            <For each={SECTIONS}>{(x) => <button type="button" class="stab" role="tab" aria-selected={settingsSection() === x.id} classList={{ on: settingsSection() === x.id }} onClick={() => setSettingsSection(x.id as SettingsSection)}>{x.label}</button>}</For>
          </div>
        </Show>
        <Spacer />
        <IconButton label={settingsQuery() === null ? "Search settings" : "Close search"} onClick={() => setSettingsQuery(settingsQuery() === null ? "" : null)}>
          <Show when={settingsQuery() === null} fallback={<X class="i" aria-hidden="true" />}><Search class="i" aria-hidden="true" /></Show>
        </IconButton>
      </header>
      <div class="settings-body">
        <div class="settings-measure">
          <Show when={settingsQuery() === null} fallback={
            <div class="results" role="list">
              <For each={results()} fallback={<p class="muted">{settingsQuery() ? "No setting matches." : "Type to search every section, Advanced included."}</p>}>
                {(r) => <button type="button" class="result" role="listitem" onClick={() => jumpTo(r.section, r.row, r.advanced)}><b>{r.label}</b><span>{r.sectionLabel}{r.advanced ? " / Advanced" : ""}</span><ChevronRight class="i sm" aria-hidden="true" /></button>}
              </For>
            </div>
          }>
            <For each={section().essentials}>{(row) => <RowView row={row} />}</For>
            <Show when={section().id === "appearance" && opacityLocked()}>
              <p class="note">{nativeSupported() ? "Your accessibility settings (Reduce transparency or Increase contrast) keep the window opaque." : "This platform draws opaque windows; opacity and blur need a compositor that supports them."}</p>
            </Show>
            <Show when={section().id === "appearance" && nativeError()}><p class="note error" role="alert">{nativeError()}</p></Show>
            <button type="button" class="more" classList={{ on: advancedOpen() }} aria-expanded={advancedOpen()} onClick={() => setAdvancedOpen(!advancedOpen())}>
              Advanced {section().label.toLowerCase()}<Spacer /><ChevronRight class="i sm" aria-hidden="true" />
            </button>
          </Show>
        </div>
      </div>
    </>
  );
}

export function AdvancedCard() {
  const section = () => sectionById(settingsSection());
  const groups = () => {
    const out: { name: string | undefined; rows: Row[] }[] = [];
    for (const r of section().advanced) {
      const last = out[out.length - 1];
      if (last && last.name === r.group) last.rows.push(r);
      else out.push({ name: r.group, rows: [r] });
    }
    return out;
  };
  return (
    <section class="card advanced" aria-label={`${section().label} advanced settings`}>
      <header class="head adv-head">
        <span class="muted small">{section().label} /</span><span class="title">Advanced</span>
        <Spacer />
        <IconButton label="Close advanced" onClick={() => setAdvancedOpen(false)}><X class="i sm" aria-hidden="true" /></IconButton>
      </header>
      <div class="adv-body">
        <For each={groups()}>
          {(g) => (
            <>
              <Show when={g.name}><div class="grp-t">{g.name}</div></Show>
              <For each={g.rows}>{(row) => <RowView row={row} />}</For>
            </>
          )}
        </For>
      </div>
    </section>
  );
}
