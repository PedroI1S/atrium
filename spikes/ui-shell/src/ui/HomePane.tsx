import { For } from "solid-js";
import { attentionThreads, openThread } from "../state";

export function HomePane() {
  return (
    <section class="pane home" aria-label="Home" data-atrium-part="home">
      <div class="home-inner">
        <h1 class="home-title">Good evening, Pedro</h1>
        <p class="home-sub">{attentionThreads().length} threads are waiting for you.</p>
        <div class="home-grid">
          <For each={attentionThreads()}>
            {(t) => (
              <button class="card home-card" onClick={() => openThread(t.id)}>
                <span class="state-dot" data-state={t.state} />
                <span class="card-title">{t.title}</span>
                <span class="muted">{t.agent} · {t.state.replace("-", " ")}</span>
              </button>
            )}
          </For>
        </div>
      </div>
    </section>
  );
}
