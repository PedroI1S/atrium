//! Atrium, the desktop app (PLAN §3.1, §3.16, §3.20; contract §11, §12).
//!
//! The app renders and sends commands; `atriumd` owns everything else. This
//! crate is the native half:
//! - [`bridge`]: the one connection to the daemon, each webview's deliveries
//!   and acknowledgements, commands with deadlines;
//! - [`shell`]: what windows share (appearance settings, which window shows
//!   which thread, unsent drafts) and what survives a relaunch;
//! - [`native`]: translucent windows (macOS);
//! - [`presence`]: the tray item's counts;
//! - [`links`]: the only way a link is opened.
//!
//! All UI is in the frontend (`app/src`).

pub mod bridge;
pub mod links;
#[cfg(target_os = "macos")]
mod macos;
pub mod native;
pub mod presence;
pub mod shell;
pub mod upgrade;

use std::collections::{BTreeMap, HashMap};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use atrium_client::Page;
use atrium_host::lifecycle::{Paths, default_paths};
use atrium_protocol::limits::MAX_LOADED_TRANSCRIPT_BYTES;
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{DaemonCommand, EventId};
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use bridge::{Bridge, CommandFailure};
use native::NativeState;
use shell::{Bounds, MAIN, OpacityMode, Persister, Popout, Session, Settings, ShellState, Theme};

const SETTINGS_FILE: &str = "settings.json";
const SESSION_FILE: &str = "session.json";

/// The main window (logical px).
const MAIN_SIZE: (f64, f64) = (1360.0, 860.0);
const MAIN_MIN_SIZE: (f64, f64) = (900.0, 560.0);
/// A new pop-out. Each one cascades from the main window's origin so that
/// windows never sit exactly on top of each other.
const POPOUT_SIZE: (f64, f64) = (760.0, 680.0);
const POPOUT_MIN_SIZE: (f64, f64) = (480.0, 420.0);
const POPOUT_CASCADE_ORIGIN: (f64, f64) = (72.0, 64.0);
const POPOUT_CASCADE_STEP: f64 = 36.0;
/// Traffic lights, inline with the sidebar's top strip and a pop-out's bar.
#[cfg(target_os = "macos")]
const MAIN_TRAFFIC_LIGHTS: (f64, f64) = (18.0, 21.0);
#[cfg(target_os = "macos")]
const POPOUT_TRAFFIC_LIGHTS: (f64, f64) = (16.0, 22.0);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0)
}

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|value| !value.is_empty())
}

/// A measurement or screenshot run: settings are pinned from the
/// environment and nothing of the user's is written.
fn harness_run() -> bool {
    env("ATRIUM_BENCH").is_some() || env("ATRIUM_WINDOWS").is_some()
}

/// The pinned settings of a harness run: `ATRIUM_OPACITY` (`off`,
/// `background`, `whole`) and `ATRIUM_THEME` (`light`, `dark`).
fn harness_settings(opacity: Option<&str>, theme: Option<&str>) -> Settings {
    Settings {
        mode: if opacity == Some("whole") {
            OpacityMode::Whole
        } else {
            OpacityMode::Background
        },
        alpha: if matches!(opacity, Some("background" | "whole")) {
            0.6
        } else {
            1.0
        },
        theme: if theme == Some("light") {
            Theme::Light
        } else {
            Theme::Dark
        },
        ..Settings::default()
    }
}

struct AppState {
    /// Wall clock at the top of [`run`], for the startup timings.
    main_epoch_ms: f64,
    harness: bool,
    paths: Paths,
    shell: Mutex<ShellState>,
    settings_file: Option<Persister<Settings>>,
    session_file: Option<Persister<Session>>,
    bridge: Bridge,
    /// What Rust applied natively to each window, so the frontend calls a
    /// native API only when its target differs.
    native: Mutex<HashMap<String, NativeState>>,
    /// Pop-outs to reopen once the main window is interactive. Reopening
    /// them during setup made every webview load at once and delayed the
    /// main window's first input (M0-06b).
    restore: Mutex<Vec<Popout>>,
    /// Harness: what each window reports having applied.
    ready: Mutex<BTreeMap<String, Value>>,
    has_tray: AtomicBool,
}

