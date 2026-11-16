// The inbox card: one ranked list across every thread (PLAN §3.4, contract §8), grouped as
// approvals and questions, failed, ready for you. Approvals and questions carry their buttons
// on the item (the same cards as the chat, compact), so most asks are answered from here.
// Approvals can be selected and allowed once or denied together (PLAN §3.4).
// Keyboard: J / K move, ↵ opens the chat, A allows once, D denies, X selects.

import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount, untrack } from "solid-js";
import { type BatchSummary, type BatchTarget, allState, batchKey, outcomeOf, pruned, selectAll, summarize, keyTargets, toggled, toggledGroup } from "../chat/batch";
import { GROUP_LABEL, type InboxFilter, filterInbox, groupInbox, itemKey, moveSelection, needsYouCount } from "../chat/inbox";
import { canAct, cardState, subjectOf } from "../chat/requests";
import { type ThreadModel, holdThread, localOf, resolveApproval } from "../chat/threads";
import { daemon } from "../daemon/client";
import { ageLabel, type Tone } from "../daemon/reduce";
import type { InboxItem } from "../daemon/types";
import { inboxFilter, inboxPick, now, setInboxFilter, setInboxPick } from "../state";
import { Dot, Kbd, Seg, Spacer, modKey } from "./controls";
import { RequestCard } from "./RequestCard";
import { openThread } from "./Sidebar";

const REASON: Record<InboxItem["reason"], { tone: Tone; label: string }> = {
  storage_full: { tone: "failed", label: "Storage is full" },
  approval: { tone: "needs-you", label: "Approval" },
  question: { tone: "needs-you", label: "Question" },
  failed: { tone: "failed", label: "Failed" },
  cleanup_unverified: { tone: "failed", label: "Cleanup unverified" },
  reply: { tone: "ready", label: "New reply" },
};

const FILTERS: { value: InboxFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "asks", label: "Approvals and questions" },
  { value: "replies", label: "Replies" },
];

/** Requests shown on one item; the chat has the rest. */
const MAX_CARDS = 2;
/**
 * Items whose thread the inbox views for its buttons. Each one costs a snapshot from the
 * client, so a long inbox views only its first asks; the others open their chat.
 */
const MAX_HELD = 12;

/** The models of the items whose buttons show here, by thread id (for the A and D keys and the batch). */
const held = new Map<string, ThreadModel>();
/** Bumped when `held` changes, so the batch sees cards come and go. */
const [heldTick, setHeldTick] = createSignal(0);

/**
 * The batch selection as the cards see it. It lives in `InboxCard`, so leaving the inbox drops
 * the selection and the last result (round 2, minor 7).
 */
interface Batch { has: (key: string) => boolean; toggle: (key: string) => void }

/** The approval card holding focus, as its batch key (round 2, nit 9). */
const focusedCardKey = (target: EventTarget | null): string | null =>
  target instanceof Element ? target.closest<HTMLElement>('[data-atrium-part="approval"]')?.dataset.batchKey ?? null : null;

/** The pending requests of one thread, as cards. Views the thread while it is shown. */
function Cards(props: { threadId: string; batch: Batch }) {
  const hold = holdThread(props.threadId);
  held.set(props.threadId, hold.model);
  setHeldTick((n) => n + 1);
  onCleanup(() => {
    held.delete(props.threadId);
    setHeldTick((n) => n + 1);
    hold.release();
  });
  const pending = () => (hold.model.tick(), hold.model.transcript.pending());
  return (
    <Show when={pending().length}>
      <div class="cards">
        <For each={pending().slice(0, MAX_CARDS)}>
          {(request) => {
            const key = batchKey(props.threadId, request);
            const pick = request.kind === "approval" ? { key, get checked() { return props.batch.has(key); }, toggle: () => props.batch.toggle(key) } : undefined;
            return <RequestCard threadId={props.threadId} item={request} tick={hold.model.tick} small keys={false} pick={pick} />;
          }}
        </For>
        <Show when={pending().length > MAX_CARDS}><span class="src">{pending().length - MAX_CARDS} more in the chat</span></Show>
      </div>
    </Show>
  );
}

