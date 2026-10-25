//! Probe for the `chromiumoxide` verdict: same measurements as `screencast-bench`, but
//! driven through chromiumoxide 0.9.1. chromiumoxide has no `--remote-debugging-pipe`
//! transport (WebSocket only), so this launches Helium on `--remote-debugging-port` (it
//! picks a free port and reads the URL from stderr) and measures what that costs.
//!
//! cargo run --release --manifest-path spikes/browser-screencast/src-tauri/Cargo.toml \
//!   --example screencast-oxide-probe -- --quality 80 --secs 10 --probes 50

use std::time::{Duration, Instant};

use atrium_browser_screencast_lib::fixture::Fixtures;
use atrium_browser_screencast_lib::{discovery, procstat};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::input::{DispatchMouseEventParams, DispatchMouseEventType, MouseButton};
use chromiumoxide::cdp::browser_protocol::page::{
    EventScreencastFrame, ScreencastFrameAckParams, StartScreencastFormat, StartScreencastParams,
};
use chromiumoxide::cdp::browser_protocol::target::{CreateBrowserContextParams, CreateTargetParams};
use futures::StreamExt;
use serde_json::json;

fn ms_now() -> f64 {
    atrium_browser_screencast_lib::cdp::epoch_ms()
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(f64::total_cmp);
    v.get(((v.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(f64::NAN)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |n: &str, d: f64| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(d);
    let (quality, secs, probes) = (get("--quality", 80.0) as i64, get("--secs", 10.0), get("--probes", 50.0) as usize);

    let info = discovery::pick(&discovery::discover(), None).ok_or_else(|| anyhow::anyhow!("no browser"))?;
    let dir = tempfile::tempdir()?;
    let launched = Instant::now();
    let (mut browser, mut handler) = Browser::launch(
        BrowserConfig::builder()
            .chrome_executable(&info.binary)
            .user_data_dir(dir.path().join("profile"))
            .new_headless_mode()
            .build()
            .map_err(anyhow::Error::msg)?,
    )
    .await?;
    let launch_ms = launched.elapsed().as_secs_f64() * 1000.0;
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    let fixtures = Fixtures::start().await?;
    let ctx = browser.create_browser_context(CreateBrowserContextParams::default()).await?;
    let page = browser
        .new_page(CreateTargetParams::builder().url(fixtures.url("/animation.html")).browser_context_id(ctx).build().map_err(anyhow::Error::msg)?)
        .await?;
    page.execute(SetDeviceMetricsOverrideParams::new(1280, 800, 1.0, false)).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut frames = page.event_listener::<EventScreencastFrame>().await?;
    page.execute(StartScreencastParams::builder().format(StartScreencastFormat::Jpeg).quality(quality).max_width(1280).max_height(800).build()).await?;
    let start = Instant::now();
    let (mut n, mut bytes) = (0u32, 0usize);
    while start.elapsed().as_secs_f64() < secs {
        let Ok(Some(f)) = tokio::time::timeout(Duration::from_secs(2), frames.next()).await else { break };
        n += 1;
        let b64: &str = f.data.as_ref();
        bytes += b64.len() / 4 * 3; // chromiumoxide leaves the frame base64-encoded
        page.execute(ScreencastFrameAckParams::new(f.session_id)).await?;
    }
    let fps = f64::from(n) / start.elapsed().as_secs_f64();

    page.goto(fixtures.url("/click.html")).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let colours: [[u8; 3]; 4] = [[255, 0, 0], [0, 0, 255], [0, 255, 0], [255, 255, 255]];
    let mut lat = vec![];
    for i in 0..probes {
        tokio::time::sleep(Duration::from_millis(120)).await;
        while tokio::time::timeout(Duration::from_millis(60), frames.next()).await.is_ok() {}
        let t0 = ms_now();
        for kind in [DispatchMouseEventType::MousePressed, DispatchMouseEventType::MouseReleased] {
            page.execute(DispatchMouseEventParams::builder().r#type(kind).x(100.0).y(100.0).button(MouseButton::Left).click_count(1).build().map_err(anyhow::Error::msg)?).await?;
        }
        let deadline = Instant::now() + Duration::from_millis(400);
        let mut cands = vec![];
        while let Ok(Some(f)) = tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), frames.next()).await {
            let rx = ms_now();
            let _ = page.execute(ScreencastFrameAckParams::new(f.session_id)).await;
            cands.push((rx, f));
            if cands.len() >= 6 {
                break;
            }
        }
        for (rx, f) in &cands {
            let b64: &str = f.data.as_ref();
            let Ok(jpeg) = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64) else { continue };
            let mut d = jpeg_decoder::Decoder::new(&jpeg[..]);
            if let Ok(px) = d.decode() {
                let w = d.info().unwrap().width as usize;
                let k = (100 * w + 100) * 3;
                if (0..3).all(|c| (i32::from(px[k + c]) - i32::from(colours[i % 4][c])).abs() < 70) {
                    lat.push(rx - t0);
                    break;
                }
            }
        }
    }
    let pid = procstat::snapshot().into_iter().filter(|r| r.comm.contains("Helium") && r.ppid > 1).map(|r| r.ppid).next();
    let result = json!({
        "driver": "chromiumoxide 0.9.1 (WebSocket on --remote-debugging-port; no pipe transport)",
        "launchMs": launch_ms, "fps": fps, "frames": n, "meanFrameKiB": bytes as f64 / f64::from(n.max(1)) / 1024.0,
        "inputToFrameMs": {"n": lat.len(), "p50": pct(&mut lat.clone(), 0.5), "p95": pct(&mut lat.clone(), 0.95), "max": pct(&mut lat, 1.0)},
        "note": pid.map(|p| format!("a parent pid seen: {p}")),
    });
    println!("{}", serde_json::to_string_pretty(&result)?);
    browser.close().await?;
    browser.wait().await?;
    Ok(())
}
