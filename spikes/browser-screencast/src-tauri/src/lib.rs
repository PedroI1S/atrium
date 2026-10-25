//! Atrium M0-08 spike: a Chromium owned by this backend, shown in a Tauri canvas.
//!
//! Data path of one frame:
//! Chromium `Page.screencastFrame` (base64 JPEG) -> CDP pipe reader thread -> base64 decode
//! -> 48-byte header + JPEG -> `tauri::ipc::Channel` (raw bytes) -> `createImageBitmap` ->
//! `<canvas>`. Input goes the other way through `Input.dispatch*`.

pub mod browser;
pub mod cdp;
pub mod control;
pub mod discovery;
pub mod fixture;
pub mod packet;
pub mod procstat;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

use browser::{Browser, FrameFormat, KeyParams, LaunchOptions, MouseParams, Page, ScreencastOptions};
use cdp::epoch_ms;
use control::Control;
use discovery::BrowserInfo;
use fixture::Fixtures;

pub const CONTEXT_LABELS: [&str; 2] = ["Context A", "Context B"];

struct Session {
    browser: Arc<Browser>,
    pages: Vec<Arc<Page>>,
    fixtures: Fixtures,
    info: StartInfo,
}

struct Stream {
    page: Arc<Page>,
    task: JoinHandle<()>,
}

struct SamplerRun {
    task: JoinHandle<()>,
    browser_samples: Arc<std::sync::Mutex<Vec<procstat::Sample>>>,
    app_samples: Arc<std::sync::Mutex<Vec<procstat::Sample>>>,
}

#[derive(Default)]
struct AppState {
    control: Control,
    session: RwLock<Option<Arc<Session>>>,
    stream: Mutex<Option<Stream>>,
    /// `true`: ack a frame as soon as it is on the channel. `false`: wait for the pane to
    /// ack after drawing (end-to-end back-pressure).
    eager_ack: AtomicBool,
    samplers: Mutex<HashMap<String, SamplerRun>>,
}

impl AppState {
    async fn session(&self) -> Result<Arc<Session>, String> {
        self.session.read().await.clone().ok_or_else(|| "browser not started".to_string())
    }

