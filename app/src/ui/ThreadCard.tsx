// The main card when it shows a chat: header (title, state pill, provider, ⧉), the transcript
// region and the composer. The transcript itself, sending, approvals and questions are
// M1-09b's; this file gives them their place and keeps the draft.

import { ChevronDown, ExternalLink, RotateCcw } from "lucide-solid";
import { For, Show, createEffect, onCleanup } from "solid-js";
import { agent, agentChoices, markRead, probes, reportFocus, resumeThread, sendMessage, sending, setAgent, startChat, stopTurn } from "../chat/actions";
import { MAX_TITLE_CHARS, renameThread, renaming, setRenaming } from "../chat/thread-actions";
import { type ThreadModel, holdThread } from "../chat/threads";
import { windowFocused } from "../state";
import { daemon } from "../daemon/client";
import { toneOf } from "../daemon/reduce";
import type { ThreadSummary } from "../daemon/types";
import { NEW_THREAD } from "../window-model";
import { bringBack, focusPopout, model, ownsView, popOut } from "../windows";
import { Composer } from "./Composer";
import { EnforcementBadge } from "./Enforcement";
import { RequestCard } from "./RequestCard";
import { ThreadMenu } from "./ThreadActions";
import { TranscriptView } from "./Transcript";
import { IconButton, Pill, Spacer } from "./controls";

const PROVIDER: Record<ThreadSummary["provider"], string> = { claude: "Claude Code", codex: "Codex", agy: "Gemini", acp: "ACP" };
export const providerLabel = (t: ThreadSummary): string => (t.model ? `${PROVIDER[t.provider]} · ${t.model}` : PROVIDER[t.provider]);

/** The title, or the field that renames it ("Rename" in the ⋯ menu or the palette). */
/**
 * A blur this soon after the field appears is focus coming back to whatever opened it (the
 * palette or the menu closing), not the user leaving: the field keeps focus.
 */
const RENAME_SETTLE_MS = 400;

function TitleText(props: { thread: ThreadSummary }) {
  let cancelled = false;
  let shownAt = 0;
  const done = (input: HTMLInputElement) => {
    if (renaming() !== props.thread.thread_id) return;
    setRenaming(null);
    if (!cancelled) void renameThread(props.thread.thread_id, input.value);
  };
  return (
    <Show when={renaming() === props.thread.thread_id} fallback={<span class="title">{props.thread.title || "Untitled chat"}</span>}>
      <input
        class="title-edit"
        aria-label="Chat name"
        data-atrium-part="rename"
        maxLength={MAX_TITLE_CHARS}
        value={props.thread.title}
        ref={(el) => { cancelled = false; shownAt = performance.now(); queueMicrotask(() => { el.focus(); el.select(); }); }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.isComposing) { e.preventDefault(); done(e.currentTarget); }
          else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); cancelled = true; done(e.currentTarget); }
        }}
        onBlur={(e) => {
          const el = e.currentTarget;
          if (performance.now() - shownAt < RENAME_SETTLE_MS) { setTimeout(() => el.focus()); return; }
          done(el);
        }}
      />
    </Show>
  );
}

/** Header of a thread's view, in the main window and (compact) in a pop-out. */
export function ThreadTitle(props: { thread: ThreadSummary }) {
  return (
    <>
      <TitleText thread={props.thread} />
      <Pill tone={toneOf(props.thread).tone} text={toneOf(props.thread).label} />
    </>
  );
}

/** The empty state of a view: Home's, or a thread's before its first event. */
function Empty(props: { thread: ThreadSummary | undefined; load?: string }) {
  return (
    <div class="thread-empty">
      <Show when={props.thread} fallback={<><b>Home</b><span>Ask about anything on this computer. Home reads freely and asks before it changes something.</span></>}>
        <span>{props.load ?? "The conversation shows here."}</span>
      </Show>
    </div>
  );
}

/** Pending approvals and questions, docked above the composer so they are never scrolled away. */
function Dock(props: { model: ThreadModel }) {
  const pending = () => (props.model.tick(), props.model.transcript.pending());
  const MAX_DOCKED = 3;
  return (
    <Show when={pending().length}>
      <div class="dock" data-atrium-part="dock">
        <For each={pending().slice(0, MAX_DOCKED)}>
          {(item, i) => <RequestCard threadId={props.model.threadId} item={item} tick={props.model.tick} autofocus={i() === 0} />}
        </For>
        <Show when={pending().length > MAX_DOCKED}><p class="muted small">{pending().length - MAX_DOCKED} more after these</p></Show>
      </div>
    </Show>
  );
}

function AgentChip() {
  return (
    <Show when={agentChoices().length} fallback={<span class="chip">{probes.loading ? "Looking for agents…" : "No agent installed"}</span>}>
      <span class="sel chip-sel">
        <select aria-label="Agent" value={agent()?.id ?? ""} onChange={(e) => setAgent(e.currentTarget.value)}>
          <For each={agentChoices()}>{(a) => <option value={a.id}>{a.label}</option>}</For>
        </select>
        <ChevronDown class="i sm" aria-hidden="true" />
      </span>
    </Show>
  );
}

