// Settings card (in place of the main card) and the Advanced card (in place of the right
// panel). Sections come from the registry: a section has essentials and Advanced rows, and the
// search covers both. M1-09a has General and Appearance; M1-09b registers Agents, Approvals
// and Notifications through `SECTIONS`.

import { ChevronLeft, ChevronRight, Search, X } from "lucide-solid";
import { For, Match, Show, Switch, createResource, createSignal } from "solid-js";
import { command, daemon } from "../daemon/client";
import { isCommandFailure } from "../daemon/types";
import { type SettingRow, type SettingsSectionSpec, registerSection, searchSettings } from "../settings-registry";
import { advancedOpen, highlightRow, setAdvancedOpen, setHighlightRow, setScreen, setSettingsQuery, setSettingsSection, settingsQuery, settingsSection, showToast, type SettingsSection } from "../state";
import { MAX_BLUR_STRENGTH, MIN_ALPHA, type AppearanceSettings } from "../window-model";
import { accessibilityOpaque, effectiveSettings, nativeError, nativeSupported, patchSettings, platform } from "../windows";
import { IconButton, Range, Seg, Select, Spacer, Toggle } from "./controls";

type Opt = { value: string; label: string };
export type Control =
  | { kind: "seg"; value: () => string; options: Opt[]; set: (v: string) => void; disabled?: () => boolean }
  | { kind: "toggle"; value: () => boolean; set: (v: boolean) => void; disabled?: () => boolean }
  | { kind: "range"; value: () => number; min: number; max: number; format: (v: number) => string; set: (v: number) => void; disabled?: () => boolean; wide?: boolean }
  | { kind: "select"; value: () => string; options: Opt[]; set: (v: string) => void };
type Row = SettingRow<Control>;

const s = effectiveSettings;
const set = (patch: Partial<AppearanceSettings>) => void patchSettings(patch);
const PERCENT = 100;
/** The theme's default overlay blur, shown until the user picks a strength. */
const THEME_BLUR = () => Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--glass-blur")) || 0;
const opacityLocked = () => !nativeSupported() || accessibilityOpaque();

// -- settings that live in the daemon -------------------------------------------------------

/** The daemon's key for "keep agents running after quitting" (`atrium-daemon::KEEP_RUNNING_SETTING`). */
const KEEP_RUNNING = "keep_agents_running";
const connected = () => daemon().connection.type === "connected";
const [daemonSettings, { mutate: mutateDaemonSettings, refetch: refetchDaemonSettings }] = createResource<Record<string, unknown>, boolean>(
  connected,
  async (isConnected) => (isConnected ? command<Record<string, unknown>>({ type: "get_settings" }).catch(() => ({})) : {}),
);
const [savingDaemonSetting, setSavingDaemonSetting] = createSignal(false);

async function setDaemonSetting(key: string, value: boolean): Promise<void> {
  const before = daemonSettings() ?? {};
  mutateDaemonSettings({ ...before, [key]: value });
  setSavingDaemonSetting(true);
  try {
    await command({ type: "set_setting", key, value });
  } catch (e) {
    mutateDaemonSettings(before);
    showToast(isCommandFailure(e) ? e.message : String(e));
    // An unknown outcome may have changed it after all: read it again.
    void refetchDaemonSettings();
  } finally {
    setSavingDaemonSetting(false);
  }
}

// -- sections -------------------------------------------------------------------------------