function Item(props: { item: InboxItem; cards: boolean; batch: Batch }) {
  const key = () => itemKey(props.item);
  const reason = () => REASON[props.item.reason];
  let row!: HTMLDivElement;
  createEffect(() => {
    if (inboxPick() === key()) row.scrollIntoView({ block: "nearest" });
  });
  return (
    <div class="item" role="listitem" ref={row} classList={{ on: inboxPick() === key() }} data-atrium-part="inbox-item" data-reason={props.item.reason} data-thread={props.item.thread_id ?? undefined} onPointerDown={() => setInboxPick(key())}
      // Focus moving into the row (Tab into a card, a checkbox) picks it, so the pick and the
      // focused card are never in different rows (round 3, blocker 2).
      onFocusIn={() => setInboxPick(key())}>
      <div class="body">
        <button type="button" class="open" disabled={props.item.thread_id === null} title={props.item.thread_id ? "Open the chat" : undefined} onFocus={() => setInboxPick(key())} onClick={() => props.item.thread_id && openThread(props.item.thread_id)}>
          <Dot tone={reason().tone} />
          <span class="body">
            <span class="ttl">{props.item.detail ?? (props.item.thread_id ? props.item.title || "Untitled chat" : reason().label)}</span>
            <span class="src">{reason().label}{props.item.detail && props.item.thread_id ? ` · ${props.item.title || "Untitled chat"}` : ""}{props.item.thread_id ? " · Home" : ""}</span>
          </span>
          <span class="tm">{ageLabel(props.item.since_ms, now())}</span>
        </button>
        <Show when={props.cards && props.item.thread_id} keyed>{(id) => <Cards threadId={id} batch={props.batch} />}</Show>
      </div>
    </div>
  );
}

