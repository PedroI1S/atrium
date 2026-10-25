import { ChevronRight, GitBranch, House, Inbox } from "lucide-solid";
import { For, createSignal } from "solid-js";
import { WORKSPACES, type Thread } from "../data/dummy";
import { activeTab, activeTabId, inboxCount, openHome, openThread, showToast } from "../state";

const stateLabel: Record<Thread["state"], string> = {
  running: "Running",
  "needs-you": "Needs you",
  failed: "Failed",
  ready: "Ready for review",
  idle: "Idle",
};

function StateDot(props: { state: Thread["state"] }) {
  return <span class="state-dot" data-state={props.state} role="img" aria-label={stateLabel[props.state]} />;
}

export function Sidebar() {
  const [open, setOpen] = createSignal<Record<string, boolean>>({ atrium: true, harness: true, sandbox: false });
  return (
    <nav class="sidebar" aria-label="Workspaces" data-atrium-part="sidebar">
      <div class="side-nav">
        <button class="side-row" onClick={() => showToast("Inbox: opens the attention queue (dummy)")}>
          <Inbox class="icon-md" />
          <span class="side-grow">Inbox</span>
          <span class="badge" aria-label={`${inboxCount()} items need attention`}>{inboxCount()}</span>
        </button>
        <button class="side-row" classList={{ "is-active": activeTab().kind === "home" }} onClick={openHome}>
          <House class="icon-md" />
          <span class="side-grow">Home</span>
        </button>
      </div>
      <div class="side-label">Workspaces</div>
      <div class="side-scroll">
        <For each={WORKSPACES}>
          {(w) => (
            <div class="ws">
              <button class="side-row ws-head" aria-expanded={open()[w.id]} onClick={() => setOpen({ ...open(), [w.id]: !open()[w.id] })}>
                <ChevronRight class="icon-sm chev" classList={{ "is-open": open()[w.id] }} />
                <span class="side-grow">{w.name}</span>
                <span class="ws-branch">
                  <GitBranch class="icon-xs" />
                  {w.branch}
                </span>
              </button>
              {open()[w.id] && (
                <For each={w.threads}>
                  {(t) => (
                    <button
                      class="side-row thread-row"
                      classList={{ "is-active": activeTab().threads.includes(t.id) && activeTabId() === activeTab().id && activeTab().kind !== "home" }}
                      onClick={() => openThread(t.id)}
                    >
                      <StateDot state={t.state} />
                      <span class="side-grow thread-title">{t.title}</span>
                      <span class="thread-meta">{t.meta}</span>
                    </button>
                  )}
                </For>
              )}
            </div>
          )}
        </For>
      </div>
    </nav>
  );
}
