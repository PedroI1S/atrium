//! Atrium UI shell spike (M0-06, layout B in M0-06b): native chrome (overlay title bar,
//! vibrancy, window alpha), the shared window state for pop-outs, and a small command surface
//! for the measurement harness. All UI is in the frontend.

mod shell;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use shell::{MAIN, Persist, Ready, Settings, Shell, ShellState};
use tauri::{Manager, State, WebviewWindow};

#[cfg(target_os = "macos")]
use tauri::Emitter;
#[cfg(target_os = "macos")]
use window_vibrancy::{NSVisualEffectMaterial, NSVisualEffectState, apply_vibrancy, clear_vibrancy};

/// Corner radius of the native vibrancy view. Matches the macOS window radius so the blur
/// doesn't poke out of the rounded corners.
#[cfg(target_os = "macos")]
const WINDOW_CORNER_RADIUS: f64 = 10.0;

fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.is_empty())
}

/// Process-wide facts captured as early as possible.
struct Boot {
    /// Wall clock (ms since the Unix epoch) at the top of `run()`.
    main_epoch_ms: f64,
}

/// What Rust has already applied natively to each window (vibrancy on/off, NSWindow alpha), so
/// the frontend only calls native APIs when its target differs.
#[derive(Default)]
struct Native(Mutex<HashMap<String, NativeState>>);

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NativeState {
    vibrancy: bool,
    /// The vibrancy material's input (dark or light material).
    dark: bool,
    alpha: f64,
}

impl Default for NativeState {
    fn default() -> Self {
        Self { vibrancy: false, dark: false, alpha: 1.0 }
    }
}

/// The native target for a window. Mirrors `appearanceFor` in `src/window-model.ts`; the
/// frontend reconciles any difference after mount, so this only saves a redundant call.
fn native_target(s: &Settings, popout: bool, reduce: bool) -> NativeState {
    let opaque = reduce || (popout && s.popouts == "opaque") || !cfg!(target_os = "macos");
    let alpha = if opaque { 1.0 } else { s.alpha };
    NativeState {
        vibrancy: s.mode == "background" && alpha < 1.0 && s.blur,
        dark: s.theme != "atrium-calm-light",
        alpha: if s.mode == "whole" { alpha } else { 1.0 },
    }
}

/// Startup facts plus the first state snapshot, in one IPC call so the frontend can paint its
/// tokens before the first render.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupInfo {
    main_epoch_ms: f64,
    label: String,
    /// `startup` (report timings and stay open) or `frames` (also run the frame benchmarks,
    /// report, and exit). `None` in normal use.
    bench_mode: Option<String>,
    open_palette: bool,
    screen: Option<String>,
    right_tab: Option<String>,
    advanced: bool,
    panel: Option<String>,
    vibrancy_supported: bool,
    windows: u32,
    opacity: String,
    accessibility_opaque: bool,
    native: NativeState,
    snapshot: ShellState,
}

#[tauri::command]
fn startup_info(window: WebviewWindow, boot: State<'_, Boot>, shell: State<'_, Shell>, native: State<'_, Native>) -> StartupInfo {
    StartupInfo {
        main_epoch_ms: boot.main_epoch_ms,
        label: window.label().to_string(),
        bench_mode: env("ATRIUM_BENCH"),
        open_palette: env("ATRIUM_PALETTE").is_some(),
        screen: env("ATRIUM_SCREEN"),
        right_tab: env("ATRIUM_RIGHT_TAB"),
        advanced: env("ATRIUM_ADVANCED").is_some(),
        panel: env("ATRIUM_PANEL"),
        windows: env("ATRIUM_WINDOWS").and_then(|v| v.parse().ok()).unwrap_or(1),
        opacity: env("ATRIUM_OPACITY").unwrap_or_else(|| "off".into()),
        vibrancy_supported: cfg!(target_os = "macos"),
        accessibility_opaque: shell::accessibility_opaque(),
        native: native.0.lock().unwrap().get(window.label()).copied().unwrap_or_default(),
        snapshot: shell.0.lock().unwrap().clone(),
    }
}

#[cfg(target_os = "macos")]
fn vibrancy_on(window: &WebviewWindow, enabled: bool, dark: bool) -> Result<bool, String> {
    if enabled {
        let material = if dark { NSVisualEffectMaterial::UnderWindowBackground } else { NSVisualEffectMaterial::Sidebar };
        apply_vibrancy(window, material, Some(NSVisualEffectState::Active), Some(WINDOW_CORNER_RADIUS)).map_err(|e| e.to_string())?;
        Ok(true)
    } else {
        clear_vibrancy(window).map_err(|e| e.to_string()).map(|_| false)
    }
}

