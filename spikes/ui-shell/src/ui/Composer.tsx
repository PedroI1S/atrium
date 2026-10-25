import { ArrowUp, ChevronDown, Paperclip, ShieldCheck } from "lucide-solid";
import { createSignal } from "solid-js";
import { markFirstInput } from "../bench";

export function Composer(props: { onSend: (text: string) => void; id?: string }) {
  const [text, setText] = createSignal("");
  let area!: HTMLTextAreaElement;
  const grow = () => {
    area.style.height = "auto";
    area.style.height = `${Math.min(area.scrollHeight, 200)}px`;
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
        <button class="icon-btn" aria-label="Attach file" title="Attach"><Paperclip size={14} /></button>
        <button class="chip" title="Model (dummy)">claude-sonnet-5.5 <ChevronDown size={12} /></button>
        <button class="chip" title="Approval mode (dummy)"><ShieldCheck size={12} /> Ask first</button>
        <span class="side-grow" />
        <button class="send-btn" aria-label="Send" title="Send (Enter)" disabled={!text().trim()} onClick={send}>
          <ArrowUp size={15} />
        </button>
      </div>
    </div>
  );
}
