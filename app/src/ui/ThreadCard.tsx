// The main card when it shows a chat: header (title, state pill, provider, ⧉), the transcript
// region and the composer. The transcript itself, sending, approvals and questions are
// M1-09b's; this file gives them their place and keeps the draft.

import { ExternalLink } from "lucide-solid";
import { Show } from "solid-js";
import { daemon } from "../daemon/client";
import { toneOf } from "../daemon/reduce";
import type { ThreadSummary } from "../daemon/types";
import { NEW_THREAD } from "../window-model";
import { bringBack, focusPopout, model, ownsView, popOut } from "../windows";
import { Composer } from "./Composer";
import { IconButton, Pill, Spacer } from "./controls";

const PROVIDER: Record<ThreadSummary["provider"], string> = { claude: "Claude Code", codex: "Codex", agy: "Gemini", acp: "ACP" };
export const providerLabel = (t: ThreadSummary): string => (t.model ? `${PROVIDER[t.provider]} · ${t.model}` : PROVIDER[t.provider]);

/** Header of a thread's view, in the main window and (compact) in a pop-out. */
export function ThreadTitle(props: { thread: ThreadSummary }) {
  return (
    <>
      <span class="title">{props.thread.title || "Untitled chat"}</span>
      <Pill tone={toneOf(props.thread).tone} text={toneOf(props.thread).label} />
    </>
  );
}

/** The transcript region and the composer of one view. `viewKey` is a thread id or NEW_THREAD. */
export function ThreadBody(props: { viewKey: string; thread: ThreadSummary | undefined }) {
  return (
    <div class="thread">
      <div class="thread-scroll" tabindex="0" role="log" aria-label="Conversation" data-atrium-part="transcript">
        <div class="thread-empty">
          <Show when={props.thread} fallback={<><b>Home</b><span>Ask about anything on this computer. Home reads freely and asks before it changes something.</span></>}>
            <span>The conversation shows here.</span>
          </Show>
        </div>
      </div>
      <div class="thread-composer">
        <Composer viewKey={props.viewKey} id="composer-primary" placeholder={props.thread ? "Reply…" : "Ask Home…"} />
      </div>
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
              <span class="chip">{providerLabel(t())}</span>
              <Show when={ownsView(t().thread_id)}>
                <IconButton label="Open in new window" onClick={() => void popOut(t().thread_id)}><ExternalLink class="i" aria-hidden="true" /></IconButton>
              </Show>
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
