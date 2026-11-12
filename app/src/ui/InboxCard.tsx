// The inbox card: one ranked list across every thread (PLAN §3.4), as the daemon ranks it.
// M1-09a lists the items and opens their chat; the approval and question controls on each
// item, the filters and the keyboard (J, K, A, D) are M1-09b's.

import { For, Show } from "solid-js";
import { daemon } from "../daemon/client";
import { ageLabel, type Tone } from "../daemon/reduce";
import type { InboxItem } from "../daemon/types";
import { inboxPick, now, setInboxPick } from "../state";
import { Dot, Spacer } from "./controls";
import { openThread } from "./Sidebar";

const REASON: Record<InboxItem["reason"], { tone: Tone; label: string }> = {
  storage_full: { tone: "failed", label: "Storage is full" },
  approval: { tone: "needs-you", label: "Approval" },
  question: { tone: "needs-you", label: "Question" },
  failed: { tone: "failed", label: "Failed" },
  cleanup_unverified: { tone: "failed", label: "Cleanup unverified" },
  reply: { tone: "ready", label: "New reply" },
};

export function InboxCard() {
  const items = () => daemon().inbox;
  return (
    <>
      <header class="head" data-tauri-drag-region>
        <span class="title">Inbox</span>
        <span class="muted small">{items().length ? `${items().length} need you` : ""}</span>
        <Spacer />
      </header>
      <div class="inbox" role="list" aria-label="Inbox" data-atrium-part="inbox">
        <For each={items()} fallback={<div class="thread-empty"><b>Nothing needs you</b><span>Approvals, questions, failures and new replies show here.</span></div>}>
          {(item) => (
            <button
              type="button"
              class="inbox-row"
              role="listitem"
              classList={{ sel: item.thread_id !== null && inboxPick() === item.thread_id }}
              disabled={item.thread_id === null}
              onFocus={() => setInboxPick(item.thread_id)}
              onClick={() => item.thread_id && openThread(item.thread_id)}
            >
              <Dot tone={REASON[item.reason].tone} />
              <span class="ir-main">
                <b>{item.detail ?? REASON[item.reason].label}</b>
                <span>{REASON[item.reason].label} · {item.title || "Untitled chat"}</span>
              </span>
              <span class="m">{ageLabel(item.since_ms, now())}</span>
            </button>
          )}
        </For>
      </div>
      <Show when={items().length}>
        <footer class="card-foot muted small">⌘J jumps here from anywhere</footer>
      </Show>
    </>
  );
}