impl AppState {
    fn save_session(&self, state: &ShellState) {
        if let Some(file) = &self.session_file {
            file.save(state.session());
        }
    }
}

fn publish(app: &AppHandle, state: &ShellState) {
    if let Err(error) = app.emit("shell-state", state) {
        eprintln!("atrium: telling the windows about a change failed: {error}");
    }
}

fn system_dark(window: &WebviewWindow) -> bool {
    matches!(window.theme(), Ok(tauri::Theme::Dark))
}

// -- startup and the shared state -----------------------------------------------------------

/// Everything a webview needs before its first render, in one call.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupInfo {
    main_epoch_ms: f64,
    label: String,
    /// The thread a pop-out shows; `None` in the main window.
    popout_thread: Option<String>,
    platform: &'static str,
    /// A development build: the dev commands are offered.
    debug: bool,
    /// Harness pins (`ATRIUM_BENCH`, `ATRIUM_SCREEN`, `ATRIUM_PANEL`,
    /// `ATRIUM_ADVANCED`, `ATRIUM_PALETTE`, `ATRIUM_WINDOWS`).
    bench_mode: Option<String>,
    screen: Option<String>,
    panel: Option<String>,
    advanced: bool,
    open_palette: bool,
    windows: u32,
    translucency: bool,
    accessibility_opaque: bool,
    native: NativeState,
    has_tray: bool,
    snapshot: ShellState,
}

#[tauri::command]
fn startup_info(window: WebviewWindow, state: State<'_, AppState>) -> StartupInfo {
    let snapshot = lock(&state.shell).clone();
    StartupInfo {
        main_epoch_ms: state.main_epoch_ms,
        label: window.label().into(),
        popout_thread: snapshot
            .popouts
            .get(window.label())
            .map(|popout| popout.thread_id.clone()),
        platform: if cfg!(target_os = "macos") {
            "macos"
        } else {
            "linux"
        },
        debug: cfg!(debug_assertions),
        bench_mode: env("ATRIUM_BENCH"),
        screen: env("ATRIUM_SCREEN"),
        panel: env("ATRIUM_PANEL"),
        advanced: env("ATRIUM_ADVANCED").is_some(),
        open_palette: env("ATRIUM_PALETTE").is_some(),
        windows: env("ATRIUM_WINDOWS")
            .and_then(|value| value.parse().ok())
            .unwrap_or(1),
        translucency: native::translucency_supported(),
        accessibility_opaque: native::accessibility_opaque(),
        native: lock(&state.native)
            .get(window.label())
            .copied()
            .unwrap_or_default(),
        has_tray: state.has_tray.load(Ordering::Relaxed),
        snapshot,
    }
}

#[tauri::command]
fn shell_patch_settings(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
    patch: Value,
) -> Result<ShellState, String> {
    if window.label() != MAIN {
        return Err("settings are changed from the main window".into());
    }
    let mut shell = lock(&state.shell);
    let next = shell::apply_settings_patch(&shell.settings, &patch)?;
    if next != shell.settings {
        if let Some(file) = &state.settings_file {
            file.save(next.clone());
        }
        shell.settings = next;
        shell.revision += 1;
        publish(&app, &shell);
    }
    Ok(shell.clone())
}

/// A view's draft or scroll position, from the window that shows it. Not
/// published: no other window draws it.
#[tauri::command]
fn shell_patch_view(
    window: WebviewWindow,
    state: State<'_, AppState>,
    key: String,
    patch: Value,
) -> Result<u64, String> {
    let mut shell = lock(&state.shell);
    let revision = shell
        .patch_view(window.label(), &key, &patch)
        .map_err(|error| error.to_string())?;
    if patch.get("draft").is_some() {
        state.save_session(&shell);
    }
    Ok(revision)
}

#[tauri::command]
fn shell_set_main_thread(
    window: WebviewWindow,
    state: State<'_, AppState>,
    thread_id: Option<String>,
) -> Result<u64, String> {
    let mut shell = lock(&state.shell);
    let revision = shell
        .set_main_thread(window.label(), thread_id)
        .map_err(|error| error.to_string())?;
    state.save_session(&shell);
    Ok(revision)
}

// -- windows ----------------------------------------------------------------------------------

