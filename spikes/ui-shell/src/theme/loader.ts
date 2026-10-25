// Theme loader: parse `themes/<name>.toml`, resolve `{references}` through the three
// token layers, validate the required semantic set and WCAG AA contrast, and emit a
// flat map of CSS custom properties. Pure functions; no DOM access (see runtime.ts).
//
// Contract: docs/contracts/m0.md section 4.

import { parse as parseToml } from "smol-toml";
import { type Rgba, contrastRatio, parseColor } from "./color";

/** Semantic tokens every theme must define (contract section 4, layer 2). */
export const REQUIRED_SEMANTIC = [
  // surfaces
  "surface-0", "surface-1", "surface-2", "surface-overlay", "surface-glass",
  // text
  "text", "text-muted", "text-faint", "text-on-accent",
  // lines
  "border", "border-strong", "focus-ring",
  // accent + states
  "accent", "accent-hover", "accent-muted", "danger", "warning", "success", "info",
  // code and diffs
  "code-bg", "diff-add", "diff-del", "diff-add-text", "diff-del-text", "ghost-text",
  // attention states (PLAN 3.4)
  "state-running", "state-needs-you", "state-failed", "state-ready",
  // density and motion
  "space-unit", "radius", "font-ui", "font-code", "text-ui", "text-code", "motion-fast", "motion-normal",
] as const;

export type Appearance = "dark" | "light";
export type Layer = "primitive" | "semantic" | "component";
const LAYERS: readonly Layer[] = ["primitive", "semantic", "component"];

export interface ThemeMeta {
  name: string;
  appearance: Appearance;
  vibrancy: boolean;
}

export interface ThemeFile {
  meta: ThemeMeta;
  primitive: Record<string, string>;
  semantic: Record<string, string>;
  component: Record<string, string>;
}

export type IssueCode =
  | "parse"
  | "meta"
  | "invalid-name"
  | "invalid-value"
  | "missing-reference"
  | "cycle"
  | "missing-token"
  | "name-collision"
  | "unparseable-color"
  | "contrast";

export interface ThemeIssue {
  code: IssueCode;
  message: string;
  /** The token (or pair) the issue is about, when there is one. */
  token?: string;
}

export class ThemeError extends Error {
  readonly issues: ThemeIssue[];
  constructor(issues: ThemeIssue[], source?: string) {
    super(`${source ? `${source}: ` : ""}${issues.map((i) => `[${i.code}] ${i.message}`).join("; ")}`);
    this.name = "ThemeError";
    this.issues = issues;
  }
  has(code: IssueCode): boolean {
    return this.issues.some((i) => i.code === code);
  }
}

export interface ContrastCheck {
  fg: string;
  bg: string;
  ratio: number;
  required: number;
  pass: boolean;
}

export interface ResolvedTheme {
  id: string;
  meta: ThemeMeta;
  /** CSS custom property name (with leading `--`) to resolved value. */
  vars: Record<string, string>;
  /** Resolved semantic tokens by bare name, handy for tests and the UI. */
  semantic: Record<string, string>;
  contrast: ContrastCheck[];
}

const NAME_RE = /^[a-z0-9][a-z0-9-]*$/;
const REF_RE = /\{([^{}]*)\}/g;
export const AA_NORMAL = 4.5;
export const RUNTIME_NAMES = ["window-alpha-chrome", "window-alpha-content", "window-opacity", "window-blur"];
export const SURFACES = ["surface-0", "surface-1", "surface-2", "surface-overlay"];


// ---------------------------------------------------------------------------
// Parsing

function asTable(v: unknown): Record<string, unknown> | null {
  return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
}

/** Parses TOML text into a `ThemeFile`, checking shape (not references or contrast). */
export function parseThemeToml(text: string, source?: string): ThemeFile {
  let raw: Record<string, unknown>;
  try {
    raw = parseToml(text) as Record<string, unknown>;
  } catch (e) {
    throw new ThemeError([{ code: "parse", message: e instanceof Error ? e.message : String(e) }], source);
  }
  return themeFileFromObject(raw, source);
}

