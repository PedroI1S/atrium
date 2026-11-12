// The composer: a rounded field whose text is the view's draft. The draft lives in Rust
// (windows.ts → shell.rs), so it survives a move to another window, a forced close and a crash.
// Sending, the mode and agent chips arrive with the chat view (M1-09b), which passes `onSend`.

import { ArrowUp } from "lucide-solid";
import { type JSX, createEffect } from "solid-js";
import { canEdit, patchView, viewOf } from "../windows";

export function Composer(props: { viewKey: string; placeholder: string; onSend?: (text: string) => void; chips?: JSX.Element; id?: string }) {
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
    if (!value || !props.onSend || !canEdit(props.viewKey)) return;
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
        <button type="button" class="send" aria-label="Send" title={props.onSend ? "Send (Enter)" : "Sending arrives with the chat view"} disabled={!props.onSend || !text().trim()} onClick={send}>
          <ArrowUp class="i" aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}
