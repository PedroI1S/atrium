// The markdown tree to DOM nodes (contract §12). Every string from the agent goes into the DOM
// as a text node or as a checked attribute value; nothing is ever parsed as HTML. The only
// attributes set from agent text are `href`/`title` of web links (after links.ts and with
// Rust checking again on click), the `src` of a checked data image or of a clicked https
// image, and `alt`/`title` strings.

import type { Block, Inline } from "./markdown";
import type { ImageTarget, LinkTarget } from "./links";

/** What a click on agent content may ask the app to do. */
export interface LinkActions {
  /** http, https, mailto: Rust opens it in the system browser after checking the scheme. */
  open(url: string): void;
  /** A `file:` path: Rust resolves it and reveals it when it is inside the thread's folder. */
  reveal(path: string): void;
}

type Doc = Pick<Document, "createElement" | "createTextNode" | "createDocumentFragment">;

function el<K extends keyof HTMLElementTagNameMap>(doc: Doc, tag: K, cls?: string): HTMLElementTagNameMap[K] {
  const node = doc.createElement(tag);
  if (cls) node.className = cls;
  return node;
}

function textOf(items: Inline[]): string {
  let out = "";
  for (const item of items) {
    if (item.t === "text" || item.t === "code") out += item.v;
    else if (item.t === "br") out += "\n";
    else if (item.t === "image") out += item.alt;
    else out += textOf(item.c);
  }
  return out;
}

function link(doc: Doc, target: LinkTarget, children: Inline[], actions: LinkActions): Node {
  if (target.kind === "none") {
    // Plain text; the address is in the tooltip so nothing is hidden from the reader.
    const span = el(doc, "span", "md-nolink");
    span.title = `Not opened by Atrium: ${target.url}`;
    span.append(...inlines(doc, children, actions));
    return span;
  }
  const a = el(doc, "a", target.kind === "file" ? "md-link md-file" : "md-link");
  a.append(...inlines(doc, children, actions));
  if (target.kind === "web") {
    a.href = target.url;
    a.title = target.url;
    a.rel = "noreferrer noopener";
    a.addEventListener("click", (e) => {
      e.preventDefault();
      actions.open(target.url);
    });
  } else {
    // No href: the webview never navigates to it. A click asks Rust to reveal it.
    a.title = `Show in the file manager: ${target.path}`;
    a.tabIndex = 0;
    a.setAttribute("role", "link");
    const reveal = (e: Event) => {
      e.preventDefault();
      actions.reveal(target.path);
    };
    a.addEventListener("click", reveal);
    a.addEventListener("keydown", (e) => {
      if ((e as KeyboardEvent).key === "Enter") reveal(e);
    });
  }
  return a;
}

function image(doc: Doc, target: ImageTarget, alt: string): Node {
  if (target.kind === "data") {
    const img = el(doc, "img", "md-img");
    img.alt = alt;
    img.decoding = "async";
    img.src = target.url;
    return img;
  }
  if (target.kind === "remote") {
    // Remote images load only when clicked (privacy): no referrer, no cookies.
    const button = el(doc, "button", "md-img-load");
    button.type = "button";
    button.title = target.url;
    button.textContent = `Load image${alt ? `: ${alt}` : ""} (${new URL(target.url).hostname})`;
    button.addEventListener("click", () => {
      const img = el(doc, "img", "md-img");
      img.alt = alt;
      img.referrerPolicy = "no-referrer";
      img.crossOrigin = "anonymous";
      img.title = target.url;
      img.addEventListener("error", () => {
        const note = el(doc, "span", "md-img-refused");
        note.textContent = `The image didn't load: ${target.url}`;
        img.replaceWith(note);
      });
      img.src = target.url;
      button.replaceWith(img);
    });
    return button;
  }
  const span = el(doc, "span", "md-img-refused");
  span.title = target.url;
  span.textContent = `[image${alt ? `: ${alt}` : ""}: ${target.reason}]`;
  return span;
}

