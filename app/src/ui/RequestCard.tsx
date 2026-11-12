// Approval and question cards (ui-direction §3 Chat and Inbox; contract §5; D-054, D-062).
// The card shows the exact command or paths, why it asks, and Allow once / Always… / Deny. It
// never decides anything itself: buttons send a command to the daemon's ledger, and the card
// shows what the ledger and the events say (a stale request can't be allowed, and says so).

import { ChevronDown } from "lucide-solid";
import { For, Show, createSignal } from "solid-js";
import type { QuestionAnswer, RememberScope } from "../daemon/types";
import { type CardState, answersComplete, askOf, canAct, cardState, rememberChoices, subjectOf } from "../chat/requests";
import { answerQuestion, localOf, resolveApproval, stopTurnForQuestion } from "../chat/threads";
import type { ApprovalItem, QuestionItem, RequestItem } from "../chat/transcript";
import { Dot, Kbd } from "./controls";
import { askedAgo } from "./Transcript";

function StateLine(props: { state: CardState }) {
  const text = () => {
    const s = props.state;
    switch (s.kind) {
      case "fresh": return null;
      case "sending": return s.action === "allow" ? "Allowing…" : s.action === "deny" ? "Denying…" : s.action === "cancel" ? "Stopping the turn…" : "Sending your answer…";
      case "sent": return s.action === "allow" ? "Allowed. Waiting for the agent." : s.action === "deny" ? "Denied. Waiting for the agent." : s.action === "cancel" ? "Stopping the turn…" : "Answered. Waiting for the agent.";
      default: return s.text;
    }
  };
  return (
    <Show when={text()}>
      <p class="ask-state" data-kind={props.state.kind} role={props.state.kind === "stale" || props.state.kind === "failed" ? "alert" : "status"}>{text()}</p>
    </Show>
  );
}

export function ApprovalCard(props: { threadId: string; item: ApprovalItem; tick: () => number; small?: boolean; autofocus?: boolean }) {
  const state = () => (props.tick(), cardState(props.item, localOf(props.threadId, props.item)));
  const subject = () => subjectOf(props.item);
  const choices = () => rememberChoices(props.item);
  const [menu, setMenu] = createSignal(false);
  const act = () => canAct(state());
  const allow = (remember: RememberScope | null = null) => { setMenu(false); void resolveApproval(props.threadId, props.item, "allow", remember); };
  const deny = () => void resolveApproval(props.threadId, props.item, "deny");
  return (
    <div
      class="ask"
      classList={{ small: props.small, done: !act() }}
      role="group"
      aria-label={askOf(props.item)}
      data-atrium-part="approval"
      data-state={state().kind}
      tabindex="-1"
      ref={(el) => props.autofocus && queueMicrotask(() => el.focus({ preventScroll: true }))}
      onKeyDown={(e) => {
        if (!act() || e.target instanceof HTMLTextAreaElement) return;
        if (e.key === "Enter" && !e.metaKey && !e.ctrlKey) { e.preventDefault(); allow(); }
        else if (e.key === "Escape") { e.preventDefault(); deny(); }
      }}
    >
      <div class="ask-h">
        <Dot tone={act() ? "needs-you" : state().kind === "stale" ? "idle" : "ready"} />
        {askOf(props.item)}
        <span class="ask-mode">{props.item.name} · {askedAgo(props.item.at)}</span>
      </div>
      <div class="ask-cmd" aria-label={subject().label}>{subject().text}</div>
      <Show when={props.item.reason}><p class="ask-note">{props.item.reason}</p></Show>
      <StateLine state={state()} />
      <Show when={act()}>
        <div class="ask-act">
          <button type="button" class="btn pri" classList={{ sm: props.small }} onClick={() => allow()}>Allow once<Kbd>↵</Kbd></button>
          <Show when={choices().length}>
            <span class="menu-wrap">
              <button type="button" class="btn" classList={{ sm: props.small }} aria-haspopup="menu" aria-expanded={menu()} onClick={() => setMenu(!menu())}>Always…<ChevronDown class="i xs" aria-hidden="true" /></button>
              <Show when={menu()}>
                <div class="menu" role="menu" onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); setMenu(false); } }}>
                  <For each={choices()}>
                    {(c) => (
                      <button type="button" class="menu-item" role="menuitem" onClick={() => allow(c.scope)}>
                        <b>{c.label}</b><span>{c.hint}</span>
                      </button>
                    )}
                  </For>
                </div>
              </Show>
            </span>
          </Show>
          <button type="button" class="btn ghost" classList={{ sm: props.small }} onClick={deny}>Deny<Kbd>esc</Kbd></button>
        </div>
      </Show>
    </div>
  );
}

