import { createVirtualizer } from "@tanstack/solid-virtual";
import { ArrowUp, Check, ExternalLink, FileText, Pencil, Search, Terminal } from "lucide-solid";
import { For, Match, Switch, createMemo, onMount } from "solid-js";
import { type Message, type ToolVerb, makeTranscript, threadById } from "../data/dummy";
import { registerScroller } from "../bench";
import { setPanel, showToast } from "../state";
import { model, patchView, persistScroll, popOut } from "../windows";
import { Composer } from "./Composer";
import { Dot, IconButton, Inline, Kbd, Pill, Spacer } from "./controls";

/** The 5,000-message transcript is the scroll benchmark's fixture. */
const BENCH_MESSAGES = 5000;
const OTHER_MESSAGES = 240;
/** Row estimate before measurement; rows are measured once rendered. */
const ROW_ESTIMATE = 92;
const OVERSCAN = 6;

const cache = new Map<string, Message[]>();
function transcriptFor(threadId: string): Message[] {
  let t = cache.get(threadId);
  if (!t) {
    t = threadId === "t-auth" ? makeTranscript(BENCH_MESSAGES, 7, true) : makeTranscript(OTHER_MESSAGES, threadId.length * 31, true);
    cache.set(threadId, t);
  }
  return t;
}

const VERB_ICON: Record<ToolVerb, typeof FileText> = { Read: FileText, Search, Edit: Pencil, Run: Terminal };

function ToolStep(props: { m: Extract<Message, { kind: "tool" }> }) {
  const Icon = VERB_ICON[props.m.verb];
  // Clicking a step opens the matching panel tab (ui-direction §3 Chat).
  const open = () => setPanel({ open: true, tab: props.m.verb === "Run" ? "terminal" : props.m.verb === "Edit" ? "changes" : "files" });
  return (
    <button type="button" class="step" onClick={open}>
      <Icon class="i sm" aria-hidden="true" />
      <span class="v">{props.m.verb}</span>
      <span class="tg">{props.m.target}</span>
      <span class="res" data-tone={props.m.result.tone}>
        <Switch>
          <Match when={props.m.result.tone === "diff"}><span class="add-t">+{props.m.result.add}</span><span class="del-t">−{props.m.result.del}</span></Match>
          <Match when={props.m.result.tone === "ok"}>{props.m.result.text}<Check class="i sm" aria-label="passed" /></Match>
          <Match when={true}>{props.m.result.text}</Match>
        </Switch>
      </span>
    </button>
  );
}

function Approval(props: { m: Extract<Message, { kind: "approval" }> }) {
  return (
    <div class="ask" role="group" aria-label="Approval request">
      <div class="ask-h"><Dot state="needs-you" /><span>Run this command?</span><Spacer /><span class="ask-mode">Ask first</span></div>
      <div class="ask-cmd">{props.m.command}</div>
      <p class="ask-note">{props.m.reason}</p>
      <div class="ask-act">
        <button type="button" class="btn pri" onClick={() => showToast("Allowed once (dummy)")}>Allow once<Kbd>↵</Kbd></button>
        <button type="button" class="btn" onClick={() => showToast("Allowed in this worktree (dummy)")}>Always in this worktree</button>
        <Spacer />
        <button type="button" class="btn ghost" onClick={() => showToast("Denied (dummy)")}>Deny<Kbd>esc</Kbd></button>
      </div>
    </div>
  );
}

function MessageView(props: { m: Message }) {
  return (
    <Switch>
      <Match when={props.m.kind === "user" && props.m}>{(m) => <div class="you">{(m() as Extract<Message, { kind: "user" }>).text}</div>}</Match>
      <Match when={props.m.kind === "assistant" && props.m}>
        {(m) => <For each={(m() as Extract<Message, { kind: "assistant" }>).paragraphs}>{(p) => <p class="msg"><Inline text={p} /></p>}</For>}
      </Match>
      <Match when={props.m.kind === "tool" && props.m}>{(m) => <ToolStep m={m() as Extract<Message, { kind: "tool" }>} />}</Match>
      <Match when={props.m.kind === "approval" && props.m}>{(m) => <Approval m={m() as Extract<Message, { kind: "approval" }>} />}</Match>
    </Switch>
  );
}

