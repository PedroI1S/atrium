// The browser pane without any UI framework: owns the <canvas>, the screencast channel,
// frame decoding and drawing, and forwards input to the backend. `Pane.tsx` only renders the
// markup around it, and `bench.ts` drives it directly.

import { Channel, invoke } from "@tauri-apps/api/core";
import { type FrameHeader, parseFrame } from "./frame";
import {
  type KeyLike,
  type MouseLike,
  type PaneGeometry,
  type WheelLike,
  keyParams,
  mouseParams,
  wheelParams,
} from "./input";

export interface PaneOptions {
  quality: number;
  format: "jpeg" | "png";
  /** Device scale factor of the emulated page (1 = frames are CSS-px sized). */
  deviceScaleFactor: number;
}

export interface DrawnFrame {
  header: FrameHeader;
  bytes: number;
  /** `performance.now()` right after `drawImage`. */
  drawnAt: number;
  /** `Date.now()` right after `drawImage`, comparable to the backend's epoch stamps. */
  drawnEpochMs: number;
  /** `Date.now()` when the channel message reached JS. */
  receivedEpochMs: number;
  bitmap: ImageBitmap;
}

export interface PaneStats {
  framesDrawn: number;
  framesDropped: number;
  bytes: number;
  lastFrameBytes: number;
  lastDecodeMs: number;
}

export class PaneController {
  readonly canvas: HTMLCanvasElement;
  private readonly g2d: CanvasRenderingContext2D;
  private channel: Channel<ArrayBuffer> | null = null;
  private lastDrawnSeq = 0;
  private lastBitmap: ImageBitmap | null = null;
  private view = { width: 0, height: 0 };
  private pendingMove: MouseLike | null = null;
  private moveRaf = 0;
  /** Which browser context is shown (0 = A, 1 = B). */
  context = 0;
  /** Mirrors the backend flag; input is only forwarded while the user has control. */
  userControl = false;
  options: PaneOptions;
  readonly stats: PaneStats = { framesDrawn: 0, framesDropped: 0, bytes: 0, lastFrameBytes: 0, lastDecodeMs: 0 };
  /** The frontend acks after drawing (end-to-end back-pressure). Off when the backend acks eagerly. */
  ackAfterDraw = true;
  onDrawn?: (f: DrawnFrame) => void;
  onError?: (message: string) => void;

  constructor(canvas: HTMLCanvasElement, options: PaneOptions) {
    this.canvas = canvas;
    this.options = options;
    const ctx = canvas.getContext("2d", { alpha: false });
    if (!ctx) throw new Error("2d canvas unavailable");
    this.g2d = ctx;
  }

  // ---- screencast ----------------------------------------------------------------------

  async attach(context: number): Promise<void> {
    this.context = context;
    this.lastDrawnSeq = 0;
    const channel = new Channel<ArrayBuffer>();
    channel.onmessage = (buf) => void this.onPacket(buf);
    this.channel = channel;
    await invoke("pane_attach", {
      context,
      quality: this.options.quality,
      format: this.options.format,
      onFrame: channel,
    });
  }

  async detach(): Promise<void> {
    this.channel = null;
    await invoke("pane_detach");
  }

  private async onPacket(raw: ArrayBuffer | number[]): Promise<void> {
    const receivedEpochMs = Date.now();
    const buf = raw instanceof ArrayBuffer ? raw : new Uint8Array(raw).buffer;
    const frame = parseFrame(buf);
    if (!frame) return;
    const t0 = performance.now();
    let bitmap: ImageBitmap;
    try {
      bitmap = await createImageBitmap(new Blob([frame.image], { type: frame.mime }));
    } catch (e) {
      this.onError?.(`decode failed: ${e}`);
      this.ack(frame.header.ackId); // never stall the stream on one bad frame
      return;
    }
    this.stats.lastDecodeMs = performance.now() - t0;
    if (frame.header.seq < this.lastDrawnSeq) {
      // A newer frame already won the race through the decoder; drawing this would go back in time.
      this.stats.framesDropped++;
      bitmap.close();
      this.ack(frame.header.ackId);
      return;
    }
    this.lastDrawnSeq = frame.header.seq;
    this.g2d.drawImage(bitmap, 0, 0, this.canvas.width, this.canvas.height);
    const drawnAt = performance.now();
    const drawnEpochMs = Date.now();
    this.stats.framesDrawn++;
    this.stats.bytes += frame.image.byteLength;
    this.stats.lastFrameBytes = frame.image.byteLength;
    this.lastBitmap?.close();
    this.lastBitmap = bitmap;
    this.onDrawn?.({
      header: frame.header,
      bytes: frame.image.byteLength,
      drawnAt,
      drawnEpochMs,
      receivedEpochMs,
      bitmap,
    });
    this.ack(frame.header.ackId);
  }

