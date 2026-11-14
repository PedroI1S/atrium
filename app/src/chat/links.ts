// What a URL in agent text may become (contract §12). Pure; the renderer and the Rust side act
// on the result, and Rust checks again (src-tauri/src/links.rs).
//
// - Links: http, https and mailto open in the system browser after one click. `file:` links
//   name a path that Rust resolves (symlinks included) and reveals in the file manager when it
//   is inside the thread's folder; never opened or run. Anything else is plain text.
// - Images: `data:` png, jpeg, gif or webp up to MAX_INLINE_IMAGE_BYTES render. SVG never
//   renders inline. Remote (https) images load only after a click, without a referrer or
//   cookies. Anything else is shown as text.

import { MAX_INLINE_IMAGE_BYTES } from "./limits";

export type LinkTarget =
  | { kind: "web"; url: string }
  | { kind: "file"; path: string }
  /** Not a link Atrium opens: shown as text. */
  | { kind: "none"; url: string };

export type ImageTarget =
  | { kind: "data"; url: string; bytes: number }
  /** https only: the CSP allows no other remote images. */
  | { kind: "remote"; url: string }
  | { kind: "refused"; url: string; reason: string };

/** Longer than any link a person would click (`links::MAX_LINK_BYTES`). */
export const MAX_LINK_CHARS = 8 * 1024;
const RASTER = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const BASE64_GROUP = 4;
const BASE64_BYTES = 3;

function parse(url: string): URL | null {
  try {
    return new URL(url);
  } catch {
    return null;
  }
}

/** Control characters and whitespace never belong in a link we act on. */
const UNSAFE_CHARS = /[\u0000-\u0020\u007f-\u009f\u2028\u2029]/;

export function classifyLink(raw: string): LinkTarget {
  const url = raw.trim();
  if (url.length > MAX_LINK_CHARS || UNSAFE_CHARS.test(url)) return { kind: "none", url };
  const parsed = parse(url);
  if (!parsed) return { kind: "none", url };
  switch (parsed.protocol) {
    case "http:":
    case "https:":
      return parsed.hostname ? { kind: "web", url: parsed.href } : { kind: "none", url };
    case "mailto:":
      return { kind: "web", url: parsed.href };
    case "file:": {
      if (!/^file:\/\//i.test(url)) return { kind: "none", url };
      // Only local files: `file:///path` or `file://localhost/path`.
      if (parsed.hostname && parsed.hostname !== "localhost") return { kind: "none", url };
      let path: string;
      try {
        path = decodeURIComponent(parsed.pathname);
      } catch {
        return { kind: "none", url };
      }
      if (!path.startsWith("/") || path.includes("\0")) return { kind: "none", url };
      return { kind: "file", path };
    }
    default:
      return { kind: "none", url };
  }
}

const DATA_URL = /^data:([a-z0-9.+-]+\/[a-z0-9.+-]+)((?:;[a-z0-9-]+=[^;,]*)*)(;base64)?,(.*)$/is;

export function classifyImage(raw: string): ImageTarget {
  const url = raw.trim();
  if (/^data:/i.test(url)) {
    const m = DATA_URL.exec(url);
    if (!m) return { kind: "refused", url: "data:…", reason: "not a valid data image" };
    const type = m[1]!.toLowerCase();
    if (type === "image/svg+xml") return { kind: "refused", url: "data:image/svg+xml…", reason: "SVG images are not shown" };
    if (!RASTER.has(type)) return { kind: "refused", url: `data:${type}…`, reason: `${type} is not shown` };
    if (!m[3]) return { kind: "refused", url: `data:${type}…`, reason: "only base64 images are shown" };
    const payload = m[4]!.replace(/\s/g, "");
    if (!/^[A-Za-z0-9+/]*={0,2}$/.test(payload)) return { kind: "refused", url: `data:${type}…`, reason: "not a valid data image" };
    const bytes = Math.floor((payload.length / BASE64_GROUP) * BASE64_BYTES) - (payload.endsWith("==") ? 2 : payload.endsWith("=") ? 1 : 0);
    if (bytes > MAX_INLINE_IMAGE_BYTES) return { kind: "refused", url: `data:${type}…`, reason: "the image is too large to show inline" };
    return { kind: "data", url: `data:${type};base64,${payload}`, bytes };
  }
  if (url.length > MAX_LINK_CHARS || UNSAFE_CHARS.test(url)) return { kind: "refused", url: url.slice(0, MAX_LINK_CHARS), reason: "not an image address" };
  const parsed = parse(url);
  if (parsed?.protocol === "https:" && parsed.hostname) {
    if (/\.svgz?$/i.test(parsed.pathname)) return { kind: "refused", url: parsed.href, reason: "SVG images are not shown" };
    return { kind: "remote", url: parsed.href };
  }
  return { kind: "refused", url, reason: "only https and inline images are shown" };
}
