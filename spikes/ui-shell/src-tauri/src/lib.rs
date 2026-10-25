//! Atrium M0-06 UI shell spike: native chrome (overlay title bar + vibrancy) and a
//! tiny command surface used by the measurement harness. All UI logic is in the
//! frontend; the Rust side only owns what the web layer cannot do.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{Manager, State, WebviewWindow};

#[cfg(target_os = "macos")]
use window_vibrancy::{
    NSVisualEffectMaterial, NSVisualEffectState, apply_vibrancy, clear_vibrancy,
};

/// Corner radius of the native vibrancy view. Matches the macOS window radius so
/// the blur doesn't poke out of the rounded corners.
#[cfg(target_os = "macos")]
const WINDOW_CORNER_RADIUS: f64 = 10.0;

fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

/// Process-wide facts captured as early as possible.
struct Boot {
    /// Wall clock (ms since the Unix epoch) at the top of `run()`.
    main_epoch_ms: f64,
}

/// Options for the automated measurement / screenshot harness, read from the
/// environment (`ATRIUM_*`). Absent in normal use.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupInfo {
    main_epoch_ms: f64,
    /// `startup` (report timings and stay open) or `frames` (also run the frame
    /// benchmarks, report, and exit). `None` in normal use.
    bench_mode: Option<String>,
    theme: Option<String>,
    preset: Option<String>,
    open_palette: bool,
    tab: Option<String>,
    right_tab: Option<String>,
    vibrancy_supported: bool,
}

#[tauri::command]
fn startup_info(boot: State<'_, Boot>) -> StartupInfo {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    StartupInfo {
        main_epoch_ms: boot.main_epoch_ms,
        bench_mode: env("ATRIUM_BENCH"),
        theme: env("ATRIUM_THEME"),
        preset: env("ATRIUM_PRESET"),
        open_palette: env("ATRIUM_PALETTE").is_some(),
        tab: env("ATRIUM_TAB"),
        right_tab: env("ATRIUM_RIGHT_TAB"),
        vibrancy_supported: cfg!(target_os = "macos"),
    }
}

/// Turns native vibrancy on or off. `dark` picks the material so the blur matches
/// the theme's appearance (the window's own appearance is set from the frontend).
#[tauri::command]
fn set_vibrancy(window: WebviewWindow, enabled: bool, dark: bool) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        if enabled {
            let material = if dark {
                NSVisualEffectMaterial::UnderWindowBackground
            } else {
                NSVisualEffectMaterial::Sidebar
            };
            apply_vibrancy(
                &window,
                material,
                Some(NSVisualEffectState::Active),
                Some(WINDOW_CORNER_RADIUS),
            )
            .map_err(|e| e.to_string())?;
            Ok(true)
        } else {
            clear_vibrancy(&window).map_err(|e| e.to_string())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&window, enabled, dark);
        Ok(false)
    }
}

/// Writes a harness report (JSON) to the path in `ATRIUM_BENCH_OUT`, if set, and
/// echoes it on stdout either way.
#[tauri::command]
fn write_report(json: String) -> Result<(), String> {
    println!("ATRIUM_REPORT {json}");
    if let Ok(path) = std::env::var("ATRIUM_BENCH_OUT") {
        std::fs::write(path, json).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

pub fn run() {
    let boot = Boot {
        main_epoch_ms: epoch_ms(),
    };
    tauri::Builder::default()
        .manage(boot)
        .invoke_handler(tauri::generate_handler![
            startup_info,
            set_vibrancy,
            write_report,
            quit
        ])
        .setup(|app| {
            // Vibrancy starts on; the frontend turns it off for themes that don't use it.
            #[cfg(target_os = "macos")]
            if let Some(window) = app.get_webview_window("main")
                && let Err(e) = apply_vibrancy(
                    &window,
                    NSVisualEffectMaterial::UnderWindowBackground,
                    Some(NSVisualEffectState::Active),
                    Some(WINDOW_CORNER_RADIUS),
                )
            {
                eprintln!("apply_vibrancy failed: {e}");
            }
            let _ = app;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Atrium UI shell");
}
