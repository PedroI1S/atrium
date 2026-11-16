// Settings → Security (contract §3, §4, §9; PLAN §3.12): what is always blocked for the calls
// Atrium sees, and the limits Atrium states plainly. Read-only. The hard-deny list comes from
// `HARD_DENY_DEFAULTS` through the app (`safety_info`), never a copy in the webview.

import { For, type JSX, Show, createResource, createSignal } from "solid-js";
import { safetyInfo } from "../platform";
import { Dot } from "./controls";

/** Short names for the hard-deny entries; the description from the protocol follows each. */
const BLOCKED_NAME: Record<string, string> = {
  "recursive-delete-outside-cwd": "Recursive deletes outside the chat's folder",
  "force-push-protected": "Force-pushes to shared branches",
  "credential-read": "Credential stores and keychains",
  "pipe-to-shell": "Downloaded code run straight from the network",
  "atrium-self": "Atrium's own data and files",
  "privilege-escalation": "Administrator privileges",
  "disk-wipe": "Formatting or overwriting disks",
};

/** Loaded once per window, the first time Security or the rules editor is shown (not at startup). */
const [wanted, setWanted] = createSignal(false);
const [safety] = createResource(wanted, () => safetyInfo());
export const loadSafety = (): void => { setWanted(true); };
/** The home folder, for `~/…` in the rules editor (null until known or in a browser). */
export const homeDir = (): string | null => safety()?.home ?? null;

function AlwaysBlocked() {
  return (
    <div class="s-list" data-atrium-part="always-blocked">
      <For each={safety()?.hardDeny ?? []} fallback={<p class="s-empty">{safety.loading ? "" : "The list isn't available here."}</p>}>
        {(d) => (
          <div class="s-item" data-id={d.id}>
            <Dot tone="failed" />
            <div class="s-main">
              <b>{BLOCKED_NAME[d.id] ?? d.id}</b>
              <span>{d.description}</span>
            </div>
          </div>
        )}
      </For>
      <p class="note">Atrium checks these on every call it sees, before it runs, whatever the agent's enforcement level. A call Atrium doesn't see (see an agent's details in Agents) isn't checked, and a program that was allowed to run isn't checked again.</p>
    </div>
  );
}

/*
 * CR-12 (rev 3.1, D-082), lands after M1-18 merges: the credential stores that always ask
 * (never auto-allowed in Home, but not blocked: an approval or a rule may allow them). The
 * Settings page "that lists what is protected names them" (contract §9). Its list mounts here,
 * between Always blocked and Known limits, read from the app like `hardDeny`.
 */
function CredentialStoresThatAsk(): JSX.Element {
  return null;
}

const LIMITS: { id: string; title: string; text: string }[] = [
  {
    id: "git-config",
    title: "Git reads trust the repositories on this computer",
    text: "Home runs some git commands without asking (git status, git log, git diff…). A repository's own git config can make git run programs during those reads (core.fsmonitor, textconv and diff drivers), so those reads trust the repositories already on disk.",
  },
  {
    id: "same-user",
    title: "Any program running as you can talk to Atrium's background service",
    text: "Atrium's service checks that commands come from the Atrium app, but it can't keep out a process running as your user: an agent you allowed to run, or anything it starts, can do what you can, including changing Atrium's own files. The check raises the bar; it isn't isolation.",
  },
  {
    id: "no-isolation",
    title: "Allowed code isn't contained",
    text: "Atrium checks the calls it sees before they run. What an allowed command then does (a build script started by an approved cargo test) runs with your access and isn't checked again. Only a sandbox limits that; Atrium's arrives in a later version.",
  },
];

function KnownLimits() {
  return (
    <div class="s-list" data-atrium-part="known-limits">
      <For each={LIMITS}>
        {(l) => (
          <div class="s-item" data-id={l.id}>
            <Dot tone="needs-you" />
            <div class="s-main"><b>{l.title}</b><span>{l.text}</span></div>
          </div>
        )}
      </For>
    </div>
  );
}

const BLOCKS: Record<string, () => JSX.Element> = {
  "always-blocked": () => <><AlwaysBlocked /><CredentialStoresThatAsk /></>,
  "known-limits": KnownLimits,
};

/** Each row (a search target) followed by what it heads. */
export default function SecurityPage<R extends { id: string }>(props: { rows: readonly R[]; row: (row: R) => JSX.Element }) {
  loadSafety();
  return (
    <For each={props.rows}>
      {(r) => (
        <>
          {props.row(r)}
          <Show when={BLOCKS[r.id]}>{(block) => block()()}</Show>
        </>
      )}
    </For>
  );
}
