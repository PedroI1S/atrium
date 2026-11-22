// Markdown to a syntax tree (contract §12: agent text is untrusted input). The tree is rendered
// to DOM nodes by render.ts, never to an HTML string. Raw HTML is never recognised: `<script>`
// in a message is text, as it would be in a plain-text field.
//
// What is parsed: ATX and setext headings, paragraphs, fenced code (``` and ~~~, a longer fence
// may contain shorter ones), block quotes, bullet and ordered lists (nested by indentation),
// thematic breaks, GFM tables; inline code, emphasis (*, _, **, __), strikethrough (~~), links,
// images, autolinks (<https://…>) and bare http(s) URLs, backslash escapes, hard breaks.
// Not parsed (shown as text): indented code blocks, reference links, HTML, footnotes.
//
// Links and images are classified here (links.ts), so the renderer never decides what a URL is.

import { type ImageTarget, type LinkTarget, classifyImage, classifyLink } from "./links";

export type Inline =
  | { t: "text"; v: string }
  | { t: "code"; v: string }
  | { t: "em" | "strong" | "del"; c: Inline[] }
  | { t: "link"; target: LinkTarget; c: Inline[] }
  | { t: "image"; target: ImageTarget; alt: string }
  | { t: "br" };

export type Align = "left" | "center" | "right" | null;

export type Block =
  | { t: "p"; c: Inline[] }
  | { t: "h"; level: number; c: Inline[] }
  /** `open`: the fence isn't closed yet (still streaming). */
  | { t: "code"; lang: string; v: string; open: boolean }
  | { t: "quote"; c: Block[] }
  | { t: "list"; ordered: boolean; start: number; items: Block[][] }
  | { t: "hr" }
  | { t: "table"; align: Align[]; head: Inline[][]; rows: Inline[][][] };

/** Nesting deeper than this (quotes in lists in quotes…) is shown as text. */
const MAX_DEPTH = 12;
/** Inline container nesting deeper than this is flattened so traversal and rendering stay bounded. */
const MAX_INLINE_DEPTH = 16;

// ---- blocks --------------------------------------------------------------------------------

const FENCE_OPEN = /^( {0,3})(`{3,}|~{3,})(.*)$/;
const QUOTE = /^ {0,3}> ?/;
const BULLET = /^( {0,3})([-*+])( {1,4}|\t|$)/;
const ORDERED = /^( {0,3})(\d{1,9})([.)])( {1,4}|\t|$)/;
const SETEXT = /^ {0,3}(=+|-+)[ \t]*$/;
const BLANK = /^[ \t]*$/;

/** Deterministic O(n) ATX heading scan (avoids lazy/whitespace regex backtracking). */
function parseAtxHeading(line: string): { level: number; text: string } | null {
  const len = line.length;
  let i = 0;
  while (i < len && i < 3 && line[i] === " ") i++;
  let level = 0;
  while (i < len && line[i] === "#") {
    level++;
    i++;
    if (level > 6) return null;
  }
  if (level === 0) return null;
  if (i === len) return { level, text: "" };
  if (line[i] !== " " && line[i] !== "\t") return null;
  let start = i;
  while (start < len && (line[start] === " " || line[start] === "\t")) start++;
  let end = len;
  while (end > start && (line[end - 1] === " " || line[end - 1] === "\t")) end--;
  if (start === end) return { level, text: "" };
  // Strip an optional closing `#` sequence preceded by at least one space/tab.
  let hashStart = end;
  while (hashStart > start && line[hashStart - 1] === "#") hashStart--;
  if (hashStart < end && (hashStart === start || line[hashStart - 1] === " " || line[hashStart - 1] === "\t")) {
    end = hashStart;
    while (end > start && (line[end - 1] === " " || line[end - 1] === "\t")) end--;
  }
  return { level, text: start === end ? "" : line.slice(start, end) };
}

/** Deterministic O(n) thematic-break scan (`---`, `***`, `___` with optional spaces/tabs). */
function isThematicBreak(line: string): boolean {
  const len = line.length;
  let i = 0;
  while (i < len && i < 3 && line[i] === " ") i++;
  const ch = line[i];
  if (ch !== "-" && ch !== "*" && ch !== "_") return false;
  let count = 0;
  while (i < len) {
    const c = line[i];
    if (c === ch) count++;
    else if (c !== " " && c !== "\t") return false;
    i++;
  }
  return count >= 3;
}

