// The inbox card: one ranked list across every thread (PLAN §3.4, contract §8), grouped as
// approvals and questions, failed, ready for you. Approvals and questions carry their buttons
// on the item (the same cards as the chat, compact), so most asks are answered from here.
// Keyboard: J / K move, ↵ opens the chat, A allows once, D denies.

import { For, Show, createEffect, createMemo, onCleanup, onMount } from "solid-js";
import { GROUP_LABEL, type InboxFilter, filterInbox, groupInbox, itemKey, moveSelection, needsYouCount } from "../chat/inbox";
import { type ThreadModel, holdThread, resolveApproval } from "../chat/threads";
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

/** The models of the items whose buttons show here, by thread id (for the A and D keys). */
const held = new Map<string, ThreadModel>();

/** The pending requests of one thread, as cards. Views the thread while it is shown. */
function Cards(props: { threadId: string }) {
  const hold = holdThread(props.threadId);
  held.set(props.threadId, hold.model);
  onCleanup(() => {
    held.delete(props.threadId);
    hold.release();
  });
  const pending = () => (hold.model.tick(), hold.model.transcript.pending());
  return (
    <Show when={pending().length}>
      <div class="cards">
        <For each={pending().slice(0, MAX_CARDS)}>
          {(request) => <RequestCard threadId={props.threadId} item={request} tick={hold.model.tick} small keys={false} />}
        </For>
        <Show when={pending().length > MAX_CARDS}><span class="src">{pending().length - MAX_CARDS} more in the chat</span></Show>
      </div>
    </Show>
  );
}

function Item(props: { item: InboxItem; cards: boolean }) {
  const key = () => itemKey(props.item);
  const reason = () => REASON[props.item.reason];
  let row!: HTMLDivElement;
  createEffect(() => {
    if (inboxPick() === key()) row.scrollIntoView({ block: "nearest" });
  });
  return (
    <div class="item" role="listitem" ref={row} classList={{ on: inboxPick() === key() }} data-atrium-part="inbox-item" data-reason={props.item.reason} onPointerDown={() => setInboxPick(key())}>
      <div class="body">
        <button type="button" class="open" disabled={props.item.thread_id === null} title={props.item.thread_id ? "Open the chat" : undefined} onFocus={() => setInboxPick(key())} onClick={() => props.item.thread_id && openThread(props.item.thread_id)}>
          <Dot tone={reason().tone} />
          <span class="body">
            <span class="ttl">{props.item.detail ?? (props.item.thread_id ? props.item.title || "Untitled chat" : reason().label)}</span>
            <span class="src">{reason().label}{props.item.detail && props.item.thread_id ? ` · ${props.item.title || "Untitled chat"}` : ""}{props.item.thread_id ? " · Home" : ""}</span>
          </span>
          <span class="tm">{ageLabel(props.item.since_ms, now())}</span>
        </button>
        <Show when={props.cards && props.item.thread_id} keyed>{(id) => <Cards threadId={id} />}</Show>
      </div>
    </div>
  );
}

export function InboxCard() {
  const all = () => daemon().inbox;
  const groups = createMemo(() => groupInbox(filterInbox(all(), inboxFilter())));
  const flat = createMemo(() => groups().flatMap((g) => g.items));
  const keys = createMemo(() => flat().map(itemKey));
  const byKey = createMemo(() => new Map(flat().map((i) => [itemKey(i), i])));
  /** The first asks get their buttons here (see MAX_HELD). */
  const withCards = createMemo(() => new Set(flat().filter((i) => i.reason === "approval" || i.reason === "question").slice(0, MAX_HELD).map(itemKey)));
  const asks = () => needsYouCount(all());

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
  const decide = (action: "allow" | "deny") => {
    const item = picked();
    if (!item?.thread_id) return;
    const request = held.get(item.thread_id)?.transcript.pending().find((r) => r.kind === "approval");
    if (request) void resolveApproval(item.thread_id, request, action);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.metaKey || e.ctrlKey || e.altKey || e.repeat) return;
    if (e.target instanceof HTMLTextAreaElement || e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
    const k = e.key.toLowerCase();
    const take = (f: () => void) => { e.preventDefault(); f(); };
    if (k === "j" || e.key === "ArrowDown") take(() => setInboxPick(moveSelection(keys(), inboxPick(), 1)));
    else if (k === "k" || e.key === "ArrowUp") take(() => setInboxPick(moveSelection(keys(), inboxPick(), -1)));
    // ↵ on a focused button presses that button.
    else if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) take(() => { const id = picked()?.thread_id; if (id) openThread(id); });
    else if (k === "a") take(() => decide("allow"));
    else if (k === "d") take(() => decide("deny"));
  };

  return (
    <>
      <header class="head" data-tauri-drag-region>
        <span class="title">Inbox</span>
        <span class="muted small">{asks() ? `${asks()} need you` : all().length ? `${all().length} to read` : ""}</span>
        <Spacer />
        <Seg label="Show" value={inboxFilter()} options={FILTERS} onChange={setInboxFilter} />
      </header>
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
                  {(key) => <Show when={byKey().get(key)}>{(item) => <Item item={item()} cards={withCards().has(key)} />}</Show>}
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
        <Spacer />
        <span><Kbd>{`${modKey()}J`}</Kbd> next item that needs you, from anywhere</span>
      </footer>
    </>
  );
}
