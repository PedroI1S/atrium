// Small shared controls of the Calm look: state dot and pill, segmented control, toggle,
// slider, select, icon button. Styling is in styles/calm.css, all from tokens.

import { ChevronDown } from "lucide-solid";
import { For, type JSX, Show, splitProps } from "solid-js";
import { STATE_LABEL, type ThreadState } from "../data/dummy";

export function Dot(props: { state: ThreadState; label?: boolean }) {
  return <span class="dot" data-state={props.state} role={props.label ? "img" : undefined} aria-label={props.label ? STATE_LABEL[props.state] : undefined} aria-hidden={props.label ? undefined : "true"} />;
}

/** State pill: the state colour on a tint of itself, always with a dot and a label. */
export function Pill(props: { state: ThreadState; text?: string }) {
  return <span class="pill" data-state={props.state}><Dot state={props.state} />{props.text ?? STATE_LABEL[props.state]}</span>;
}

export function IconButton(props: JSX.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; small?: boolean; on?: boolean; children: JSX.Element }) {
  const [local, rest] = splitProps(props, ["label", "small", "on", "children", "class"]);
  return (
    <button type="button" {...rest} class={`ib ${local.class ?? ""}`} classList={{ s: local.small, on: local.on }} aria-label={local.label} title={local.label} aria-pressed={local.on === undefined ? undefined : local.on}>
      {local.children}
    </button>
  );
}

export function Seg<T extends string>(props: { value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; label: string; disabled?: boolean }) {
  return (
    <div class="seg" role="radiogroup" aria-label={props.label}>
      <For each={props.options}>
        {(o) => (
          <button type="button" role="radio" aria-checked={props.value === o.value} classList={{ on: props.value === o.value }} disabled={props.disabled} onClick={() => props.onChange(o.value)}>
            {o.label}
          </button>
        )}
      </For>
    </div>
  );
}

export function Toggle(props: { on: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return <button type="button" class="tog" role="switch" aria-checked={props.on} aria-label={props.label} classList={{ on: props.on }} disabled={props.disabled} onClick={() => props.onChange(!props.on)} />;
}

export function Range(props: { value: number; min: number; max: number; onInput: (v: number) => void; label: string; format: (v: number) => string; disabled?: boolean; wide?: boolean }) {
  return (
    <div class="rng" classList={{ narrow: !props.wide }}>
      <input type="range" min={props.min} max={props.max} value={props.value} aria-label={props.label} disabled={props.disabled} onInput={(e) => props.onInput(Number(e.currentTarget.value))} />
      <span class="rng-value">{props.format(props.value)}</span>
    </div>
  );
}

export function Select<T extends string>(props: { value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; label: string }) {
  return (
    <span class="sel">
      <select value={props.value} aria-label={props.label} onChange={(e) => props.onChange(e.currentTarget.value as T)}>
        <For each={props.options}>{(o) => <option value={o.value}>{o.label}</option>}</For>
      </select>
      <ChevronDown class="i sm" aria-hidden="true" />
    </span>
  );
}

/** Inline text with `backtick` spans rendered as code. */
export function Inline(props: { text: string }) {
  const parts = () => props.text.split(/(`[^`]+`)/g);
  return <For each={parts()}>{(p) => (p.startsWith("`") ? <code>{p.slice(1, -1)}</code> : p)}</For>;
}

export function Kbd(props: { children: string }) {
  return <span class="kbd">{props.children}</span>;
}

export function Spacer() {
  return <span class="sp" />;
}

export function When(props: { when: boolean; children: JSX.Element }) {
  return <Show when={props.when}>{props.children}</Show>;
}