/** A thread's transcript, its pending requests and its composer, while the view holds it. */
function LiveThread(props: { viewKey: string; thread: ThreadSummary }) {
  const held = holdThread(props.thread.thread_id);
  let shown = true;
  onCleanup(() => { shown = false; held.release(); });
  const model = held.model;
  const running = () => props.thread.state.type === "running" || props.thread.state.type === "starting" || props.thread.state.type === "needs_approval" || props.thread.state.type === "needs_input";
  // Looking at a thread with a new reply reads it (contract §8), while the window has focus.
  createEffect(() => {
    if (props.thread.unread && windowFocused()) markRead(props.thread.thread_id, () => shown && windowFocused());
  });
  // The daemon doesn't notify about the chat the user is looking at (contract §8, `SetFocus`).
  createEffect(() => reportFocus(windowFocused() ? props.thread.thread_id : null));
  onCleanup(() => reportFocus(null));
  const load = () => model.load();
  return (
    <>
      <TranscriptView model={model} viewKey={props.viewKey} empty={<Empty thread={props.thread} load={load().kind === "failed" ? `This chat can't be shown: ${(load() as { message: string }).message}` : load().kind === "loading" ? "" : undefined} />} />
      <div class="thread-composer">
        <Dock model={model} />
        <Composer
          viewKey={props.viewKey}
          id="composer-primary"
          placeholder={running() ? "Reply… (it waits for the running turn)" : "Reply…"}
          running={running()}
          busy={sending() === props.thread.thread_id}
          onSend={(text) => void sendMessage(props.thread, text)}
          onStop={() => void stopTurn(props.thread.thread_id)}
        />
      </div>
    </>
  );
}

/** The transcript region and the composer of one view. `viewKey` is a thread id or NEW_THREAD. */
export function ThreadBody(props: { viewKey: string; thread: ThreadSummary | undefined }) {
  return (
    <div class="thread">
      <Show when={props.thread?.thread_id} keyed fallback={
        <>
          <div class="thread-scroll" tabindex="0" role="log" aria-label="Conversation" data-atrium-part="transcript"><Empty thread={undefined} /></div>
          <div class="thread-composer">
            <Composer viewKey={props.viewKey} id="composer-primary" placeholder="Ask Home…" busy={sending() === NEW_THREAD} chips={<AgentChip />} onSend={(text) => void startChat(text)} />
          </div>
        </>
      }>
        {/* Keyed by the thread id, never by the summary: a summary changes with every event, and a
            view that was rebuilt for each one would ask for a new snapshot and redraw everything. */}
        {(id) => {
          const first = props.thread!;
          return <LiveThread viewKey={props.viewKey} thread={daemon().threads[id] ?? first} />;
        }}
      </Show>
    </div>
  );
}

/** While a thread is in its own window, its place in the main window says so (ui-direction §3). */
function PoppedOut(props: { threadId: string }) {
  return (
    <div class="placeholder">
      <p class="muted">This chat is open in its own window.</p>
      <div class="row-actions">
        <button type="button" class="btn sm" onClick={() => void focusPopout(props.threadId)}>Show the window</button>
        <button type="button" class="btn ghost sm" onClick={() => void bringBack(props.threadId)}>Bring back</button>
      </div>
    </div>
  );
}

/** The chat card in the main window. */
export function ThreadCard() {
  const threadId = () => model().mainThread;
  const thread = () => { const id = threadId(); return id ? daemon().threads[id] : undefined; };
  const key = () => threadId() ?? NEW_THREAD;
  return (
    <>
      <header class="head" data-tauri-drag-region>
        <Show when={thread()} fallback={<span class="title">{threadId() && daemon().loaded ? "This chat is no longer here" : threadId() ? "" : "New chat"}</span>}>
          {(t) => <ThreadTitle thread={t()} />}
        </Show>
        <Spacer />
        <Show when={thread()}>
          {(t) => (
            <>
              <Show when={t().state.type === "interrupted" && !(t().native_session_id ?? "").startsWith("atrium-fake:")}>
                <button type="button" class="btn sm" title="Starts the agent again from where it was cut off" onClick={() => void resumeThread(t().thread_id)}><RotateCcw class="i xs" aria-hidden="true" />Resume</button>
              </Show>
              <span class="chip">{providerLabel(t())}</span>
              <EnforcementBadge thread={t()} />
              <Show when={ownsView(t().thread_id)}>
                <IconButton label="Open in new window" onClick={() => void popOut(t().thread_id)}><ExternalLink class="i" aria-hidden="true" /></IconButton>
              </Show>
              <ThreadMenu thread={t()} />
            </>
          )}
        </Show>
      </header>
      <Show when={ownsView(key())} fallback={<PoppedOut threadId={key()} />}>
        <ThreadBody viewKey={key()} thread={thread()} />
      </Show>
    </>
  );
}
