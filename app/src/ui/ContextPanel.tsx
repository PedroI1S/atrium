// Right panel (ui-direction §2.2): a card that minimizes to a 46 px strip. In M1 it has one
// tab, Details: what Atrium knows about the thread, what it can enforce for it (D-044), and the
// session's usage and context. Beside the inbox it previews the selected item.
// Changes, Checks, Files and Terminal arrive with workspaces (M2).

import { ChevronsLeft, ChevronsRight, Info } from "lucide-solid";
import { For, Show, createSignal, onCleanup } from "solid-js";
import { holdThread } from "../chat/threads";
import { daemon } from "../daemon/client";
import { toneOf } from "../daemon/reduce";
import type { InboxItem, ThreadSummary, Usage } from "../daemon/types";
import { inboxPick, screen, setPanel } from "../state";
import { model, ownsView } from "../windows";
import { IconButton, Spacer } from "./controls";
import { DetailsLink } from "./Enforcement";
import { openThread } from "./Sidebar";
import { providerLabel } from "./ThreadCard";

function shownThread(): ThreadSummary | undefined {
  const id = screen() === "inbox" ? inboxPick() : model().mainThread;
  return id ? daemon().threads[id] : undefined;
}

/** The inbox item under the keyboard (its key is its thread id, or `@storage_full`). */
function pickedItem(): InboxItem | undefined {
  const pick = inboxPick();
  return pick === null ? undefined : daemon().inbox.find((i) => (i.thread_id ?? `@${i.reason}`) === pick);
}

function Fact(props: { label: string; value: string | undefined | null; mono?: boolean }) {
  return (
    <Show when={props.value}>
      <div class="fact"><span class="fl">{props.label}</span><span class="fv" classList={{ code: props.mono }}>{props.value}</span></div>
    </Show>
  );
}

const PERCENT = 100;
const count = (n: number | null | undefined) => (n === null || n === undefined ? null : n.toLocaleString());

function tokensLine(u: Usage): string | null {
  const parts = [
    u.input_tokens !== null ? `${count(u.input_tokens)} in` : null,
    u.output_tokens !== null ? `${count(u.output_tokens)} out` : null,
    u.reasoning_tokens ? `${count(u.reasoning_tokens)} reasoning` : null,
    u.cache_read_tokens ? `${count(u.cache_read_tokens)} read from cache` : null,
  ].filter((p): p is string => p !== null);
  return parts.length ? parts.join(" · ") : null;
}

/** Usage and context of the thread's session, from its events (the view holds the thread while shown). */
function UsageFacts(props: { threadId: string }) {
  const held = holdThread(props.threadId);
  onCleanup(held.release);
  const t = () => (held.model.tick(), held.model.transcript);
  const context = () => {
    const c = t().context;
    if (!c) return null;
    return c.window ? `${count(c.used)} of ${count(c.window)} tokens (${Math.round((c.used / c.window) * PERCENT)}%)` : `${count(c.used)} tokens`;
  };
  const usage = () => t().usage;
  return (
    <Show when={context() || usage()}>
      <div class="facts-t">Usage</div>
      <div class="facts" data-atrium-part="usage">
        <Fact label="Context" value={context()} />
        <Fact label="Tokens" value={usage() ? tokensLine(usage()!) : null} />
        <Fact label="Cost" value={usage()?.cost_usd ? `$${usage()!.cost_usd!.toFixed(2)}` : null} />
      </div>
    </Show>
  );
}

/** Reasons shown before "N more" (a partial session can list one per tool class). */
const REASONS_SHOWN = 3;

/** Why enforcement is partial: the first few, the rest one click away. */
function Reasons(props: { reasons: readonly { message: string }[] }) {
  const [all, setAll] = createSignal(false);
  const shown = () => (all() ? props.reasons : props.reasons.slice(0, REASONS_SHOWN));
  const more = () => props.reasons.length - REASONS_SHOWN;
  return (
    <>
      <For each={shown()}>{(r) => <p class="fact-note">{r.message}</p>}</For>
      <Show when={more() > 0}>
        <p class="fact-note"><button type="button" class="link-btn" aria-expanded={all()} onClick={() => setAll(!all())}>{all() ? "Show fewer" : `${more()} more`}</button></p>
      </Show>
    </>
  );
}

