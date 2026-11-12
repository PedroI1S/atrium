// The app's themes: Atrium Calm light and dark only, parsed once and cached. Kept apart from
// registry.ts so startup never evaluates the M0-06 comparison themes or the VS Code importer
// (review #5: themes were loaded twice, all eight of them plus an import, before first render).

import type { CalmTheme } from "../window-model";
import { type ResolvedTheme, loadTheme } from "./loader";

const raw = import.meta.glob("../../themes/atrium-calm-*.toml", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const cache = new Map<CalmTheme, ResolvedTheme>();

export function calmTheme(id: CalmTheme): ResolvedTheme {
  let theme = cache.get(id);
  if (!theme) {
    const text = raw[`../../themes/${id}.toml`];
    if (text === undefined) throw new Error(`Unknown theme ${id}`);
    theme = loadTheme(text, id);
    cache.set(id, theme);
  }
  return theme;
}