    async fn page(&self, context: usize) -> Result<(Arc<Session>, Arc<Page>), String> {
        let s = self.session().await?;
        let page = s.pages.get(context).cloned().ok_or_else(|| format!("no context {context}"))?;
        Ok((s, page))
    }
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Options for the automated harness, read from `ATRIUM_*` env vars. Absent in normal use.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupInfo {
    bench: Option<String>,
    quality: u8,
    format: String,
    device_scale_factor: Option<f64>,
    eager_ack: bool,
    clicks: u32,
    phase_secs: f64,
    probe_iterations: u32,
}

fn env_num<T: std::str::FromStr>(k: &str) -> Option<T> {
    std::env::var(k).ok().and_then(|v| v.parse().ok())
}

#[tauri::command]
fn startup_info(state: State<'_, AppState>) -> StartupInfo {
    let eager_ack = std::env::var("ATRIUM_ACK").map(|v| v == "eager").unwrap_or(false);
    state.eager_ack.store(eager_ack, Ordering::SeqCst);
    StartupInfo {
        bench: std::env::var("ATRIUM_BENCH").ok().filter(|v| !v.is_empty()),
        quality: env_num("ATRIUM_QUALITY").unwrap_or(80),
        format: std::env::var("ATRIUM_FORMAT").unwrap_or_else(|_| "jpeg".into()),
        device_scale_factor: env_num("ATRIUM_DSF"),
        eager_ack,
        clicks: env_num("ATRIUM_CLICKS").unwrap_or(100),
        phase_secs: env_num("ATRIUM_PHASE_SECS").unwrap_or(10.0),
        probe_iterations: env_num("ATRIUM_PROBES").unwrap_or(100),
    }
}

#[tauri::command]
fn browser_discover() -> Vec<BrowserInfo> {
    discovery::discover()
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StartInfo {
    browser: BrowserInfo,
    product: String,
    pid: u32,
    fixture_port: u16,
    user_data_dir: PathBuf,
    contexts: Vec<String>,
}

fn data_dir(app: &AppHandle) -> PathBuf {
    std::env::var_os("ATRIUM_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| app.path().app_data_dir().ok())
        .unwrap_or_else(|| std::env::temp_dir().join("atrium-browser-screencast"))
}

/// Launches Chromium (idempotent), serves the fixtures, and opens two isolated contexts.
#[tauri::command]
async fn browser_start(
    app: AppHandle,
    state: State<'_, AppState>,
    default_browser: Option<String>,
) -> Result<StartInfo, String> {
    if let Some(s) = state.session.read().await.clone() {
        return Ok(s.info.clone());
    }
    let found = discovery::discover();
    let picked = discovery::pick(&found, default_browser.as_deref()).ok_or("no Chromium-based browser found")?;
    let dir = data_dir(&app);
    let extra_args = std::env::var("ATRIUM_EXTRA_ARGS")
        .map(|v| v.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();
    let opts = LaunchOptions {
        binary: picked.binary.clone(),
        user_data_dir: browser::profile_dir(&dir, "pane"),
        log_path: Some(dir.join("browser-stderr.log")),
        extra_args,
    };
    let browser = Browser::launch(&opts).await.map_err(err)?;
    let fixtures = Fixtures::start().await.map_err(err)?;
    let mut pages = Vec::new();
    for _ in CONTEXT_LABELS {
        pages.push(browser.new_page(&fixtures.url("/")).await.map_err(err)?);
    }
    for (i, page) in pages.iter().enumerate() {
        spawn_nav_events(&app, i, page.clone());
    }
    let info = StartInfo {
        browser: picked,
        product: browser.version().await.map_err(err)?,
        pid: browser.pid,
        fixture_port: fixtures.port,
        user_data_dir: browser.user_data_dir.clone(),
        contexts: CONTEXT_LABELS.iter().map(|s| (*s).to_string()).collect(),
    };
    *state.session.write().await = Some(Arc::new(Session { browser, pages, fixtures, info: info.clone() }));
    Ok(info)
}

fn spawn_nav_events(app: &AppHandle, context: usize, page: Arc<Page>) {
    let app = app.clone();
    let mut rx = page.subscribe();
    tauri::async_runtime::spawn(async move {
        while let Ok(ev) = rx.recv().await {
            if ev.session_id.as_deref() != Some(&page.session_id) {
                continue;
            }
            let url = match ev.method.as_str() {
                "Page.frameNavigated" => {
                    let Ok(p) = ev.params::<Value>() else { continue };
                    if p["frame"].get("parentId").is_some() {
                        continue; // sub-frame
                    }
                    p["frame"]["url"].as_str().map(str::to_string)
                }
                "Page.navigatedWithinDocument" => {
                    ev.params::<Value>().ok().and_then(|p| p["url"].as_str().map(str::to_string))
                }
                _ => None,
            };
            if let Some(url) = url {
                let _ = app.emit("browser://nav", json!({"context": context, "url": url}));
            }
        }
    });
}

/// Starts the screencast of one context into `on_frame`, replacing any running stream.
#[tauri::command]
async fn pane_attach(
    state: State<'_, AppState>,
    context: usize,
    quality: Option<u8>,
    format: Option<String>,
    on_frame: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    let (_, page) = state.page(context).await?;
    stop_stream(&state).await;
    let png = format.as_deref() == Some("png");
    let opts = ScreencastOptions {
        format: if png { FrameFormat::Png } else { FrameFormat::Jpeg },
        quality: quality.unwrap_or(80),
        ..Default::default()
    };
    let mut frames = page.start_screencast(opts).await.map_err(err)?;
    let eager = state.eager_ack.load(Ordering::SeqCst);
    let task_page = page.clone();
    let task = tokio::spawn(async move {
        while let Some(f) = frames.next().await {
            let header = packet::Header {
                ack_id: f.ack_id as u32,
                context: context as u32,
                seq: f.seq,
                flags: if png { packet::FLAG_PNG } else { 0 },
                device_width: f.meta.device_width as f32,
                device_height: f.meta.device_height as f32,
                cdp_timestamp: f.meta.timestamp,
                rx_epoch_ms: f.rx_epoch_ms,
                sent_epoch_ms: epoch_ms(),
            };
            let wire = packet::encode(&header, &f.data);
            if on_frame.send(InvokeResponseBody::Raw(wire)).is_err() {
                break; // the webview is gone
            }
            if eager {
                task_page.ack(f.ack_id);
            }
        }
    });
    *state.stream.lock().await = Some(Stream { page, task });
    Ok(())
}

async fn stop_stream(state: &AppState) {
    if let Some(s) = state.stream.lock().await.take() {
        s.task.abort();
        let _ = s.page.stop_screencast().await;
    }
}

#[tauri::command]
async fn pane_detach(state: State<'_, AppState>) -> Result<(), String> {
    stop_stream(&state).await;
    Ok(())
}

/// The pane calls this after it has drawn a frame (not used when `ATRIUM_ACK=eager`).
#[tauri::command]
async fn pane_ack(state: State<'_, AppState>, ack_id: i64) -> Result<(), String> {
    if let Some(s) = state.stream.lock().await.as_ref() {
        s.page.ack(ack_id);
    }
    Ok(())
}

/// Sizes every context's viewport to the canvas: `Emulation.setDeviceMetricsOverride`.
#[tauri::command]
async fn pane_resize(state: State<'_, AppState>, width: u32, height: u32, device_scale_factor: f64) -> Result<(), String> {
    let s = state.session().await?;
    for p in &s.pages {
        p.set_viewport(width.max(1), height.max(1), device_scale_factor).await.map_err(err)?;
    }
    Ok(())
}

// ---- user input (the pane): only while the user has taken over ------------------------

#[tauri::command]
async fn pane_navigate(state: State<'_, AppState>, context: usize, url: String) -> Result<(), String> {
    state.control.require_user()?;
    let (_, page) = state.page(context).await?;
    // Do not wait for load: the pane keeps streaming while the page loads.
    let r = page.navigate_start(&url).await.map_err(err)?;
    match r["errorText"].as_str() {
        Some(e) => Err(format!("navigate: {e}")),
        None => Ok(()),
    }
}

#[tauri::command]
async fn pane_history(state: State<'_, AppState>, context: usize, action: String) -> Result<(), String> {
    state.control.require_user()?;
    let (_, page) = state.page(context).await?;
    match action.as_str() {
        "back" => page.history_step(-1).await,
        "forward" => page.history_step(1).await,
        "reload" => page.reload().await,
        other => Err(anyhow::anyhow!("unknown action {other}")),
    }
    .map_err(err)
}

#[tauri::command]
async fn pane_mouse(state: State<'_, AppState>, context: usize, params: MouseParams) -> Result<(), String> {
    state.control.require_user()?;
    let (_, page) = state.page(context).await?;
    page.mouse(&params).await.map_err(err)
}

#[tauri::command]
async fn pane_key(state: State<'_, AppState>, context: usize, params: KeyParams) -> Result<(), String> {
    state.control.require_user()?;
    let (_, page) = state.page(context).await?;
    page.key(&params).await.map_err(err)
}

/// IME: `Input.imeSetComposition` while composing, `Input.insertText` to commit.
#[tauri::command]
async fn pane_ime(state: State<'_, AppState>, context: usize, kind: String, text: String) -> Result<(), String> {
    state.control.require_user()?;
    let (_, page) = state.page(context).await?;
    let len = i32::try_from(text.encode_utf16().count()).unwrap_or(0);
    match kind.as_str() {
        "compose" => page.ime_set_composition(&text, len, len).await,
        "commit" => page.insert_text(&text).await,
        other => Err(anyhow::anyhow!("unknown ime kind {other}")),
    }
    .map_err(err)
}

// ---- take over --------------------------------------------------------------------------

#[tauri::command]
fn set_user_control(state: State<'_, AppState>, on: bool) {
    state.control.set_user_control(on);
}

#[tauri::command]
fn get_user_control(state: State<'_, AppState>) -> bool {
    state.control.user_has_control()
}

/// Stand-in for the browser MCP tools: an "agent" click or navigation. Rejected while the
/// user has control; this is where the real tools would check the same flag.
#[tauri::command]
async fn agent_action(
    state: State<'_, AppState>,
    context: usize,
    kind: String,
    x: Option<f64>,
    y: Option<f64>,
    url: Option<String>,
) -> Result<String, String> {
    state.control.require_agent()?;
    let (_, page) = state.page(context).await?;
    match kind.as_str() {
        "click" => {
            let (x, y) = (x.unwrap_or(100.0), y.unwrap_or(100.0));
            for kind in ["mousePressed", "mouseReleased"] {
                let p = MouseParams {
                    kind: kind.into(),
                    x,
                    y,
                    button: Some("left".into()),
                    buttons: Some(i32::from(kind == "mousePressed")),
                    click_count: Some(1),
                    modifiers: None,
                    delta_x: None,
                    delta_y: None,
                };
                page.mouse(&p).await.map_err(err)?;
            }
            Ok(format!("agent clicked ({x}, {y})"))
        }
        "navigate" => {
            let url = url.ok_or("url required")?;
            page.navigate(&url).await.map_err(err)?;
            Ok(format!("agent navigated to {url}"))
        }
        other => Err(format!("unknown agent action {other}")),
    }
}

// ---- measurement harness (ATRIUM_BENCH) -------------------------------------------------

fn require_bench() -> Result<(), String> {
    if std::env::var_os("ATRIUM_BENCH").is_some() { Ok(()) } else { Err("bench commands need ATRIUM_BENCH".into()) }
}

/// Evaluates JS in a page (bench only): used to read page-side counters.
#[tauri::command]
async fn bench_eval(state: State<'_, AppState>, context: usize, expression: String) -> Result<Value, String> {
    require_bench()?;
    let (_, page) = state.page(context).await?;
    page.eval(&expression).await.map_err(err)
}

#[tauri::command]
async fn bench_fixture_url(state: State<'_, AppState>, path: String) -> Result<String, String> {
    require_bench()?;
    Ok(state.session().await?.fixtures.url(&path))
}

#[tauri::command]
async fn bench_sampler_start(state: State<'_, AppState>, label: String) -> Result<(), String> {
    require_bench()?;
    let s = state.session().await?;
    let (browser_samples, app_samples) = (Arc::new(std::sync::Mutex::new(vec![])), Arc::new(std::sync::Mutex::new(vec![])));
    let (bs, as_) = (browser_samples.clone(), app_samples.clone());
    let (browser_pid, app_pid) = (s.browser.pid, std::process::id());
    let task = tokio::spawn(async move {
        loop {
            let (b, a) = tokio::task::spawn_blocking(move || {
                (procstat::sample_tree(browser_pid), procstat::sample_process(app_pid))
            })
            .await
            .unwrap();
            bs.lock().unwrap().push(b);
            as_.lock().unwrap().push(a);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
    state.samplers.lock().await.insert(label, SamplerRun { task, browser_samples, app_samples });
    Ok(())
}

#[tauri::command]
async fn bench_sampler_stop(state: State<'_, AppState>, label: String) -> Result<Value, String> {
    require_bench()?;
    let run = state.samplers.lock().await.remove(&label).ok_or("no such sampler")?;
    run.task.abort();
    let browser = procstat::summarize(&run.browser_samples.lock().unwrap());
    let app = procstat::summarize(&run.app_samples.lock().unwrap());
    Ok(json!({"label": label, "chromium": browser, "tauriAppProcessOnly": app}))
}

/// Writes the report (to `ATRIUM_BENCH_OUT` or stdout) and quits.
#[tauri::command]
async fn bench_finish(app: AppHandle, report: Value) -> Result<(), String> {
    require_bench()?;
    let text = serde_json::to_string_pretty(&report).map_err(err)?;
    match std::env::var_os("ATRIUM_BENCH_OUT") {
        Some(p) => std::fs::write(p, text).map_err(err)?,
        None => println!("{text}"),
    }
    app.exit(0);
    Ok(())
}

pub fn run() {
    let app = tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            startup_info,
            browser_discover,
            browser_start,
            pane_attach,
            pane_detach,
            pane_ack,
            pane_resize,
            pane_navigate,
            pane_history,
            pane_mouse,
            pane_key,
            pane_ime,
            set_user_control,
            get_user_control,
            agent_action,
            bench_eval,
            bench_fixture_url,
            bench_sampler_start,
            bench_sampler_stop,
            bench_finish,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the app");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // Close Chromium on the way out so no helper process is left behind.
            let state = handle.state::<AppState>();
            let session = tauri::async_runtime::block_on(async { state.session.write().await.take() });
            if let Some(s) = session {
                tauri::async_runtime::block_on(s.browser.close());
            }
        }
    });
}