function Details(props: { thread: ThreadSummary }) {
  const enforcement = () => props.thread.enforcement;
  return (
    <div class="facts">
      <Fact label="State" value={toneOf(props.thread).label} />
      <Fact label="Agent" value={providerLabel(props.thread)} />
      <Fact label="Where" value="Home (this computer)" />
      <Show when={enforcement()} fallback={<Fact label="Enforcement" value="Known once the session starts" />}>
        {(e) => (
          <>
            <Fact label="Enforcement" value={e().level === "full" ? "Full: every tool call passes Atrium's rules" : "Partial: Atrium sees only some of what this agent does"} />
            <Reasons reasons={e().reasons} />
            <p class="fact-note"><DetailsLink descriptor={e()} subject={props.thread.title || "Untitled chat"} /></p>
            <Fact label="CLI version" value={e().cli_version ? `${e().cli_version}${e().version_tested ? "" : " (not tested with Atrium)"}` : null} />
          </>
        )}
      </Show>
      <Fact label="Session" value={props.thread.native_session_id} mono />
      <Fact label="Started" value={new Date(props.thread.created_at_ms).toLocaleString()} />
    </div>
  );
}

const REASON_TEXT: Record<InboxItem["reason"], string> = {
  storage_full: "Atrium's storage is full. Chats are paused until there is room.",
  approval: "Waiting for your approval",
  question: "Waiting for your answer",
  failed: "The session failed",
  cleanup_unverified: "Atrium couldn't confirm that this chat's agent stopped",
  reply: "A new reply",
};

/** Beside the inbox: the selected item's ask and its chat. */
function Preview(props: { item: InboxItem }) {
  return (
    <div class="preview" data-atrium-part="inbox-preview">
      <b>{props.item.title || (props.item.thread_id ? "Untitled chat" : "Storage is full")}</b>
      <span class="muted">{REASON_TEXT[props.item.reason]}</span>
      <Show when={props.item.detail}><p>{props.item.detail}</p></Show>
      <Show when={props.item.thread_id}>
        {(id) => <div class="row-actions"><button type="button" class="btn sm" onClick={() => openThread(id())}>Open chat</button></div>}
      </Show>
    </div>
  );
}

export function ContextPanel() {
  return (
    <section class="card panel" aria-label="Context panel">
      <header class="head panel-head">
        <div class="stabs" role="tablist" aria-label="Panel tabs">
          <button type="button" class="stab on" role="tab" aria-selected="true">{screen() === "inbox" ? "Preview" : "Details"}</button>
        </div>
        <Spacer />
        <IconButton small label="Minimize panel (⌘⌥B)" onClick={() => setPanel({ open: false })}><ChevronsRight class="i sm" aria-hidden="true" /></IconButton>
      </header>
      <div class="panel-body" data-atrium-part="panel">
        <Show when={screen() === "inbox" && pickedItem()}>{(item) => <Preview item={item()} />}</Show>
        <Show when={shownThread()} fallback={<Show when={!(screen() === "inbox" && pickedItem())}><p class="placeholder muted">{screen() === "inbox" ? "Select an item to see its chat." : "Details of the chat show here."}</p></Show>}>
          {(t) => (
            <>
              <Details thread={t()} />
              {/* Only for the chat this window shows: moving through the inbox doesn't view every thread. */}
              <Show when={screen() === "chat" && ownsView(t().thread_id) && t().thread_id} keyed>{(id) => <UsageFacts threadId={id} />}</Show>
            </>
          )}
        </Show>
      </div>
    </section>
  );
}

/** Minimized panel: expand, then one icon per tab. */
export function PanelStrip() {
  return (
    <nav class="card strip" aria-label="Panel">
      <button type="button" class="sb" aria-label="Expand panel" title="Expand panel" onClick={() => setPanel({ open: true })}><ChevronsLeft class="i sm" aria-hidden="true" /></button>
      <span class="sdiv" />
      <button type="button" class="sb" aria-label="Details" title="Details" onClick={() => setPanel({ open: true, tab: "details" })}><Info class="i" aria-hidden="true" /></button>
    </nav>
  );
}
