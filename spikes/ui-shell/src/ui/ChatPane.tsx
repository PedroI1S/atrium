import { createVirtualizer } from "@tanstack/solid-virtual";
import { Check, CircleAlert, Terminal } from "lucide-solid";
import { For, Match, Switch, createMemo, createSignal, onMount } from "solid-js";
import { type Message, makeTranscript, threadById } from "../data/dummy";
import { registerScroller } from "../bench";
import { showToast } from "../state";
import { Composer } from "./Composer";

const cache = new Map<string, Message[]>();
function transcriptFor(threadId: string): Message[] {
  let t = cache.get(threadId);
  if (!t) {
    // The first thread gets the big transcript used by the scroll benchmark.
    t = threadId === "t-auth" ? makeTranscript(5000, 7) : makeTranscript(240, threadId.length * 31);
    cache.set(threadId, t);
  }
  return t;
}

function Inline(props: { text: string }) {
  const parts = () => props.text.split(/(`[^`]+`)/g);
  return <For each={parts()}>{(p) => (p.startsWith("`") ? <code class="inline-code">{p.slice(1, -1)}</code> : p)}</For>;
}

function MessageView(props: { m: Message }) {
  return (
    <div class="msg" data-kind={props.m.kind}>
      <Switch>
        <Match when={props.m.kind === "user" && props.m}>
          {(m) => <div class="bubble-user">{(m() as Extract<Message, { kind: "user" }>).text}</div>}
        </Match>
        <Match when={props.m.kind === "assistant" && props.m}>
          {(m) => (
            <div class="prose">
              <For each={(m() as Extract<Message, { kind: "assistant" }>).paragraphs}>{(p) => <p><Inline text={p} /></p>}</For>
            </div>
          )}
        </Match>
        <Match when={props.m.kind === "tool" && props.m}>
          {(m) => {
            const t = m() as Extract<Message, { kind: "tool" }>;
            return (
              <div class="tool" data-status={t.status}>
                <div class="tool-head">
                  <Terminal class="icon-xs" />
                  <span class="tool-name">{t.tool}</span>
                  <span class="tool-target">{t.target}</span>
                  {t.status === "ok" ? <Check class="icon-xs tool-ok" /> : <CircleAlert class="icon-xs tool-err" />}
                </div>
                <pre class="tool-out">{t.lines.join("\n")}</pre>
              </div>
            );
          }}
        </Match>
        <Match when={props.m.kind === "approval" && props.m}>
          {(m) => {
            const a = m() as Extract<Message, { kind: "approval" }>;
            return (
              <div class="approval" role="group" aria-label="Approval request">
                <div class="approval-title">Needs your approval</div>
                <code class="approval-cmd">{a.command}</code>
                <div class="approval-reason">{a.reason}</div>
                <div class="approval-actions">
                  <button class="btn" onClick={() => showToast("Denied (dummy)")}>Deny</button>
                  <button class="btn btn-primary" onClick={() => showToast("Allowed once (dummy)")}>Allow once</button>
                </div>
              </div>
            );
          }}
        </Match>
      </Switch>
    </div>
  );
}

export function ChatPane(props: { threadId: string; primary?: boolean }) {
  const thread = () => threadById(props.threadId);
  const [local, setLocal] = createSignal<Message[]>([]);
  const base = createMemo(() => transcriptFor(props.threadId));
  const all = createMemo(() => (local().length ? [...base(), ...local()] : base()));
  let scroller!: HTMLDivElement;
  const virt = createVirtualizer({
    get count() {
      return all().length;
    },
    getScrollElement: () => scroller,
    estimateSize: () => 92,
    overscan: 6,
  });
  onMount(() => {
    virt.scrollToIndex(all().length - 1, { align: "end" });
    if (props.primary) registerScroller(scroller);
  });
  const send = (text: string) => {
    const id = all().length;
    setLocal((l) => [...l, { kind: "user", id, text }]);
    requestAnimationFrame(() => virt.scrollToIndex(all().length - 1, { align: "end" }));
    window.setTimeout(() => {
      setLocal((l) => [...l, { kind: "assistant", id: id + 1, paragraphs: ["Understood. This is a dummy reply: nothing is running behind the mock."] }]);
      requestAnimationFrame(() => virt.scrollToIndex(all().length - 1, { align: "end" }));
    }, 350);
  };
  return (
    <section class="pane chat" aria-label={`Thread: ${thread().title}`} data-atrium-part="chat">
      <header class="pane-head">
        <span class="state-dot" data-state={thread().state} />
        <span class="pane-title">{thread().title}</span>
        <span class="pane-sub">{thread().agent}</span>
        <span class="side-grow" />
        <span class="pane-sub">{all().length.toLocaleString()} messages</span>
      </header>
      <div class="chat-scroll" ref={scroller} tabindex="0" role="log" aria-label="Conversation">
        <div class="chat-sizer" style={{ height: `${virt.getTotalSize()}px` }}>
          <For each={virt.getVirtualItems()}>
            {(item) => (
              <div
                class="chat-row"
                data-index={item.index}
                ref={(el) => queueMicrotask(() => virt.measureElement(el))}
                style={{ transform: `translateY(${item.start}px)` }}
              >
                <div class="chat-inner">
                  <MessageView m={all()[item.index]!} />
                </div>
              </div>
            )}
          </For>
        </div>
      </div>
      <div class="chat-composer">
        <div class="chat-inner">
          <Composer onSend={send} id={props.primary ? "composer-primary" : undefined} />
        </div>
      </div>
    </section>
  );
}