/** Same as `parseThemeToml` for an already-parsed object (the VS Code importer uses this). */
export function themeFileFromObject(raw: Record<string, unknown>, source?: string): ThemeFile {
  const issues: ThemeIssue[] = [];
  const meta = asTable(raw.meta);
  const name = meta && typeof meta.name === "string" && meta.name.trim() ? meta.name : null;
  const appearance = meta?.appearance;
  if (!meta) issues.push({ code: "meta", message: "missing [meta] table" });
  else {
    if (!name) issues.push({ code: "meta", message: "meta.name must be a non-empty string", token: "meta.name" });
    if (appearance !== "dark" && appearance !== "light")
      issues.push({ code: "meta", message: 'meta.appearance must be "dark" or "light"', token: "meta.appearance" });
    if (meta.vibrancy !== undefined && typeof meta.vibrancy !== "boolean")
      issues.push({ code: "meta", message: "meta.vibrancy must be a boolean", token: "meta.vibrancy" });
  }
  const layerTables: Record<Layer, Record<string, string>> = { primitive: {}, semantic: {}, component: {} };
  for (const layer of LAYERS) {
    const table = raw[layer] === undefined ? {} : asTable(raw[layer]);
    if (!table) {
      issues.push({ code: "invalid-value", message: `[${layer}] must be a table`, token: layer });
      continue;
    }
    for (const [k, v] of Object.entries(table)) {
      if (!NAME_RE.test(k)) {
        issues.push({ code: "invalid-name", message: `${layer}.${k}: names are lowercase letters, digits and dashes`, token: k });
        continue;
      }
      if (typeof v === "string") layerTables[layer][k] = v;
      else if (typeof v === "number") layerTables[layer][k] = String(v);
      else issues.push({ code: "invalid-value", message: `${layer}.${k} must be a string`, token: k });
    }
  }
  if (issues.length) throw new ThemeError(issues, source);
  return {
    meta: { name: name as string, appearance: appearance as Appearance, vibrancy: meta?.vibrancy === true },
    ...layerTables,
  };
}

// ---------------------------------------------------------------------------
// Resolution

/**
 * Resolves every token. A `{name}` reference is looked up in the token's own layer
 * and the layers below it, lowest layer first (primitive, then semantic, then
 * component). Resolution is memoised; a cycle or an unknown name is an error.
 */
export function resolveLayers(file: ThemeFile): Record<Layer, Record<string, string>> {
  const tables: Record<Layer, Record<string, string>> = {
    primitive: file.primitive,
    semantic: file.semantic,
    component: file.component,
  };
  const memo = new Map<string, string>();
  const issues: ThemeIssue[] = [];
  const failed = new Set<string>();

  const resolve = (layer: Layer, name: string, stack: string[]): string => {
    const key = `${layer}.${name}`;
    const hit = memo.get(key);
    if (hit !== undefined) return hit;
    if (stack.includes(key)) {
      const chain = [...stack.slice(stack.indexOf(key)), key].join(" -> ");
      throw new ThemeError([{ code: "cycle", message: `reference cycle: ${chain}`, token: name }]);
    }
    const value = tables[layer][name] as string;
    const upTo = LAYERS.indexOf(layer);
    const out = value.replace(REF_RE, (_m, refRaw: string) => {
      const ref = refRaw.trim();
      for (const l of LAYERS.slice(0, upTo + 1)) {
        if (Object.hasOwn(tables[l], ref)) return resolve(l, ref, [...stack, key]);
      }
      throw new ThemeError([
        { code: "missing-reference", message: `${key} references {${ref}}, which is not defined in ${LAYERS.slice(0, upTo + 1).join("/")}`, token: name },
      ]);
    });
    memo.set(key, out);
    return out;
  };

  const result: Record<Layer, Record<string, string>> = { primitive: {}, semantic: {}, component: {} };
  for (const layer of LAYERS) {
    for (const name of Object.keys(tables[layer])) {
      try {
        result[layer][name] = resolve(layer, name, []);
      } catch (e) {
        if (!(e instanceof ThemeError)) throw e;
        for (const issue of e.issues) {
          const id = `${issue.code}:${issue.message}`;
          if (!failed.has(id)) {
            failed.add(id);
            issues.push(issue);
          }
        }
      }
    }
  }
  if (issues.length) throw new ThemeError(issues);
  return result;
}

// ---------------------------------------------------------------------------
// Validation

/** The pairs the contract requires to pass WCAG AA (4.5:1). */
export const CONTRAST_PAIRS: readonly (readonly [fg: string, bg: string])[] = [
  ["text", "surface-0"], ["text", "surface-1"], ["text", "surface-2"],
  ["text-muted", "surface-0"], ["text-muted", "surface-1"], ["text-muted", "surface-2"],
  ["text-on-accent", "accent"],
];

