//! Spike-only shared window state (no daemon contract). Rust is the single writer: every
//! window sends patches here, and every accepted change is broadcast to all windows with a
//! new revision. The pure functions at the top hold the rules and are unit-tested; the
//! Tauri commands below them only lock, call them and publish.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const MAIN: &str = "main";
pub const VIEW_IDS: [&str; 3] = ["chat", "editor", "database"];
const THEMES: [&str; 3] = ["atrium-calm-light", "atrium-calm-dark", "system"];
const MODES: [&str; 2] = ["background", "whole"];
const POPOUTS: [&str; 2] = ["same", "opaque"];
/// Lower bound of the opacity slider (ui-direction §5: keeps text readable).
pub const MIN_ALPHA: f64 = 0.3;
/// Quiet period before settings are written to disk; a slider drag writes once.
const PERSIST_DEBOUNCE: Duration = Duration::from_millis(250);
pub const SETTINGS_FILE: &str = "calm-settings.json";

/// Pop-out geometry (logical px). Each new pop-out cascades from the main window's origin so
/// windows never stack exactly on top of each other.
const POPOUT_SIZE: (f64, f64) = (760.0, 680.0);
const POPOUT_MIN_SIZE: (f64, f64) = (480.0, 420.0);
const POPOUT_CASCADE_ORIGIN: (f64, f64) = (72.0, 64.0);
const POPOUT_CASCADE_STEP: f64 = 36.0;
/// Traffic lights of a pop-out, centred in its title bar (`--popout-bar-height`).
#[cfg(target_os = "macos")]
const POPOUT_TRAFFIC_LIGHTS: (f64, f64) = (16.0, 22.0);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: String,
    pub alpha: f64,
    pub mode: String,
    pub solid: bool,
    pub blur: bool,
    /// Popped-out windows: "same" (follow the main window) or "opaque".
    pub popouts: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "atrium-calm-dark".into(),
            alpha: 1.0,
            mode: "background".into(),
            solid: false,
            blur: true,
            popouts: "same".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewState {
    pub thread_id: String,
    pub owner: String,
    pub draft: String,
    pub messages: Vec<String>,
    pub scroll: f64,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellState {
    pub revision: u64,
    pub settings: Settings,
    pub views: BTreeMap<String, ViewState>,
}

const EDITOR_TEXT: &str = "use std::time::{Duration, Instant};\nuse crate::retry::{retry_once, BACKOFF};\n\n/// How early a token counts as expired.\nconst SKEW: Duration = Duration::from_secs(30);\n\nimpl SessionStore {\n    pub fn refresh(&mut self, now: Instant) -> Result<&Token> {\n        if let Some(tok) = &self.current {\n            if tok.expires_at > now + SKEW {\n                return Ok(tok);\n            }\n        }\n        let fresh = retry_once(|| self.fetch(), BACKOFF)?;\n        self.current = Some(fresh);\n        Ok(self.current.as_ref().expect(\"just set\"))\n    }\n}\n";
const QUERY_TEXT: &str = "select id, thread, state, agent\nfrom sessions\nwhere state <> 'idle'\norder by updated_at desc;";

impl ShellState {
    pub fn new(settings: Settings) -> Self {
        let views = VIEW_IDS
            .iter()
            .map(|id| {
                let content = if *id == "editor" { EDITOR_TEXT } else { QUERY_TEXT };
                let view = ViewState {
                    thread_id: "t-auth".into(),
                    owner: MAIN.into(),
                    draft: String::new(),
                    messages: Vec::new(),
                    scroll: -1.0,
                    content: content.into(),
                };
                ((*id).to_string(), view)
            })
            .collect();
        Self { revision: 0, settings, views }
    }
}

pub fn popout_label(view: &str) -> String {
    format!("view-{view}")
}

fn one_of(v: &Value, allowed: &[&str], key: &str) -> Result<String, String> {
    v.as_str()
        .filter(|s| allowed.contains(s))
        .map(str::to_string)
        .ok_or_else(|| format!("invalid {key}"))
}

fn apply_setting(next: &mut Settings, key: &str, v: &Value) -> Result<(), String> {
    match key {
        "theme" => next.theme = one_of(v, &THEMES, key)?,
        "mode" => next.mode = one_of(v, &MODES, key)?,
        "popouts" => next.popouts = one_of(v, &POPOUTS, key)?,
        "alpha" => {
            next.alpha = v
                .as_f64()
                .filter(|a| a.is_finite() && (MIN_ALPHA..=1.0).contains(a))
                .ok_or("invalid alpha")?;
        }
        "solid" => next.solid = v.as_bool().ok_or("invalid solid")?,
        "blur" => next.blur = v.as_bool().ok_or("invalid blur")?,
        other => return Err(format!("unknown setting {other}")),
    }
    Ok(())
}

/// Applies a settings patch. Only whitelisted keys with valid values are accepted; any bad
/// field rejects the whole patch, so nothing arbitrary is ever persisted.
pub fn apply_settings_patch(current: &Settings, patch: &Value) -> Result<Settings, String> {
    let fields = patch.as_object().ok_or("patch must be an object")?;
    let mut next = current.clone();
    for (k, v) in fields {
        apply_setting(&mut next, k, v)?;
    }
    Ok(next)
}

/// Reads persisted settings leniently: each valid known field is kept, the rest fall back to
/// the defaults (a file from another version must not reset everything).
pub fn settings_from_bytes(bytes: &[u8]) -> Settings {
    let mut settings = Settings::default();
    if let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(bytes) {
        for (k, v) in &fields {
            let mut candidate = settings.clone();
            if apply_setting(&mut candidate, k, v).is_ok() {
                settings = candidate;
            }
        }
    }
    settings
}

/// The pinned configuration of a harness run (`ATRIUM_OPACITY`): off = alpha 1, otherwise 0.6.
pub fn harness_settings(opacity: &str, theme: Option<&str>) -> Settings {
    let mut s = Settings {
        mode: if opacity == "whole" { "whole" } else { "background" }.into(),
        alpha: if opacity == "off" { 1.0 } else { 0.6 },
        ..Settings::default()
    };
    if let Some(t) = theme.filter(|t| THEMES.contains(t)) {
        s.theme = t.into();
    }
    s
}

#[derive(Debug, PartialEq)]
pub enum PatchError {
    UnknownView,
    /// The view moved to another window. Expected during a pop-out or bring-back race; the
    /// frontend drops the edit instead of treating it as a failure.
    NotOwner,
    Invalid(String),
}

impl PatchError {
    pub fn message(&self) -> String {
        match self {
            Self::UnknownView => "unknown-view: no such view".into(),
            Self::NotOwner => "not-owner: the view is owned by another window".into(),
            Self::Invalid(why) => format!("invalid: {why}"),
        }
    }
}

/// Applies a view patch from `window`. Validates every field before writing any, never
/// inserts a missing view, and only the owning window may edit.
pub fn apply_view_patch(state: &mut ShellState, window: &str, view: &str, patch: &Value) -> Result<(), PatchError> {
    let target = state.views.get_mut(view).ok_or(PatchError::UnknownView)?;
    if target.owner != window {
        return Err(PatchError::NotOwner);
    }
    let fields = patch.as_object().ok_or_else(|| PatchError::Invalid("patch must be an object".into()))?;
    let mut next = target.clone();
    for (k, v) in fields {
        let bad = || PatchError::Invalid(format!("bad field {k}"));
        match k.as_str() {
            "draft" => next.draft = v.as_str().ok_or_else(bad)?.into(),
            "content" => next.content = v.as_str().ok_or_else(bad)?.into(),
            "threadId" => next.thread_id = v.as_str().ok_or_else(bad)?.into(),
            "scroll" => next.scroll = v.as_f64().filter(|n| n.is_finite()).ok_or_else(bad)?,
            "messages" => {
                next.messages = v
                    .as_array()
                    .and_then(|a| a.iter().map(|m| m.as_str().map(str::to_string)).collect::<Option<Vec<_>>>())
                    .ok_or_else(bad)?;
            }
            _ => return Err(bad()),
        }
    }
    *target = next;
    Ok(())
}

/// Moves a view from the main window to its pop-out. Fails if it is not in main.
pub fn begin_pop_out(state: &mut ShellState, view: &str) -> Result<String, String> {
    let target = state.views.get_mut(view).ok_or("unknown view")?;
    if target.owner != MAIN {
        return Err("view is already in its own window".into());
    }
    let label = popout_label(view);
    target.owner = label.clone();
    Ok(label)
}

/// Gives every view owned by `label` back to the main window. Returns whether anything moved.
pub fn restore_owned_views(state: &mut ShellState, label: &str) -> bool {
    let mut moved = false;
    for view in state.views.values_mut() {
        if view.owner == label {
            view.owner = MAIN.into();
            moved = true;
        }
    }
    moved
}

/// Writes `bytes` to `path` atomically (temp file + rename), so a crash mid-write never
/// leaves a truncated file. Used for settings and for harness reports.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Debounced background writer: `save` never blocks the IPC thread, a burst of saves (a
/// slider drag) is written once after `debounce` of quiet, and `shutdown` flushes the last one.
pub struct Persister {
    tx: Mutex<Option<mpsc::Sender<Settings>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Persister {
    pub fn spawn(debounce: Duration, mut write: impl FnMut(&Settings) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel::<Settings>();
        let worker = std::thread::spawn(move || {
            while let Ok(mut latest) = rx.recv() {
                loop {
                    match rx.recv_timeout(debounce) {
                        Ok(next) => latest = next,
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => {
                            write(&latest);
                            return;
                        }
                    }
                }
                write(&latest);
            }
        });
        Self { tx: Mutex::new(Some(tx)), worker: Mutex::new(Some(worker)) }
    }

    pub fn save(&self, settings: Settings) {
        if let Some(tx) = self.tx.lock().unwrap().as_ref() {
            let _ = tx.send(settings);
        }
    }

    /// Flushes the pending save (if any) and stops the worker. Idempotent.
    pub fn shutdown(&self) {
        drop(self.tx.lock().unwrap().take());
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

pub fn settings_path(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join(SETTINGS_FILE))
}

/// Settings persistence. `None` in harness runs, which pin their configuration and must never
/// overwrite the user's file.
pub struct Persist(pub Option<Persister>);

pub fn persister_for(path: PathBuf) -> Persister {
    Persister::spawn(PERSIST_DEBOUNCE, move |s| {
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| write_atomic(&path, serde_json::to_string(s).unwrap_or_default().as_bytes()));
        if let Err(e) = result {
            eprintln!("saving settings failed: {e}");
        }
    })
}

// -- Tauri glue ----------------------------------------------------------------------------

pub struct Shell(pub Mutex<ShellState>);

fn publish(app: &tauri::AppHandle, state: &mut ShellState) -> ShellState {
    state.revision += 1;
    if let Err(e) = app.emit("shell-state", &*state) {
        eprintln!("broadcasting shell state failed: {e}");
    }
    state.clone()
}

#[tauri::command]
pub fn shell_snapshot(shell: State<'_, Shell>) -> ShellState {
    shell.0.lock().unwrap().clone()
}

#[tauri::command]
pub fn shell_patch(
    app: tauri::AppHandle,
    window: WebviewWindow,
    shell: State<'_, Shell>,
    persist: State<'_, Persist>,
    view: Option<String>,
    patch: Value,
) -> Result<ShellState, String> {
    let mut state = shell.0.lock().unwrap();
    match view {
        Some(id) => apply_view_patch(&mut state, window.label(), &id, &patch).map_err(|e| e.message())?,
        None => {
            let next = apply_settings_patch(&state.settings, &patch)?;
            if next == state.settings {
                return Ok(state.clone());
            }
            if let Some(p) = &persist.0 {
                p.save(next.clone());
            }
            state.settings = next;
        }
    }
    Ok(publish(&app, &mut state))
}

pub async fn set_window_alpha(window: WebviewWindow, alpha: f64) -> Result<f64, String> {
    if !alpha.is_finite() || !(MIN_ALPHA..=1.0).contains(&alpha) {
        return Err("alpha outside 0.3..1".into());
    }
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        window
            .with_webview(move |webview| {
                // Tauri runs this closure on the main thread. The pointer is borrowed only for
                // this callback; it belongs to the still-live WebviewWindow.
                let native = unsafe { &*webview.ns_window().cast::<objc2_app_kit::NSWindow>() };
                native.setAlphaValue(alpha);
                let _ = tx.send(native.alphaValue());
            })
            .map_err(|e| e.to_string())?;
        rx.recv_timeout(Duration::from_secs(5)).map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window;
        Ok(1.0) // Explicit opaque fallback; no compositor assumptions.
    }
}

/// Native window theme for the settings ("system" follows the OS).
pub fn native_theme(settings: &Settings) -> Option<tauri::Theme> {
    match settings.theme.as_str() {
        "atrium-calm-light" => Some(tauri::Theme::Light),
        "atrium-calm-dark" => Some(tauri::Theme::Dark),
        _ => None,
    }
}

/// Creates the pop-out window for a view that `begin_pop_out` has already moved.
pub fn build_popout(app: &tauri::AppHandle, view: &str, theme: Option<tauri::Theme>) -> tauri::Result<WebviewWindow> {
    let label = popout_label(view);
    let index = app.webview_windows().keys().filter(|l| l.starts_with("view-")).count() as f64;
    let (ox, oy) = app
        .get_webview_window(MAIN)
        .and_then(|m| {
            let scale = m.scale_factor().ok()?;
            m.outer_position().ok().map(|p| p.to_logical::<f64>(scale))
        })
        .map_or((0.0, 0.0), |p| (p.x, p.y));
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(format!("index.html?view={view}").into()))
        .title(format!("Atrium · {view}"))
        .inner_size(POPOUT_SIZE.0, POPOUT_SIZE.1)
        .min_inner_size(POPOUT_MIN_SIZE.0, POPOUT_MIN_SIZE.1)
        .position(
            ox + POPOUT_CASCADE_ORIGIN.0 + POPOUT_CASCADE_STEP * index,
            oy + POPOUT_CASCADE_ORIGIN.1 + POPOUT_CASCADE_STEP * index,
        )
        .theme(theme);
    #[cfg(target_os = "macos")]
    let builder = builder
        .transparent(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(POPOUT_TRAFFIC_LIGHTS.0, POPOUT_TRAFFIC_LIGHTS.1));
    let window = builder.build()?;
    #[cfg(target_os = "linux")]
    let _ = window.with_webview(crate::linux::configure);
    Ok(window)
}

