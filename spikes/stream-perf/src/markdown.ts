import { Marked } from "marked";

const md = new Marked({ gfm: true, breaks: false, async: false });

const escapeHtml = (s: string): string => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

// Raw HTML inside agent text is shown as text, never injected. (M1 also needs a link-scheme
// allow-list; the spike renders links without following them.)
md.use({
  renderer: {
    html({ text }: { text: string }): string {
      return escapeHtml(text);
    },
  },
});

/** Parses one block (or the open tail). Synchronous and pure. */
export function renderBlock(text: string): string {
  return md.parse(text) as string;
}

/**
 * The open block is re-rendered on every frame. An unfinished code fence is the one case that can
 * grow large, so it skips the parser and is shown as plain preformatted text.
 */
export function renderOpen(text: string, fenceOpen: boolean): string {
  if (fenceOpen) return "<pre><code>" + escapeHtml(text.slice(text.indexOf("\n") + 1)) + "</code></pre>";
  return md.parse(text) as string;
}
