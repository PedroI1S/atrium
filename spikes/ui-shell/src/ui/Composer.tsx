import { ArrowUp, Shield } from "lucide-solid";
import { Show } from "solid-js";
import { markFirstInput } from "../bench";
import { canEdit, model, patchView } from "../windows";

/** Rounded composer: the field, then chips for approval mode and agent, and a round send button. */
export function Composer(props: { onSend: (text: string) => void; id?: string; agent: string; compact?: boolean; placeholder?: string; scope?: string }) {
  const text = () => model().views.chat.draft;
  let area!: HTMLTextAreaElement;
  const grow = () => {
    area.style.height = "auto";
    // The cap is the `--composer-max-height` token, applied by CSS (max-height).
    area.style.height = `${area.scrollHeight}px`;
  };
  const send = () => {
    const v = text().trim();
    if (!v || !canEdit("chat")) return;
    props.onSend(v);
    area.value = "";
    grow();
  };
  return (
    <div class="composer" classList={{ compact: props.compact }} data-atrium-part="composer">
      <textarea
        ref={area}
        id={props.id}
        class="composer-input"
        rows={1}
        placeholder={props.placeholder ?? "Reply…"}
        aria-label="Message"
        value={text()}
        readOnly={!canEdit("chat")}
        onInput={(e) => {
          markFirstInput("input");
          void patchView("chat", { draft: e.currentTarget.value });
          grow();
        }}
        onKeyDown={(e) => {
          markFirstInput("keydown");
          if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
            e.preventDefault();
            send();
          }
        }}
      />
      <div class="cm-row">
        <Show when={props.scope} fallback={<span class="chip"><Shield class="i sm" aria-hidden="true" />Ask first</span>}>
          <span class="chip">{props.scope}</span>
        </Show>
        <Show when={!props.scope}><span class="chip">{props.agent}</span></Show>
        <button type="button" class="send" aria-label="Send" title="Send (Enter)" disabled={!text().trim()} onClick={send}>
          <ArrowUp class="i" aria-hidden="true" />
        </button>
      </div>
    </div>
  );
}