fn build_main(app: &AppHandle, settings: &Settings) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Atrium")
        .inner_size(MAIN_SIZE.0, MAIN_SIZE.1)
        .min_inner_size(MAIN_MIN_SIZE.0, MAIN_MIN_SIZE.1)
        .theme(native::window_theme(settings))
        .disable_drag_drop_handler();
    #[cfg(target_os = "macos")]
    let builder = builder
        .transparent(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(
            MAIN_TRAFFIC_LIGHTS.0,
            MAIN_TRAFFIC_LIGHTS.1,
        ));
    // Linux: the window draws its own controls on the frame (D-059).
    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false);
    builder.build()
}

fn build_popout(
    app: &AppHandle,
    label: &str,
    popout: &Popout,
    settings: &Settings,
) -> tauri::Result<WebviewWindow> {
    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title("Atrium")
        .min_inner_size(POPOUT_MIN_SIZE.0, POPOUT_MIN_SIZE.1)
        .theme(native::window_theme(settings))
        .disable_drag_drop_handler();
    let builder = match popout.bounds {
        Some(bounds) => builder
            .inner_size(
                bounds.width.max(POPOUT_MIN_SIZE.0),
                bounds.height.max(POPOUT_MIN_SIZE.1),
            )
            .position(bounds.x, bounds.y),
        None => {
            let index = app
                .webview_windows()
                .keys()
                .filter(|other| other.as_str() != MAIN)
                .count() as f64;
            let origin = app
                .get_webview_window(MAIN)
                .and_then(|main| {
                    let scale = main.scale_factor().ok()?;
                    main.outer_position()
                        .ok()
                        .map(|at| at.to_logical::<f64>(scale))
                })
                .map_or((0.0, 0.0), |at| (at.x, at.y));
            builder.inner_size(POPOUT_SIZE.0, POPOUT_SIZE.1).position(
                origin.0 + POPOUT_CASCADE_ORIGIN.0 + POPOUT_CASCADE_STEP * index,
                origin.1 + POPOUT_CASCADE_ORIGIN.1 + POPOUT_CASCADE_STEP * index,
            )
        }
    };
    #[cfg(target_os = "macos")]
    let builder = builder
        .transparent(true)
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(
            POPOUT_TRAFFIC_LIGHTS.0,
            POPOUT_TRAFFIC_LIGHTS.1,
        ));
    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false);
    builder.build()
}

fn apply_native(app: &AppHandle, window: &WebviewWindow, settings: &Settings, popout: bool) {
    let target = native::native_target(
        settings,
        popout,
        native::accessibility_opaque(),
        system_dark(window),
        native::translucency_supported(),
    );
    let applied = native::apply_to_new_window(window, target);
    lock(&app.state::<AppState>().native).insert(window.label().into(), applied);
}

/// Moves a thread's view into its own window. The new owner is published
/// before the window exists, so its webview's first snapshot is already
/// right.
fn open_popout(
    app: &AppHandle,
    thread_id: &str,
    bounds: Option<Bounds>,
) -> Result<ShellState, String> {
    let state = app.state::<AppState>();
    let (label, popout, settings) = {
        let mut shell = lock(&state.shell);
        let label = shell.begin_pop_out(thread_id, bounds)?;
        publish(app, &shell);
        state.save_session(&shell);
        let popout = shell.popouts.get(&label).cloned();
        (label, popout, shell.settings.clone())
    };
    let built = popout
        .ok_or_else(|| "the pop-out vanished".to_string())
        .and_then(|popout| {
            build_popout(app, &label, &popout, &settings).map_err(|error| error.to_string())
        });
    match built {
        Ok(window) => {
            apply_native(app, &window, &settings, true);
            Ok(lock(&state.shell).clone())
        }
        Err(error) => {
            return_view(app, &label);
            Err(error)
        }
    }
}

/// Gives a pop-out's view back to the main window.
fn return_view(app: &AppHandle, label: &str) {
    let state = app.state::<AppState>();
    let mut shell = lock(&state.shell);
    if shell.end_pop_out(label) {
        publish(app, &shell);
        state.save_session(&shell);
    }
}

/// Synchronous on purpose: window creation and the native calls belong on
/// the main thread, where Tauri runs synchronous commands.
#[tauri::command]
fn pop_out(app: AppHandle, window: WebviewWindow, thread_id: String) -> Result<ShellState, String> {
    if window.label() != MAIN {
        return Err("only the main window moves a thread to its own window".into());
    }
    open_popout(&app, &thread_id, None)
}

