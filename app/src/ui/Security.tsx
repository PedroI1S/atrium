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

export interface AskStore {
  id: string;
  title: string;
  paths: string;
}

/**
 * Credential stores that always ask in Home (contract §9; CR-12, CR-17, D-082): never
 * auto-allowed, but not hard-denied — an approval or a remembered rule may allow them.
 */
export const ASK_STORES: readonly AskStore[] = [
  {
    id: "packages",
    title: "Package & registry credentials",
    paths: "~/.cargo/credentials.toml, ~/.cargo/credentials, ~/.pypirc, ~/.gem/credentials, ~/.yarnrc.yml",
  },
  {
    id: "cloud-infra",
    title: "Cloud, database & secret-manager configs",
    paths: "~/.aws/sso/cache/, ~/.azure/accessTokens.json, ~/.azure/msal_token_cache.*, ~/.terraform.d/credentials.tfrc.json, ~/.config/op/config, ~/.vault-token, ~/.pgpass",
  },
  {
    id: "git-env",
    title: "Git credentials & environment files",
    paths: "~/.config/git/credentials, ~/.env, and git show / git cat-file on committed credential files",
  },
  {
    id: "shell-history",
    title: "Shell history files",
    paths: "~/.bash_history, ~/.zsh_history, ~/.zhistory, ~/.sh_history, ~/.ksh_history, ~/.history, ~/.local/share/fish/fish_history, ~/.config/fish/fish_history, ~/.zsh_sessions/, ~/.bash_sessions/",
  },
];

function CredentialStoresThatAsk(): JSX.Element {
  return (
    <div class="s-sec" data-atrium-part="ask-stores-section">
      <div class="row">
        <div class="rl">
          <b>Credential stores that ask</b>
          <span>Home never reads or searches these without asking. Unlike Always blocked, you can approve once or add a rule.</span>
        </div>
      </div>
      <div class="s-list" data-atrium-part="ask-stores">
        <For each={ASK_STORES}>
          {(s) => (
            <div class="s-item" data-id={s.id}>
              <Dot tone="needs-you" />
              <div class="s-main">
                <b>{s.title}</b>
                <span>{s.paths}</span>
              </div>
            </div>
          )}
        </For>
      </div>
    </div>
  );
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