/** Deterministic O(n) GFM table-delimiter row scan (avoids adjacent whitespace quantifiers). */
function isTableDelimiter(line: string): boolean {
  const len = line.length;
  let i = 0;
  while (i < len && i < 3 && line[i] === " ") i++;
  if (i < len && line[i] === "|") i++;
  let cells = 0;
  while (true) {
    while (i < len && (line[i] === " " || line[i] === "\t")) i++;
    if (i >= len) return cells > 0;
    if (line[i] === ":") i++;
    const dashStart = i;
    while (i < len && line[i] === "-") i++;
    if (i === dashStart) return false;
    if (i < len && line[i] === ":") i++;
    cells++;
    while (i < len && (line[i] === " " || line[i] === "\t")) i++;
    if (i >= len) return true;
    if (line[i] !== "|") return false;
    i++;
    while (i < len && (line[i] === " " || line[i] === "\t")) i++;
    if (i >= len) return true;
  }
}

interface Marker { ordered: boolean; char: string; start: number; indent: number; content: number }

function listMarker(line: string): Marker | null {
  const b = BULLET.exec(line);
  if (b) {
    const indent = b[1]!.length;
    const pad = b[3] === "" ? 1 : b[3]!.length;
    return { ordered: false, char: b[2]!, start: 1, indent, content: indent + 1 + pad };
  }
  const o = ORDERED.exec(line);
  if (o) {
    const indent = o[1]!.length;
    const pad = o[4] === "" ? 1 : o[4]!.length;
    return { ordered: true, char: o[3]!, start: Number(o[2]), indent, content: indent + o[2]!.length + 1 + pad };
  }
  return null;
}

const expandTabs = (line: string) => (line.includes("\t") ? line.replace(/\t/g, "    ") : line);

function startsBlock(line: string): boolean {
  return FENCE_OPEN.test(line) || parseAtxHeading(line) !== null || isThematicBreak(line) || QUOTE.test(line) || listMarker(line) !== null;
}

function splitRow(line: string): string[] {
  let s = line.trim();
  if (s.startsWith("|")) s = s.slice(1);
  if (s.endsWith("|") && !s.endsWith("\\|")) s = s.slice(0, -1);
  const cells: string[] = [];
  let cur = "";
  for (let i = 0; i < s.length; i++) {
    const ch = s[i]!;
    if (ch === "\\" && s[i + 1] === "|") { cur += "|"; i++; continue; }
    if (ch === "|") { cells.push(cur.trim()); cur = ""; continue; }
    cur += ch;
  }
  cells.push(cur.trim());
  return cells;
}

function alignOf(cell: string): Align {
  const left = cell.startsWith(":");
  const right = cell.endsWith(":");
  return left && right ? "center" : right ? "right" : left ? "left" : null;
}

