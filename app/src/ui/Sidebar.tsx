// Left sidebar (ui-direction §2.1): sits on the window frame, no card around it. In M1 every
// thread is a Home thread, so there is one list (topic groups arrive with workspaces; D-059).

import { House, Inbox, PanelLeft, Plus, Search, SlidersHorizontal } from "lucide-solid";
import { For, Show } from "solid-js";
import { daemon } from "../daemon/client";
import { needsYouCount } from "../chat/inbox";
import { ageLabel, toneOf } from "../daemon/reduce";
import { now, screen, setPaletteOpen, setScreen, setSidebarOpen } from "../state";
import { model, platform, setMainThread } from "../windows";
import { Dot, IconButton, Kbd, Spacer } from "./controls";

export function openThread(threadId: string | null): void {
  setMainThread(threadId);
  setScreen("chat");
}

// What the Inbox tile says: how many need the user (approvals, questions), as the inbox header
// does; with none, how many are there to read (failed, replies).
const needCount = () => needsYouCount(daemon().inbox);
const readCount = () => daemon().inbox.length;

export function Sidebar() {
  const selected = (id: string | null) => screen() === "chat" && model().mainThread === id;
  return (
    <aside class="sidebar" aria-label="Chats">
      <div class="side-top" data-tauri-drag-region classList={{ lights: platform() === "macos" }}>
        <IconButton small label="Hide sidebar (⌘B)" onClick={() => setSidebarOpen(false)}><PanelLeft class="i sm" aria-hidden="true" /></IconButton>
      </div>
      <button type="button" class="cmdbar" onClick={() => setPaletteOpen(true)}><Search class="i sm" aria-hidden="true" />Ask, search or jump<Kbd>⌘K</Kbd></button>
      <div class="tiles">
        <button type="button" class="tile" classList={{ on: screen() === "inbox" }} aria-current={screen() === "inbox" ? "page" : undefined} onClick={() => setScreen("inbox")}>
          <span class="tn"><Inbox class="i sm" aria-hidden="true" />Inbox</span>
          <span class="ts" classList={{ need: needCount() > 0 }}>{needCount() > 0 ? `${needCount()} need you` : readCount() > 0 ? `${readCount()} to read` : "All clear"}</span>
        </button>
        <button type="button" class="tile" classList={{ on: selected(null) }} aria-current={selected(null) ? "page" : undefined} onClick={() => openThread(null)}>
          <span class="tn"><House class="i sm" aria-hidden="true" />Home</span><span class="ts">This computer</span>
        </button>
      </div>
      <div class="space-h">Chats</div>
      <button type="button" class="newchat" onClick={() => openThread(null)}><Plus class="i sm" aria-hidden="true" />New chat</button>
      <nav class="chats" aria-label="Chats">
        <For each={daemon().order} fallback={<p class="side-empty">{daemon().loaded ? "No chats yet." : ""}</p>}>
          {(id) => {
            const thread = () => daemon().threads[id];
            return (
              <Show when={thread()}>
                {(t) => (
                  <button type="button" class="chat-row" classList={{ sel: selected(id) }} aria-current={selected(id) ? "page" : undefined} onClick={() => openThread(id)}>
                    <Dot tone={toneOf(t()).tone} label={toneOf(t()).label} /><span class="t">{t().title || "Untitled chat"}</span><span class="m">{ageLabel(t().updated_at_ms, now())}</span>
                  </button>
                )}
              </Show>
            );
          }}
        </For>
      </nav>
      <div class="spaces">
        <Spacer />
        <IconButton label="Settings (⌘,)" on={screen() === "settings" ? true : undefined} onClick={() => setScreen("settings")}><SlidersHorizontal class="i" aria-hidden="true" /></IconButton>
      </div>
    </aside>
  );
}

/** Hidden sidebar (⌘B): show-sidebar, Inbox with a dot when something needs you, Home, Settings. */
export function SidebarStrip() {
  return (
    <nav class="strip side-strip" aria-label="Navigation" classList={{ lights: platform() === "macos" }}>
      <button type="button" class="sb" aria-label="Show sidebar (⌘B)" title="Show sidebar" onClick={() => setSidebarOpen(true)}><PanelLeft class="i sm" aria-hidden="true" /></button>
      <button type="button" class="sb" aria-label={needCount() > 0 ? `Inbox, ${needCount()} need you` : readCount() > 0 ? `Inbox, ${readCount()} to read` : "Inbox"} title="Inbox" onClick={() => setScreen("inbox")}>
        <Inbox class="i" aria-hidden="true" /><Show when={readCount() > 0}><span class="bd" data-state={needCount() > 0 ? "needs-you" : "ready"} /></Show>
      </button>
      <button type="button" class="sb" aria-label="Home" title="Home" onClick={() => openThread(null)}><House class="i" aria-hidden="true" /></button>
      <span class="sp" />
      <button type="button" class="sb" aria-label="Settings" title="Settings" onClick={() => setScreen("settings")}><SlidersHorizontal class="i" aria-hidden="true" /></button>
    </nav>
  );
}
