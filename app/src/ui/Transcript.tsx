// A thread's transcript: virtualized rows (a message has one row per markdown block, M0-07),
// following the tail until the user scrolls up, paging older history in at the top (contract
// §11). Agent text is rendered by chat/render.ts into DOM nodes (contract §12).

import { createVirtualizer } from "@tanstack/solid-virtual";
import { ArrowDown, Brain, Check, ChevronDown, ChevronRight, CircleStop, FileText, Globe, ListChecks, Pencil, Plug, Search, Terminal, Trash2, TriangleAlert, Wrench, X } from "lucide-solid";
import { type JSX, For, Index, Match, Show, Switch, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { ageLabel } from "../daemon/reduce";
import type { ToolKind } from "../daemon/types";
import { type BlockText } from "../chat/blocks";
import { parseMarkdown } from "../chat/markdown";
import { type LinkActions, renderBlocks } from "../chat/render";
import { cardState } from "../chat/requests";
import { type ThreadModel, localOf } from "../chat/threads";
import type { MessageItem, NoteItem, RequestItem, Row, ToolItem } from "../chat/transcript";
import { askOf, subjectOf } from "../chat/requests";
import { now, showToast } from "../state";
import { openLink, revealPath } from "../platform";
import { persistScroll, viewOf } from "../windows";
import { Dot } from "./controls";

/** Distance from the top (px) at which older history is asked for. */
const PAGE_TRIGGER_PX = 600;
/** Distance from the bottom (px) that still counts as "at the tail". */
const TAIL_SLACK_PX = 4;
/** Rows rendered beyond the viewport. */
const OVERSCAN = 6;
/** Tool output shown in a row (the rest stays in the model; the adapter caps it at 1 MiB). */
const OUTPUT_SHOWN_CHARS = 32 * 1024;
const KIB = 1024;

export const linkActions: LinkActions = {
  open: (url) => { openLink(url).catch((e) => showToast(String(e))); },
  reveal: (path) => { revealPath(path).catch((e) => showToast(String(e))); },
};

/** Row height estimates before measurement (px), from text length. */
const LINE_PX = 24;
const CHARS_PER_LINE = 80;
const ROW_PX = 34;
function estimate(row: Row | undefined): number {
  if (!row) return ROW_PX;
  if (row.type === "block") return LINE_PX + LINE_PX * Math.ceil(row.block.text.length / CHARS_PER_LINE);
  if (row.type === "user") return ROW_PX + LINE_PX * Math.ceil(row.item.chars / CHARS_PER_LINE);
  return ROW_PX;
}

// -- rows ------------------------------------------------------------------------------------

function BlockRow(props: { item: MessageItem; block: BlockText; tick: () => number }) {
  let el!: HTMLDivElement;
  let seen: BlockText | null = null;
  let seenV = -1;
  createEffect(() => {
    props.tick();
    const b = props.block;
    if (b === seen && b.v === seenV) return;
    seen = b;
    seenV = b.v;
    el.replaceChildren(renderBlocks(document, parseMarkdown(b.text), linkActions));
  });
  return <div ref={el} class="md" classList={{ cont: props.block.cont, reasoning: props.item.role === "reasoning", streaming: !props.block.closed }} />;
}

function UserRow(props: { item: MessageItem; tick: () => number }) {
  // A user's own text: shown as typed, never as markdown.
  const text = () => (props.tick(), props.item.splitter.blocks.map((b) => b.text).join("\n\n").replace(/\n$/, ""));
  return <div class="you-row"><div class="you">{text()}</div></div>;
}

const TOOL_ICON: Partial<Record<ToolKind, (p: { class: string }) => JSX.Element>> = {
  execute: (p) => <Terminal {...p} aria-hidden="true" />,
  read: (p) => <FileText {...p} aria-hidden="true" />,
  edit: (p) => <Pencil {...p} aria-hidden="true" />,
  delete: (p) => <Trash2 {...p} aria-hidden="true" />,
  move: (p) => <Pencil {...p} aria-hidden="true" />,
  search: (p) => <Search {...p} aria-hidden="true" />,
  fetch: (p) => <Globe {...p} aria-hidden="true" />,
  mcp: (p) => <Plug {...p} aria-hidden="true" />,
  think: (p) => <Brain {...p} aria-hidden="true" />,
};

const STATUS: Record<ToolItem["status"], { text: string; tone: "ok" | "fail" | "plain" }> = {
  running: { text: "Running", tone: "plain" },
  succeeded: { text: "Done", tone: "ok" },
  failed: { text: "Failed", tone: "fail" },
  denied: { text: "Denied", tone: "fail" },
  cancelled: { text: "Cancelled", tone: "plain" },
  interrupted: { text: "Interrupted", tone: "plain" },
};

function ToolRow(props: { item: ToolItem; tick: () => number }) {
  const [open, setOpen] = createSignal(false);
  const t = () => (props.tick(), props.item);
  const status = () => STATUS[t().status];
  const shown = () => {
    const out = t().output;
    return out.length > OUTPUT_SHOWN_CHARS ? out.slice(-OUTPUT_SHOWN_CHARS) : out;
  };
  const hidden = () => Math.max(0, t().output.length - OUTPUT_SHOWN_CHARS);
  return (
    <div class="tool">
      <button type="button" class="step" aria-expanded={open()} onClick={() => setOpen(!open())}>
        {(TOOL_ICON[t().toolKind] ?? ((p: { class: string }) => <Wrench {...p} aria-hidden="true" />))({ class: "i sm" })}
        <span class="v">{t().name}</span>
        <span class="tg">{t().title ?? ""}</span>
        <span class="res" data-tone={status().tone}>
          <Show when={t().status === "running"} fallback={<Show when={status().tone === "ok"} fallback={<Show when={status().tone === "fail"}><X class="i xs" aria-hidden="true" /></Show>}><Check class="i xs" aria-hidden="true" /></Show>}>
            <Dot tone="running" />
          </Show>
          {status().text}
        </span>
        <Show when={open()} fallback={<ChevronRight class="i xs muted" aria-hidden="true" />}><ChevronDown class="i xs muted" aria-hidden="true" /></Show>
      </button>
      <Show when={open()}>
        <div class="tool-out">
          <Show when={t().error}><p class="tool-err">{t().error}</p></Show>
          <Show when={hidden() > 0}><p class="muted small">{Math.round(hidden() / KIB)} KB of earlier output not shown</p></Show>
          <Show when={shown()} fallback={<p class="muted small">{t().status === "running" ? "No output yet." : "No output."}</p>}>
            <pre class="tool-pre">{shown()}</pre>
          </Show>
          <Show when={t().truncatedBytes > 0}><p class="muted small">{Math.round(t().truncatedBytes / KIB)} KB more output was dropped by the agent's adapter.</p></Show>
        </div>
      </Show>
    </div>
  );
}

function RequestRow(props: { threadId: string; item: RequestItem; tick: () => number }) {
  const state = () => (props.tick(), cardState(props.item, localOf(props.threadId, props.item)));
  const what = () => (props.item.kind === "approval" ? `${askOf(props.item)} ${subjectOf(props.item).text}` : (props.item.questions[0]?.prompt ?? "A question"));
  const text = () => {
    const s = state();
    if (s.kind === "resolved" || s.kind === "stale" || s.kind === "failed") return s.text;
    if (s.kind === "sending" || s.kind === "sent") return "Sending…";
    return "Waiting for you, below";
  };
  const tone = () => (state().kind === "fresh" ? "needs-you" : state().kind === "stale" ? "idle" : "ready");
  return (
    <div class="req-row" data-atrium-part="request-row">
      <Dot tone={tone()} />
      <span class="req-what">{what()}</span>
      <span class="req-state">{text()}</span>
    </div>
  );
}

const TOKENS_K = 1000;
const fmtTokens = (n: number | null | undefined) => (n === null || n === undefined ? null : n >= TOKENS_K ? `${(n / TOKENS_K).toFixed(1)}k` : String(n));

/** A note never changes once made, so it renders once per item. */
function NoteRow(props: { item: NoteItem }) {
  const [open, setOpen] = createSignal(false);
  const n = props.item.note;
  switch (n.type) {
    case "session_started":
      return <div class="note-row"><span>Session started{n.model ? ` · ${n.model}` : ""}</span><span class="muted">· Enforcement: {n.enforcement.level === "full" ? "full" : "partial"}</span></div>;
    case "enforcement_changed":
      return <div class="note-row"><TriangleAlert class="i xs" aria-hidden="true" /><span>What Atrium can enforce for this session changed: {n.enforcement.level === "full" ? "full" : "partial"}. See Details.</span></div>;
    case "session_ended":
      return <div class="note-row"><CircleStop class="i xs" aria-hidden="true" /><span>Session ended{n.reason ? `: ${n.reason}` : n.exitCode !== null && n.exitCode !== 0 ? ` (exit ${n.exitCode})` : ""}</span></div>;
    case "turn_completed":
      return (
        <div class="note-row turn-end">
          <span>{n.outcome === "completed" ? "Done" : n.outcome === "interrupted" ? "Stopped" : n.outcome === "limit_reached" ? "Stopped at a limit" : "The turn failed"}</span>
          <Show when={n.usage?.output_tokens}><span class="muted">· {fmtTokens(n.usage?.output_tokens)} tokens out</span></Show>
        </div>
      );
    case "error":
      return (
        <div class="note-row err" role="alert">
          <TriangleAlert class="i xs" aria-hidden="true" />
          <span class="err-msg">{n.message}</span>
          <Show when={n.details.length}>
            <button type="button" class="btn ghost sm" onClick={() => setOpen(!open())}>{open() ? "Hide details" : "Details"}</button>
          </Show>
          <Show when={open()}><pre class="tool-pre">{n.details.join("\n")}</pre></Show>
        </div>
      );
    case "rate_limited":
      return <div class="note-row"><span>Paused by a limit{n.resetsAtMs ? ` until ${new Date(n.resetsAtMs).toLocaleTimeString()}` : ""}{n.message ? `: ${n.message}` : ""}</span></div>;
    case "plan":
      return (
        <div class="plan">
          <div class="plan-h"><ListChecks class="i sm" aria-hidden="true" />Plan</div>
          <For each={n.entries}>{(e) => <div class="plan-e" data-status={e.status}><Dot tone={e.status === "completed" ? "ready" : e.status === "in_progress" ? "running" : "idle"} />{e.content}</div>}</For>
        </div>
      );
  }
}

function RowView(props: { threadId: string; row: Row; tick: () => number }) {
  return (
    <Switch>
      <Match when={props.row.type === "block" && props.row}>{(r) => <BlockRow item={r().item} block={r().block} tick={props.tick} />}</Match>
      <Match when={props.row.type === "user" && props.row}>{(r) => <UserRow item={r().item} tick={props.tick} />}</Match>
      <Match when={props.row.type === "tool" && props.row}>{(r) => <ToolRow item={r().item} tick={props.tick} />}</Match>
      <Match when={props.row.type === "request" && props.row}>{(r) => <RequestRow threadId={props.threadId} item={r().item} tick={props.tick} />}</Match>
      <Match when={props.row.type === "note" && props.row.item} keyed>{(item) => <NoteRow item={item as NoteItem} />}</Match>
    </Switch>
  );
}

// -- the list --------------------------------------------------------------------------------

export function TranscriptView(props: { model: ThreadModel; viewKey: string; empty: JSX.Element }) {
  let scroller!: HTMLDivElement;
  const t = () => props.model.transcript;
  const rows = createMemo(() => (props.model.tick(), t().rows()), undefined, { equals: false });
  const virt = createVirtualizer({
    get count() { return rows().length; },
    getScrollElement: () => scroller,
    estimateSize: (i) => estimate(rows()[i]),
    getItemKey: (i) => rows()[i]?.key ?? i,
    overscan: OVERSCAN,
  });

  // Follow the tail until the user scrolls up, decided from user input, not from scroll
  // distance (M0-07: rows arriving faster than a frame grow the distance by themselves).
  const saved = viewOf(props.viewKey).scroll;
  let follow = saved === null;
  const [atTail, setAtTail] = createSignal(follow);
  const unfollow = () => { follow = false; setAtTail(false); };
  const toTail = () => {
    follow = true;
    setAtTail(true);
    if (t().detached) props.model.reattach();
    scroller.scrollTop = scroller.scrollHeight;
  };
  let paging = false;
  const onScroll = () => {
    const bottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    if (bottom < TAIL_SLACK_PX && !t().detached) { follow = true; setAtTail(true); }
    persistScroll({ key: props.viewKey, scroll: follow ? null : Math.round(scroller.scrollTop) });
    if (!paging && scroller.scrollTop < PAGE_TRIGGER_PX && t().hasOlder && !follow) {
      paging = true;
      void props.model.pageOlder().then((added) => {
        // Keep the row that was at the top where it was.
        if (added > 0) virt.scrollToIndex(added, { align: "start" });
        paging = false;
      });
    }
  };
  const onWheel = (e: WheelEvent) => { if (e.deltaY < 0) unfollow(); };
  const onKey = (e: KeyboardEvent) => { if (["ArrowUp", "PageUp", "Home"].includes(e.key)) unfollow(); };

  createEffect(on(() => [rows(), virt.getTotalSize()], () => {
    if (follow) scroller.scrollTop = scroller.scrollHeight;
  }));
  onMount(() => {
    if (saved !== null) queueMicrotask(() => { scroller.scrollTop = saved; });
  });
  onCleanup(() => persistScroll.flush());

  const items = createMemo(() => virt.getVirtualItems());
  return (
    <div class="thread-scroll" ref={scroller} tabindex="0" role="log" aria-label="Conversation" data-atrium-part="transcript"
      onScroll={onScroll} onWheel={onWheel} onPointerDown={(e) => { if (e.target === scroller) unfollow(); }} onTouchMove={unfollow} onKeyDown={onKey}>
      <Show when={rows().length} fallback={props.empty}>
        <Show when={t().truncated && !t().hasOlder}>
          <p class="history-note">Older messages of this chat are kept by Atrium's background service but not loaded here. Export the chat to read all of it.</p>
        </Show>
        <div class="thread-sizer" style={{ height: `${virt.getTotalSize()}px` }}>
          <Index each={items()}>
            {(v) => {
              const row = () => rows()[v().index];
              let el!: HTMLDivElement;
              // A reused row element shows another index: measure it again once it has rendered.
              createEffect(on(() => row(), () => queueMicrotask(() => el.isConnected && virt.measureElement(el))));
              return (
                <div class="thread-row" data-index={v().index} ref={el} style={{ transform: `translateY(${v().start}px)` }}>
                  <div class="col">
                    <Show when={row()}>{(r) => <RowView threadId={props.model.threadId} row={r()} tick={props.model.tick} />}</Show>
                  </div>
                </div>
              );
            }}
          </Index>
        </div>
      </Show>
      <Show when={!atTail() || t().detached}>
        <button type="button" class="to-tail" onClick={toTail}>
          <ArrowDown class="i sm" aria-hidden="true" />{t().detached ? `Newer messages${t().missed ? ` (${t().missed})` : ""}` : "Jump to latest"}
        </button>
      </Show>
    </div>
  );
}

/** "3m ago" for a request's card. */
export const askedAgo = (atMs: number) => ageLabel(atMs, now());
