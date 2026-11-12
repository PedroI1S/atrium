// Settings registry (ui-direction §3 Settings, "Extensible"): a feature registers a section
// with essentials[] and advanced[]. Pure rules, tested in settings-registry.test.ts.

/** A section shows at most this many essentials; the rest go to Advanced. */
export const MAX_ESSENTIALS = 5;

export interface SettingRow<C = unknown> {
  id: string;
  label: string;
  hint?: string;
  /** A hint that depends on where the app runs (it replaces `hint`). */
  hintNow?: () => string;
  /** Advanced rows are grouped under small titles ("Opacity", "Look"). */
  group?: string;
  control: C;
}
export interface SettingsSectionSpec<C = unknown> {
  id: string;
  label: string;
  essentials: SettingRow<C>[];
  advanced: SettingRow<C>[];
}

export function registerSection<C>(spec: SettingsSectionSpec<C>): SettingsSectionSpec<C> {
  if (spec.essentials.length <= MAX_ESSENTIALS) return spec;
  return {
    ...spec,
    essentials: spec.essentials.slice(0, MAX_ESSENTIALS),
    advanced: [...spec.essentials.slice(MAX_ESSENTIALS).map((r) => ({ ...r, group: r.group ?? "More" })), ...spec.advanced],
  };
}

export interface SettingsMatch { section: string; sectionLabel: string; row: string; label: string; advanced: boolean }

/** Searches labels and hints of every section, Advanced included; every word must match. */
export function searchSettings<C>(sections: SettingsSectionSpec<C>[], query: string): SettingsMatch[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return [];
  const out: SettingsMatch[] = [];
  for (const s of sections) {
    for (const [rows, advanced] of [[s.essentials, false], [s.advanced, true]] as const) {
      for (const r of rows) {
        const hay = `${r.label} ${r.hintNow?.() ?? r.hint ?? ""} ${s.label}`.toLowerCase();
        if (words.every((w) => hay.includes(w))) out.push({ section: s.id, sectionLabel: s.label, row: r.id, label: r.label, advanced });
      }
    }
  }
  return out;
}
