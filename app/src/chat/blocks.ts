// Incremental block splitting of a streaming message (from the M0-07 spike, extended). A block
// is the text between blank lines, except that a fenced code block is one block whatever it
// holds. Closed blocks never change again, so they are parsed and rendered once; only the open
// block is redone as text arrives, and the transcript virtualizes by block.
//
// Long blocks are cut so no single row grows without bound (a 100k-character paragraph would be
// re-parsed and re-laid-out every frame): an open paragraph past SOFT_BLOCK_CHARS closes at its
// last line break (or, with none, its last space), and an open fence past SOFT_FENCE_LINES
// closes and continues as a second code block. Such a block is marked `cont` so the view joins
// it to the one before. The cost: inline markup that spans a cut (a `**` opened before it) is
// shown as text on both sides. Only paragraphs over 4 KB without blank lines are affected.

/** An open paragraph past this many characters is cut. */
export const SOFT_BLOCK_CHARS = 4096;
/** An open fence past this many lines is cut. */
export const SOFT_FENCE_LINES = 200;

const FENCE = /^ {0,3}(`{3,}|~{3,})/;

export interface BlockText {
  text: string;
  /** Continues the block before it (a cut, see above). */
  cont: boolean;
  /** Will not change again. */
  closed: boolean;
  /** Bumped on every change, for the view. */
  v: number;
}

export class BlockSplitter {
  readonly blocks: BlockText[] = [];
  /** Complete lines of the open block. */
  private body = "";
  /** The unfinished last line. */
  private partial = "";
  private fence: { marker: string; opener: string; lines: number } | null = null;
  private cont = false;
  /** Blocks whose content changed since the last `takeChanged`. */
  private changed = new Set<BlockText>();
  /** Set when a block was added or closed. */
  structural = false;

  /** Adds streamed text. */
  feed(text: string): void {
    let start = 0;
    for (;;) {
      const nl = text.indexOf("\n", start);
      if (nl === -1) {
        this.partial += start === 0 ? text : text.slice(start);
        break;
      }
      const line = this.partial + text.slice(start, nl);
      this.partial = "";
      start = nl + 1;
      this.line(line);
    }
    this.softCut();
    this.syncOpen();
  }

  /** Ends the message: whatever is open closes. */
  seal(): void {
    const t = this.openText();
    this.body = "";
    this.partial = "";
    this.fence = null;
    const open = this.openBlock();
    if (open) {
      open.text = t;
      open.closed = true;
      open.v++;
      this.changed.add(open);
      this.structural = true;
    }
  }

  /** Text of the open block (complete lines and the unfinished one). */
  openText(): string {
    return this.partial === "" ? this.body : this.body + this.partial;
  }

  takeChanged(): Set<BlockText> {
    const c = this.changed;
    this.changed = new Set();
    return c;
  }

  private openBlock(): BlockText | undefined {
    const last = this.blocks[this.blocks.length - 1];
    return last && !last.closed ? last : undefined;
  }

  private close(text: string): void {
    const open = this.openBlock();
    if (open) {
      open.text = text;
      open.closed = true;
      open.v++;
      this.changed.add(open);
    } else {
      this.blocks.push({ text, cont: this.cont, closed: true, v: 0 });
    }
    this.cont = false;
    this.structural = true;
  }

  /** Makes the last block show the open text (adding one when needed). */
  private syncOpen(): void {
    const t = this.openText();
    const open = this.openBlock();
    if (t === "") {
      if (open) {
        this.blocks.pop();
        this.structural = true;
      }
      return;
    }
    if (open) {
      if (open.text !== t) {
        open.text = t;
        open.v++;
        this.changed.add(open);
      }
    } else {
      this.blocks.push({ text: t, cont: this.cont, closed: false, v: 0 });
      this.structural = true;
    }
  }

  private line(line: string): void {
    const f = FENCE.exec(line);
    if (this.fence !== null) {
      this.body += line + "\n";
      this.fence.lines++;
      if (f && f[1]![0] === this.fence.marker[0] && f[1]!.length >= this.fence.marker.length && line.trim() === f[1]) {
        this.fence = null;
        const b = this.body;
        this.body = "";
        this.close(b);
      }
      return;
    }
    if (f) {
      if (this.body !== "") {
        const b = this.body;
        this.body = "";
        this.close(b);
      }
      this.fence = { marker: f[1]!, opener: line, lines: 0 };
      this.body = line + "\n";
      return;
    }
    if (line.trim() === "") {
      if (this.body !== "") {
        const b = this.body;
        this.body = "";
        this.close(b);
      }
      return;
    }
    this.body += line + "\n";
  }

  private softCut(): void {
    if (this.fence) {
      if (this.fence.lines < SOFT_FENCE_LINES) return;
      // Close this part of the code block and carry on in a new one.
      const closer = this.fence.marker;
      const b = this.body + closer + "\n";
      this.close(b);
      this.cont = true;
      this.body = this.fence.opener + "\n";
      this.fence.lines = 0;
      return;
    }
    const t = this.openText();
    if (t.length <= SOFT_BLOCK_CHARS) return;
    if (this.body !== "") {
      // Cut at the last complete line.
      const b = this.body;
      this.body = "";
      this.close(b);
      this.cont = true;
      return;
    }
    // One long line: cut at its last space, if it has one.
    const space = this.partial.lastIndexOf(" ");
    if (space <= 0) return;
    this.close(this.partial.slice(0, space));
    this.partial = this.partial.slice(space + 1);
    this.cont = true;
  }
}