function parseLines(lines: string[], depth: number): Block[] {
  const out: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i]!;
    if (BLANK.test(line)) { i++; continue; }

    const fence = FENCE_OPEN.exec(line);
    if (fence && !(fence[2]![0] === "`" && fence[3]!.includes("`"))) {
      const marker = fence[2]!;
      const indent = fence[1]!.length;
      const body: string[] = [];
      let open = true;
      i++;
      while (i < lines.length) {
        const l = lines[i]!;
        const close = /^ {0,3}(`{3,}|~{3,})[ \t]*$/.exec(l);
        if (close && close[1]![0] === marker[0] && close[1]!.length >= marker.length) { open = false; i++; break; }
        // Content lines lose up to the fence's own indentation.
        body.push(l.replace(new RegExp(`^ {0,${indent}}`), ""));
        i++;
      }
      out.push({ t: "code", lang: fence[3]!.trim().split(/\s+/)[0] ?? "", v: body.join("\n"), open });
      continue;
    }

    const atx = parseAtxHeading(line);
    if (atx) {
      out.push({ t: "h", level: atx.level, c: parseInline(atx.text) });
      i++;
      continue;
    }
    if (isThematicBreak(line)) { out.push({ t: "hr" }); i++; continue; }

    if (QUOTE.test(line)) {
      const inner: string[] = [];
      while (i < lines.length && !BLANK.test(lines[i]!)) {
        const l = lines[i]!;
        if (QUOTE.test(l)) inner.push(l.replace(QUOTE, ""));
        else if (inner.length && !startsBlock(l)) inner.push(l); // lazy continuation
        else break;
        i++;
      }
      out.push(depth < MAX_DEPTH ? { t: "quote", c: parseLines(inner, depth + 1) } : { t: "p", c: [{ t: "text", v: inner.join("\n") }] });
      continue;
    }

    const first = listMarker(line);
    if (first) {
      const items: Block[][] = [];
      while (i < lines.length) {
        const m = listMarker(lines[i]!);
        if (!m || m.ordered !== first.ordered || m.char !== first.char || m.indent >= first.content) break;
        const body: string[] = [expandTabs(lines[i]!).slice(m.content)];
        i++;
        let lastBlank = false;
        while (i < lines.length) {
          const l = expandTabs(lines[i]!);
          if (BLANK.test(l)) { body.push(""); lastBlank = true; i++; continue; }
          const indent = /^ */.exec(l)![0].length;
          if (indent >= m.content) { body.push(l.slice(m.content)); lastBlank = false; i++; continue; }
          // A lazy continuation of the item's paragraph.
          if (!lastBlank && !startsBlock(l)) { body.push(l.trimStart()); i++; continue; }
          break;
        }
        while (body.length && body[body.length - 1] === "") body.pop();
        items.push(depth < MAX_DEPTH ? parseLines(body, depth + 1) : [{ t: "p", c: [{ t: "text", v: body.join("\n") }] }]);
        // A blank line, then something that isn't this list's next item, ends the list.
        if (i < lines.length && BLANK.test(lines[i - 1] ?? "")) {
          const next = listMarker(lines[i]!);
          if (!next || next.ordered !== first.ordered || next.char !== first.char) break;
        }
      }
      out.push({ t: "list", ordered: first.ordered, start: first.start, items });
      continue;
    }

    if (line.includes("|") && i + 1 < lines.length && isTableDelimiter(lines[i + 1]!)) {
      const head = splitRow(line);
      const delims = splitRow(lines[i + 1]!);
      if (delims.length === head.length) {
        const align = delims.map(alignOf);
        i += 2;
        const rows: Inline[][][] = [];
        while (i < lines.length && !BLANK.test(lines[i]!) && lines[i]!.includes("|") && !startsBlock(lines[i]!)) {
          const cells = splitRow(lines[i]!);
          rows.push(head.map((_, k) => parseInline(cells[k] ?? "")));
          i++;
        }
        out.push({ t: "table", align, head: head.map((h) => parseInline(h)), rows });
        continue;
      }
    }

    // A paragraph: lines until a blank line or the start of another block.
    const para: string[] = [line];
    i++;
    while (i < lines.length && !BLANK.test(lines[i]!)) {
      const l = lines[i]!;
      const setext = SETEXT.exec(l);
      if (setext) {
        out.push({ t: "h", level: setext[1]![0] === "=" ? 1 : 2, c: parseInline(para.join("\n").trim()) });
        para.length = 0;
        i++;
        break;
      }
      if (startsBlock(l) && !(ORDERED.test(l) && !/^ {0,3}1[.)]/.test(l))) break;
      para.push(l);
      i++;
    }
    if (para.length) out.push({ t: "p", c: parseInline(para.map((l) => l.replace(/^ {0,3}/, "")).join("\n")) });
  }
  return out;
}

/** Parses a markdown document (or one block of a streaming message). */
export function parseMarkdown(src: string): Block[] {
  return parseLines(src.replace(/\r\n?/g, "\n").split("\n"), 0);
}

// ---- inlines -------------------------------------------------------------------------------

const PUNCT = /[!-/:-@[-`{-~]/;
const WS = /\s/;
const PUNCTUATION = /[\p{P}\p{S}]/u;

/** Singly-linked inline sequence supporting O(1) concatenation when flattening deep nesting. */
interface InlineCell {
  item: Inline;
  next: InlineCell | null;
}

interface InlineSeq {
  head: InlineCell;
  tail: InlineCell;
  depth: number;
}

function singleSeq(item: Inline, depth: number): InlineSeq {
  const cell: InlineCell = { item, next: null };
  return { head: cell, tail: cell, depth };
}

function appendSeq(a: InlineSeq | null, b: InlineSeq): InlineSeq {
  if (!a) return b;
  a.tail.next = b.head;
  a.tail = b.tail;
  if (b.depth > a.depth) a.depth = b.depth;
  return a;
}

/** A node of the inline list while parsing: text, finished inlines and delimiter runs. */
interface Node {
  prev: Node | null;
  next: Node | null;
  seq: number;
  items: InlineSeq | null;
  /** Delimiter run (`*`, `_`, `~`) or bracket (`[`, `![`); null for finished content. */
  delim: { char: string; count: number; orig: number; canOpen: boolean; canClose: boolean } | null;
  bracket: { image: boolean; active: boolean; pos: number } | null;
}

class List {
  head: Node | null = null;
  tail: Node | null = null;
  private nextSeq = 1;
  push(n: Omit<Node, "prev" | "next" | "seq">): Node {
    const node: Node = { ...n, prev: this.tail, next: null, seq: this.nextSeq++ };
    if (this.tail) this.tail.next = node;
    else this.head = node;
    this.tail = node;
    return node;
  }
  remove(n: Node): void {
    if (n.prev) n.prev.next = n.next;
    else this.head = n.next;
    if (n.next) n.next.prev = n.prev;
    else this.tail = n.prev;
  }
  /** Removes the nodes strictly between `a` and `b` and returns them, in order. */
  cut(a: Node, b: Node): Node[] {
    const taken: Node[] = [];
    let n = a.next;
    while (n && n !== b) {
      taken.push(n);
      n = n.next;
    }
    a.next = b;
    b.prev = a;
    return taken;
  }
}

function text(v: string): Inline {
  return { t: "text", v };
}

/** Concatenates a node run into an O(1)-appendable inline sequence (unmatched delimiters and brackets become text). */
function concatNodes(nodes: Node[]): InlineSeq | null {
  let out: InlineSeq | null = null;
  for (const n of nodes) {
    const seg: InlineSeq | null =
      n.items ??
      (n.delim
        ? singleSeq(text(n.delim.char.repeat(n.delim.count)), 0)
        : n.bracket
          ? singleSeq(text(n.bracket.image ? "![" : "["), 0)
          : null);
    if (seg) out = appendSeq(out, seg);
  }
  return out;
}

/** Materialises an inline sequence into an array, coalescing adjacent text nodes. */
function seqToArray(seq: InlineSeq | null): Inline[] {
  const out: Inline[] = [];
  let cell = seq ? seq.head : null;
  while (cell) {
    const item = cell.item;
    const last = out[out.length - 1];
    if (item.t === "text" && last?.t === "text") last.v += item.v;
    else out.push(item.t === "text" ? { ...item } : item);
    cell = cell.next;
  }
  return out;
}

interface LinkScanState {
  lastParen: number | null;
  angleEnd: number;
  angleHasGt: boolean;
  parenTitleEnd: number;
  dquoteTitleEnd: number;
  squoteTitleEnd: number;
  failedAfterDest: Set<number>;
  failedAfterTitle: Set<number>;
}

/** Link destination and optional title after `](`. Returns the end index (after `)`) or null. */
function linkTail(s: string, at: number, state: LinkScanState): { dest: string; end: number } | null {
  let i = at;
  while (i < s.length && (s[i] === " " || s[i] === "\n")) i++;
  let destStart = i;
  let destEnd = i;
  let unescapeBare = false;
  if (s[i] === "<") {
    let j: number;
    if (i < state.angleEnd) {
      if (!state.angleHasGt) return null;
      j = state.angleEnd;
    } else {
      j = i + 1;
      while (j < s.length && s[j] !== ">" && s[j] !== "\n") j++;
      state.angleEnd = j;
      state.angleHasGt = j < s.length && s[j] === ">";
      if (!state.angleHasGt) return null;
    }
    destStart = i + 1;
    destEnd = j;
    i = j + 1;
  } else {
    let depth = 0;
    destStart = i;
    while (i < s.length) {
      const ch = s[i]!;
      if (ch === "\\" && i + 1 < s.length && PUNCT.test(s[i + 1]!)) { i += 2; continue; }
      if (ch === "(") {
        if (++depth > 32) return null;
      } else if (ch === ")") {
        if (depth === 0) break;
        depth--;
      } else if (WS.test(ch) || ch < " ") break;
      i++;
    }
    destEnd = i;
    unescapeBare = true;
  }
  const afterDest = i;
  if (state.failedAfterDest.has(afterDest)) return null;
  while (i < s.length && (s[i] === " " || s[i] === "\n")) i++;
  // An optional title, which is not shown (the URL itself is, on hover).
  const q = s[i];
  if (q === '"' || q === "'" || q === "(") {
    const closer = q === "(" ? ")" : q;
    const cachedEnd =
      q === "(" ? state.parenTitleEnd : q === '"' ? state.dquoteTitleEnd : state.squoteTitleEnd;
    let j: number;
    if (i + 1 < cachedEnd) {
      j = cachedEnd;
    } else {
      j = i + 1;
      while (j < s.length && s[j] !== closer) j += s[j] === "\\" ? 2 : 1;
      if (q === "(") state.parenTitleEnd = j;
      else if (q === '"') state.dquoteTitleEnd = j;
      else state.squoteTitleEnd = j;
    }
    if (j >= s.length) {
      state.failedAfterDest.add(afterDest);
      return null;
    }
    const afterTitle = j + 1;
    if (state.failedAfterTitle.has(afterTitle)) {
      state.failedAfterDest.add(afterDest);
      return null;
    }
    i = afterTitle;
    while (i < s.length && (s[i] === " " || s[i] === "\n")) i++;
    if (s[i] !== ")") {
      state.failedAfterTitle.add(afterTitle);
      state.failedAfterDest.add(afterDest);
      return null;
    }
  } else if (s[i] !== ")") {
    state.failedAfterDest.add(afterDest);
    return null;
  }
  const rawDest = s.slice(destStart, destEnd);
  const dest = unescapeBare ? rawDest.replace(/\\([!-/:-@[-`{-~])/g, "$1") : rawDest;
  return { dest, end: i + 1 };
}

const AUTOLINK = /^<([a-zA-Z][a-zA-Z0-9+.-]{1,31}:[^\s<>]*)>/;
const EMAIL = /^<([a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*)>/;
const BARE_URL = /https?:\/\/[^\s<>"]+/g;

function isBareUrlTrailingPunct(ch: string | undefined): boolean {
  return (
    ch === "." ||
    ch === "," ||
    ch === ":" ||
    ch === ";" ||
    ch === "!" ||
    ch === "?" ||
    ch === "'" ||
    ch === "*" ||
    ch === "_" ||
    ch === "~"
  );
}

/** Splits bare http(s) URLs out of a text inline (GFM's extended autolinks, http only). */
function linkify(items: Inline[]): Inline[] {
  const out: Inline[] = [];
  for (const item of items) {
    if (item.t !== "text" || !item.v.includes("http")) { out.push(item); continue; }
    let last = 0;
    for (const m of item.v.matchAll(BARE_URL)) {
      const raw = m[0];
      let end = raw.length;
      let openParens = 0;
      let closeParens = 0;
      if (raw.includes(")")) {
        for (let k = 0; k < raw.length; k++) {
          const c = raw[k];
          if (c === "(") openParens++;
          else if (c === ")") closeParens++;
        }
      }
      // Trailing punctuation belongs to the sentence; an unbalanced `)` too.
      while (end > 0) {
        const ch = raw[end - 1];
        if (isBareUrlTrailingPunct(ch)) {
          end--;
        } else if (ch === ")" && openParens < closeParens) {
          closeParens--;
          end--;
        } else {
          break;
        }
      }
      const url = end === raw.length ? raw : raw.slice(0, end);
      const start = m.index;
      if (start > last) out.push(text(item.v.slice(last, start)));
      out.push({ t: "link", target: classifyLink(url), c: [text(url)] });
      last = start + url.length;
    }
    if (last < item.v.length) out.push(text(item.v.slice(last)));
  }
  return out;
}

function mapText(items: Inline[], f: (list: Inline[]) => Inline[]): Inline[] {
  return f(items).map((item) => (item.t === "em" || item.t === "strong" || item.t === "del" ? { ...item, c: mapText(item.c, f) } : item));
}

function isWs(c: string): boolean {
  const code = c.charCodeAt(0);
  if (code < 128) return code === 32 || (code >= 9 && code <= 13);
  return WS.test(c);
}

function isPunctuation(c: string): boolean {
  const code = c.charCodeAt(0);
  if (code < 128) {
    return (
      (code >= 33 && code <= 47) ||
      (code >= 58 && code <= 64) ||
      (code >= 91 && code <= 96) ||
      (code >= 123 && code <= 126)
    );
  }
  return PUNCTUATION.test(c);
}

function flanking(s: string, start: number, end: number): { left: boolean; right: boolean } {
  const before = start > 0 ? s[start - 1]! : " ";
  const after = end < s.length ? s[end]! : " ";
  const wsAfter = isWs(after);
  const wsBefore = isWs(before);
  const pAfter = isPunctuation(after);
  const pBefore = isPunctuation(before);
  const left = !wsAfter && (!pAfter || wsBefore || pBefore);
  const right = !wsBefore && (!pBefore || wsAfter || pAfter);
  return { left, right };
}

/** Matches emphasis delimiters after `bottom` (CommonMark's "process emphasis", simplified). */
function processEmphasis(list: List, bottom: Node | null): void {
  const bottomSeq = bottom ? bottom.seq : 0;
  // Store monotonic node sequence numbers so removing/cutting nodes never
  // causes a failed search to rescan back to the start of the list (O(N)).
  const openersBottom = new Map<string, number>();
  let closer: Node | null = bottom ? bottom.next : list.head;
  while (closer) {
    const cd = closer.delim;
    if (!cd || !cd.canClose) { closer = closer.next; continue; }
    const key = `${cd.char}${cd.canOpen ? 1 : 0}${cd.orig % 3}`;
    const floorSeq = openersBottom.get(key) ?? bottomSeq;
    let opener = closer.prev;
    let found: Node | null = null;
    while (opener && opener.seq > floorSeq) {
      const od = opener.delim;
      if (od && od.char === cd.char && od.canOpen) {
        const odd = (od.canClose || cd.canOpen) && (od.orig + cd.orig) % 3 === 0 && !(od.orig % 3 === 0 && cd.orig % 3 === 0);
        const tildeOk = cd.char !== "~" || (od.count === 2 && cd.count === 2);
        if (!odd && tildeOk) { found = opener; break; }
      }
      opener = opener.prev;
    }
    if (!found) {
      openersBottom.set(key, Math.max(floorSeq, closer.seq - 1));
      if (!cd.canOpen) closer.delim = null, (closer.items = singleSeq(text(cd.char.repeat(cd.count)), 0));
      closer = closer.next;
      continue;
    }
    const od = found.delim!;
    const use = cd.char === "~" ? 2 : od.count >= 2 && cd.count >= 2 ? 2 : 1;
    const innerSeq = concatNodes(list.cut(found, closer));
    const innerDepth = innerSeq ? innerSeq.depth : 0;
    const kind = cd.char === "~" ? "del" : use === 2 ? "strong" : "em";
    od.count -= use;
    cd.count -= use;
    // Flatten excess inline nesting so traversal and rendering stay bounded.
    const nextItems: InlineSeq | null =
      innerDepth < MAX_INLINE_DEPTH
        ? singleSeq({ t: kind, c: seqToArray(innerSeq) }, innerDepth + 1)
        : innerSeq;
    if (nextItems) {
      const node: Node = { prev: found, next: closer, seq: found.seq, items: nextItems, delim: null, bracket: null };
      found.next = node;
      closer.prev = node;
    }
    if (od.count === 0) list.remove(found);
    if (cd.count === 0) {
      const next = closer.next;
      list.remove(closer);
      closer = next;
    }
  }
  // Whatever is left over is text.
  let n = bottom ? bottom.next : list.head;
  while (n) {
    if (n.delim) {
      n.items = singleSeq(text(n.delim.char.repeat(n.delim.count)), 0);
      n.delim = null;
    }
    n = n.next;
  }
}

function trimTrailingSpaces(s: string): string {
  let end = s.length;
  while (end > 0 && s[end - 1] === " ") end--;
  return end === s.length ? s : s.slice(0, end);
}

/** Parses inline markdown in linear time. */
export function parseInline(s: string): Inline[] {
  const list = new List();
  const brackets: Node[] = [];
  const activeLinks: Node[] = [];
  let backtickRuns: Map<number, { positions: number[]; cursor: number }> | null = null;
  const linkScan: LinkScanState = {
    lastParen: null,
    angleEnd: -1,
    angleHasGt: false,
    parenTitleEnd: -1,
    dquoteTitleEnd: -1,
    squoteTitleEnd: -1,
    failedAfterDest: new Set(),
    failedAfterTitle: new Set(),
  };
  let buf = "";
  const flush = () => {
    if (buf) list.push({ items: singleSeq(text(buf), 0), delim: null, bracket: null });
    buf = "";
  };
  let i = 0;
  while (i < s.length) {
    const ch = s[i]!;
    if (ch === "\\") {
      const next = s[i + 1];
      if (next === "\n") { flush(); list.push({ items: singleSeq({ t: "br" }, 0), delim: null, bracket: null }); i += 2; continue; }
      if (next !== undefined && PUNCT.test(next)) { buf += next; i += 2; continue; }
      buf += ch;
      i++;
      continue;
    }
    if (ch === "`") {
      let n = 1;
      while (s[i + n] === "`") n++;
      if (backtickRuns === null) {
        backtickRuns = new Map();
        let p = i;
        while (p < s.length) {
          const k = s.indexOf("`", p);
          if (k < 0) break;
          let end = k + 1;
          while (end < s.length && s[end] === "`") end++;
          const len = end - k;
          let entry = backtickRuns.get(len);
          if (!entry) {
            entry = { positions: [], cursor: 0 };
            backtickRuns.set(len, entry);
          }
          entry.positions.push(k);
          p = end;
        }
      }
      let close = -1;
      const entry = backtickRuns.get(n);
      if (entry) {
        const minPos = i + n;
        while (entry.cursor < entry.positions.length && entry.positions[entry.cursor]! < minPos) {
          entry.cursor++;
        }
        if (entry.cursor < entry.positions.length) {
          close = entry.positions[entry.cursor]!;
        }
      }
      if (close < 0) { buf += "`".repeat(n); i += n; continue; }
      let code = s.slice(i + n, close).replace(/\n/g, " ");
      if (code.length > 2 && code.startsWith(" ") && code.endsWith(" ") && code.trim()) code = code.slice(1, -1);
      flush();
      list.push({ items: singleSeq({ t: "code", v: code }, 0), delim: null, bracket: null });
      i = close + n;
      continue;
    }
    if (ch === "<") {
      const auto = AUTOLINK.exec(s.slice(i, i + 4096));
      if (auto) {
        flush();
        list.push({ items: singleSeq({ t: "link", target: classifyLink(auto[1]!), c: [text(auto[1]!)] }, 1), delim: null, bracket: null });
        i += auto[0].length;
        continue;
      }
      const mail = EMAIL.exec(s.slice(i, i + 512));
      if (mail) {
        flush();
        list.push({ items: singleSeq({ t: "link", target: classifyLink(`mailto:${mail[1]!}`), c: [text(mail[1]!)] }, 1), delim: null, bracket: null });
        i += mail[0].length;
        continue;
      }
      buf += ch;
      i++;
      continue;
    }
    if (ch === "*" || ch === "_" || ch === "~") {
      let n = 1;
      while (s[i + n] === ch) n++;
      const { left, right } = flanking(s, i, i + n);
      let canOpen = left;
      let canClose = right;
      if (ch === "_") {
        const before = i > 0 ? s[i - 1]! : " ";
        const after = i + n < s.length ? s[i + n]! : " ";
        canOpen = left && (!right || isPunctuation(before));
        canClose = right && (!left || isPunctuation(after));
      }
      if (ch === "~" && n !== 2) { buf += ch.repeat(n); i += n; continue; }
      flush();
      list.push({ items: null, delim: { char: ch, count: n, orig: n, canOpen, canClose }, bracket: null });
      i += n;
      continue;
    }
    if (ch === "!" && s[i + 1] === "[") {
      flush();
      brackets.push(list.push({ items: null, delim: null, bracket: { image: true, active: true, pos: i + 2 } }));
      i += 2;
      continue;
    }
    if (ch === "[") {
      flush();
      const node = list.push({ items: null, delim: null, bracket: { image: false, active: true, pos: i + 1 } });
      brackets.push(node);
      activeLinks.push(node);
      i++;
      continue;
    }
    if (ch === "]") {
      flush();
      const opener = brackets.pop();
      if (opener && !opener.bracket!.image && activeLinks[activeLinks.length - 1] === opener) {
        activeLinks.pop();
      }
      if (!opener || !opener.bracket!.active || s[i + 1] !== "(") {
        if (opener) opener.items = singleSeq(text(opener.bracket!.image ? "![" : "["), 0), (opener.bracket = null);
        buf += "]";
        i++;
        continue;
      }
      linkScan.lastParen ??= s.lastIndexOf(")");
      const tail = i + 2 <= linkScan.lastParen ? linkTail(s, i + 2, linkScan) : null;
      if (!tail) {
        opener.items = singleSeq(text(opener.bracket!.image ? "![" : "["), 0);
        opener.bracket = null;
        buf += "]";
        i++;
        continue;
      }
      const image = opener.bracket!.image;
      const marker = list.push({ items: null, delim: null, bracket: null });
      processEmphasis(list, opener);
      const innerSeq = concatNodes(list.cut(opener, marker));
      const inner = seqToArray(innerSeq);
      list.remove(marker);
      if (image) {
        const alt = plainText(inner);
        opener.items = singleSeq({ t: "image", target: classifyImage(tail.dest), alt }, 0);
      } else {
        const innerDepth = innerSeq ? innerSeq.depth : 0;
        opener.items = singleSeq({ t: "link", target: classifyLink(tail.dest), c: inner }, innerDepth + 1);
        // No links inside links: earlier `[` openers can't make one any more.
        while (activeLinks.length > 0) {
          const prev = activeLinks.pop()!;
          if (prev.bracket) prev.bracket.active = false;
        }
      }
      opener.bracket = null;
      i = tail.end;
      continue;
    }
    if (ch === "\n") {
      // Two trailing spaces make a hard break; otherwise a soft break stays a newline.
      if (buf.endsWith("  ")) {
        buf = trimTrailingSpaces(buf);
        flush();
        list.push({ items: singleSeq({ t: "br" }, 0), delim: null, bracket: null });
      } else {
        buf = trimTrailingSpaces(buf) + "\n";
      }
      i++;
      while (s[i] === " ") i++;
      continue;
    }
    buf += ch;
    i++;
  }
  flush();
  processEmphasis(list, null);
  const nodes: Node[] = [];
  for (let n = list.head; n; n = n.next) nodes.push(n);
  // Bare URLs only in text that isn't already inside a link.
  return mapText(seqToArray(concatNodes(nodes)), linkify);
}

/** The text of inlines, without markup (image alt text, titles). */
export function plainText(items: Inline[]): string {
  let out = "";
  const stack: { list: Inline[]; idx: number }[] = [{ list: items, idx: 0 }];
  while (stack.length > 0) {
    const top = stack[stack.length - 1]!;
    if (top.idx >= top.list.length) {
      stack.pop();
      continue;
    }
    const item = top.list[top.idx++]!;
    if (item.t === "text" || item.t === "code") out += item.v;
    else if (item.t === "br") out += "\n";
    else if (item.t === "image") out += item.alt;
    else stack.push({ list: item.c, idx: 0 });
  }
  return out;
}
