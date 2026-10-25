// Applies a resolved theme to the document. Switching replaces the custom
// properties in place: no reload, no stylesheet swap.

import type { ResolvedTheme } from "./loader";

let applied: string[] = [];

/**
 * `vibrancy` is whether native vibrancy will actually be applied (requested by the theme AND
 * supported). The theme's request is kept in `data-vibrancy-requested`, so CSS can paint an opaque
 * fallback when a theme asks for glass on a platform that cannot provide it.
 */
export function applyTheme(theme: ResolvedTheme, vibrancy: boolean, root: HTMLElement = document.documentElement): void {
  const style = root.style;
  // One frame without transitions so a theme swap is a cut, not a 200 ms smear.
  root.setAttribute("data-theme-switching", "");
  const next = Object.keys(theme.vars);
  const keep = new Set(next);
  for (const name of applied) if (!keep.has(name)) style.removeProperty(name);
  for (const name of next) style.setProperty(name, theme.vars[name] as string);
  applied = next;
  root.setAttribute("data-theme", theme.id);
  root.setAttribute("data-appearance", theme.meta.appearance);
  root.setAttribute("data-vibrancy-requested", String(theme.meta.vibrancy));
  root.setAttribute("data-vibrancy", String(vibrancy));
  style.setProperty("color-scheme", theme.meta.appearance);
  requestAnimationFrame(() => requestAnimationFrame(() => root.removeAttribute("data-theme-switching")));
}