export function InboxCard() {
  /** Approvals selected for the batch, by `batchKey`. */
  const [selection, setSelection] = createSignal<ReadonlySet<string>>(new Set());
  const [batchBusy, setBatchBusy] = createSignal(false);
  const [batchResult, setBatchResult] = createSignal<BatchSummary | null>(null);
  const select = (next: Set<string>) => { setBatchResult(null); setSelection(next); };
  const batch: Batch = { has: (key) => selection().has(key), toggle: (key) => select(toggled(selection(), key)) };
  const all = () => daemon().inbox;
  const groups = createMemo(() => groupInbox(filterInbox(all(), inboxFilter())));
  const flat = createMemo(() => groups().flatMap((g) => g.items));
  const keys = createMemo(() => flat().map(itemKey));
  const byKey = createMemo(() => new Map(flat().map((i) => [itemKey(i), i])));
  /** The first asks get their buttons here (see MAX_HELD). */
  const withCards = createMemo(() => new Set(flat().filter((i) => i.reason === "approval" || i.reason === "question").slice(0, MAX_HELD).map(itemKey)));
  const asks = () => needsYouCount(all());

  /** The approvals shown here that can still be answered: what the batch acts on. */
  const targets = createMemo<BatchTarget[]>(() => {
    heldTick();
    const out: BatchTarget[] = [];
    for (const item of flat()) {
      const id = item.thread_id;
      if (id === null || !withCards().has(itemKey(item))) continue;
      const m = held.get(id);
      if (!m) continue;
      m.tick();
      for (const r of m.transcript.pending().slice(0, MAX_CARDS)) {
        if (r.kind !== "approval" || !canAct(cardState(r, localOf(id, r)))) continue;
        out.push({ key: batchKey(id, r), threadId: id, request: r, chat: item.title || "Untitled chat", subject: subjectOf(r).text });
      }
    }
    return out;
  });
  const offered = createMemo(() => targets().map((t) => t.key));
  const chosen = () => targets().filter((t) => selection().has(t.key));
  // An approval answered elsewhere, or expired, leaves the selection.
  createEffect(() => {
    const keys = offered();
    const current = untrack(selection);
    const next = pruned(current, keys);
    if (next.size !== current.size) setSelection(next);
  });
  const runBatch = async (action: "allow" | "deny") => {
    const batch = chosen();
    if (!batch.length || batchBusy()) return;
    setBatchBusy(true);
    setBatchResult(null);
    try {
      // Each one goes through the ledger alone; one that fails doesn't stop the others.
      await Promise.all(batch.map((t) => resolveApproval(t.threadId, t.request, action)));
      setBatchResult(summarize(action, batch.map((t) => ({ target: t, outcome: outcomeOf(localOf(t.threadId, t.request)) }))));
      setSelection(new Set<string>());
    } finally {
      setBatchBusy(false);
    }
  };

  // The picked item follows the list: gone from it, the pick moves to the first item.
  createEffect(() => {
    const pick = inboxPick();
    if (pick !== null && !keys().includes(pick)) setInboxPick(keys()[0] ?? null);
  });

  let list!: HTMLDivElement;
  onMount(() => {
    if (inboxPick() === null) setInboxPick(keys()[0] ?? null);
    list.focus({ preventScroll: true });
  });

  const picked = () => { const k = inboxPick(); return k === null ? undefined : byKey().get(k); };
  /** A and D: the focused approval card, or with none focused the picked chat's first (`keyTargets`). */
  const decide = (action: "allow" | "deny", focused: string | null) => {
    const [t] = keyTargets(targets(), focused, picked()?.thread_id ?? null, false);
    if (t) void resolveApproval(t.threadId, t.request, action);
  };
  /** J and K move the pick, and focus with it: focus never stays in the row left behind. */
  const move = (by: 1 | -1) => {
    setInboxPick(moveSelection(keys(), inboxPick(), by));
    list.focus({ preventScroll: true });
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.metaKey || e.ctrlKey || e.altKey || e.repeat) return;
    // A batch checkbox keeps the inbox's keys (round 2, minor 7); a text field doesn't.
    const checkbox = e.target instanceof HTMLInputElement && e.target.type === "checkbox";
    if (e.target instanceof HTMLTextAreaElement || (e.target instanceof HTMLInputElement && !checkbox) || e.target instanceof HTMLSelectElement) return;
    const k = e.key.toLowerCase();
    const take = (f: () => void) => { e.preventDefault(); f(); };
    if (k === "j" || e.key === "ArrowDown") take(() => move(1));
    else if (k === "k" || e.key === "ArrowUp") take(() => move(-1));
    // ↵ on a focused button presses that button.
    else if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) take(() => { const id = picked()?.thread_id; if (id) openThread(id); });
    else if (k === "a") take(() => decide("allow", focusedCardKey(e.target)));
    else if (k === "d") take(() => decide("deny", focusedCardKey(e.target)));
    else if (k === "x") take(() => { const ks = keyTargets(targets(), focusedCardKey(e.target), picked()?.thread_id ?? null, true).map((t) => t.key); if (ks.length) select(toggledGroup(selection(), ks)); });
  };

  return (
    <>
      <header class="head" data-tauri-drag-region>
        <span class="title">Inbox</span>
        <span class="muted small">{asks() ? `${asks()} need you` : all().length ? `${all().length} to read` : ""}</span>
        <Spacer />
        <Seg label="Show" value={inboxFilter()} options={FILTERS} onChange={setInboxFilter} />
      </header>
      <Show when={targets().length || batchResult()}>
        <div class="batch-bar" data-atrium-part="batch-bar" onKeyDown={onKey}>
          <Show when={targets().length}>
            <label>
              <input type="checkbox" data-atrium-part="batch-all" checked={allState(selection(), offered()) === "all"} ref={(el) => createEffect(() => { el.indeterminate = allState(selection(), offered()) === "some"; })}
                onChange={() => select(selectAll(selection(), offered()))} />
              Select all approvals ({targets().length})
            </label>
            <Spacer />
            <button type="button" class="btn pri sm" data-atrium-part="batch-allow" disabled={!chosen().length || batchBusy()} onClick={() => void runBatch("allow")}>Allow once{chosen().length ? ` (${chosen().length})` : ""}</button>
            <button type="button" class="btn sm" data-atrium-part="batch-deny" disabled={!chosen().length || batchBusy()} onClick={() => void runBatch("deny")}>Deny{chosen().length ? ` (${chosen().length})` : ""}</button>
          </Show>
          <Show when={batchResult()}>
            {(r) => (
              <div class="batch-result" role="status" data-atrium-part="batch-result" data-failed={r().failures.length ? "" : undefined}>
                {r().text}
                <Show when={r().failures.length}>
                  <ul><For each={r().failures}>{(f) => <li>{f.chat}: <span class="code">{f.subject}</span>: {f.reason}</li>}</For></ul>
                </Show>
              </div>
            )}
          </Show>
        </div>
      </Show>
      <div class="inbox-list" role="list" aria-label="Inbox" tabindex="0" ref={list} data-atrium-part="inbox" onKeyDown={onKey}>
        {/* Keyed by group name and item key (strings): an inbox update keeps every item that is
            still listed, so its cards and their thread views aren't torn down and set up again. */}
        <For each={groups().map((g) => g.group)} fallback={
          <div class="thread-empty">
            <b>{all().length ? "Nothing here" : "Nothing needs you"}</b>
            <span>{all().length ? "Other items are under All." : "Approvals, questions, failures and new replies show here."}</span>
          </div>
        }>
          {(name) => {
            const items = () => groups().find((g) => g.group === name)?.items ?? [];
            return (
              <>
                <div class="igrp">{GROUP_LABEL[name]}<span>{items().length}</span></div>
                <For each={items().map(itemKey)}>
                  {(key) => <Show when={byKey().get(key)}>{(item) => <Item item={item()} cards={withCards().has(key)} batch={batch} />}</Show>}
                </For>
              </>
            );
          }}
        </For>
      </div>
      <footer class="statusb" data-atrium-part="inbox-keys">
        <span><Kbd>J</Kbd> <Kbd>K</Kbd> move</span>
        <span><Kbd>↵</Kbd> open</span>
        <span><Kbd>A</Kbd> allow once</span>
        <span><Kbd>D</Kbd> deny</span>
        <span><Kbd>X</Kbd> select</span>
        <Spacer />
        <span><Kbd>{`${modKey()}J`}</Kbd> <Kbd>{modKey() === "⌘" ? "⌘⇧J" : "Ctrl+Shift+J"}</Kbd> next or previous item that needs you, from anywhere</span>
      </footer>
    </>
  );
}