/// Moves `view` into its own window (or focuses it if it already is).
pub fn pop_out_view(app: &tauri::AppHandle, view: &str) -> Result<ShellState, String> {
    if !VIEW_IDS.contains(&view) {
        return Err("unknown view".into());
    }
    if let Some(w) = app.get_webview_window(&popout_label(view)) {
        w.set_focus().map_err(|e| e.to_string())?;
        return Ok(app.state::<Shell>().0.lock().unwrap().clone());
    }
    // Publish the ownership before the new webview can read its first snapshot.
    let theme = {
        let shell = app.state::<Shell>();
        let mut state = shell.0.lock().unwrap();
        begin_pop_out(&mut state, view)?;
        publish(app, &mut state);
        native_theme(&state.settings)
    };
    if let Err(e) = build_popout(app, view, theme) {
        return_view(app, &popout_label(view));
        return Err(e.to_string());
    }
    Ok(app.state::<Shell>().0.lock().unwrap().clone())
}

pub fn return_view(app: &tauri::AppHandle, label: &str) {
    let shell = app.state::<Shell>();
    let mut state = shell.0.lock().unwrap();
    if restore_owned_views(&mut state, label) {
        publish(app, &mut state);
    }
}

/// The owning pop-out returns its view (after flushing its edits) and closes itself.
#[tauri::command]
pub async fn bring_back(app: tauri::AppHandle, window: WebviewWindow, view: String) -> Result<ShellState, String> {
    let label = popout_label(&view);
    if window.label() != label {
        return Err("only the owning window can return a live view".into());
    }
    return_view(&app, &label);
    window.destroy().map_err(|e| e.to_string())?;
    Ok(app.state::<Shell>().0.lock().unwrap().clone())
}

