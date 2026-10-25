// The hidden <textarea> that gives the OS input method a text target, and turns what it does
// into CDP input. Kept separate from the component so it can be tested with a fake DOM.
//
// Rules, each of which matters for a real IME:
// * While composing, the textarea's own value is the input method's marked text. It must not be
//   touched (clearing it on every `input` event, as a first version did, wipes the candidate
//   text under the user's fingers). It is cleared only when the composition ends.
// * Candidate updates go out as `compose`; the final text as one `commit`; a composition that
//   ends with no text (Escape, focus loss) is sent as an empty `compose`, which cancels it.
// * Key events that belong to a composition are never forwarded as keys.

import type { KeyLike } from "./input";

export interface SinkTarget {
  /** Returns true when the key was forwarded (and the default action should be prevented). */
  key(type: "keyDown" | "keyUp", e: KeyLike): boolean;
  ime(kind: "compose" | "commit", text: string): void;
}

export function bindImeSink(el: HTMLTextAreaElement, target: SinkTarget): () => void {
  let composing = false;
  const clear = () => {
    el.value = "";
  };

  const on = <K extends string>(type: K, fn: (e: Event & Record<string, unknown>) => void) => {
    el.addEventListener(type, fn as EventListener);
    return () => el.removeEventListener(type, fn as EventListener);
  };

  const offs = [
    on("keydown", (e) => {
      if (target.key("keyDown", e as unknown as KeyLike)) e.preventDefault();
    }),
    on("keyup", (e) => {
      if (target.key("keyUp", e as unknown as KeyLike)) e.preventDefault();
    }),
    on("compositionstart", () => {
      composing = true;
    }),
    on("compositionupdate", (e) => {
      composing = true;
      target.ime("compose", String(e.data ?? ""));
    }),
    on("compositionend", (e) => {
      composing = false;
      const text = String(e.data ?? "");
      if (text === "") target.ime("compose", ""); // cancelled: drop the marked text on the page
      else target.ime("commit", text);
      clear();
    }),
    on("input", (e) => {
      if (composing || e.isComposing === true) return; // marked text: leave it to the input method
      // Text that did not come from a handled key or a composition (emoji picker, dictation).
      if (typeof e.data === "string" && e.data !== "") target.ime("commit", e.data);
      clear();
    }),
    on("blur", () => {
      if (!composing) return;
      composing = false;
      target.ime("compose", ""); // focus left mid-composition: cancel it on the page too
      clear();
    }),
  ];
  return () => offs.forEach((off) => off());
}