/** Virtualized thread at the reading measure, then the composer. */
export function ChatThread(props: { primary?: boolean; compact?: boolean }) {
  const threadId = () => model().views.chat.threadId;
  const base = createMemo(() => transcriptFor(threadId()));
  const local = () => model().views.chat.messages.map((text, i) => ({ kind: "user" as const, id: base().length + i, text }));
  const all = createMemo(() => (local().length ? [...base(), ...local()] : base()));
  let scroller!: HTMLDivElement;
  const virt = createVirtualizer({
    get count() { return all().length; },
    getScrollElement: () => scroller,
    estimateSize: () => ROW_ESTIMATE,
    overscan: OVERSCAN,
  });
  onMount(() => {
    const saved = model().views.chat.scroll;
    if (saved >= 0) scroller.scrollTop = saved;
    else virt.scrollToIndex(all().length - 1, { align: "end" });
    if (props.primary) registerScroller(scroller);
  });
  const send = (text: string) => {
    void patchView("chat", { messages: [...model().views.chat.messages, text], draft: "" });
    requestAnimationFrame(() => virt.scrollToIndex(all().length - 1, { align: "end" }));
  };
  // Consecutive tool steps sit together as one compact group.
  const tight = (i: number) => all()[i]?.kind === "tool" && all()[i + 1]?.kind === "tool";
  return (
    <div class="thread" classList={{ compact: props.compact }}>
      <div class="thread-scroll" ref={scroller} onScroll={() => persistScroll(scroller.scrollTop)} tabindex="0" role="log" aria-label="Conversation">
        <div class="thread-sizer" style={{ height: `${virt.getTotalSize()}px` }}>
          <For each={virt.getVirtualItems()}>
            {(item) => (
              <div class="thread-row" data-index={item.index} classList={{ tight: tight(item.index) }} ref={(el) => queueMicrotask(() => virt.measureElement(el))} style={{ transform: `translateY(${item.start}px)` }}>
                <div class="col"><MessageView m={all()[item.index]!} /></div>
              </div>
            )}
          </For>
        </div>
      </div>
      <div class="thread-composer">
        <Composer onSend={send} id={props.primary ? "composer-primary" : undefined} agent={threadById(threadId()).agent} compact={props.compact} />
      </div>
    </div>
  );
}

/** The chat card in the main window: title, state pill, worktree chip, ⧉, then the thread. */
export function ChatCard() {
  const thread = () => threadById(model().views.chat.threadId);
  return (
    <>
      <header class="head" data-tauri-drag-region>
        <span class="title">{thread().title}</span>
        <Pill state={thread().state} />
        <Spacer />
        <span class="chip">{thread().worktree}</span>
        <IconButton label="Open in new window" onClick={() => void popOut("chat")}><ExternalLink class="i" aria-hidden="true" /></IconButton>
      </header>
      <ChatThread primary />
    </>
  );
}

/** Compact, static chat for the editor's side panel (ui-direction §3 Editor). */
export function MiniChat() {
  return (
    <div class="mini">
      <div class="mini-ctx"><Dot state="needs-you" />Rework session token refresh</div>
      <div class="you">Can SKEW come from config instead?</div>
      <p class="msg">Yes. I'd read it from <code>[session] skew</code> in atrium.toml and keep 30 s as the default. I've put that in the editor as a suggestion so you can accept it there.</p>
      <div class="ask small">
        <div class="ask-h"><Dot state="needs-you" />Run the reconnect test?</div>
        <div class="ask-act"><button type="button" class="btn pri sm">Allow once</button><button type="button" class="btn ghost sm">Deny</button></div>
      </div>
      <Spacer />
      <div class="composer compact static">
        <div class="ph">Ask about this file…</div>
        <div class="cm-row"><span class="chip">session.rs · L43–52</span><button type="button" class="send" aria-label="Send"><ArrowUp class="i" aria-hidden="true" /></button></div>
      </div>
    </div>
  );
}