  private ack(ackId: number): void {
    if (this.ackAfterDraw) void invoke("pane_ack", { ackId });
  }

  // ---- size ----------------------------------------------------------------------------

  /** Sizes the canvas backing store and the emulated viewport to a CSS-px box. */
  async resize(cssWidth: number, cssHeight: number): Promise<void> {
    const w = Math.max(1, Math.round(cssWidth));
    const h = Math.max(1, Math.round(cssHeight));
    const dsf = this.options.deviceScaleFactor;
    this.view = { width: w, height: h };
    this.canvas.width = Math.round(w * dsf);
    this.canvas.height = Math.round(h * dsf);
    // Resizing clears the canvas: put the last frame back, stretched, until a fresh one lands.
    if (this.lastBitmap) this.g2d.drawImage(this.lastBitmap, 0, 0, this.canvas.width, this.canvas.height);
    await invoke("pane_resize", { width: w, height: h, deviceScaleFactor: dsf });
  }

  geometry(): PaneGeometry {
    const r = this.canvas.getBoundingClientRect();
    return {
      left: r.left,
      top: r.top,
      width: r.width,
      height: r.height,
      viewWidth: this.view.width || r.width,
      viewHeight: this.view.height || r.height,
    };
  }

  // ---- input (all of it gated on `userControl`; the backend enforces it too) ----------

  private send(cmd: string, args: Record<string, unknown>): void {
    invoke(cmd, args).catch((e) => this.onError?.(String(e)));
  }

  mouseDown = (e: MouseLike): void => {
    this.flushMove();
    if (!this.userControl) return;
    this.send("pane_mouse", { context: this.context, params: mouseParams("mousePressed", e, this.geometry()) });
  };

  mouseUp = (e: MouseLike): void => {
    this.flushMove();
    if (!this.userControl) return;
    this.send("pane_mouse", { context: this.context, params: mouseParams("mouseReleased", e, this.geometry()) });
  };

  /** Moves are coalesced to one per animation frame: the latest position wins. */
  mouseMove = (e: MouseLike): void => {
    if (!this.userControl) return;
    this.pendingMove = e;
    if (!this.moveRaf) this.moveRaf = requestAnimationFrame(() => this.flushMove());
  };

  private flushMove(): void {
    if (this.moveRaf) cancelAnimationFrame(this.moveRaf);
    this.moveRaf = 0;
    const e = this.pendingMove;
    this.pendingMove = null;
    if (e && this.userControl) {
      this.send("pane_mouse", { context: this.context, params: mouseParams("mouseMoved", e, this.geometry()) });
    }
  }

  wheel = (e: WheelLike): void => {
    if (!this.userControl) return;
    this.send("pane_mouse", { context: this.context, params: wheelParams(e, this.geometry()) });
  };

  key = (type: "keyDown" | "keyUp", e: KeyLike): boolean => {
    if (!this.userControl) return false;
    const params = keyParams(type, e);
    if (!params) return false;
    this.send("pane_key", { context: this.context, params });
    return true;
  };

  /** IME: `compose` while the candidate text changes, `commit` with the final text. */
  ime = (kind: "compose" | "commit", text: string): void => {
    if (!this.userControl) return;
    this.send("pane_ime", { context: this.context, kind, text });
  };

  // ---- navigation ----------------------------------------------------------------------

  navigate = (url: string): Promise<void> => invoke("pane_navigate", { context: this.context, url });
  history = (action: "back" | "forward" | "reload"): Promise<void> => invoke("pane_history", { context: this.context, action });
}
