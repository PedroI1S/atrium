// The enforcement badge and its "details" (PLAN §3.12, contract §3): the session's level, its
// coverage per tool class, the configuration it loads and why it is Partial. Coverage, policy
// and isolation are three separate sections, never merged into one "safety" score.

import { Dialog } from "@kobalte/core/dialog";
import { X } from "lucide-solid";
import { For, Show, createSignal } from "solid-js";
import { providerName } from "../chat/actions";
import { coverageRows, levelLine, sourceRows, versionLine } from "../chat/enforcement";
import type { EnforcementDescriptor, ThreadSummary } from "../daemon/types";
import { setScreen, setSettingsSection } from "../state";
import { isPopout } from "../windows";
import { Dot, IconButton } from "./controls";

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

function Body(props: { d: EnforcementDescriptor; subject: string }) {
  const rows = () => coverageRows(props.d);
  const sources = () => sourceRows(props.d.config_sources);
  const toRules = () => {
    setShown(null);
    setScreen("settings");
    setSettingsSection("security");
  };
  return (
    <div class="sheet-scroll">
      <p class="sheet-body">{levelLine(props.d)}</p>
      <p class="sheet-meta">{providerName(props.d.provider)} · {versionLine(props.d)}</p>
      <Show when={props.d.reasons.length}>
        <ul class="enf-reasons" aria-label="Why it is partial">
          <For each={props.d.reasons}>{(r) => <li>{r.message}</li>}</For>
        </ul>
      </Show>

      <section class="enf-sec" aria-labelledby="enf-coverage" data-atrium-part="enforcement-coverage">
        <h3 id="enf-coverage">Coverage</h3>
        <p class="enf-what">Which calls Atrium sees before they run, per kind of tool.</p>
        <div class="enf-grid" role="table" aria-label="Coverage per kind of tool">
          <For each={rows()}>
            {(r) => (
              <div class="enf-row" role="row" data-class={r.cls} data-coverage={r.coverage}>
                <span role="rowheader">{r.label}</span>
                <span role="cell" class="enf-cov"><Dot tone={r.tone} />{r.text}</span>
                <Show when={r.evidence}><span role="cell" class="enf-ev code">{r.evidence}</span></Show>
              </div>
            )}
          </For>
        </div>
        <Show when={sources().length} fallback={<p class="enf-what">The session loads no configuration Atrium found.</p>}>
          <h4>Configuration this session loads</h4>
          <For each={sources()}>
            {(s) => (
              <div class="enf-src">
                <span><b>{s.kind}</b> · {s.inspected}</span>
                <span class="code">{s.path}{s.hash ? ` · sha256 ${s.hash}…` : ""}</span>
              </div>
            )}
          </For>
        </Show>
      </section>

      <section class="enf-sec" aria-labelledby="enf-policy" data-atrium-part="enforcement-policy">
        <h3 id="enf-policy">Policy</h3>
        <p class="enf-what">What Atrium does with the calls it sees.</p>
        <p>Home reads and searches freely, and asks before a change or any other command. Your remembered rules apply, and some actions are always blocked. This holds for every call Atrium sees, whatever the level.</p>
        <Show when={!isPopout()}>
          <p><button type="button" class="link-btn" onClick={toRules}>Rules and what is always blocked</button></p>
        </Show>
      </section>

      <section class="enf-sec" aria-labelledby="enf-isolation" data-atrium-part="enforcement-isolation">
        <h3 id="enf-isolation">Isolation</h3>
        <p class="enf-what">What limits code once it was allowed to run.</p>
        <p>None in this version. A command you allow runs as you, with your access, and what it starts isn't checked again. Only a sandbox limits that; Atrium's arrives in a later version.</p>
      </section>
    </div>
  );
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
                <Body d={s().descriptor} subject={s().subject} />
              </Dialog.Content>
            )}
          </Show>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
