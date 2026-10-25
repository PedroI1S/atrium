// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { bindImeSink } from "./imeSink";
import { type KeyLike, keyParams } from "./input";

type Call = ["compose" | "commit", string];

function setup() {
  const el = document.createElement("textarea");
  document.body.appendChild(el);
  const ime: Call[] = [];
  const keys: string[] = [];
  bindImeSink(el, {
    key: (type, e: KeyLike) => {
      const p = keyParams(type, e);
      if (p) keys.push(`${p.type}:${p.key}`);
      return p !== null;
    },
    ime: (kind, text) => ime.push([kind, text]),
  });
  const fire = (type: string, props: Record<string, unknown> = {}) => {
    const ev = new Event(type, { bubbles: true, cancelable: true });
    Object.assign(ev, props);
    el.dispatchEvent(ev);
    return ev;
  };
  return { el, ime, keys, fire };
}

describe("IME sink", () => {
  it("keeps the marked text in the textarea for the whole composition, then commits once", () => {
    const { el, ime, fire } = setup();
    fire("compositionstart", { data: "" });
    // The input method writes its marked text into the textarea and reports input events.
    el.value = "に";
    fire("compositionupdate", { data: "に" });
    fire("input", { data: "に", isComposing: true });
    expect(el.value).toBe("に"); // wiped by the old `input` handler
    el.value = "日本";
    fire("compositionupdate", { data: "日本" });
    fire("input", { data: "日本", isComposing: true });
    expect(el.value).toBe("日本");
    fire("compositionend", { data: "日本語" });
    expect(ime).toEqual([["compose", "に"], ["compose", "日本"], ["commit", "日本語"]]);
    expect(el.value).toBe(""); // cleared only once the composition is over
  });

  it("cancelling a composition (Escape) cancels it on the page and commits nothing", () => {
    const { el, ime, fire } = setup();
    fire("compositionstart", { data: "" });
    el.value = "に";
    fire("compositionupdate", { data: "に" });
    fire("compositionend", { data: "" });
    expect(ime).toEqual([["compose", "に"], ["compose", ""]]);
    expect(el.value).toBe("");
  });

  it("losing focus mid-composition cancels it", () => {
    const { ime, fire } = setup();
    fire("compositionstart", { data: "" });
    fire("compositionupdate", { data: "か" });
    fire("blur");
    expect(ime).toEqual([["compose", "か"], ["compose", ""]]);
    fire("blur"); // nothing left to cancel
    expect(ime).toHaveLength(2);
  });

  it("does not forward keys that belong to a composition, but forwards normal ones", () => {
    const { keys, fire } = setup();
    const base = { code: "KeyA", location: 0, repeat: false, altKey: false, ctrlKey: false, metaKey: false, shiftKey: false };
    const down = fire("keydown", { ...base, key: "a", keyCode: 65, isComposing: false });
    expect(down.defaultPrevented).toBe(true);
    const composing = fire("keydown", { ...base, key: "Process", keyCode: 229, isComposing: true });
    expect(composing.defaultPrevented).toBe(false);
    fire("keyup", { ...base, key: "a", keyCode: 65, isComposing: false });
    expect(keys).toEqual(["keyDown:a", "keyUp:a"]);
  });

  it("forwards text that arrives outside a composition (emoji picker, dictation) and clears the sink", () => {
    const { el, ime, fire } = setup();
    el.value = "😀";
    fire("input", { data: "😀", isComposing: false });
    expect(ime).toEqual([["commit", "😀"]]);
    expect(el.value).toBe("");
  });
});
