// What the composer and the thread header do: start a Home chat, send, stop, resume. Every one
// is a daemon command through `invoke` (contract §11); failures are shown, never retried.

import { createResource, createSignal } from "solid-js";
import { command, daemon } from "../daemon/client";
import { type AgentLaunch, type ProviderProbe, type ThreadSummary, isCommandFailure } from "../daemon/types";
import { showToast } from "../state";
import { NEW_THREAD } from "../window-model";
import { debugBuild, patchView, setMainThread } from "../windows";

export type AgentChoice = { id: string; label: string; launch: AgentLaunch; note?: string };

const PROVIDER_LABEL: Record<string, string> = { claude: "Claude Code", codex: "Codex", agy: "Gemini", acp: "ACP agent" };
export const providerName = (p: string) => PROVIDER_LABEL[p] ?? p;

/** The synthetic agent (D-059): offered in development builds while no real provider ships. */
export const SYNTHETIC: AgentChoice = {
  id: "synthetic",
  label: "Synthetic (test)",
  launch: { type: "fake", tokens_per_second: 200, chunk_chars: 16, duration_ms: 8_000, tool_call_every_ms: 3_000 },
  note: "Streams placeholder text; for testing Atrium itself.",
};

const connected = () => daemon().connection.type === "connected";

/** `ProbeProviders` for Home's preset: what is installed and what Atrium can enforce for it (§3). */
export const [probes, { refetch: refetchProbes }] = createResource<ProviderProbe[], boolean>(connected, async (isConnected) => {
  if (!isConnected) return [];
  try {
    return await command<ProviderProbe[]>({ type: "probe_providers", preset: "home" });
  } catch {
    return [];
  }
});

/** Agents a new chat can start with: installed providers, then the synthetic one in dev builds. */
export function agentChoices(): AgentChoice[] {
  const native: AgentChoice[] = (probes() ?? [])
    .filter((p) => p.enforcement !== null)
    .map((p) => ({ id: p.provider, label: providerName(p.provider), launch: { type: "native", provider: p.provider, model: null, effort: null, preset: "home" } }));
  return debugBuild() ? [...native, SYNTHETIC] : native;
}

const [chosen, setChosen] = createSignal<string | null>(null);
export const setAgent = setChosen;
export const agent = (): AgentChoice | undefined => {
  const all = agentChoices();
  return all.find((a) => a.id === chosen()) ?? all[0];
};

const [busy, setBusy] = createSignal<string | null>(null);
/** The view whose send is in flight (its composer waits). */
export const sending = busy;

function fail(e: unknown): void {
  showToast(isCommandFailure(e) ? e.message : String(e));
}

/** A new Home chat with this first message. Returns the new thread, or null. */
export async function startChat(text: string, choice = agent()): Promise<ThreadSummary | null> {
  if (!choice) {
    showToast("No agent is installed. See Settings → Agents.");
    return null;
  }
  setBusy(NEW_THREAD);
  try {
    const summary = await command<ThreadSummary>({ type: "start_thread", agent: choice.launch, message: text });
    await patchView(NEW_THREAD, { draft: "" });
    setMainThread(summary.thread_id);
    return summary;
  } catch (e) {
    fail(e);
    return null;
  } finally {
    setBusy(null);
  }
}

const LIVE = new Set(["starting", "running", "needs_approval", "needs_input", "idle", "paused"]);

/** Sends a message in a thread, resuming its session first if it has none (contract §6.2). */
export async function sendMessage(thread: ThreadSummary, text: string): Promise<boolean> {
  setBusy(thread.thread_id);
  try {
    if (!LIVE.has(thread.state.type)) await command({ type: "resume_thread", thread_id: thread.thread_id });
    await command({ type: "thread_command", thread_id: thread.thread_id, command: { type: "send_user_message", text } });
    await patchView(thread.thread_id, { draft: "" });
    return true;
  } catch (e) {
    fail(e);
    return false;
  } finally {
    setBusy(null);
  }
}

export async function stopTurn(threadId: string): Promise<void> {
  try {
    await command({ type: "thread_command", thread_id: threadId, command: { type: "interrupt" } });
  } catch (e) {
    fail(e);
  }
}

export async function resumeThread(threadId: string): Promise<void> {
  try {
    await command({ type: "resume_thread", thread_id: threadId });
  } catch (e) {
    fail(e);
  }
}

/** The thread's unread flag is cleared once someone looks at it (contract §8). */
export function markRead(threadId: string): void {
  void command({ type: "mark_read", thread_id: threadId }).catch(() => {});
}

let focused: string | null | undefined;
/** The thread this window shows while it has focus, for the daemon's notification suppression (§8). */
export function reportFocus(threadId: string | null): void {
  if (focused === threadId) return;
  focused = threadId;
  void command({ type: "set_focus", thread_id: threadId }).catch(() => { focused = undefined; });
}
