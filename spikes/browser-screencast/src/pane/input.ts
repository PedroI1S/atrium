// Maps DOM input events to the parameters of CDP `Input.dispatch*` (what the Rust side
// forwards unchanged). Pure functions so they can be unit tested without a browser.

export interface MouseParams {
  type: "mousePressed" | "mouseReleased" | "mouseMoved" | "mouseWheel";
  x: number;
  y: number;
  button?: "none" | "left" | "middle" | "right" | "back" | "forward";
  buttons?: number;
  clickCount?: number;
  modifiers?: number;
  deltaX?: number;
  deltaY?: number;
}

export interface KeyParams {
  type: "keyDown" | "keyUp" | "rawKeyDown" | "char";
  modifiers?: number;
  text?: string;
  unmodifiedText?: string;
  key?: string;
  code?: string;
  windowsVirtualKeyCode?: number;
  autoRepeat?: boolean;
  location?: number;
  commands?: string[];
}

export interface ModifierState {
  altKey: boolean;
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}

/** The size of the page viewport in CSS px, and where the canvas sits on screen. */
export interface PaneGeometry {
  left: number;
  top: number;
  width: number;
  height: number;
  /** Emulated viewport size (`Emulation.setDeviceMetricsOverride`) in CSS px. */
  viewWidth: number;
  viewHeight: number;
}

/** CDP modifier bit field: Alt = 1, Ctrl = 2, Meta/Command = 4, Shift = 8. */
export function modifiers(e: ModifierState): number {
  return (e.altKey ? 1 : 0) | (e.ctrlKey ? 2 : 0) | (e.metaKey ? 4 : 0) | (e.shiftKey ? 8 : 0);
}

const BUTTON_NAMES = ["left", "middle", "right", "back", "forward"] as const;

export function buttonName(domButton: number): NonNullable<MouseParams["button"]> {
  return BUTTON_NAMES[domButton] ?? "none";
}

/** Client coordinates -> page CSS px (the canvas is scaled to fill its box). */
export function toPagePoint(clientX: number, clientY: number, g: PaneGeometry): { x: number; y: number } {
  const sx = g.width > 0 ? g.viewWidth / g.width : 1;
  const sy = g.height > 0 ? g.viewHeight / g.height : 1;
  return { x: (clientX - g.left) * sx, y: (clientY - g.top) * sy };
}

export interface MouseLike extends ModifierState {
  clientX: number;
  clientY: number;
  button: number;
  buttons: number;
  detail: number;
}

export function mouseParams(type: "mousePressed" | "mouseReleased" | "mouseMoved", e: MouseLike, g: PaneGeometry): MouseParams {
  const { x, y } = toPagePoint(e.clientX, e.clientY, g);
  const p: MouseParams = { type, x, y, modifiers: modifiers(e), buttons: e.buttons };
  if (type === "mouseMoved") {
    // A move with a button held reports that button, otherwise "none".
    p.button = e.buttons & 1 ? "left" : e.buttons & 4 ? "middle" : e.buttons & 2 ? "right" : "none";
  } else {
    p.button = buttonName(e.button);
    p.clickCount = Math.max(1, e.detail);
  }
  return p;
}

export interface WheelLike extends ModifierState {
  clientX: number;
  clientY: number;
  deltaX: number;
  deltaY: number;
  /** 0 = pixels, 1 = lines, 2 = pages. */
  deltaMode: number;
}

const LINE_PX = 40;

export function wheelParams(e: WheelLike, g: PaneGeometry): MouseParams {
  const { x, y } = toPagePoint(e.clientX, e.clientY, g);
  const unit = e.deltaMode === 1 ? LINE_PX : e.deltaMode === 2 ? g.viewHeight : 1;
  return { type: "mouseWheel", x, y, modifiers: modifiers(e), deltaX: e.deltaX * unit, deltaY: e.deltaY * unit };
}

export interface KeyLike extends ModifierState {
  key: string;
  code: string;
  keyCode: number;
  location: number;
  repeat: boolean;
  isComposing: boolean;
}

/** macOS editing commands: Cmd+key is not delivered as text, the renderer wants a command. */
const MAC_COMMANDS: Record<string, string> = {
  a: "selectAll",
  c: "copy",
  x: "cut",
  v: "paste",
  z: "undo",
};

function editingCommands(e: KeyLike): string[] | undefined {
  if (!e.metaKey || e.ctrlKey || e.altKey) return undefined;
  const k = e.key.toLowerCase();
  if (k === "z" && e.shiftKey) return ["redo"];
  const c = MAC_COMMANDS[k];
  return c ? [c] : undefined;
}

/** `Enter` produces "\r", `Tab` "\t"; other named keys produce no text. */
function textOf(e: KeyLike): string | undefined {
  if (e.ctrlKey || e.metaKey) return undefined;
  if (e.key === "Enter") return "\r";
  if (e.key === "Tab") return "\t";
  return e.key.length === 1 ? e.key : undefined;
}

/**
 * Returns the CDP key event for a DOM keydown/keyup, or `null` for events that belong to an
 * IME composition (those go through `Input.imeSetComposition` / `insertText` instead).
 */
export function keyParams(type: "keyDown" | "keyUp", e: KeyLike): KeyParams | null {
  if (e.isComposing || e.keyCode === 229) return null;
  const text = type === "keyDown" ? textOf(e) : undefined;
  const p: KeyParams = {
    type: type === "keyDown" ? (text !== undefined ? "keyDown" : "rawKeyDown") : "keyUp",
    modifiers: modifiers(e),
    key: e.key,
    code: e.code,
    windowsVirtualKeyCode: e.keyCode,
    location: e.location,
  };
  if (text !== undefined) {
    p.text = text;
    p.unmodifiedText = text;
  }
  if (e.repeat) p.autoRepeat = true;
  if (type === "keyDown") {
    const commands = editingCommands(e);
    if (commands) p.commands = commands;
  }
  return p;
}
