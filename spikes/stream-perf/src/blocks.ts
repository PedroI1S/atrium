// Incremental markdown block splitting. A block is the text between blank lines, except that a
// fenced code block is one block no matter how many blank lines it holds. Closed blocks never change
// again, so they are parsed once; only the open block (the tail) is re-parsed as text arrives.
//
// Known simplification: a list or quote with blank lines between its items becomes several blocks
// (an ordered list would restart numbering). A real implementation needs a proper block parser.

const FENCE = /^ {0,3}(`{3,}|~{3,})/;

export class BlockSplitter {
  /** Complete lines of the open block. */
  private body = "";
  /** The unfinished last line. */
  private partial = "";
  private fence: string | null = null;

  /** Adds streamed text; calls `onClose(block)` for every block that the text completes. */
  feed(text: string, onClose: (block: string) => void): void {
    let start = 0;
    for (;;) {
      const nl = text.indexOf("\n", start);
      if (nl === -1) {
        if (start === 0) this.partial += text;
        else this.partial += text.slice(start);
        return;
      }
      const line = this.partial + text.slice(start, nl);
      this.partial = "";
      start = nl + 1;
      this.line(line, onClose);
    }
  }

  private line(line: string, onClose: (block: string) => void): void {
    const f = FENCE.exec(line);
    if (this.fence !== null) {
      this.body += line + "\n";
      if (f && f[1]![0] === this.fence[0] && f[1]!.length >= this.fence.length && line.trim() === f[1]) {
        this.fence = null;
        // A closed fence ends the block.
        const b = this.body;
        this.body = "";
        onClose(b);
      }
      return;
    }
    if (f) {
      if (this.body !== "") {
        const b = this.body;
        this.body = "";
        onClose(b);
      }
      this.fence = f[1]!;
      this.body = line + "\n";
      return;
    }
    if (line.trim() === "") {
      if (this.body !== "") {
        const b = this.body;
        this.body = "";
        onClose(b);
      }
      return;
    }
    this.body += line + "\n";
  }

  /** The text of the block that is still open (complete lines plus the unfinished one). */
  openText(): string {
    return this.partial === "" ? this.body : this.body + this.partial;
  }

  isFenceOpen(): boolean {
    return this.fence !== null;
  }

  /** Closes whatever is open (end of message, or a tool call interrupted the text). */
  seal(onClose: (block: string) => void): void {
    const t = this.openText();
    this.body = "";
    this.partial = "";
    this.fence = null;
    if (t !== "") onClose(t);
  }
}