function inlines(doc: Doc, items: Inline[], actions: LinkActions): Node[] {
  const out: Node[] = [];
  for (const item of items) {
    switch (item.t) {
      case "text":
        out.push(doc.createTextNode(item.v));
        break;
      case "code": {
        const code = el(doc, "code");
        code.textContent = item.v;
        out.push(code);
        break;
      }
      case "em":
      case "strong":
      case "del": {
        const node = el(doc, item.t);
        node.append(...inlines(doc, item.c, actions));
        out.push(node);
        break;
      }
      case "br":
        out.push(el(doc, "br"));
        break;
      case "link":
        out.push(link(doc, item.target, item.c, actions));
        break;
      case "image":
        out.push(image(doc, item.target, item.alt));
        break;
    }
  }
  return out;
}

const ALIGN_CLASS = { left: "al-l", center: "al-c", right: "al-r" } as const;

function block(doc: Doc, b: Block, actions: LinkActions): Node {
  switch (b.t) {
    case "p": {
      const p = el(doc, "p");
      p.append(...inlines(doc, b.c, actions));
      return p;
    }
    case "h": {
      const h = el(doc, `h${Math.min(6, Math.max(1, b.level))}` as "h1");
      h.append(...inlines(doc, b.c, actions));
      return h;
    }
    case "code": {
      const pre = el(doc, "pre", "md-code");
      if (b.lang) pre.dataset.lang = b.lang.slice(0, 32);
      const code = el(doc, "code");
      code.textContent = b.v;
      pre.append(code);
      return pre;
    }
    case "quote": {
      const q = el(doc, "blockquote");
      for (const c of b.c) q.append(block(doc, c, actions));
      return q;
    }
    case "list": {
      const list = el(doc, b.ordered ? "ol" : "ul");
      if (b.ordered && b.start !== 1) (list as HTMLOListElement).start = b.start;
      for (const item of b.items) {
        const li = el(doc, "li");
        // A one-paragraph item is shown without the paragraph's margins (a tight item).
        if (item.length === 1 && item[0]!.t === "p") li.append(...inlines(doc, item[0]!.c, actions));
        else for (const c of item) li.append(block(doc, c, actions));
        list.append(li);
      }
      return list;
    }
    case "hr":
      return el(doc, "hr");
    case "table": {
      const wrap = el(doc, "div", "md-table");
      const table = el(doc, "table");
      const head = el(doc, "thead");
      const tr = el(doc, "tr");
      b.head.forEach((cell, k) => {
        const th = el(doc, "th", b.align[k] ? ALIGN_CLASS[b.align[k]!] : undefined);
        th.append(...inlines(doc, cell, actions));
        tr.append(th);
      });
      head.append(tr);
      const body = el(doc, "tbody");
      for (const row of b.rows) {
        const r = el(doc, "tr");
        row.forEach((cell, k) => {
          const td = el(doc, "td", b.align[k] ? ALIGN_CLASS[b.align[k]!] : undefined);
          td.append(...inlines(doc, cell, actions));
          r.append(td);
        });
        body.append(r);
      }
      table.append(head, body);
      wrap.append(table);
      return wrap;
    }
  }
}

/** Renders blocks into a fragment. */
export function renderBlocks(doc: Doc, blocks: Block[], actions: LinkActions): DocumentFragment {
  const frag = doc.createDocumentFragment();
  for (const b of blocks) frag.append(block(doc, b, actions));
  return frag;
}

/** Plain text of a block list, for copying and for measuring. */
export function blocksText(blocks: Block[]): string {
  return blocks
    .map((b) => {
      switch (b.t) {
        case "p":
        case "h":
          return textOf(b.c);
        case "code":
          return b.v;
        case "quote":
          return blocksText(b.c);
        case "list":
          return b.items.map((i) => blocksText(i)).join("\n");
        case "hr":
          return "";
        case "table":
          return [b.head, ...b.rows].map((r) => r.map(textOf).join("\t")).join("\n");
      }
    })
    .join("\n\n");
}
