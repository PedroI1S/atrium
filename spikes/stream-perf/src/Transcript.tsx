import { createVirtualizer } from "@tanstack/solid-virtual";
import { type Accessor, For, createEffect, createMemo } from "solid-js";
import type { Row, Transcript } from "./model";

export interface ViewSettings {
  overscan: number;
}

const estimate = (row: Row | undefined): number => {
  if (!row) return 40;
  if (row.kind === "md") return 12 + 19 * Math.max(1, Math.ceil(row.text.length / 95));
  if (row.kind === "tool") return 32;
  return 20;
};

function RowView(props: { row: Row }) {
  const r = props.row;
  if (r.kind === "md") return <div class={r.reasoning ? "md reasoning" : "md"} innerHTML={r.rendered()} />;
  if (r.kind === "tool") {
    return (
      <div class="tool">
        <span>{r.name}</span>
        <span>{(r.rev(), r.title)}</span>
        <span class={`status ${r.status}`}>{(r.rev(), r.status)}</span>
      </div>
    );
  }
  return <div class={r.tone === "error" ? "note error" : "note"}>{r.text}</div>;
}

/** A virtualized transcript that follows the tail until the user scrolls up. */
export function TranscriptView(props: { t: Transcript; tick: Accessor<number>; settings: ViewSettings }) {
  let scroller!: HTMLDivElement;
  const virt = createVirtualizer({
    get count() {
      return props.t.count();
    },
    getScrollElement: () => scroller,
    estimateSize: (i) => estimate(props.t.rows[i]),
    get overscan() {
      return props.settings.overscan;
    },
  });
  let follow = true;
  const onScroll = () => {
    follow = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 40;
  };
  // Runs in the same synchronous pass as the row updates and again after the virtualizer re-measured.
  createEffect(() => {
    props.tick();
    virt.getTotalSize();
    if (follow) scroller.scrollTop = scroller.scrollHeight;
  });
  const indexes = createMemo(() => virt.getVirtualItems().map((v) => v.index));
  return (
    <div class="transcript" ref={scroller} onScroll={onScroll}>
      <div class="spacer" style={{ height: `${virt.getTotalSize()}px` }}>
        <For each={indexes()}>
          {(index) => {
            const item = createMemo(() => virt.getVirtualItems().find((v) => v.index === index));
            return (
              <div class="vrow" data-index={index} ref={(el) => queueMicrotask(() => virt.measureElement(el))} style={{ transform: `translateY(${item()?.start ?? 0}px)` }}>
                <RowView row={props.t.rows[index]!} />
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}
