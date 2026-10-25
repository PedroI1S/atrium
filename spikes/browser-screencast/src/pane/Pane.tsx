import { createEffect, onCleanup, onMount } from "solid-js";
import { PaneController, type PaneOptions } from "./controller";
import { bindImeSink } from "./imeSink";

interface PaneProps {
  options: PaneOptions;
  userControl: boolean;
  /** Called once with the controller after the canvas exists. */
  ready: (c: PaneController) => void;
  onError: (message: string) => void;
}

/** The canvas, its IME text sink and the resize observer. All logic lives in the controller. */
export function Pane(props: PaneProps) {
  let canvas!: HTMLCanvasElement;
  let sink!: HTMLTextAreaElement;
  let box!: HTMLDivElement;
  let controller: PaneController | undefined;

  onMount(() => {
    const c = new PaneController(canvas, props.options);
    controller = c;
    c.onError = props.onError;
    c.setUserControl(props.userControl);
    (window as unknown as { __pane: PaneController }).__pane = c;

    // Pointer events with capture: a drag that leaves the canvas still delivers its release.
    const down = (e: PointerEvent) => {
      if (!props.userControl) return;
      try {
        canvas.setPointerCapture(e.pointerId);
      } catch {
        // Synthetic events (the bench) have no active pointer to capture.
      }
      sink.focus({ preventScroll: true });
      c.mouseDown(e);
    };
    const up = (e: PointerEvent) => c.mouseUp(e);
    const move = (e: PointerEvent) => c.mouseMove(e);
    // Anything that ends the gesture without a pointerup must release the held buttons.
    const release = () => void c.releaseAll();
    const wheel = (e: WheelEvent) => {
      if (!props.userControl) return;
      e.preventDefault();
      c.wheel(e);
    };
    canvas.addEventListener("pointerdown", down);
    canvas.addEventListener("pointerup", up);
    canvas.addEventListener("pointermove", move);
    canvas.addEventListener("pointercancel", release);
    canvas.addEventListener("lostpointercapture", release);
    canvas.addEventListener("wheel", wheel, { passive: false });
    window.addEventListener("blur", release);
    const unbindSink = bindImeSink(sink, c);

    let timer = 0;
    const observer = new ResizeObserver(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        const r = box.getBoundingClientRect();
        void c.resize(r.width, r.height).catch((e) => props.onError(String(e)));
      }, 60);
    });
    observer.observe(box);
    onCleanup(() => {
      observer.disconnect();
      unbindSink();
      window.removeEventListener("blur", release);
    });
    props.ready(c);
  });

  // The controller gates input on this flag; keep it in step with the prop.
  createEffect(() => {
    const on = props.userControl;
    controller?.setUserControl(on);
    if (on) sink?.focus({ preventScroll: true });
  });

  return (
    <div class="viewport" ref={box}>
      <canvas
        ref={canvas}
        classList={{ driving: props.userControl }}
        aria-label="Browser page"
        onContextMenu={(e) => e.preventDefault()}
      />
      {/* Keyboard and IME sink: focus lives here so the OS input method has a text target. */}
      <textarea
        ref={sink}
        class="ime"
        aria-label="Keyboard input for the page"
        autocomplete="off"
        autocapitalize="off"
        spellcheck={false}
      />
    </div>
  );
}