type Picks = Record<string, { selected: string[]; text: string }>;

export function QuestionCard(props: { threadId: string; item: QuestionItem; tick: () => number; small?: boolean }) {
  const state = () => (props.tick(), cardState(props.item, localOf(props.threadId, props.item)));
  const [picks, setPicks] = createSignal<Picks>({});
  const pick = (id: string) => picks()[id] ?? { selected: [], text: "" };
  const choose = (qid: string, oid: string, multi: boolean) => {
    const cur = pick(qid);
    const selected = multi ? (cur.selected.includes(oid) ? cur.selected.filter((x) => x !== oid) : [...cur.selected, oid]) : [oid];
    setPicks({ ...picks(), [qid]: { ...cur, selected } });
  };
  const ready = () => answersComplete(props.item.questions, picks());
  const submit = () => {
    if (!ready() || !canAct(state())) return;
    const answers: QuestionAnswer[] = props.item.questions.map((q) => ({ question_id: q.id, selected: pick(q.id).selected, text: pick(q.id).text.trim() || null }));
    void answerQuestion(props.threadId, props.item, answers);
  };
  const single = () => props.item.questions.length === 1 && !props.item.questions[0]!.multi_select;
  return (
    <div class="ask" classList={{ small: props.small, done: !canAct(state()) }} role="group" aria-label="A question from the agent" data-atrium-part="question" data-state={state().kind}>
      <div class="ask-h"><Dot tone={canAct(state()) ? "needs-you" : "idle"} />The agent asks<span class="ask-mode">{askedAgo(props.item.at)}</span></div>
      <For each={props.item.questions}>
        {(q) => (
          <div class="q">
            <p class="q-prompt">{q.prompt}</p>
            <Show when={q.options?.length}>
              <div class="q-opts" role={q.multi_select ? "group" : "radiogroup"} aria-label={q.prompt}>
                <For each={q.options}>
                  {(o) => (
                    <button
                      type="button"
                      class="btn q-opt"
                      classList={{ sm: props.small, on: pick(q.id).selected.includes(o.id) }}
                      role={q.multi_select ? "checkbox" : "radio"}
                      aria-checked={pick(q.id).selected.includes(o.id)}
                      title={o.description ?? undefined}
                      disabled={!canAct(state())}
                      onClick={() => {
                        choose(q.id, o.id, q.multi_select ?? false);
                        // One question, one choice, no text: the click is the answer (the inbox's buttons).
                        if (props.small && single() && !q.allow_text) submit();
                      }}
                    >
                      {o.label}
                    </button>
                  )}
                </For>
              </div>
            </Show>
            <Show when={q.allow_text && canAct(state())}>
              <textarea
                class="q-text"
                rows={1}
                placeholder={q.options?.length ? "Or answer in your own words" : "Your answer"}
                aria-label={`Answer: ${q.prompt}`}
                value={pick(q.id).text}
                onInput={(e) => setPicks({ ...picks(), [q.id]: { ...pick(q.id), text: e.currentTarget.value } })}
                onKeyDown={(e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); submit(); } }}
              />
            </Show>
          </div>
        )}
      </For>
      <StateLine state={state()} />
      <Show when={canAct(state())}>
        <div class="ask-act">
          <button type="button" class="btn pri" classList={{ sm: props.small }} disabled={!ready()} onClick={submit}>Answer<Kbd>↵</Kbd></button>
          <button type="button" class="btn ghost" classList={{ sm: props.small }} title="The agent gets no answer; its turn stops." onClick={() => void stopTurnForQuestion(props.threadId, props.item)}>Stop the turn</button>
        </div>
      </Show>
    </div>
  );
}

export function RequestCard(props: { threadId: string; item: RequestItem; tick: () => number; small?: boolean; autofocus?: boolean }) {
  return (
    <Show when={props.item.kind === "approval"} fallback={<QuestionCard threadId={props.threadId} item={props.item as QuestionItem} tick={props.tick} small={props.small} />}>
      <ApprovalCard threadId={props.threadId} item={props.item as ApprovalItem} tick={props.tick} small={props.small} autofocus={props.autofocus} />
    </Show>
  );
}
