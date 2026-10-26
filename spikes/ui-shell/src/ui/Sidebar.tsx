// Left sidebar (ui-direction §2.1): sits on the window frame, no card around it.

import { Ellipsis, House, Inbox, PanelLeft, Plus, Search, SlidersHorizontal } from "lucide-solid";
import { For, Show } from "solid-js";
import { TOPICS, needsYouCount, topicNeedsYou, type Topic } from "../data/dummy";
import { screen, setPaletteOpen, setScreen, setSidebarOpen, setTopicId, showToast, topicId } from "../state";
import { model, patchView } from "../windows";
import { Dot, IconButton, Kbd, Spacer } from "./controls";

export const currentTopic = (): Topic => TOPICS.find((t) => t.id === topicId()) ?? TOPICS[0]!;

export function openChat(threadId: string): void {
  setScreen("chat");
  if (model().views.chat.owner === "main") void patchView("chat", { threadId });
}

function TopicCircle(props: { topic: Topic }) {
  const on = () => props.topic.id === topicId();
  const attention = () => topicNeedsYou(props.topic);
  return (
    <button type="button" class="spb" classList={{ on: on() }} role="tab" aria-selected={on()} aria-label={props.topic.name + (attention() ? ", needs you" : "")} title={props.topic.name} onClick={() => setTopicId(props.topic.id)}>
      {props.topic.monogram}
      <Show when={!on() && attention()}><span class="badge" aria-hidden="true" /></Show>
    </button>
  );
}

export function Sidebar() {
  const chatShown = () => screen() === "chat";
  return (
    <aside class="sidebar" aria-label="Topics and chats">
      <div class="side-top" data-tauri-drag-region>
        <IconButton small label="Hide sidebar (⌘B)" onClick={() => setSidebarOpen(false)}><PanelLeft class="i sm" aria-hidden="true" /></IconButton>
      </div>
      <button type="button" class="cmdbar" onClick={() => setPaletteOpen(true)}><Search class="i sm" aria-hidden="true" />Ask, search or jump<Kbd>⌘K</Kbd></button>
      <div class="tiles">
        <button type="button" class="tile" classList={{ on: screen() === "inbox" }} onClick={() => setScreen("inbox")}>
          <span class="tn"><Inbox class="i sm" aria-hidden="true" />Inbox</span><span class="ts need">{needsYouCount()} need you</span>
        </button>
        <button type="button" class="tile" onClick={() => showToast("Home is not designed in this round")}>
          <span class="tn"><House class="i sm" aria-hidden="true" />Home</span><span class="ts">This Mac</span>
        </button>
      </div>
      <div class="space-h">{currentTopic().name}<Spacer /><IconButton small label="Topic options"><Ellipsis class="i sm" aria-hidden="true" /></IconButton></div>
      <button type="button" class="newchat" onClick={() => showToast("New chat in this topic (dummy)")}><Plus class="i sm" aria-hidden="true" />New chat in this topic</button>
      <nav class="chats" aria-label={`Chats in ${currentTopic().name}`}>
        <For each={currentTopic().threads}>
          {(t) => {
            const selected = () => chatShown() && model().views.chat.threadId === t.id;
            return (
              <button type="button" class="chat-row" classList={{ sel: selected() }} aria-current={selected() ? "page" : undefined} onClick={() => openChat(t.id)}>
                <Dot state={t.state} label /><span class="t">{t.title}</span><span class="m">{t.age}</span>
              </button>
            );
          }}
        </For>
      </nav>
      <Spacer />
      <div class="spaces" role="tablist" aria-label="Topics">
        <For each={TOPICS}>{(t) => <TopicCircle topic={t} />}</For>
        <button type="button" class="spb" aria-label="New topic" title="New topic" onClick={() => showToast("New topic (dummy)")}><Plus class="i sm" aria-hidden="true" /></button>
        <Spacer />
        <IconButton label="Settings" on={screen() === "settings" ? true : undefined} onClick={() => setScreen("settings")}><SlidersHorizontal class="i" aria-hidden="true" /></IconButton>
      </div>
    </aside>
  );
}

/** Hidden sidebar (⌘B): show-sidebar, Inbox with a dot, and the topic circles. */
export function SidebarStrip() {
  return (
    <nav class="strip side-strip" aria-label="Topics">
      <button type="button" class="sb" aria-label="Show sidebar (⌘B)" title="Show sidebar" onClick={() => setSidebarOpen(true)}><PanelLeft class="i sm" aria-hidden="true" /></button>
      <button type="button" class="sb" aria-label={`Inbox, ${needsYouCount()} need you`} title="Inbox" onClick={() => setScreen("inbox")}><Inbox class="i" aria-hidden="true" /><span class="bd" data-state="needs-you" /></button>
      <span class="sdiv" />
      <For each={TOPICS}>{(t) => <TopicCircle topic={t} />}</For>
    </nav>
  );
}
