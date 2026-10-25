//! Backend-only screencast benchmark (no webview): Chromium over the CDP pipe, frames
//! consumed and acked in-process. It separates "what Chromium and the pipe can do" from
//! "what the Tauri IPC and WKWebView add" (the in-app bench in `src/bench.ts`).
//!
//! Prints one JSON object. Run in release mode:
//!
//! ```text
//! cargo run --release --manifest-path spikes/browser-screencast/src-tauri/Cargo.toml \
//!   --example screencast-bench -- --quality 80 --width 1280 --height 800 --dsf 1 --secs 10 --probes 100
//! ```
//!
//! Flags: `--quality N`, `--format jpeg|png`, `--width`, `--height`, `--dsf`, `--secs`,
//! `--probes`, `--every-nth N`, `--extra "<chromium flags>"`, `--idle-secs`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use atrium_browser_screencast_lib::browser::{Browser, FrameFormat, LaunchOptions, MouseParams, Page, ScreencastOptions};
use atrium_browser_screencast_lib::cdp::epoch_ms;
use atrium_browser_screencast_lib::fixture::Fixtures;
use atrium_browser_screencast_lib::{discovery, procstat};
use serde_json::{Value, json};

struct Args(Vec<String>);
impl Args {
    fn get<T: std::str::FromStr>(&self, name: &str, default: T) -> T {
        self.0
            .iter()
            .position(|a| a == name)
            .and_then(|i| self.0.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
    fn string(&self, name: &str) -> Option<String> {
        self.0.iter().position(|a| a == name).and_then(|i| self.0.get(i + 1)).cloned()
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn dist(mut v: Vec<f64>) -> Value {
    v.sort_by(f64::total_cmp);
    json!({
        "n": v.len(),
        "min": percentile(&v, 0.0),
        "p50": percentile(&v, 0.5),
        "p95": percentile(&v, 0.95),
        "max": percentile(&v, 1.0),
        "mean": v.iter().sum::<f64>() / v.len().max(1) as f64,
    })
}

async fn sample_for(pid: u32, secs: f64) -> Value {
    let mut samples = vec![];
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        let p = pid;
        samples.push(tokio::task::spawn_blocking(move || procstat::sample_tree(p)).await.unwrap());
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    serde_json::to_value(procstat::summarize(&samples)).unwrap()
}

/// Footprint (physical memory) and RSS of the whole tree, plus per process, at one moment.
fn stage(name: &str, pid: u32) -> Value {
    let rows = procstat::snapshot();
    let t = procstat::tree(&rows, pid);
    let per: Vec<Value> = t
        .iter()
        .map(|r| {
            json!({"pid": r.pid, "process": r.comm.rsplit('/').next().unwrap_or(&r.comm),
                   "rssMb": r.rss_kb / 1024, "footprintMb": procstat::footprint_mb(&[r.pid])})
        })
        .collect();
    let ids: Vec<u32> = t.iter().map(|r| r.pid).collect();
    json!({"stage": name, "processes": t.len(), "rssMbSum": t.iter().map(|r| r.rss_kb).sum::<u64>() / 1024,
           "footprintMbTotal": procstat::footprint_mb(&ids), "perProcess": per})
}

fn mouse(kind: &str, x: f64, y: f64) -> MouseParams {
    MouseParams {
        kind: kind.into(),
        x,
        y,
        button: Some("left".into()),
        buttons: Some(i32::from(kind == "mousePressed")),
        click_count: Some(1),
        modifiers: None,
        delta_x: None,
        delta_y: None,
    }
}

/// Is the pixel at (x, y) of this JPEG close to `rgb`? (PNG frames are not decoded here.)
fn pixel_matches(data: &[u8], png: bool, x: usize, y: usize, rgb: [u8; 3]) -> bool {
    if png {
        return false;
    }
    let mut d = jpeg_decoder::Decoder::new(data);
    let Ok(px) = d.decode() else { return false };
    let Some(info) = d.info() else { return false };
    let i = (y * info.width as usize + x) * 3;
    (0..3).all(|c| (i32::from(px[i + c]) - i32::from(rgb[c])).abs() < 70)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args(std::env::args().skip(1).collect());
    let quality: u8 = args.get("--quality", 80);
    let png = args.string("--format").as_deref() == Some("png");
    let (width, height): (u32, u32) = (args.get("--width", 1280), args.get("--height", 800));
    let dsf: f64 = args.get("--dsf", 1.0);
    let secs: f64 = args.get("--secs", 10.0);
    let idle_secs: f64 = args.get("--idle-secs", 10.0);
    let probes: usize = args.get("--probes", 100);
    let every_nth: u32 = args.get("--every-nth", 1);
    let extra: Vec<String> =
        args.string("--extra").map(|s| s.split_whitespace().map(str::to_string).collect()).unwrap_or_default();

    let info = discovery::pick(&discovery::discover(), None).ok_or_else(|| anyhow::anyhow!("no browser"))?;
    let dir = tempfile::tempdir()?;
    let browser: Arc<Browser> = Browser::launch(&LaunchOptions {
        binary: info.binary.clone(),
        user_data_dir: dir.path().join("profile"),
        log_path: Some(dir.path().join("stderr.log")),
        extra_args: extra.clone(),
    })
    .await?;
    let fixtures = Fixtures::start().await?;
    let mut memory_stages = vec![];
    tokio::time::sleep(Duration::from_secs(3)).await;
    memory_stages.push(stage("browser launched, about:blank only", browser.pid));
    let page: Arc<Page> = browser.new_page(&fixtures.url("/animation.html")).await?;
    page.set_viewport(width, height, dsf).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    memory_stages.push(stage("1 context (animation page)", browser.pid));
    let page_b = browser.new_page(&fixtures.url("/click.html")).await?;
    tokio::time::sleep(Duration::from_secs(3)).await;
    memory_stages.push(stage("2 contexts", browser.pid));
    drop(page_b);
    tokio::time::sleep(Duration::from_secs(2)).await; // settle: startup work, not the metric
    let opts = ScreencastOptions {
        format: if png { FrameFormat::Png } else { FrameFormat::Jpeg },
        quality,
        max_width: (f64::from(width) * dsf).ceil() as u32,
        max_height: (f64::from(height) * dsf).ceil() as u32,
        every_nth_frame: every_nth,
    };

    // 1. Idle: a static page, nobody watching (no screencast).
    page.navigate(&fixtures.url("/click.html")).await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let idle = sample_for(browser.pid, idle_secs).await;

    // 2. Streaming: the animation, frames acked as soon as they are read.
    page.navigate(&fixtures.url("/animation.html")).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut frames = page.start_screencast(opts).await?;
    let sampler = tokio::spawn({
        let pid = browser.pid;
        async move { sample_for(pid, secs).await }
    });
    let start = Instant::now();
    let mut arrivals: Vec<f64> = vec![];
    let mut sizes: Vec<f64> = vec![];
    let mut dims = (0.0, 0.0);
    let mut image_px = (0u16, 0u16);
    while start.elapsed().as_secs_f64() < secs {
        let Ok(Some(f)) = tokio::time::timeout(Duration::from_secs(2), frames.next()).await else { break };
        arrivals.push(f.rx_epoch_ms);
        sizes.push(f.data.len() as f64 / 1024.0);
        dims = (f.meta.device_width, f.meta.device_height);
        if image_px == (0, 0) && !png {
            let mut d = jpeg_decoder::Decoder::new(&f.data[..]);
            if d.read_info().is_ok() {
                let i = d.info().unwrap();
                image_px = (i.width, i.height);
            }
        }
        page.ack(f.ack_id);
    }
    let wall = start.elapsed().as_secs_f64();
    let streaming = sampler.await?;
    let intervals: Vec<f64> = arrivals.windows(2).map(|w| w[1] - w[0]).collect();
    let fps = arrivals.len() as f64 / wall;
    let raf_total = page.eval("window.__raf").await?.as_f64().unwrap_or(0.0);

    // 3. Static page with the screencast running (damage-driven: should send ~nothing).
    page.navigate(&fixtures.url("/click.html")).await?;
    tokio::time::sleep(Duration::from_secs(1)).await;
    while let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(300), frames.next()).await {
        page.ack(f.ack_id);
    }
    let static_sampler = tokio::spawn({
        let pid = browser.pid;
        async move { sample_for(pid, 5.0).await }
    });
    let static_start = Instant::now();
    let mut static_frames = 0u32;
    while static_start.elapsed() < Duration::from_secs(5) {
        if let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(250), frames.next()).await {
            static_frames += 1;
            page.ack(f.ack_id);
        }
    }
    let static_screencast = static_sampler.await?;

    // 4. Input-to-frame latency on the click page. Frames are collected first and decoded
    // afterwards, so JPEG decoding does not inflate the number: it is the time from just
    // before sending mousePressed to the arrival (at the pipe reader thread) of the first
    // frame whose probe square shows the new colour.
    let colours: [[u8; 3]; 4] = [[255, 0, 0], [0, 0, 255], [0, 255, 0], [255, 255, 255]];
    let (px, py) = ((100.0 * dsf) as usize, (100.0 * dsf) as usize);
    let mut lat = vec![];
    let mut misses = 0;
    for i in 0..probes {
        tokio::time::sleep(Duration::from_millis(120)).await;
        while let Ok(Some(f)) = tokio::time::timeout(Duration::from_millis(60), frames.next()).await {
            page.ack(f.ack_id); // drain, but keep the flow-control window open
        }
        let expected = colours[i % 4];
        let t0 = epoch_ms();
        page.mouse(&mouse("mousePressed", 100.0, 100.0)).await?;
        page.mouse(&mouse("mouseReleased", 100.0, 100.0)).await?;
        let mut candidates = vec![];
        let deadline = Instant::now() + Duration::from_millis(400);
        while let Ok(Some(f)) =
            tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), frames.next()).await
        {
            page.ack(f.ack_id);
            candidates.push(f);
            if candidates.len() >= 6 {
                break;
            }
        }
        match candidates.iter().find(|f| pixel_matches(&f.data, png, px, py, expected)) {
            Some(f) => lat.push(f.rx_epoch_ms - t0),
            None => misses += 1,
        }
    }

    let result = json!({
        "browser": info.name,
        "product": browser.version().await?,
        "options": {"quality": quality, "format": if png {"png"} else {"jpeg"}, "width": width, "height": height,
                    "deviceScaleFactor": dsf, "everyNthFrame": every_nth, "extraArgs": extra, "transport": "pipe"},
        "memoryStages": memory_stages,
        "idleNoScreencast": idle,
        "streaming": {
            "seconds": wall, "frames": arrivals.len(), "fps": fps,
            "frameIntervalMs": dist(intervals),
            "frameKiB": dist(sizes.clone()),
            "mbitPerSecond": sizes.iter().sum::<f64>() * 1024.0 * 8.0 / wall / 1e6,
            "frameCssSize": [dims.0, dims.1],
            "frameImagePixels": [image_px.0, image_px.1],
            "pageRafCountTotal": raf_total,
            "droppedEvents": frames.lagged,
            "cpuRss": streaming,
        },
        "staticPageWithScreencast": {"framesIn5s": static_frames, "cpuRss": static_screencast},
        "inputToFrameMs": {"backendOnly": true, "misses": misses, "dist": dist(lat)},
    });
    println!("{}", serde_json::to_string_pretty(&result)?);
    browser.close().await;
    Ok(())
}
