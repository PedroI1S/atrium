// The set of themes shipped in the shell: `themes/*.toml` (bundled at build time by Vite's
// glob import) plus one VS Code theme converted by the importer. All go through the same
// loader and the same validation.

import catppuccinMocha from "../../themes/vscode/catppuccin-mocha.json";
import { type ResolvedTheme, ThemeError, loadTheme, resolveTheme } from "./loader";
import { type ImportResult, importVscodeTheme } from "./vscode";

export interface Direction {
  id: "atrium" | "glass" | "flat" | "paper" | "imported";
  name: string;
  blurb: string;
}

export const DIRECTIONS: readonly Direction[] = [
  { id: "atrium", name: "Calm", blurb: "Floating spaces, neutral ink" },
  { id: "glass", name: "Glass", blurb: "Native vibrancy, floating rounded panels, indigo" },
  { id: "flat", name: "Flat", blurb: "Opaque, dense hairline grid, mono labels, emerald" },
  { id: "paper", name: "Paper", blurb: "Warm editorial, serif conversation, terracotta" },
  { id: "imported", name: "Imported", blurb: "VS Code theme mapped onto tokens" },
];

export interface ThemeEntry {
  id: string;
  direction: Direction["id"];
  theme: ResolvedTheme;
}

const raw = import.meta.glob("../../themes/*.toml", { query: "?raw", import: "default", eager: true }) as Record<string, string>;

export const importedMocha: ImportResult = importVscodeTheme(catppuccinMocha, { name: "Catppuccin Mocha (VS Code import)" });

export function loadAllThemes(): { entries: ThemeEntry[]; errors: ThemeError[] } {
  const entries: ThemeEntry[] = [];
  const errors: ThemeError[] = [];
  for (const [path, text] of Object.entries(raw).sort(([a], [b]) => a.localeCompare(b))) {
    const id = path.replace(/^.*\//, "").replace(/\.toml$/, "");
    const direction = id.split("-")[0] as Direction["id"];
    try {
      entries.push({ id, direction, theme: loadTheme(text, id) });
    } catch (e) {
      if (e instanceof ThemeError) errors.push(e);
      else throw e;
    }
  }
  try {
    entries.push({ id: "imported-catppuccin-mocha", direction: "imported", theme: resolveTheme(importedMocha.file, "imported-catppuccin-mocha") });
  } catch (e) {
    if (e instanceof ThemeError) errors.push(e);
    else throw e;
  }
  return { entries, errors };
}