/// Main-window fallback when a pop-out does not answer a polite bring-back in time (hung or
/// busy): destroy it and take the view back. Edits that pop-out never sent are lost.
#[tauri::command]
pub async fn force_bring_back(app: tauri::AppHandle, window: WebviewWindow, view: String) -> Result<ShellState, String> {
    if window.label() != MAIN {
        return Err("only the main window can force a view back".into());
    }
    let label = popout_label(&view);
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.destroy();
    }
    return_view(&app, &label);
    Ok(app.state::<Shell>().0.lock().unwrap().clone())
}

/// Per-window readiness for the harness: each window reports, after mounting and applying its
/// appearance, what it actually applied. The harness checks the count and every entry.
#[derive(Default)]
pub struct Ready(pub Mutex<BTreeMap<String, Value>>);

#[tauri::command]
pub fn window_ready(window: WebviewWindow, ready: State<'_, Ready>, applied: Value) -> usize {
    #[cfg(target_os = "linux")]
    {
        // Picked up by measure.py into the run log (which monitor, which pacing).
        let label = window.label().to_string();
        let _ = window.with_webview(move |w| println!("linux-display {label}: {}", crate::linux::describe(&w)));
    }
    let mut states = ready.0.lock().unwrap();
    states.insert(window.label().to_string(), applied);
    states.len()
}

