import { createSignal } from "solid-js";
import { typing } from "./typing";

export function Composer() {
  const [draft, setDraft] = createSignal("");
  const [sent, setSent] = createSignal(0);
  let area!: HTMLTextAreaElement;

  const onKeyDown = (e: KeyboardEvent) => {
    typing.keys++;
    typing.pending.push(e.timeStamp);
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (draft().trim() !== "") {
        setSent((n) => n + 1);
        area.value = "";
        setDraft("");
      }
    }
  };

  const onInput = (e: InputEvent) => {
    const handlerAt = performance.now();
    const origin = typing.pending.shift() ?? e.timeStamp;
    typing.inputs++;
    setDraft(area.value);
    typing.handler.push(handlerAt - origin);
    requestAnimationFrame(() => {
      typing.frame.push(performance.now() - origin);
      requestAnimationFrame(() => typing.paint.push(performance.now() - origin));
    });
  };

  return (
    <div class="composer">
      <textarea ref={area} placeholder="Type here while the transcript streams (Enter clears)" onKeyDown={onKeyDown} onInput={onInput} spellcheck={false} />
      <span class="count">
        {draft().length} ch · {sent()} sent
      </span>
    </div>
  );
}
