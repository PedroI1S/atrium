// Serializable dummy state. Rust is the single writer in the native app.
export type ViewId = "chat" | "editor" | "database";
export type OpacityMode = "background" | "whole";
export interface AppearanceSettings { theme: string; alpha: number; mode: OpacityMode; solid: boolean; blur: boolean }
export interface ViewState { threadId: string; owner: string; draft: string; messages: string[]; scroll: number; content: string }
export interface WindowModel { revision: number; settings: AppearanceSettings; views: Record<ViewId, ViewState> }
export const initialModel = (): WindowModel => ({
  revision: 0,
  settings: { theme: "atrium-calm-dark", alpha: 1, mode: "background", solid: false, blur: true },
  views: Object.fromEntries(["chat", "editor", "database"].map(id => [id, {
    threadId: "t-auth", owner: "main", draft: "", messages: [], scroll: -1,
    content: id === "editor" ? "// session.rs\nfn refresh_token() {\n    // Retry once before asking the user.\n}\n" : "select * from sessions;",
  }])) as unknown as WindowModel["views"],
});
export function opacityTokens(s: AppearanceSettings, supported: boolean, reduce = false) {
  const a = supported && !reduce ? Math.min(1, Math.max(0.3, Number.isFinite(s.alpha) ? s.alpha : 1)) : 1;
  return {
    "--window-alpha-chrome": String(s.mode === "background" ? a : 1),
    "--window-alpha-content": String(s.mode === "background" && !s.solid ? a + (1 - a) * 0.35 : 1),
    // This value is passed to NSWindow, never used as CSS opacity.
    "--window-opacity": String(s.mode === "whole" ? a : 1),
    "--window-blur": s.blur && a < 1 ? "24px" : "0px",
  };
}
export function moveView(model: WindowModel, id: ViewId, owner: string): WindowModel {
  return { ...model, revision: model.revision + 1, views: { ...model.views, [id]: { ...model.views[id], owner } } };
}
