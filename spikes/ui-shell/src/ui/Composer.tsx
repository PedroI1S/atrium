import { ArrowUp, ChevronDown, Paperclip, ShieldCheck } from "lucide-solid";
import { model, patchView } from "../windows";
import { markFirstInput } from "../bench";

export function Composer(props: { onSend: (text: string) => void; id?: string }) {
  const text = () => model().views.chat.draft;
  const setText = (draft: string) => void patchView("chat", { draft });
  let area!: HTMLTextAreaElement;
  const grow = () => {
    area.style.height = "auto";
    // The cap is the `--composer-max-height` token, applied by CSS (max-height).
    area.style.height = `${area.scrollHeight}px`;
  };
  const send = () => {
    const v = text().trim();
    if (!v) return;
    props.onSend(v);
    setText("");
    area.value = "";
    grow();
  };
  return (
    <div class="composer" data-atrium-part="composer">
      <textarea
        ref={area}
        id={props.id}
        class="composer-input"
        rows={1}
        placeholder="Ask, or type / for commands"
        aria-label="Message"
        value={text()}
        onInput={(e) => {
          markFirstInput("input");
          setText(e.currentTarget.value);
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
      <div class="composer-bar">
        <button class="icon-btn" aria-label="Attach file" title="Attach"><Paperclip class="icon-md" /></button>
        <button class="chip" title="Model (dummy)">claude-sonnet-5.5 <ChevronDown class="icon-xs" /></button>
        <button class="chip" title="Approval mode (dummy)"><ShieldCheck class="icon-xs" /> Ask first</button>
        <span class="side-grow" />
        <button class="send-btn" aria-label="Send" title="Send (Enter)" disabled={!text().trim()} onClick={send}>
          <ArrowUp class="icon-md" />
        </button>
      </div>
    </div>
  );
}
