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

// ---- blocks --------------------------------------------------------------------------------

const FENCE_OPEN = /^( {0,3})(`{3,}|~{3,})(.*)$/;
const ATX = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/;
const HR = /^ {0,3}([-*_])(?:[ \t]*\1){2,}[ \t]*$/;
const QUOTE = /^ {0,3}> ?/;
const BULLET = /^( {0,3})([-*+])( {1,4}|\t|$)/;
const ORDERED = /^( {0,3})(\d{1,9})([.)])( {1,4}|\t|$)/;
const SETEXT = /^ {0,3}(=+|-+)[ \t]*$/;
const TABLE_DELIM = /^ {0,3}\|?[ \t]*:?-+:?[ \t]*(\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*$/;
const BLANK = /^[ \t]*$/;

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
  return FENCE_OPEN.test(line) || ATX.test(line) || HR.test(line) || QUOTE.test(line) || listMarker(line) !== null;
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

    const atx = ATX.exec(line);
    if (atx) {
      out.push({ t: "h", level: atx[1]!.length, c: parseInline(atx[2] ?? "") });
      i++;
      continue;
    }
    if (HR.test(line)) { out.push({ t: "hr" }); i++; continue; }

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

    if (line.includes("|") && i + 1 < lines.length && TABLE_DELIM.test(lines[i + 1]!) && lines[i + 1]!.includes("-")) {
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

/** A node of the inline list while parsing: text, finished inlines and delimiter runs. */
interface Node {
  prev: Node | null;
  next: Node | null;
  inline: Inline | null;
  /** Delimiter run (`*`, `_`, `~`) or bracket (`[`, `![`); null for finished content. */
  delim: { char: string; count: number; orig: number; canOpen: boolean; canClose: boolean } | null;
  bracket: { image: boolean; active: boolean; pos: number } | null;
}

class List {
  head: Node | null = null;
  tail: Node | null = null;
  push(n: Omit<Node, "prev" | "next">): Node {
    const node: Node = { ...n, prev: this.tail, next: null };
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
  /** Removes the nodes strictly between `a` and `b` and returns their inlines, in order. */
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

/** Collapses a node run into inlines (unmatched delimiters and brackets become text). */
function inlinesOf(nodes: Node[]): Inline[] {
  const out: Inline[] = [];
  for (const n of nodes) {
    const item: Inline | null = n.inline ?? (n.delim ? text(n.delim.char.repeat(n.delim.count)) : n.bracket ? text(n.bracket.image ? "![" : "[") : null);
    if (!item) continue;
    const last = out[out.length - 1];
    if (item.t === "text" && last?.t === "text") last.v += item.v;
    else out.push(item.t === "text" ? { ...item } : item);
  }
  return out;
}

/** Link destination and optional title after `](`. Returns the end index (after `)`) or -1. */
function linkTail(s: string, at: number): { dest: string; end: number } | null {
  let i = at;
  while (i < s.length && (s[i] === " " || s[i] === "\n")) i++;
  let dest = "";
  if (s[i] === "<") {
    const close = s.indexOf(">", i + 1);
    if (close < 0 || s.slice(i + 1, close).includes("\n")) return null;
    dest = s.slice(i + 1, close);
    i = close + 1;
  } else {
    let depth = 0;
    const startDest = i;
    while (i < s.length) {
      const ch = s[i]!;
      if (ch === "\\" && i + 1 < s.length && PUNCT.test(s[i + 1]!)) { i += 2; continue; }
      if (ch === "(") depth++;
      else if (ch === ")") { if (depth === 0) break; depth--; }
      else if (WS.test(ch) || ch < " ") break;
      i++;
    }
    dest = s.slice(startDest, i).replace(/\\([!-/:-@[-`{-~])/g, "$1");
  }
  while (i < s.length && (s[i] === " " || s[i] === "\n")) i++;
  // An optional title, which is not shown (the URL itself is, on hover).
  const q = s[i];
  if (q === '"' || q === "'" || q === "(") {
    const closer = q === "(" ? ")" : q;
    let j = i + 1;
    while (j < s.length && s[j] !== closer) j += s[j] === "\\" ? 2 : 1;
    if (j >= s.length) return null;
    i = j + 1;
    while (i < s.length && (s[i] === " " || s[i] === "\n")) i++;
  }
  if (s[i] !== ")") return null;
  return { dest, end: i + 1 };
}