/// Turns native vibrancy on or off. `dark` picks the material so the blur matches the theme.
/// Returns whether vibrancy is now applied.
#[tauri::command]
fn set_vibrancy(window: WebviewWindow, native: State<'_, Native>, enabled: bool, dark: bool) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    let applied = vibrancy_on(&window, enabled, dark)?;
    #[cfg(not(target_os = "macos"))]
    let applied = {
        let _ = (enabled, dark);
        false
    };
    let mut shown = native.0.lock().unwrap();
    let entry = shown.entry(window.label().to_string()).or_default();
    entry.vibrancy = applied;
    entry.dark = dark;
    Ok(applied)
}

#[tauri::command]
async fn set_window_alpha(window: WebviewWindow, native: State<'_, Native>, alpha: f64) -> Result<f64, String> {
    let applied = shell::set_window_alpha(window.clone(), alpha).await?;
    native.0.lock().unwrap().entry(window.label().to_string()).or_default().alpha = applied;
    Ok(applied)
}

/// Applies the native part of the appearance to a new window, before its webview has loaded,
/// so the first frame is already right (no "vibrancy applied, then cleared").
fn apply_native(app: &tauri::AppHandle, window: &WebviewWindow, settings: &Settings, popout: bool) {
    let target = native_target(settings, popout, shell::accessibility_opaque());
    #[allow(unused_mut)]
    let mut applied = NativeState::default();
    #[cfg(target_os = "macos")]
    {
        if target.vibrancy {
            match vibrancy_on(window, true, target.dark) {
                Ok(v) => {
                    applied.vibrancy = v;
                    applied.dark = target.dark;
                }
                Err(e) => eprintln!("apply_vibrancy failed: {e}"),
            }
        }
        if target.alpha < 1.0 {
            // On the main thread already (setup or a sync command), so set it directly.
            if let Ok(ns) = window.ns_window() {
                let native = unsafe { &*ns.cast::<objc2_app_kit::NSWindow>() };
                native.setAlphaValue(target.alpha);
                applied.alpha = native.alphaValue();
            }
        }
    }
    let _ = target;
    app.state::<Native>().0.lock().unwrap().insert(window.label().to_string(), applied);
}

