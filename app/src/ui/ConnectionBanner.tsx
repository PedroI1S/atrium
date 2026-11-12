// The connection to the daemon, when it isn't simply connected: a quiet line in the frame.
// An older daemon (contract §10) is replaced from here: once its turns finish, or now.

import { Show, createSignal } from "solid-js";
import { daemon, reconnect, replaceDaemon } from "../daemon/client";
import type { ConnectionState } from "../daemon/types";
import { showToast } from "../state";
import { isPopout } from "../windows";

/** A first attempt is not worth a banner: the daemon is usually up within this many tries. */
const QUIET_ATTEMPTS = 2;

type Incompatible = Extract<ConnectionState, { type: "incompatible" }>;

function Upgrade(props: { state: Incompatible }) {
  const [asked, setAsked] = createSignal<"idle" | "now" | null>(null);
  const turns = () => props.state.running_turns;
  const replace = (force: boolean) => {
    setAsked(force ? "now" : "idle");
    replaceDaemon(force).catch((e) => { setAsked(null); showToast(String(e)); });
  };
  return (
    <>
      <span class="cb-text">
        <b>The background service is from another version of Atrium</b>
        <span>{props.state.daemon_version ? `It is ${props.state.daemon_version}. ` : ""}{props.state.message}</span>
      </span>
      <Show when={!isPopout()}>
        <Show when={asked() === null} fallback={<span class="muted">{asked() === "now" ? "Restarting it…" : turns() > 0 ? "Waiting for its running turns to finish…" : "Restarting it…"}</span>}>
          <button type="button" class="btn sm pri" onClick={() => replace(false)}>{turns() > 0 ? `Restart when ${turns()} running ${turns() === 1 ? "turn finishes" : "turns finish"}` : "Restart it"}</button>
        </Show>
        <Show when={turns() > 0 && asked() !== "now"}>
          <button type="button" class="btn sm ghost" title="Running turns are interrupted. They can be resumed." onClick={() => replace(true)}>Interrupt and restart now</button>
        </Show>
      </Show>
    </>
  );
}

export function ConnectionBanner() {
  const state = () => daemon().connection;
  const shown = () => {
    const s = state();
    return s.type === "incompatible" || s.type === "failed" || (s.type === "connecting" && s.attempt > QUIET_ATTEMPTS);
  };
  return (
    <Show when={shown()}>
      <div class="connbar" role="status" data-kind={state().type} data-atrium-part="connection">
        <span class="dot" data-state={state().type === "connecting" ? "running" : "failed"} aria-hidden="true" />
        {(() => {
          const s = state();
          if (s.type === "incompatible") return <Upgrade state={s} />;
          if (s.type === "failed") {
            return (
              <>
                <span class="cb-text"><b>Atrium can't reach its background service</b><span>{s.message}</span></span>
                <button type="button" class="btn sm" onClick={reconnect}>Try again</button>
              </>
            );
          }
          return <span class="cb-text"><b>Connecting to the background service…</b><span>Your chats keep running there; they show again once it answers.</span></span>;
        })()}
      </div>
    </Show>
  );
}