const AUTOLINK = /^<([a-zA-Z][a-zA-Z0-9+.-]{1,31}:[^\s<>]*)>/;
const EMAIL = /^<([a-zA-Z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?(?:\.[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?)*)>/;
const BARE_URL = /https?:\/\/[^\s<>"]+/g;

/** Splits bare http(s) URLs out of a text inline (GFM's extended autolinks, http only). */
function linkify(items: Inline[]): Inline[] {
  const out: Inline[] = [];
  for (const item of items) {
    if (item.t !== "text" || !item.v.includes("http")) { out.push(item); continue; }
    let last = 0;
    for (const m of item.v.matchAll(BARE_URL)) {
      let url = m[0];
      // Trailing punctuation belongs to the sentence; an unbalanced `)` too.
      while (/[.,:;!?'*_~]$/.test(url) || (url.endsWith(")") && (url.match(/\(/g)?.length ?? 0) < (url.match(/\)/g)?.length ?? 0))) url = url.slice(0, -1);
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

function flanking(s: string, start: number, end: number): { left: boolean; right: boolean } {
  const before = start > 0 ? s[start - 1]! : " ";
  const after = end < s.length ? s[end]! : " ";
  const ws = (c: string) => WS.test(c);
  const p = (c: string) => PUNCTUATION.test(c);
  const left = !ws(after) && (!p(after) || ws(before) || p(before));
  const right = !ws(before) && (!p(before) || ws(after) || p(after));
  return { left, right };
}

/** Matches emphasis delimiters after `bottom` (CommonMark's "process emphasis", simplified). */
function processEmphasis(list: List, bottom: Node | null): void {
  const openersBottom = new Map<string, Node | null>();
  let closer: Node | null = bottom ? bottom.next : list.head;
  while (closer) {
    const cd = closer.delim;
    if (!cd || !cd.canClose) { closer = closer.next; continue; }
    const key = `${cd.char}${cd.canOpen ? 1 : 0}${cd.orig % 3}`;
    const floor = openersBottom.get(key) ?? bottom;
    let opener = closer.prev;
    let found: Node | null = null;
    while (opener && opener !== floor && opener !== bottom) {
      const od = opener.delim;
      if (od && od.char === cd.char && od.canOpen) {
        const odd = (od.canClose || cd.canOpen) && (od.orig + cd.orig) % 3 === 0 && !(od.orig % 3 === 0 && cd.orig % 3 === 0);
        const tildeOk = cd.char !== "~" || (od.count === 2 && cd.count === 2);
        if (!odd && tildeOk) { found = opener; break; }
      }
      opener = opener.prev;
    }
    if (!found) {
      openersBottom.set(key, closer.prev);
      if (!cd.canOpen) closer.delim = null, (closer.inline = text(cd.char.repeat(cd.count)));
      closer = closer.next;
      continue;
    }
    const od = found.delim!;
    const use = cd.char === "~" ? 2 : od.count >= 2 && cd.count >= 2 ? 2 : 1;
    const inner = inlinesOf(list.cut(found, closer));
    const kind = cd.char === "~" ? "del" : use === 2 ? "strong" : "em";
    od.count -= use;
    cd.count -= use;
    // The emphasis node goes right after the opener.
    const node: Node = { prev: found, next: closer, inline: { t: kind, c: inner }, delim: null, bracket: null };
    found.next = node;
    closer.prev = node;
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
      n.inline = text(n.delim.char.repeat(n.delim.count));
      n.delim = null;
    }
    n = n.next;
  }
}

/** Parses inline markdown. Linear apart from emphasis matching, which is bounded per run. */
export function parseInline(s: string): Inline[] {
  const list = new List();
  let buf = "";
  const flush = () => {
    if (buf) list.push({ inline: text(buf), delim: null, bracket: null });
    buf = "";
  };
  let i = 0;
  while (i < s.length) {
    const ch = s[i]!;
    if (ch === "\\") {
      const next = s[i + 1];
      if (next === "\n") { flush(); list.push({ inline: { t: "br" }, delim: null, bracket: null }); i += 2; continue; }
      if (next !== undefined && PUNCT.test(next)) { buf += next; i += 2; continue; }
      buf += ch;
      i++;
      continue;
    }
    if (ch === "`") {
      let n = 1;
      while (s[i + n] === "`") n++;
      const run = "`".repeat(n);
      let j = i + n;
      let close = -1;
      while (j < s.length) {
        const k = s.indexOf(run, j);
        if (k < 0) break;
        let m = n;
        while (s[k + m] === "`") m++;
        if (m === n) { close = k; break; }
        j = k + m;
      }
      if (close < 0) { buf += run; i += n; continue; }
      let code = s.slice(i + n, close).replace(/\n/g, " ");
      if (code.length > 2 && code.startsWith(" ") && code.endsWith(" ") && code.trim()) code = code.slice(1, -1);
      flush();
      list.push({ inline: { t: "code", v: code }, delim: null, bracket: null });
      i = close + n;
      continue;
    }
    if (ch === "<") {
      const auto = AUTOLINK.exec(s.slice(i, i + 4096));
      if (auto) {
        flush();
        list.push({ inline: { t: "link", target: classifyLink(auto[1]!), c: [text(auto[1]!)] }, delim: null, bracket: null });
        i += auto[0].length;
        continue;
      }
      const mail = EMAIL.exec(s.slice(i, i + 512));
      if (mail) {
        flush();
        list.push({ inline: { t: "link", target: classifyLink(`mailto:${mail[1]!}`), c: [text(mail[1]!)] }, delim: null, bracket: null });
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
        canOpen = left && (!right || PUNCTUATION.test(before));
        canClose = right && (!left || PUNCTUATION.test(after));
      }
      if (ch === "~" && n !== 2) { buf += ch.repeat(n); i += n; continue; }
      flush();
      list.push({ inline: null, delim: { char: ch, count: n, orig: n, canOpen, canClose }, bracket: null });
      i += n;
      continue;
    }
    if (ch === "!" && s[i + 1] === "[") {
      flush();
      list.push({ inline: null, delim: null, bracket: { image: true, active: true, pos: i + 2 } });
      i += 2;
      continue;
    }
    if (ch === "[") {
      flush();
      list.push({ inline: null, delim: null, bracket: { image: false, active: true, pos: i + 1 } });
      i++;
      continue;
    }
    if (ch === "]") {
      flush();
      // The nearest opening bracket.
      let opener = list.tail;
      while (opener && !opener.bracket) opener = opener.prev;
      if (!opener || !opener.bracket!.active || s[i + 1] !== "(") {
        if (opener) opener.inline = text(opener.bracket!.image ? "![" : "["), (opener.bracket = null);
        buf += "]";
        i++;
        continue;
      }
      const tail = linkTail(s, i + 2);
      if (!tail) {
        opener.inline = text(opener.bracket!.image ? "![" : "[");
        opener.bracket = null;
        buf += "]";
        i++;
        continue;
      }
      const image = opener.bracket!.image;
      const marker = list.push({ inline: null, delim: null, bracket: null });
      processEmphasis(list, opener);
      const inner = inlinesOf(list.cut(opener, marker));
      list.remove(marker);
      if (image) {
        const alt = plainText(inner);
        opener.inline = { t: "image", target: classifyImage(tail.dest), alt };
      } else {
        opener.inline = { t: "link", target: classifyLink(tail.dest), c: inner };
        // No links inside links: earlier `[` openers can't make one any more.
        for (let n = opener.prev; n; n = n.prev) if (n.bracket && !n.bracket.image) n.bracket.active = false;
      }
      opener.bracket = null;
      i = tail.end;
      continue;
    }
    if (ch === "\n") {
      // Two trailing spaces make a hard break; otherwise a soft break stays a newline.
      if (buf.endsWith("  ")) {
        buf = buf.replace(/ +$/, "");
        flush();
        list.push({ inline: { t: "br" }, delim: null, bracket: null });
      } else {
        buf = buf.replace(/ +$/, "") + "\n";
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
  return mapText(inlinesOf(nodes), linkify);
}

/** The text of inlines, without markup (image alt text, titles). */
export function plainText(items: Inline[]): string {
  let out = "";
  for (const item of items) {
    if (item.t === "text" || item.t === "code") out += item.v;
    else if (item.t === "br") out += "\n";
    else if (item.t === "image") out += item.alt;
    else out += plainText(item.c);
  }
  return out;
}
