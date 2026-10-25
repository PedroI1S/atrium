import Resizable from "corvu/resizable";
import { Match, Show, Switch } from "solid-js";
import { activeTab, animateLayout, innerSizes, outerSizes, setInnerSizes, setOuterSizes } from "../state";
import { ChatPane } from "./ChatPane";
import { FilePreview } from "./FilePreview";
import { HomePane } from "./HomePane";
import { RightPanel } from "./RightPanel";
import { Sidebar } from "./Sidebar";

function Center() {
  return (
    <Show when={activeTab()} keyed>
      {(tab) => (
        <Switch>
          <Match when={tab.kind === "home"}>
            <HomePane />
          </Match>
          <Match when={true}>
            <Resizable class="rz" orientation="horizontal" sizes={innerSizes()} onSizesChange={setInnerSizes} data-atrium-part="split-panes">
              <Resizable.Panel class="rz-panel" minSize={0.28}>
                <ChatPane threadId={tab.threads[0]!} primary />
              </Resizable.Panel>
              <Resizable.Handle class="handle" aria-label="Resize split" />
              <Resizable.Panel class="rz-panel" minSize={0.2} collapsible collapsedSize={0}>
                {tab.kind === "split" ? <ChatPane threadId={tab.threads[1]!} /> : <FilePreview file={tab.file ?? "session.rs"} />}
              </Resizable.Panel>
            </Resizable>
          </Match>
        </Switch>
      )}
    </Show>
  );
}

export function Layout() {
  return (
    <main class="workspace" classList={{ animating: animateLayout() }}>
      <Resizable class="rz" orientation="horizontal" sizes={outerSizes()} onSizesChange={setOuterSizes}>
        <Resizable.Panel class="rz-panel rz-side" minSize={0.12} maxSize={0.32} collapsible collapsedSize={0}>
          <Sidebar />
        </Resizable.Panel>
        <Resizable.Handle class="handle" aria-label="Resize sidebar" />
        <Resizable.Panel class="rz-panel" minSize={0.26}>
          <Center />
        </Resizable.Panel>
        <Resizable.Handle class="handle" aria-label="Resize right panel" />
        <Resizable.Panel class="rz-panel rz-right" minSize={0.18} maxSize={0.8} collapsible collapsedSize={0}>
          <RightPanel />
        </Resizable.Panel>
      </Resizable>
    </main>
  );
}
