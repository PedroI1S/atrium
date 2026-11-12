// The composer: a rounded field whose text is the view's draft. The draft lives in Rust
// (windows.ts → shell.rs), so it survives a move to another window, a forced close and a crash.
// The chat view (ThreadCard) passes `onSend`, `onStop` and its chips.

import { ArrowUp, Square } from "lucide-solid";
import { type JSX, Show, createEffect } from "solid-js";
import { canEdit, patchView, viewOf } from "../windows";

export function Composer(props: { viewKey: string; placeholder: string; onSend?: (text: string) => void; onStop?: () => void; running?: boolean; busy?: boolean; chips?: JSX.Element; id?: string }) {
  const text = () => viewOf(props.viewKey).draft;
  let area!: HTMLTextAreaElement;
  const grow = () => {
    area.style.height = "auto";
    // The cap is the `--composer-max-height` token, applied by CSS (max-height).
    area.style.height = `${area.scrollHeight}px`;
  };
  // A draft that arrives from elsewhere (another thread selected, a view returned) resizes the field.
  createEffect(() => { text(); queueMicrotask(grow); });
  const send = () => {
    const value = text().trim();
    if (!value || !props.onSend || props.busy || !canEdit(props.viewKey)) return;
    props.onSend(value);
  };
  return (
    <div class="composer" data-atrium-part="composer">
      <textarea
        ref={area}
        id={props.id}
        class="composer-input"
        rows={1}
        placeholder={props.placeholder}
        aria-label="Message"
        value={text()}
        readOnly={!canEdit(props.viewKey)}
        onInput={(e) => { void patchView(props.viewKey, { draft: e.currentTarget.value }); grow(); }}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
            e.preventDefault();
            send();
          }
        }}
      />
      <div class="cm-row">
        {props.chips}
        {/* While a turn runs, a message waits for it (the session queues it); Stop ends the turn. */}
        <Show when={props.running && props.onStop}>
          <button type="button" class="send stop" aria-label="Stop the turn" title="Stop the turn" onClick={() => props.onStop?.()}>
            <Square class="i sm" aria-hidden="true" />
          </button>
        </Show>
        <Show when={!props.running || text().trim()}>
          <button type="button" class="send" aria-label="Send" title={props.running ? "Send (Enter): it waits for the running turn" : "Send (Enter)"} disabled={!props.onSend || props.busy || !text().trim()} onClick={send}>
            <ArrowUp class="i" aria-hidden="true" />
          </button>
        </Show>
      </div>
    </div>
  );
}
