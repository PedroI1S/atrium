import { createEffect, onCleanup, onMount } from "solid-js";
import { PaneController, type PaneOptions } from "./controller";

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

  onMount(() => {
    const c = new PaneController(canvas, props.options);
    c.onError = props.onError;
    c.userControl = props.userControl;
    (window as unknown as { __pane: PaneController }).__pane = c;

    let timer = 0;
    const observer = new ResizeObserver(() => {
      window.clearTimeout(timer);
      timer = window.setTimeout(() => {
        const r = box.getBoundingClientRect();
        void c.resize(r.width, r.height).catch((e) => props.onError(String(e)));
      }, 60);
    });
    observer.observe(box);
    onCleanup(() => observer.disconnect());
    props.ready(c);
  });

  const controller = () => (window as unknown as { __pane?: PaneController }).__pane;
  // The controller gates input on this flag; keep it in step with the prop.
  createEffect(() => {
    const c = controller();
    if (c) c.userControl = props.userControl;
    if (props.userControl) sink?.focus({ preventScroll: true });
  });

  return (
    <div class="viewport" ref={box}>
      <canvas
        ref={canvas}
        classList={{ driving: props.userControl }}
        aria-label="Browser page"
        onMouseDown={(e) => {
          if (!props.userControl) return;
          sink.focus({ preventScroll: true });
          controller()?.mouseDown(e);
        }}
        onMouseUp={(e) => controller()?.mouseUp(e)}
        onMouseMove={(e) => controller()?.mouseMove(e)}
        onContextMenu={(e) => e.preventDefault()}
        on:wheel={{
          passive: false,
          handleEvent: (e: WheelEvent) => {
            if (!props.userControl) return;
            e.preventDefault();
            controller()?.wheel(e);
          },
        }}
      />
      {/* Keyboard and IME sink: focus lives here so the OS input method has a text target. */}
      <textarea
        ref={sink}
        class="ime"
        aria-label="Keyboard input for the page"
        autocomplete="off"
        autocapitalize="off"
        spellcheck={false}
        onKeyDown={(e) => {
          if (controller()?.key("keyDown", e)) e.preventDefault();
        }}
        onKeyUp={(e) => {
          if (controller()?.key("keyUp", e)) e.preventDefault();
        }}
        onCompositionUpdate={(e) => controller()?.ime("compose", e.data)}
        onCompositionEnd={(e) => {
          controller()?.ime("commit", e.data);
          sink.value = "";
        }}
        onInput={() => {
          // Text reaches the page through keydown and composition events, never through this element.
          sink.value = "";
        }}
      />
    </div>
  );
}
