/* @refresh reload */
import { render } from "solid-js/web";
import { App } from "./App";
import "./theme.css";
import "./app.css";
import { runMicro, runScenario } from "./bench";
import { startupInfo } from "./platform";

async function boot() {
  const info = await startupInfo();
  render(() => <App info={info} />, document.getElementById("root")!);
  if (info.bench) {
    const sc = JSON.parse(info.bench);
    void (sc.mode === "micro" ? runMicro() : runScenario(sc, info));
  }
}

void boot();
