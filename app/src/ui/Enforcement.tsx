// The enforcement badge and its "details" (PLAN §3.12, contract §3): the session's level, its
// coverage per tool class, the configuration it loads and why it is Partial. Coverage, policy
// and isolation are three separate sections, never merged into one "safety" score.

import { Dialog } from "@kobalte/core/dialog";
import { X } from "lucide-solid";
import { Show, createSignal, lazy } from "solid-js";
import type { EnforcementDescriptor, ThreadSummary } from "../daemon/types";
import { Dot, IconButton } from "./controls";

/** The details themselves load the first time they're opened, not at startup. */
const Body = lazy(() => import("./EnforcementDetails"));

const [shown, setShown] = createSignal<{ descriptor: EnforcementDescriptor; subject: string } | null>(null);

/** Opens the details of a descriptor; `subject` names whose it is (a chat, or an agent in Settings). */
export function openEnforcementDetails(descriptor: EnforcementDescriptor, subject: string): void {
  setShown({ descriptor, subject });
}

export const levelLabel = (d: EnforcementDescriptor): string => (d.level === "full" ? "Full enforcement" : "Partial enforcement");

/** In the thread header: the session's level, which opens the details. */
export function EnforcementBadge(props: { thread: ThreadSummary }) {
  return (
    <Show when={props.thread.enforcement}>
      {(e) => (
        <button
          type="button"
          class="chip enf"
          data-level={e().level}
          data-atrium-part="enforcement-badge"
          title="What Atrium can enforce for this session: details"
          onClick={() => openEnforcementDetails(e(), props.thread.title || "Untitled chat")}
        >
          <Dot tone={e().level === "full" ? "ready" : "needs-you"} />{levelLabel(e())}
        </button>
      )}
    </Show>
  );
}

/** A "Details" link next to a level (Context panel, Settings → Agents). */
export function DetailsLink(props: { descriptor: EnforcementDescriptor; subject: string }) {
  return <button type="button" class="link-btn" data-atrium-part="enforcement-details-link" onClick={() => openEnforcementDetails(props.descriptor, props.subject)}>Details</button>;
}

/** The details dialog, one per window. */
export function EnforcementHost() {
  return (
    <Dialog open={shown() !== null} onOpenChange={(open) => { if (!open) setShown(null); }}>
      <Dialog.Portal>
        <Dialog.Overlay class="overlay" />
        <div class="palette-pos">
          <Show when={shown()}>
            {(s) => (
              <Dialog.Content class="palette sheet wide" data-atrium-part="enforcement-details">
                <header class="sheet-head">
                  <Dialog.Title class="sheet-title">{levelLabel(s().descriptor)} · {s().subject}</Dialog.Title>
                  <IconButton small label="Close" onClick={() => setShown(null)}><X class="i sm" aria-hidden="true" /></IconButton>
                </header>
                <Body d={s().descriptor} close={() => setShown(null)} />
              </Dialog.Content>
            )}
          </Show>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
