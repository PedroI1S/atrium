// Right panel (ui-direction §2.2): a card that minimizes to a 46 px strip. In M1 it has one
// tab, Details: what Atrium knows about the thread and what it can enforce for it (D-044).
// Changes, Checks, Files and Terminal arrive with workspaces (M2).

import { ChevronsLeft, ChevronsRight, Info } from "lucide-solid";
import { For, Show } from "solid-js";
import { daemon } from "../daemon/client";
import { toneOf } from "../daemon/reduce";
import type { ThreadSummary } from "../daemon/types";
import { inboxPick, screen, setPanel } from "../state";
import { model } from "../windows";
import { IconButton, Spacer } from "./controls";
import { providerLabel } from "./ThreadCard";

function shownThread(): ThreadSummary | undefined {
  const id = screen() === "inbox" ? inboxPick() : model().mainThread;
  return id ? daemon().threads[id] : undefined;
}

function Fact(props: { label: string; value: string | undefined | null; mono?: boolean }) {
  return (
    <Show when={props.value}>
      <div class="fact"><span class="fl">{props.label}</span><span class="fv" classList={{ code: props.mono }}>{props.value}</span></div>
    </Show>
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
            <For each={e().reasons}>{(r) => <p class="fact-note">{r.message}</p>}</For>
            <Fact label="CLI version" value={e().cli_version ? `${e().cli_version}${e().version_tested ? "" : " (not tested with Atrium)"}` : null} />
          </>
        )}
      </Show>
      <Fact label="Session" value={props.thread.native_session_id} mono />
      <Fact label="Started" value={new Date(props.thread.created_at_ms).toLocaleString()} />
    </div>
  );
}

export function ContextPanel() {
  return (
    <section class="card panel" aria-label="Context panel">
      <header class="head panel-head">
        <div class="stabs" role="tablist" aria-label="Panel tabs">
          <button type="button" class="stab on" role="tab" aria-selected="true">Details</button>
        </div>
        <Spacer />
        <IconButton small label="Minimize panel (⌘⌥B)" onClick={() => setPanel({ open: false })}><ChevronsRight class="i sm" aria-hidden="true" /></IconButton>
      </header>
      <div class="panel-body" data-atrium-part="panel">
        <Show when={shownThread()} fallback={<p class="placeholder muted">{screen() === "inbox" ? "Select an item to see its chat." : "Details of the chat show here."}</p>}>
          {(t) => <Details thread={t()} />}
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
