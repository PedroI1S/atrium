import { describe, expect, it } from "vitest";
import { type KeyLike, type PaneGeometry, keyParams, modifiers, mouseParams, toPagePoint, wheelParams } from "./input";

const none = { altKey: false, ctrlKey: false, metaKey: false, shiftKey: false };
// The canvas is drawn at 800x500 on screen at (100, 50); the page viewport is 1000x625 CSS px.
const g: PaneGeometry = { left: 100, top: 50, width: 800, height: 500, viewWidth: 1000, viewHeight: 625 };

const key = (over: Partial<KeyLike>): KeyLike => ({
  key: "a",
  code: "KeyA",
  keyCode: 65,
  location: 0,
  repeat: false,
  isComposing: false,
  ...none,
  ...over,
});

describe("modifiers", () => {
  it("builds the CDP bit field", () => {
    expect(modifiers(none)).toBe(0);
    expect(modifiers({ ...none, altKey: true })).toBe(1);
    expect(modifiers({ ...none, ctrlKey: true })).toBe(2);
    expect(modifiers({ ...none, metaKey: true })).toBe(4);
    expect(modifiers({ ...none, shiftKey: true })).toBe(8);
    expect(modifiers({ altKey: true, ctrlKey: true, metaKey: true, shiftKey: true })).toBe(15);
  });
});

describe("pointer mapping", () => {
  it("maps client coordinates to page CSS px through the canvas scale", () => {
    expect(toPagePoint(100, 50, g)).toEqual({ x: 0, y: 0 });
    expect(toPagePoint(500, 300, g)).toEqual({ x: 500, y: 312.5 });
    expect(toPagePoint(900, 550, g)).toEqual({ x: 1000, y: 625 });
  });

  it("sends the pressed button and click count; reports the held button on moves", () => {
    const down = mouseParams("mousePressed", { clientX: 300, clientY: 250, button: 2, buttons: 2, detail: 2, ...none }, g);
    expect(down).toMatchObject({ type: "mousePressed", x: 250, y: 250, button: "right", buttons: 2, clickCount: 2 });
    const up = mouseParams("mouseReleased", { clientX: 300, clientY: 250, button: 0, buttons: 0, detail: 0, ...none }, g);
    expect(up).toMatchObject({ button: "left", clickCount: 1, buttons: 0 });
    const hover = mouseParams("mouseMoved", { clientX: 300, clientY: 250, button: 0, buttons: 0, detail: 0, ...none }, g);
    expect(hover.button).toBe("none");
    expect(hover.clickCount).toBeUndefined();
    const drag = mouseParams("mouseMoved", { clientX: 300, clientY: 250, button: 0, buttons: 1, detail: 0, ...none }, g);
    expect(drag.button).toBe("left");
  });

  it("scales wheel deltas by delta mode", () => {
    const base = { clientX: 300, clientY: 250, deltaX: 0, deltaY: 3, ...none };
    expect(wheelParams({ ...base, deltaMode: 0 }, g)).toMatchObject({ type: "mouseWheel", deltaY: 3 });
    expect(wheelParams({ ...base, deltaMode: 1 }, g).deltaY).toBe(120);
    expect(wheelParams({ ...base, deltaMode: 2 }, g).deltaY).toBe(1875);
  });
});

describe("keyboard mapping", () => {
  it("types printable characters as keyDown with text", () => {
    expect(keyParams("keyDown", key({}))).toEqual({
      type: "keyDown",
      modifiers: 0,
      key: "a",
      code: "KeyA",
      windowsVirtualKeyCode: 65,
      location: 0,
      text: "a",
      unmodifiedText: "a",
    });
    expect(keyParams("keyDown", key({ key: "A", shiftKey: true }))?.text).toBe("A");
  });

  it("uses rawKeyDown for non-text keys and text for Enter and Tab", () => {
    expect(keyParams("keyDown", key({ key: "ArrowLeft", code: "ArrowLeft", keyCode: 37 }))).toMatchObject({
      type: "rawKeyDown",
      windowsVirtualKeyCode: 37,
    });
    expect(keyParams("keyDown", key({ key: "Enter", code: "Enter", keyCode: 13 }))).toMatchObject({ type: "keyDown", text: "\r" });
    expect(keyParams("keyDown", key({ key: "Tab", code: "Tab", keyCode: 9 }))?.text).toBe("\t");
    expect(keyParams("keyDown", key({ key: "Shift", code: "ShiftLeft", keyCode: 16, shiftKey: true }))?.type).toBe("rawKeyDown");
  });

  it("keyUp carries no text; repeats are flagged", () => {
    const up = keyParams("keyUp", key({}))!;
    expect(up.type).toBe("keyUp");
    expect(up.text).toBeUndefined();
    expect(keyParams("keyDown", key({ repeat: true }))?.autoRepeat).toBe(true);
  });

  it("maps Cmd shortcuts to macOS editing commands and sends no text", () => {
    const sel = keyParams("keyDown", key({ metaKey: true }))!;
    expect(sel.commands).toEqual(["selectAll"]);
    expect(sel.text).toBeUndefined();
    expect(keyParams("keyDown", key({ key: "z", metaKey: true, shiftKey: true }))?.commands).toEqual(["redo"]);
    expect(keyParams("keyDown", key({ key: "v", metaKey: true }))?.commands).toEqual(["paste"]);
    expect(keyParams("keyDown", key({ key: "a", ctrlKey: true }))?.commands).toBeUndefined();
  });

  it("leaves IME composition keys to the composition path", () => {
    expect(keyParams("keyDown", key({ isComposing: true }))).toBeNull();
    expect(keyParams("keyDown", key({ keyCode: 229 }))).toBeNull();
  });
});