/// Writes a harness report (JSON) atomically to the path in `ATRIUM_BENCH_OUT`, if set, and
/// echoes it on stdout either way.
#[tauri::command]
fn write_report(json: String) -> Result<(), String> {
    println!("ATRIUM_REPORT {json}");
    if let Ok(path) = std::env::var("ATRIUM_BENCH_OUT") {
        shell::write_atomic(std::path::Path::new(&path), json.as_bytes()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Labels of the windows that exist natively right now (the round-trip check uses it to prove a
/// pop-out was destroyed, not merely that it stopped reporting).
#[tauri::command]
fn window_labels(app: tauri::AppHandle) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().into_keys().collect();
    labels.sort();
    labels
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

fn pop_out_and_apply(app: &tauri::AppHandle, view: &str) -> Result<ShellState, String> {
    let state = shell::pop_out_view(app, view)?;
    if let Some(w) = app.get_webview_window(&shell::popout_label(view)) {
        apply_native(app, &w, &state.settings, true);
    }
    Ok(state)
}

/// Sync on purpose: sync commands run on the main thread, where window creation and the
/// native calls in `apply_native` belong.
#[tauri::command]
fn pop_out(app: tauri::AppHandle, view: String) -> Result<ShellState, String> {
    pop_out_and_apply(&app, &view)
}

/// Pop-outs to restore once the main window is interactive. Restoring them in `setup` made
/// three webviews load at once and delayed the main window's first input by their load time.
#[derive(Default)]
struct Restore(Mutex<Vec<String>>);

/// Called by main after its first render: recreates the remembered pop-outs, then keeps main
/// in front (harness runs measure main's frames with the others open behind it).
#[tauri::command]
fn restore_windows(app: tauri::AppHandle, window: WebviewWindow) -> Result<usize, String> {
    if window.label() != MAIN {
        return Err("only the main window restores pop-outs".into());
    }
    let views: Vec<String> = std::mem::take(&mut *app.state::<Restore>().0.lock().unwrap());
    for view in &views {
        pop_out_and_apply(&app, view)?;
    }
    if !views.is_empty() && (env("ATRIUM_WINDOWS").is_some() || env("ATRIUM_BENCH").is_some()) {
        let _ = window.set_focus();
    }
    Ok(views.len())
}

/// Re-checks Reduce transparency / Increase contrast whenever macOS says they changed and tells
/// every window (no polling, no focus-driven re-checks).
#[cfg(target_os = "macos")]
fn observe_accessibility(app: tauri::AppHandle) {
    use block2::RcBlock;
    use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
    use objc2_foundation::NSNotification;
    let block = RcBlock::new(move |_: std::ptr::NonNull<NSNotification>| {
        let _ = app.emit("accessibility-changed", shell::accessibility_opaque());
    });
    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    // SAFETY: a valid notification name, no object filter, delivered on the posting thread;
    // the block only emits a Tauri event, which is thread-safe.
    let token = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
            None,
            None,
            &block,
        )
    };
    std::mem::forget(token); // observe for the life of the process
}

pub fn run() {
    let boot = Boot { main_epoch_ms: epoch_ms() };
    let app = tauri::Builder::default()
        .manage(boot)
        .manage(Ready::default())
        .manage(Native::default())
        .manage(Restore::default())
        .invoke_handler(tauri::generate_handler![
            shell::opaque_accessibility,
            shell::window_ready,
            shell::ready_states,
            shell::shell_snapshot,
            shell::shell_patch,
            shell::bring_back,
            shell::force_bring_back,
            pop_out,
            restore_windows,
            set_window_alpha,
            startup_info,
            set_vibrancy,
            write_report,
            window_labels,
            quit
        ])
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::Destroyed) {
                let app = window.app_handle();
                app.state::<Ready>().0.lock().unwrap().remove(window.label());
                app.state::<Native>().0.lock().unwrap().remove(window.label());
                if window.label() == MAIN {
                    app.exit(0);
                } else {
                    shell::return_view(app, window.label());
                }
            }
        })
        .setup(|app| {
            let handle = app.handle().clone();
            let harness = env("ATRIUM_WINDOWS").is_some() || env("ATRIUM_BENCH").is_some();
            let settings = if harness {
                shell::harness_settings(&env("ATRIUM_OPACITY").unwrap_or_else(|| "off".into()), env("ATRIUM_THEME").as_deref())
            } else {
                shell::settings_path(&handle)
                    .and_then(|p| std::fs::read(p).ok())
                    .map_or_else(Settings::default, |b| shell::settings_from_bytes(&b))
            };
            let persist = if harness { None } else { shell::settings_path(&handle).map(shell::persister_for) };
            app.manage(Persist(persist));
            app.manage(Shell(Mutex::new(ShellState::new(settings.clone()))));

            if let Some(main) = app.get_webview_window(MAIN) {
                let _ = main.set_theme(shell::native_theme(&settings));
                apply_native(&handle, &main, &settings, false);
            }
            #[cfg(target_os = "macos")]
            observe_accessibility(handle.clone());

            if harness {
                // Three-window runs restore two pop-outs, as a relaunch would; main creates them
                // once it is interactive (`restore_windows`). `ATRIUM_POPOUTS=chat,editor` picks
                // them explicitly (screenshots).
                let popouts: Vec<String> = match env("ATRIUM_POPOUTS") {
                    Some(list) => list.split(',').map(str::to_string).collect(),
                    None if env("ATRIUM_WINDOWS").as_deref() == Some("3") => vec!["editor".into(), "database".into()],
                    None => Vec::new(),
                };
                *app.state::<Restore>().0.lock().unwrap() = popouts;
                // Activate the app and bring main to the front, as a launch from the Dock or
                // Finder does: a background, non-key window gets throttled timers and no rAF,
                // which skewed (and hung) earlier runs. No process activity is held (review 2 #4):
                // measured runs schedule like normal launches.
                if let Some(main) = app.get_webview_window(MAIN) {
                    let _ = main.set_focus();
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building the Atrium UI shell");
    app.run(|app, event| {
        if let tauri::RunEvent::Exit = event
            && let Some(p) = &app.state::<Persist>().0
        {
            p.shutdown();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_target_follows_the_opacity_rules() {
        let mac = cfg!(target_os = "macos");
        let bg = Settings { alpha: 0.6, ..Settings::default() };
        let t = native_target(&bg, false, false);
        assert_eq!((t.vibrancy, t.alpha), (mac, 1.0));
        let whole = Settings { mode: "whole".into(), ..bg.clone() };
        let t = native_target(&whole, false, false);
        assert!(!t.vibrancy);
        assert_eq!(t.alpha, if mac { 0.6 } else { 1.0 });
        let opaque_popouts = Settings { popouts: "opaque".into(), ..whole };
        assert_eq!(native_target(&opaque_popouts, true, false).alpha, 1.0);
        assert_eq!(native_target(&opaque_popouts, false, false).alpha, if mac { 0.6 } else { 1.0 }, "main is not a pop-out");
        assert!(!native_target(&bg, false, true).vibrancy, "accessibility forces opaque");
        assert!(!native_target(&Settings { blur: false, ..bg.clone() }, false, false).vibrancy);
        assert!(native_target(&bg, false, false).dark);
        assert!(!native_target(&Settings { theme: "atrium-calm-light".into(), ..bg }, false, false).dark, "light material");
    }
}