export const SECTIONS: SettingsSectionSpec<Control>[] = [
  registerSection<Control>({
    id: "general", label: "General",
    essentials: [
      { id: "keep-running", label: "Keep agents running after quitting", hint: "Starts Atrium's background service when you log in, so chats keep working with the app closed. Takes effect at the next login.",
        control: { kind: "toggle", value: () => daemonSettings()?.[KEEP_RUNNING] === true, set: (v) => void setDaemonSetting(KEEP_RUNNING, v), disabled: () => !connected() || savingDaemonSetting() } },
    ],
    advanced: [],
  }),
  registerSection<Control>({
    id: "appearance", label: "Appearance",
    essentials: [
      { id: "theme", label: "Theme", hint: "Atrium Calm", control: { kind: "seg", value: () => s().theme, set: (v) => set({ theme: v as AppearanceSettings["theme"] }), options: [{ value: "light", label: "Light" }, { value: "dark", label: "Dark" }, { value: "system", label: "System" }] } },
      { id: "opacity", label: "Window opacity", hint: "Lets the desktop show through. Text stays solid.",
        control: { kind: "range", wide: true, min: MIN_ALPHA * PERCENT, max: PERCENT, value: () => Math.round(s().alpha * PERCENT), format: (v) => `${v}%`, set: (v) => set({ alpha: v / PERCENT }), disabled: opacityLocked } },
      { id: "blur", label: "Blur behind the window", hint: "Softens what shows through", control: { kind: "toggle", value: () => s().blur, set: (v) => set({ blur: v }), disabled: opacityLocked } },
      { id: "text-size", label: "Text size", hint: "Scales the whole interface", control: { kind: "seg", value: () => s().textSize, set: (v) => set({ textSize: v as AppearanceSettings["textSize"] }), options: [{ value: "small", label: "Small" }, { value: "default", label: "Default" }, { value: "large", label: "Large" }] } },
    ],
    advanced: [
      { id: "mode", group: "Opacity", label: "Applies to", hint: "Background fades surfaces; whole window fades everything", control: { kind: "seg", value: () => s().mode, set: (v) => set({ mode: v as AppearanceSettings["mode"] }), options: [{ value: "background", label: "Background" }, { value: "whole", label: "Whole window" }], disabled: opacityLocked } },
      { id: "solid", group: "Opacity", label: "Keep reading areas solid", hint: "Cards stay opaque, the frame fades", control: { kind: "toggle", value: () => s().solid, set: (v) => set({ solid: v }), disabled: () => opacityLocked() || s().mode === "whole" } },
      { id: "popouts", group: "Opacity", label: "Popped-out windows", control: { kind: "select", value: () => s().popouts, set: (v) => set({ popouts: v as AppearanceSettings["popouts"] }), options: [{ value: "same", label: "Same as main" }, { value: "opaque", label: "Always opaque" }] } },
      { id: "blur-strength", group: "Overlays", label: "Blur strength",
        // Contract §11.1: the note says, per platform, what this does and doesn't touch (L1).
        hintNow: () => (platform() === "macos" ? "macOS sets the window's blur; this only affects overlays" : "Atrium uses opaque windows on Linux. This only affects overlays."),
        control: { kind: "range", min: 0, max: MAX_BLUR_STRENGTH, value: () => s().blurStrength ?? THEME_BLUR(), format: (v) => `${v} px`, set: (v) => set({ blurStrength: v }) } },
    ],
  }),
];

const sectionById = (id: string) => SECTIONS.find((x) => x.id === id) ?? SECTIONS[0]!;

function ControlView(props: { row: Row }) {
  const c = props.row.control;
  return (
    <Switch>
      <Match when={c.kind === "seg" && c}>{(x) => <Seg label={props.row.label} value={x().value()} options={x().options} onChange={x().set} disabled={x().disabled?.()} />}</Match>
      <Match when={c.kind === "toggle" && c}>{(x) => <Toggle label={props.row.label} on={x().value()} onChange={x().set} disabled={x().disabled?.()} />}</Match>
      <Match when={c.kind === "range" && c}>{(x) => <Range label={props.row.label} value={x().value()} min={x().min} max={x().max} format={x().format} onInput={x().set} disabled={x().disabled?.()} wide={x().wide} />}</Match>
      <Match when={c.kind === "select" && c}>{(x) => <Select label={props.row.label} value={x().value()} options={x().options} onChange={x().set} />}</Match>
    </Switch>
  );
}

function RowView(props: { row: Row }) {
  const hint = () => props.row.hintNow?.() ?? props.row.hint;
  return (
    <div class="row" id={`setting-${props.row.id}`} classList={{ flash: highlightRow() === props.row.id }}>
      <div class="rl"><b>{props.row.label}</b><Show when={hint()}><span>{hint()}</span></Show></div>
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

/** Why the opacity controls are off, when they are (contract §11.1, L1). */
function OpacityNote() {
  return (
    <Show when={opacityLocked()}>
      <p class="note">{nativeSupported() ? "Your accessibility settings (Reduce transparency or Increase contrast) keep the window opaque." : "Atrium uses opaque windows on Linux."}</p>
    </Show>
  );
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
            <Show when={section().id === "general" && !connected()}><p class="note">These settings belong to Atrium's background service, which isn't connected right now.</p></Show>
            <Show when={section().id === "appearance"}><OpacityNote /></Show>
            <Show when={section().id === "appearance" && nativeError()}><p class="note error" role="alert">{nativeError()}</p></Show>
            <Show when={section().advanced.length}>
              <button type="button" class="more" classList={{ on: advancedOpen() }} aria-expanded={advancedOpen()} onClick={() => setAdvancedOpen(!advancedOpen())}>
                Advanced {section().label.toLowerCase()}<Spacer /><ChevronRight class="i sm" aria-hidden="true" />
              </button>
            </Show>
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