/// The pop-out returns its view (after sending its last edits) and closes.
#[tauri::command]
async fn bring_back(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    if window.label() == MAIN {
        return Err("the main window has no view to return".into());
    }
    return_view(&app, window.label());
    window.destroy().map_err(|error| error.to_string())
}

/// The main window's fallback when a pop-out doesn't answer in time (hung or
/// busy): destroy it and take the view back. `label` must be a pop-out that
/// exists; edits that window never sent are lost.
#[tauri::command]
async fn force_bring_back(
    app: AppHandle,
    window: WebviewWindow,
    label: String,
) -> Result<(), String> {
    if window.label() != MAIN {
        return Err("only the main window can force a view back".into());
    }
    if !lock(&app.state::<AppState>().shell)
        .popouts
        .contains_key(&label)
    {
        return Err("no such pop-out".into());
    }
    if let Some(popout) = app.get_webview_window(&label) {
        let _ = popout.destroy();
    }
    return_view(&app, &label);
    Ok(())
}

#[tauri::command]
fn focus_popout(app: AppHandle, state: State<'_, AppState>, label: String) -> Result<(), String> {
    if !lock(&state.shell).popouts.contains_key(&label) {
        return Err("no such pop-out".into());
    }
    match app.get_webview_window(&label) {
        Some(popout) => popout.set_focus().map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

/// Called by the main window after its first render: reopens the pop-outs
/// of the last session.
#[tauri::command]
fn restore_windows(
    app: AppHandle,
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    if window.label() != MAIN {
        return Err("only the main window restores pop-outs".into());
    }
    let popouts = std::mem::take(&mut *lock(&state.restore));
    let mut opened = 0;
    for popout in &popouts {
        match open_popout(&app, &popout.thread_id, popout.bounds) {
            Ok(_) => opened += 1,
            Err(error) => {
                eprintln!("atrium: a window of the last session wasn't reopened: {error}")
            }
        }
    }
    if opened > 0 && state.harness {
        let _ = window.set_focus();
    }
    Ok(opened)
}

#[tauri::command]
fn set_vibrancy(
    window: WebviewWindow,
    state: State<'_, AppState>,
    enabled: bool,
    dark: bool,
) -> Result<bool, String> {
    let applied = native::set_vibrancy(&window, enabled, dark)?;
    let mut shown = lock(&state.native);
    let entry = shown.entry(window.label().into()).or_default();
    entry.vibrancy = applied;
    entry.dark = dark;
    Ok(applied)
}

#[tauri::command]
async fn set_window_alpha(
    window: WebviewWindow,
    state: State<'_, AppState>,
    alpha: f64,
) -> Result<f64, String> {
    let applied = native::set_alpha(&window, alpha)?;
    lock(&state.native)
        .entry(window.label().into())
        .or_default()
        .alpha = applied;
    Ok(applied)
}

#[tauri::command]
fn accessibility_opaque() -> bool {
    native::accessibility_opaque()
}

// -- the bridge ---------------------------------------------------------------------------------

/// The webview starts receiving its deliveries on `channel`. Calling it
/// again (a reload) starts over from the current state.
#[tauri::command]
fn bridge_attach(
    window: WebviewWindow,
    state: State<'_, AppState>,
    channel: Channel<InvokeResponseBody>,
) {
    state.bridge.attach(window.label(), move |frame: String| {
        channel.send(InvokeResponseBody::Json(frame)).is_ok()
    });
}

#[tauri::command]
fn bridge_ack(window: WebviewWindow, state: State<'_, AppState>, seq: u64) {
    if let Some(handle) = state.bridge.client().window(window.label()) {
        handle.ack(seq);
    }
}

#[tauri::command]
fn bridge_show_inbox(window: WebviewWindow, state: State<'_, AppState>, show: bool) {
    if let Some(handle) = state.bridge.client().window(window.label()) {
        handle.show_inbox(show);
    }
}

#[tauri::command]
fn bridge_view(window: WebviewWindow, state: State<'_, AppState>, thread_id: String) {
    if let Some(handle) = state.bridge.client().window(window.label()) {
        handle.view(ThreadId(thread_id));
    }
}

#[tauri::command]
fn bridge_unview(window: WebviewWindow, state: State<'_, AppState>, thread_id: String) {
    if let Some(handle) = state.bridge.client().window(window.label()) {
        handle.unview(&ThreadId(thread_id));
    }
}

/// Older history of a thread the window views; `None` when it doesn't.
#[tauri::command]
fn bridge_page_before(
    window: WebviewWindow,
    state: State<'_, AppState>,
    thread_id: String,
    before: u64,
    max_bytes: usize,
) -> Option<Page> {
    let handle = state.bridge.client().window(window.label())?;
    handle.page_before(
        &ThreadId(thread_id),
        EventId(before),
        max_bytes.min(MAX_LOADED_TRANSCRIPT_BYTES),
    )
}

#[tauri::command]
fn bridge_reconnect(state: State<'_, AppState>) {
    state.bridge.client().reconnect();
}

/// The one way a webview sends a command to the daemon. `command` is decoded
/// by serde into a [`DaemonCommand`]: anything else is refused before this
/// function runs.
#[tauri::command]
async fn daemon_command(
    state: State<'_, AppState>,
    command: DaemonCommand,
) -> Result<Value, CommandFailure> {
    let limit = bridge::deadline(&command);
    bridge::run_command(state.bridge.client(), command, limit).await
}

/// Asks the older daemon that refused this app to stop (contract §10): once
/// no turn is running, or now when `force` (the user confirmed). The pid is
/// the one the client holds, not one the webview names, and it is checked
/// against the daemon's lock file before anything is sent.
#[tauri::command]
fn replace_daemon(
    window: WebviewWindow,
    state: State<'_, AppState>,
    force: bool,
) -> Result<(), String> {
    if window.label() != MAIN {
        return Err("only the main window replaces the daemon".into());
    }
    let atrium_client::ConnectionState::Incompatible { daemon_pid, .. } = state.bridge.connection()
    else {
        return Err("the daemon is not an older one".into());
    };
    let how = if force {
        upgrade::Stop::Now
    } else {
        upgrade::Stop::WhenIdle
    };
    match upgrade::stop_old_daemon(&state.paths.lock, daemon_pid, how) {
        // Already gone: the next connection attempt starts the new one.
        Ok(()) | Err(upgrade::UpgradeError::Gone) => {
            state.bridge.client().reconnect();
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    let url = links::check(&url).map_err(|error| error.to_string())?;
    links::open(&url).map_err(|error| error.to_string())
}

// -- harness --------------------------------------------------------------------------------

/// Each window reports, after mounting and applying its appearance, what it
/// applied. The harness checks the count and every entry.
#[tauri::command]
fn window_ready(window: WebviewWindow, state: State<'_, AppState>, applied: Value) -> usize {
    let mut ready = lock(&state.ready);
    ready.insert(window.label().into(), applied);
    ready.len()
}

#[tauri::command]
fn ready_states(state: State<'_, AppState>) -> BTreeMap<String, Value> {
    lock(&state.ready).clone()
}

/// Echoes a harness report on stdout and, in a harness run, writes it to the
/// file named by `ATRIUM_BENCH_OUT`.
#[tauri::command]
fn write_report(state: State<'_, AppState>, json: String) -> Result<(), String> {
    println!("ATRIUM_REPORT {json}");
    if state.harness
        && let Some(path) = env("ATRIUM_BENCH_OUT")
    {
        shell::write_private(std::path::Path::new(&path), json.as_bytes())
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The windows that exist natively right now.
#[tauri::command]
fn window_labels(app: AppHandle) -> Vec<String> {
    let mut labels: Vec<String> = app.webview_windows().into_keys().collect();
    labels.sort();
    labels
}

#[tauri::command]
fn quit(app: AppHandle, window: WebviewWindow) -> Result<(), String> {
    if window.label() != MAIN {
        return Err("only the main window quits the app".into());
    }
    app.exit(0);
    Ok(())
}

// -- tray -----------------------------------------------------------------------------------

/// The tray item: a status line, "Show Atrium" and "Quit". `None` when the
/// machine has no tray (on Linux the indicator library may be missing, and
/// loading it panics).
fn build_tray(
    app: &AppHandle,
) -> Option<(tauri::tray::TrayIcon, tauri::menu::MenuItem<tauri::Wry>)> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let build = || -> tauri::Result<_> {
        let status = MenuItem::with_id(
            app,
            "status",
            presence::Presence::default().label(),
            false,
            None::<&str>,
        )?;
        let show = MenuItem::with_id(app, "show", "Show Atrium", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit Atrium", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&status, &show, &quit])?;
        let mut builder = TrayIconBuilder::with_id("atrium")
            .tooltip("Atrium")
            .menu(&menu)
            .on_menu_event(|app, event| match event.id.as_ref() {
                "show" => show_main(app),
                "quit" => app.exit(0),
                _ => {}
            });
        if let Some(icon) = app.default_window_icon() {
            builder = builder.icon(icon.clone());
        }
        Ok((builder.build(app)?, status))
    };
    // The panic message of a missing indicator library is not an error to
    // show: the app runs without a tray.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build));
    std::panic::set_hook(hook);
    match outcome {
        Ok(Ok(tray)) => Some(tray),
        Ok(Err(error)) => {
            eprintln!("atrium: no tray item: {error}");
            None
        }
        Err(_) => {
            eprintln!("atrium: no tray item: this machine has no tray support");
            None
        }
    }
}

fn show_main(app: &AppHandle) {
    if let Some(main) = app.get_webview_window(MAIN) {
        let _ = main.show();
        let _ = main.unminimize();
        let _ = main.set_focus();
    }
}

/// Keeps the tray's status line current from the tray's own client window.
fn follow_presence(
    app: &AppHandle,
    tray: tauri::tray::TrayIcon,
    status: tauri::menu::MenuItem<tauri::Wry>,
) {
    let client = app.state::<AppState>().bridge.client().clone();
    tauri::async_runtime::spawn(async move {
        let mut window = client.open_window(presence::TRAY_WINDOW);
        let handle = window.handle();
        handle.show_inbox(true);
        let mut tracker = presence::Tracker::default();
        let mut shown = None;
        while let Some(outgoing) = window.recv().await {
            if let Ok(delivery) = outgoing.delivery() {
                let now = tracker.apply(&delivery);
                if shown != Some(now) {
                    let label = now.label();
                    let _ = status.set_text(&label);
                    let _ = tray.set_tooltip(Some(&label));
                    shown = Some(now);
                }
            }
            handle.ack(outgoing.seq);
        }
    });
}

// -- the app --------------------------------------------------------------------------------

fn logical_bounds(window: &tauri::Window) -> Option<Bounds> {
    let scale = window.scale_factor().ok()?;
    let at = window.outer_position().ok()?.to_logical::<f64>(scale);
    let size = window.inner_size().ok()?.to_logical::<f64>(scale);
    Some(Bounds {
        x: at.x,
        y: at.y,
        width: size.width,
        height: size.height,
    })
}

fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    let app = window.app_handle();
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let label = window.label();
    match event {
        tauri::WindowEvent::CloseRequested { api, .. } if label == MAIN => {
            // With a tray the app stays reachable, so closing the main
            // window only hides it. Without one, closing it quits.
            if state.has_tray.load(Ordering::Relaxed) && !state.harness {
                api.prevent_close();
                let _ = window.hide();
            }
        }
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) if label != MAIN => {
            if let Some(bounds) = logical_bounds(window) {
                let mut shell = lock(&state.shell);
                if shell.set_bounds(label, bounds) {
                    state.save_session(&shell);
                }
            }
        }
        tauri::WindowEvent::Destroyed => {
            state.bridge.detach(label);
            lock(&state.ready).remove(label);
            lock(&state.native).remove(label);
            if label == MAIN {
                app.exit(0);
            } else {
                // Also the forced close and the crash of a pop-out: the view
                // goes back to main with what Rust holds for it.
                return_view(app, label);
            }
        }
        _ => {}
    }
}

fn setup(
    app: &mut tauri::App,
    main_epoch_ms: f64,
    paths: Paths,
) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    let harness = harness_run();
    let app_dir = bridge::app_dir(&paths)?;
    let (settings, session) = if harness {
        (
            harness_settings(
                env("ATRIUM_OPACITY").as_deref(),
                env("ATRIUM_THEME").as_deref(),
            ),
            Session::default(),
        )
    } else {
        (
            std::fs::read(app_dir.join(SETTINGS_FILE)).map_or_else(
                |_| Settings::default(),
                |bytes| shell::settings_from_bytes(&bytes),
            ),
            std::fs::read(app_dir.join(SESSION_FILE))
                .map_or_else(|_| Session::default(), |bytes| Session::from_bytes(&bytes)),
        )
    };

    // A development daemon learns which executable is the app from this file
    // (contract §4). It must exist before the first connection attempt.
    if cfg!(debug_assertions)
        && let Err(error) = bridge::write_dev_identity(&paths)
    {
        eprintln!("atrium: the development identity wasn't written: {error}");
    }
    let options = bridge::client_options(&paths);
    let bridge = tauri::async_runtime::block_on(async move { Bridge::start(options) });

    app.manage(AppState {
        main_epoch_ms,
        harness,
        paths: paths.clone(),
        shell: Mutex::new(ShellState::restore(settings.clone(), &session)),
        settings_file: (!harness).then(|| shell::json_persister(app_dir.join(SETTINGS_FILE))),
        session_file: (!harness).then(|| shell::json_persister(app_dir.join(SESSION_FILE))),
        bridge,
        native: Mutex::new(HashMap::new()),
        restore: Mutex::new(session.popouts.clone()),
        ready: Mutex::new(BTreeMap::new()),
        has_tray: AtomicBool::new(false),
    });

    let main = build_main(&handle, &settings)?;
    apply_native(&handle, &main, &settings, false);
    #[cfg(target_os = "macos")]
    macos::observe_accessibility(handle.clone());

    if !harness && let Some((tray, status)) = build_tray(&handle) {
        handle
            .state::<AppState>()
            .has_tray
            .store(true, Ordering::Relaxed);
        follow_presence(&handle, tray, status);
    }
    if harness {
        // As a launch from the Dock or a launcher does: a background window
        // gets throttled timers and no animation frames.
        let _ = main.set_focus();
    }
    Ok(())
}

/// Runs the app until it exits.
pub fn run() -> ExitCode {
    let main_epoch_ms = epoch_ms();
    let paths = match default_paths() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("atrium: {error}");
            return ExitCode::FAILURE;
        }
    };
    let built = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            startup_info,
            shell_patch_settings,
            shell_patch_view,
            shell_set_main_thread,
            pop_out,
            bring_back,
            force_bring_back,
            focus_popout,
            restore_windows,
            set_vibrancy,
            set_window_alpha,
            accessibility_opaque,
            bridge_attach,
            bridge_ack,
            bridge_show_inbox,
            bridge_view,
            bridge_unview,
            bridge_page_before,
            bridge_reconnect,
            daemon_command,
            replace_daemon,
            open_link,
            window_ready,
            ready_states,
            write_report,
            window_labels,
            quit
        ])
        .on_window_event(on_window_event)
        .setup(move |app| setup(app, main_epoch_ms, paths))
        .build(tauri::generate_context!());
    let app = match built {
        Ok(app) => app,
        Err(error) => {
            eprintln!("atrium: {error}");
            return ExitCode::FAILURE;
        }
    };
    app.run(|app, event| match event {
        tauri::RunEvent::Exit => {
            if let Some(state) = app.try_state::<AppState>() {
                // The last settings and drafts reach the disk, and the
                // daemon sees the connection close. The daemon keeps running.
                if let Some(file) = &state.settings_file {
                    file.shutdown();
                }
                if let Some(file) = &state.session_file {
                    file.shutdown();
                }
                state.bridge.client().shutdown();
            }
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => show_main(app),
        _ => {}
    });
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_settings_pin_the_configuration() {
        assert_eq!(harness_settings(None, None).alpha, 1.0);
        assert_eq!(harness_settings(Some("off"), None).alpha, 1.0);
        let background = harness_settings(Some("background"), Some("light"));
        assert_eq!(
            (background.alpha, background.mode, background.theme),
            (0.6, OpacityMode::Background, Theme::Light)
        );
        let whole = harness_settings(Some("whole"), Some("bogus"));
        assert_eq!((whole.mode, whole.theme), (OpacityMode::Whole, Theme::Dark));
    }
}
