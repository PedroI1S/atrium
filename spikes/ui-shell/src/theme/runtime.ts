// Applies a resolved theme to the document. Switching replaces the custom
// properties in place: no reload, no stylesheet swap.

import type { ResolvedTheme } from "./loader";

let applied: string[] = [];

export function applyTheme(theme: ResolvedTheme, root: HTMLElement = document.documentElement): void {
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
  root.setAttribute("data-vibrancy", String(theme.meta.vibrancy));
  style.setProperty("color-scheme", theme.meta.appearance);
  requestAnimationFrame(() => requestAnimationFrame(() => root.removeAttribute("data-theme-switching")));
}