#[tauri::command]
pub fn ready_states(ready: State<'_, Ready>) -> BTreeMap<String, Value> {
    ready.0.lock().unwrap().clone()
}

pub fn accessibility_opaque() -> bool {
    #[cfg(target_os = "macos")]
    {
        let workspace = objc2_app_kit::NSWorkspace::sharedWorkspace();
        workspace.accessibilityDisplayShouldReduceTransparency() || workspace.accessibilityDisplayShouldIncreaseContrast()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

#[tauri::command]
pub fn opaque_accessibility() -> bool {
    accessibility_opaque()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn restore_returns_only_the_closed_windows_views_and_keeps_their_content() {
        let mut state = ShellState::new(Settings::default());
        apply_view_patch(&mut state, MAIN, "chat", &json!({"draft": "unfinished", "messages": ["hello"], "scroll": 425.0})).unwrap();
        begin_pop_out(&mut state, "chat").unwrap();
        begin_pop_out(&mut state, "editor").unwrap();
        let before = state.views["chat"].clone();
        assert!(restore_owned_views(&mut state, "view-chat"));
        assert_eq!(state.views["chat"].owner, MAIN);
        assert_eq!(state.views["chat"].draft, before.draft);
        assert_eq!(state.views["chat"].messages, before.messages);
        assert_eq!(state.views["editor"].owner, "view-editor", "other pop-outs keep their view");
        assert!(!restore_owned_views(&mut state, "view-chat"), "second restore is a no-op");
    }

    #[test]
    fn only_the_owner_can_patch_a_view() {
        let mut state = ShellState::new(Settings::default());
        begin_pop_out(&mut state, "chat").unwrap();
        let before = state.clone();
        assert_eq!(apply_view_patch(&mut state, MAIN, "chat", &json!({"draft": "x"})), Err(PatchError::NotOwner));
        assert_eq!(state, before);
        apply_view_patch(&mut state, "view-chat", "chat", &json!({"draft": "x"})).unwrap();
        assert_eq!(state.views["chat"].draft, "x");
        assert!(PatchError::NotOwner.message().starts_with("not-owner"), "the frontend matches on this prefix");
    }

    #[test]
    fn patching_an_unknown_view_does_not_insert_it() {
        let mut state = ShellState::new(Settings::default());
        let before = state.clone();
        assert_eq!(apply_view_patch(&mut state, MAIN, "terminal", &json!({"draft": "x"})), Err(PatchError::UnknownView));
        assert_eq!(state, before, "the old IndexMut code inserted a null view here");
        assert!(begin_pop_out(&mut state, "terminal").is_err());
        assert_eq!(state, before);
    }

    #[test]
    fn an_invalid_field_rejects_the_whole_view_patch() {
        let mut state = ShellState::new(Settings::default());
        let before = state.clone();
        for bad in [json!({"draft": "ok", "scroll": "far"}), json!({"owner": "view-chat"}), json!({"messages": [1]}), json!("x")] {
            assert!(matches!(apply_view_patch(&mut state, MAIN, "chat", &bad), Err(PatchError::Invalid(_))), "{bad}");
            assert_eq!(state, before);
        }
    }

    #[test]
    fn pop_out_requires_the_view_in_main() {
        let mut state = ShellState::new(Settings::default());
        assert_eq!(begin_pop_out(&mut state, "editor").unwrap(), "view-editor");
        assert!(begin_pop_out(&mut state, "editor").is_err());
    }

    #[test]
    fn settings_patches_are_whitelisted_and_validated() {
        let s = Settings::default();
        let next = apply_settings_patch(&s, &json!({"alpha": 0.6, "mode": "whole", "popouts": "opaque", "theme": "system"})).unwrap();
        assert_eq!((next.alpha, next.mode.as_str(), next.popouts.as_str(), next.theme.as_str()), (0.6, "whole", "opaque", "system"));
        for bad in [
            json!({"alpha": 0.2}),
            json!({"alpha": "0.5"}),
            json!({"mode": "css-opacity"}),
            json!({"theme": "glass-dark"}),
            json!({"blur": "yes"}),
            json!({"alpha": 0.5, "evil": "x"}),
            json!([1]),
        ] {
            assert!(apply_settings_patch(&s, &bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn persisted_settings_keep_valid_fields_and_drop_the_rest() {
        let s = settings_from_bytes(br#"{"theme":"atrium-calm-light","alpha":7,"mode":"whole","unknown":1}"#);
        assert_eq!(s.theme, "atrium-calm-light");
        assert_eq!(s.alpha, 1.0, "invalid alpha falls back to the default");
        assert_eq!(s.mode, "whole");
        assert_eq!(settings_from_bytes(b"not json"), Settings::default());
    }

    #[test]
    fn harness_settings_pin_the_configuration() {
        assert_eq!(harness_settings("off", None).alpha, 1.0);
        let bg = harness_settings("background", Some("atrium-calm-light"));
        assert_eq!((bg.alpha, bg.mode.as_str(), bg.theme.as_str()), (0.6, "background", "atrium-calm-light"));
        assert_eq!(harness_settings("whole", Some("bogus")).mode, "whole");
        assert_eq!(harness_settings("whole", Some("bogus")).theme, "atrium-calm-dark");
    }

    #[test]
    fn persister_coalesces_a_burst_and_flushes_the_last_value_on_shutdown() {
        let writes = Arc::new(Mutex::new(Vec::<f64>::new()));
        let sink = writes.clone();
        let p = Persister::spawn(Duration::from_millis(200), move |s| sink.lock().unwrap().push(s.alpha));
        for i in 0..100 {
            p.save(Settings { alpha: 0.3 + f64::from(i) * 0.007, ..Settings::default() });
        }
        p.shutdown();
        let w = writes.lock().unwrap();
        assert_eq!(w.len(), 1, "100 slider events, one write: {w:?}");
        assert!((w[0] - (0.3 + 99.0 * 0.007)).abs() < 1e-9);
        p.shutdown();
    }

    #[test]
    fn persister_writes_again_after_a_quiet_period() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let p = Persister::spawn(Duration::from_millis(20), move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        p.save(Settings::default());
        std::thread::sleep(Duration::from_millis(150));
        p.save(Settings::default());
        p.shutdown();
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temp() {
        let dir = std::env::temp_dir().join(format!("atrium-m0-06b-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SETTINGS_FILE);
        write_atomic(&path, b"{\"a\":1}").unwrap();
        write_atomic(&path, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":2}");
        assert!(!dir.join(format!("{SETTINGS_FILE}.tmp")).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
