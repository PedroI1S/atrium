// The body of the enforcement "details" (PLAN §3.12, contract §3), loaded on first open:
// coverage per tool class and the configuration the session loads, then Policy and Isolation
// as separate sections.

import { For, Show } from "solid-js";
import { providerName } from "../chat/actions";
import { coverageRows, levelLine, sourceRows, versionLine } from "../chat/enforcement";
import type { EnforcementDescriptor } from "../daemon/types";
import { setScreen, setSettingsSection } from "../state";
import { isPopout } from "../windows";
import { Dot } from "./controls";

export default function EnforcementDetails(props: { d: EnforcementDescriptor; close: () => void }) {
  const rows = () => coverageRows(props.d);
  const sources = () => sourceRows(props.d.config_sources);
  const toRules = () => {
    props.close();
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
