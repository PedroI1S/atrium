// Keystroke-to-paint measurement for the composer.
//
// For a real key the event's `timeStamp` is the time the OS handed it to WebKit; for a synthetic
// event (the harness' typist) it is the time the event object was created. Three points are kept:
//   handler  = input handler start - origin   (how long the main thread was busy before it could run)
//   frame    = start of the first rAF after the input - origin
//   paint    = start of the rAF after that - origin (the frame containing the new text has been committed)

import { summarizeValues } from "./metrics";

export class TypingStats {
  handler: number[] = [];
  frame: number[] = [];
  paint: number[] = [];
  keys = 0;
  inputs = 0;
  /** How late each scheduled keystroke fired (ms after its deadline); synthetic typing only. */
  lateness: number[] = [];
  /** Set by the synthetic typist to the keystroke's scheduled time, so latency counts from the deadline. */
  nextOrigin: number | null = null;
  /** Keydowns whose origin has not been matched to an input event yet. */
  pending: number[] = [];
  reset(): void {
    this.handler = [];
    this.frame = [];
    this.paint = [];
    this.keys = 0;
    this.inputs = 0;
    this.lateness = [];
    this.nextOrigin = null;
    this.pending = [];
  }
  summary() {
    return {
      keys: this.keys,
      inputs: this.inputs,
      timerLatenessMs: summarizeValues(this.lateness),
      handlerMs: summarizeValues(this.handler),
      frameMs: summarizeValues(this.frame),
      paintMs: summarizeValues(this.paint),
    };
  }
}

export const typing = new TypingStats();