export function checkContrast(semantic: Record<string, string>, appearance: Appearance, pairs: readonly (readonly [string, string])[] = CONTRAST_PAIRS): { checks: ContrastCheck[]; issues: ThemeIssue[] } {
  const base: Rgba = appearance === "dark" ? { r: 0, g: 0, b: 0, a: 1 } : { r: 255, g: 255, b: 255, a: 1 };
  const checks: ContrastCheck[] = [];
  const issues: ThemeIssue[] = [];
  const unparseable = new Set<string>();
  for (const [fgName, bgName] of pairs) {
    const fgRaw = semantic[fgName];
    const bgRaw = semantic[bgName];
    if (fgRaw === undefined || bgRaw === undefined) continue; // reported as missing-token
    const fg = parseColor(fgRaw);
    const bg = parseColor(bgRaw);
    for (const [n, c, raw] of [[fgName, fg, fgRaw], [bgName, bg, bgRaw]] as const) {
      if (!c && !unparseable.has(n)) {
        unparseable.add(n);
        issues.push({ code: "unparseable-color", message: `--${n} = "${raw}" is not a colour the contrast check can read (use #hex or rgb())`, token: n });
      }
    }
    if (!fg || !bg) continue;
    const ratio = contrastRatio(fg, bg, base);
    const pass = ratio >= AA_NORMAL;
    checks.push({ fg: fgName, bg: bgName, ratio, required: AA_NORMAL, pass });
    if (!pass)
      issues.push({ code: "contrast", message: `--${fgName} on --${bgName} is ${ratio.toFixed(2)}:1, needs ${AA_NORMAL}:1`, token: `${fgName}/${bgName}` });
  }
  return { checks, issues };
}

/** Parse, resolve and validate in one go. Throws `ThemeError` with every issue found. */
export function loadTheme(text: string, id: string): ResolvedTheme {
  return resolveTheme(parseThemeToml(text, id), id);
}

/**
 * Every layer is emitted into one CSS namespace: primitives as `--p-<name>`, semantic and component
 * tokens as `--<name>`. A component (or semantic) token that reuses an emitted name would silently
 * overwrite a validated variable (e.g. `[component] text = ...` replacing `--text` after the contrast
 * check), so any such collision is an error.
 */
export function nameCollisions(file: ThemeFile): ThemeIssue[] {
  const issues: ThemeIssue[] = [];
  const emitted = new Map<string, string>([...RUNTIME_NAMES, ...SURFACES.map(n => `${n}-rgb`)].map(n => [`--${n}`, "reserved runtime/derived token"])); // css var -> "layer.name"
  for (const [k] of Object.entries(file.primitive)) {
    if (RUNTIME_NAMES.includes(k)) issues.push({ code: "name-collision", token: k, message: `primitive.${k} is a reserved runtime token` });
    emitted.set(`--p-${k}`, `primitive.${k}`);
  }
  for (const layer of ["semantic", "component"] as const) {
    for (const k of Object.keys(file[layer])) {
      const css = `--${k}`;
      const prior = emitted.get(css);
      if (prior)
        issues.push({ code: "name-collision", message: `${layer}.${k} would overwrite ${prior} (both emit ${css})`, token: k });
      else emitted.set(css, `${layer}.${k}`);
    }
  }
  return issues;
}

export function resolveTheme(file: ThemeFile, id: string): ResolvedTheme {
  const issues: ThemeIssue[] = [];
  // Required tokens are checked against what the file defines, so a missing token is reported
  // even when other tokens reference it (which also fails resolution).
  for (const req of REQUIRED_SEMANTIC)
    if (!Object.hasOwn(file.semantic, req)) issues.push({ code: "missing-token", message: `required semantic token --${req} is missing`, token: req });
  issues.push(...nameCollisions(file));
  let layers: Record<Layer, Record<string, string>> | null = null;
  try {
    layers = resolveLayers(file);
  } catch (e) {
    if (!(e instanceof ThemeError)) throw e;
    issues.push(...e.issues);
  }
  let checks: ContrastCheck[] = [];
  if (layers) {
    const c = checkContrast(layers.semantic, file.meta.appearance);
    checks = c.checks;
    issues.push(...c.issues);
  }
  if (issues.length || !layers) throw new ThemeError(issues, id);

  const vars: Record<string, string> = {};
  for (const [k, v] of Object.entries(layers.primitive)) vars[`--p-${k}`] = v;
  for (const [k, v] of Object.entries(layers.semantic)) vars[`--${k}`] = v;
  for (const [k, v] of Object.entries(layers.component)) vars[`--${k}`] = v;
  for (const name of SURFACES) {
    const raw = layers.semantic[name]!;
    const color = parseColor(raw);
    if (!/^#[0-9a-f]{6}$/i.test(raw) || !color || color.a !== 1)
      throw new ThemeError([{ code: "invalid-value", token: name, message: `${name} must be opaque six-digit hex for surface channels` }], id);
    vars[`--${name}-rgb`] = `${color.r} ${color.g} ${color.b}`;
  }
  // CR-02: state colours are text, including in imported themes.
  {
    const extra = checkContrast({ ...layers.semantic }, file.meta.appearance,
      ["state-running", "state-needs-you", "state-failed", "state-ready"].flatMap(fg =>
        ["surface-0", "surface-1", "surface-2"].map(bg => [fg, bg] as const)));
    if (extra.issues.length) throw new ThemeError(extra.issues, id);
    checks.push(...extra.checks);
  }
  return { id, meta: file.meta, vars, semantic: layers.semantic, contrast: checks };
}
