//! One Chromium process owned by this backend, driven over `--remote-debugging-pipe`.
//!
//! * `Browser::launch` spawns the browser with a dedicated profile and wires fds 3 and 4.
//! * `Browser::new_page` creates an isolated `Target.createBrowserContext`, one page in it,
//!   and a flat session. Two of those share one browser process but not cookies or storage.
//! * `Page` wraps the per-page commands this spike needs: navigation, viewport, the
//!   screencast (`Page.startScreencast` + ack) and the `Input.*` family.

use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast};

use crate::cdp::{Cdp, CdpEvent};
use crate::profile::{self, ProfileLock, Reaped};

#[derive(Debug, Clone)]
pub struct LaunchOptions {
    pub binary: PathBuf,
    /// Dedicated profile. Chrome refuses remote debugging on the default profile, and we
    /// would never want to touch the user's own anyway.
    pub user_data_dir: PathBuf,
    /// Where Chromium's stderr goes (kept for diagnosing launch problems).
    pub log_path: Option<PathBuf>,
    pub extra_args: Vec<String>,
}

impl LaunchOptions {
    pub fn args(&self) -> Vec<String> {
        let mut a = vec![
            "--headless=new".to_string(),
            "--remote-debugging-pipe".to_string(),
            format!("--user-data-dir={}", self.user_data_dir.display()),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--mute-audio".into(),
            "--hide-scrollbars".into(),
        ];
        a.extend(self.extra_args.iter().cloned());
        a.push("about:blank".into());
        a
    }
}

/// `pipe(2)` with both ends close-on-exec and moved above fd 10, so the `dup2` onto fds
/// 3 and 4 in the child can never collide with them.
fn make_pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0i32; 2];
    // SAFETY: `fds` is a valid out-array of two ints.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let lift = |fd: i32| -> std::io::Result<OwnedFd> {
        // SAFETY: `fd` is open; F_DUPFD_CLOEXEC returns a fresh fd >= 10 or -1.
        let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) };
        let err = std::io::Error::last_os_error();
        // SAFETY: closing the original, which we own.
        unsafe { libc::close(fd) };
        if n < 0 {
            return Err(err);
        }
        // SAFETY: `n` is a valid fd we exclusively own.
        Ok(unsafe { OwnedFd::from_raw_fd(n) })
    };
    Ok((lift(fds[0])?, lift(fds[1])?))
}

pub struct Browser {
    pub cdp: Cdp,
    pub pid: u32,
    pub binary: PathBuf,
    pub user_data_dir: PathBuf,
    child: Mutex<Option<tokio::process::Child>>,
    /// Held for the browser's lifetime: proves this process owns the profile.
    lock: ProfileLock,
}

impl Browser {
    pub async fn launch(opts: &LaunchOptions) -> Result<Arc<Self>> {
        // Refuses (instead of killing anything) when another live Atrium owns this profile.
        let lock = ProfileLock::acquire(&opts.user_data_dir)?;
        match profile::reap_stale(&lock) {
            Reaped::Killed(pid) => eprintln!("reaped orphaned browser {pid} from a previous run"),
            Reaped::Refused(why) => eprintln!("left a leftover browser record alone: {why}"),
            Reaped::Nothing => {}
        }
        // Parent -> child commands (child fd 3) and child -> parent responses (child fd 4).
        let (cmd_r, cmd_w) = make_pipe()?;
        let (resp_r, resp_w) = make_pipe()?;

        let mut command = tokio::process::Command::new(&opts.binary);
        command.args(opts.args()).stdin(Stdio::null()).stdout(Stdio::null()).kill_on_drop(true);
        match &opts.log_path {
            Some(p) => {
                command.stderr(Stdio::from(File::create(p).context("create browser log")?));
            }
            None => {
                command.stderr(Stdio::null());
            }
        }
        let (child_in, child_out) = (cmd_r.as_raw_fd(), resp_w.as_raw_fd());
        // SAFETY: only async-signal-safe calls (`dup2`) run between fork and exec.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(child_in, 3) < 0 || libc::dup2(child_out, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().with_context(|| format!("spawn {}", opts.binary.display()))?;
        let pid = child.id().ok_or_else(|| anyhow!("browser exited immediately"))?;
        if let Some(record) = profile::describe(pid, &opts.user_data_dir) {
            profile::write_record(&lock, &record).context("write browser record")?;
        }
        // The parent must not keep the child's ends open, or EOF would never be seen.
        drop(cmd_r);
        drop(resp_w);

        let cdp = Cdp::new(File::from(resp_r), File::from(cmd_w));
        let browser = Arc::new(Self {
            cdp,
            pid,
            binary: opts.binary.clone(),
            user_data_dir: opts.user_data_dir.clone(),
            child: Mutex::new(Some(child)),
            lock,
        });
        // Fails fast (instead of hanging) when the browser refused to start.
        let _version: Value = tokio::time::timeout(
            Duration::from_secs(20),
            browser.cdp.call("Browser.getVersion", json!({}), None),
        )
        .await
        .map_err(|_| anyhow!("browser did not answer Browser.getVersion in 20 s"))?
        .map_err(|e| anyhow!("Browser.getVersion: {e}"))?;
        Ok(browser)
    }

    pub async fn version(&self) -> Result<String> {
        let v: Value = self.cdp.call("Browser.getVersion", json!({}), None).await?;
        Ok(v["product"].as_str().unwrap_or("?").to_string())
    }

    /// New isolated context (own cookie jar, localStorage, cache) with one page in it.
    pub async fn new_page(&self, url: &str) -> Result<Arc<Page>> {
        let ctx: Value = self.cdp.call("Target.createBrowserContext", json!({"disposeOnDetach": true}), None).await?;
        let context_id = ctx["browserContextId"].as_str().context("no browserContextId")?.to_string();
        let target: Value = self
            .cdp
            .call("Target.createTarget", json!({"url": "about:blank", "browserContextId": context_id}), None)
            .await?;
        let target_id = target["targetId"].as_str().context("no targetId")?.to_string();
        let attached: Value = self
            .cdp
            .call("Target.attachToTarget", json!({"targetId": target_id, "flatten": true}), None)
            .await?;
        let session_id = attached["sessionId"].as_str().context("no sessionId")?.to_string();
        let page = Arc::new(Page { cdp: self.cdp.clone(), context_id, target_id, session_id });
        page.call("Page.enable", json!({})).await?;
        // Headless pages in a background context otherwise think they are unfocused.
        page.call("Emulation.setFocusEmulationEnabled", json!({"enabled": true})).await?;
        if url != "about:blank" {
            page.navigate(url).await?;
        }
        Ok(page)
    }

    pub async fn close(&self) {
        let _ = tokio::time::timeout(Duration::from_secs(3), self.cdp.call_raw("Browser.close", json!({}), None)).await;
        let mut guard = self.child.lock().await;
        if let Some(child) = guard.as_mut()
            && tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err()
        {
            let _ = child.kill().await;
        }
        *guard = None;
        profile::clear_record(&self.lock);
    }
}


pub struct Page {
    pub cdp: Cdp,
    pub context_id: String,
    pub target_id: String,
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    Jpeg,
    Png,
}

#[derive(Debug, Clone, Copy)]
pub struct ScreencastOptions {
    pub format: FrameFormat,
    pub quality: u8,
    pub max_width: u32,
    pub max_height: u32,
    pub every_nth_frame: u32,
}

impl Default for ScreencastOptions {
    fn default() -> Self {
        Self { format: FrameFormat::Jpeg, quality: 80, max_width: 4096, max_height: 4096, every_nth_frame: 1 }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FrameMeta {
    #[serde(default)]
    pub offset_top: f64,
    #[serde(default)]
    pub page_scale_factor: f64,
    pub device_width: f64,
    pub device_height: f64,
    #[serde(default)]
    pub scroll_offset_x: f64,
    #[serde(default)]
    pub scroll_offset_y: f64,
    /// Seconds since the Unix epoch, stamped by Chromium when the frame was produced.
    #[serde(default)]
    pub timestamp: f64,
}

pub struct Frame {
    /// Pass back to `Page::ack`.
    pub ack_id: i64,
    pub meta: FrameMeta,
    pub data: Vec<u8>,
    pub seq: u32,
    /// When the backend's reader thread received the frame (ms since the Unix epoch).
    pub rx_epoch_ms: f64,
}

#[derive(Deserialize)]
struct RawFrame {
    data: String,
    metadata: FrameMeta,
    #[serde(rename = "sessionId")]
    session_id: i64,
}

pub struct FrameStream {
    rx: broadcast::Receiver<Arc<CdpEvent>>,
    session_id: String,
    seq: u32,
    /// Events dropped because this consumer fell behind the 1024-event buffer.
    pub lagged: u64,
}

impl FrameStream {
    /// Next frame, or `None` once the browser connection closed.
    pub async fn next(&mut self) -> Option<Frame> {
        loop {
            let ev = match self.rx.recv().await {
                Ok(ev) => ev,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    self.lagged += n;
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            };
            if ev.method != "Page.screencastFrame" || ev.session_id.as_deref() != Some(&self.session_id) {
                continue;
            }
            let Ok(raw) = ev.params::<RawFrame>() else { continue };
            let Ok(data) = B64.decode(raw.data.as_bytes()) else { continue };
            self.seq = self.seq.wrapping_add(1);
            return Some(Frame {
                ack_id: raw.session_id,
                meta: raw.metadata,
                data,
                seq: self.seq,
                rx_epoch_ms: ev.received_epoch_ms,
            });
        }
    }
}

/// Parameters of `Input.dispatchMouseEvent` (also what the pane sends over IPC).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MouseParams {
    /// `mousePressed`, `mouseReleased`, `mouseMoved` or `mouseWheel`.
    #[serde(rename = "type")]
    pub kind: String,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buttons: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub click_count: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta_y: Option<f64>,
}

/// Parameters of `Input.dispatchKeyEvent`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyParams {
    /// `keyDown`, `keyUp`, `rawKeyDown` or `char`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unmodified_text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows_virtual_key_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_repeat: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<i32>,
    /// macOS editing commands (`selectAll`, `copy`, ...), which the renderer needs
    /// because Cmd+key combos are not delivered as plain key events there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
}

impl Page {
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        Ok(self.cdp.call(method, params, Some(&self.session_id)).await?)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<CdpEvent>> {
        self.cdp.subscribe()
    }

    /// `Page.navigate`, returning the raw result. The first navigation shortly after launch
    /// is sometimes answered with `net::ERR_ABORTED` (seen reliably in the app, about 1 in 12
    /// in a headless probe; cause not found). One retry after 300 ms always succeeded.
    ///
    /// `still_allowed` is checked before the retry, so a retry never fires after the caller lost
    /// the right to drive (the user took over while we slept).
    pub async fn navigate_start(&self, url: &str, still_allowed: impl Fn() -> Result<(), String>) -> Result<Value> {
        let mut r = self.call("Page.navigate", json!({"url": url})).await?;
        if r["errorText"].as_str() == Some("net::ERR_ABORTED") {
            tokio::time::sleep(Duration::from_millis(300)).await;
            still_allowed().map_err(anyhow::Error::msg)?;
            r = self.call("Page.navigate", json!({"url": url})).await?;
        }
        Ok(r)
    }

    /// Navigates and waits for `Page.loadEventFired` (15 s cap).
    pub async fn navigate(&self, url: &str) -> Result<()> {
        self.navigate_cancellable(url, || Ok(())).await
    }

    /// Like `navigate`, but `check` is polled while waiting and before any retry; an `Err` aborts
    /// the wait (used so an agent navigation never outlives a user take-over).
    pub async fn navigate_cancellable(&self, url: &str, check: impl Fn() -> Result<(), String>) -> Result<()> {
        let mut rx = self.subscribe();
        let r = self.navigate_start(url, &check).await?;
        if let Some(err) = r["errorText"].as_str() {
            bail!("navigate {url}: {err}");
        }
        // A same-document navigation (hash change) has no loaderId and fires no load event.
        if r.get("loaderId").is_none() {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            check().map_err(anyhow::Error::msg)?;
            let slice = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await;
            match slice {
                Ok(Ok(ev)) if ev.method == "Page.loadEventFired" && ev.session_id.as_deref() == Some(&self.session_id) => {
                    return Ok(());
                }
                Ok(Err(broadcast::error::RecvError::Closed)) => return Ok(()),
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("load of {url} timed out");
            }
        }
    }

    pub async fn eval(&self, expression: &str) -> Result<Value> {
        let r = self
            .call(
                "Runtime.evaluate",
                json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
            )
            .await?;
        if let Some(ex) = r.get("exceptionDetails") {
            bail!("eval failed: {}", ex["exception"]["description"].as_str().unwrap_or("exception"));
        }
        Ok(r["result"]["value"].clone())
    }

    pub async fn set_viewport(&self, width: u32, height: u32, device_scale_factor: f64) -> Result<()> {
        self.call(
            "Emulation.setDeviceMetricsOverride",
            json!({"width": width, "height": height, "deviceScaleFactor": device_scale_factor, "mobile": false}),
        )
        .await?;
        Ok(())
    }

    pub async fn history_step(&self, delta: i32) -> Result<()> {
        let h = self.call("Page.getNavigationHistory", json!({})).await?;
        let cur = h["currentIndex"].as_i64().unwrap_or(0) + i64::from(delta);
        let Some(entry) = usize::try_from(cur).ok().and_then(|i| h["entries"].get(i)) else {
            return Ok(()); // nothing to go back/forward to
        };
        self.call("Page.navigateToHistoryEntry", json!({"entryId": entry["id"]})).await?;
        Ok(())
    }

    pub async fn reload(&self) -> Result<()> {
        self.call("Page.reload", json!({})).await?;
        Ok(())
    }

    pub async fn start_screencast(&self, o: ScreencastOptions) -> Result<FrameStream> {
        // Subscribe first so the first frame cannot be missed.
        let rx = self.subscribe();
        self.call(
            "Page.startScreencast",
            json!({
                "format": if o.format == FrameFormat::Jpeg { "jpeg" } else { "png" },
                "quality": o.quality,
                "maxWidth": o.max_width,
                "maxHeight": o.max_height,
                "everyNthFrame": o.every_nth_frame,
            }),
        )
        .await?;
        Ok(FrameStream { rx, session_id: self.session_id.clone(), seq: 0, lagged: 0 })
    }

    pub async fn stop_screencast(&self) -> Result<()> {
        self.call("Page.stopScreencast", json!({})).await?;
        Ok(())
    }

    /// Lets Chromium produce the next frame. Without it the stream stalls after a few.
    pub fn ack(&self, ack_id: i64) {
        let _ = self.cdp.notify("Page.screencastFrameAck", json!({"sessionId": ack_id}), Some(&self.session_id));
    }

    pub async fn mouse(&self, p: &MouseParams) -> Result<()> {
        self.cdp.call_raw("Input.dispatchMouseEvent", p, Some(&self.session_id)).await?;
        Ok(())
    }

    pub async fn key(&self, p: &KeyParams) -> Result<()> {
        self.cdp.call_raw("Input.dispatchKeyEvent", p, Some(&self.session_id)).await?;
        Ok(())
    }

    pub async fn ime_set_composition(&self, text: &str, selection_start: i32, selection_end: i32) -> Result<()> {
        self.call(
            "Input.imeSetComposition",
            json!({"text": text, "selectionStart": selection_start, "selectionEnd": selection_end}),
        )
        .await?;
        Ok(())
    }

    pub async fn insert_text(&self, text: &str) -> Result<()> {
        self.call("Input.insertText", json!({"text": text})).await?;
        Ok(())
    }

    pub async fn bring_to_front(&self) -> Result<()> {
        self.call("Page.bringToFront", json!({})).await?;
        Ok(())
    }
}

/// `<data dir>/profile-<name>`: one dedicated profile per run purpose.
pub fn profile_dir(data_dir: &Path, name: &str) -> PathBuf {
    data_dir.join(format!("profile-{name}"))
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    use super::*;

    /// A browser stand-in that answers every command with `{}` until it is dropped.
    fn answer_everything(server: UnixStream) {
        std::thread::spawn(move || {
            let mut s = server;
            let mut acc = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let Ok(n) = s.read(&mut buf) else { return };
                if n == 0 {
                    return;
                }
                acc.extend_from_slice(&buf[..n]);
                while let Some(p) = acc.iter().position(|&b| b == 0) {
                    let line: Vec<u8> = acc.drain(..=p).collect();
                    let v: Value = serde_json::from_slice(&line[..line.len() - 1]).unwrap();
                    let mut out = serde_json::to_vec(&json!({"id": v["id"], "result": {}})).unwrap();
                    out.push(0);
                    if s.write_all(&out).is_err() {
                        return;
                    }
                }
            }
        });
    }

    /// Before the fix a `FrameStream` waiting for a frame never returned once the browser was
    /// gone, because the event channel's sender lived on inside the `Cdp` the page still held.
    #[tokio::test]
    async fn frame_stream_ends_when_the_browser_connection_closes() {
        let (client, server) = UnixStream::pair().unwrap();
        let keep = server.try_clone().unwrap();
        answer_everything(server);
        let cdp = Cdp::new(client.try_clone().unwrap(), client);
        let page = Page { cdp, context_id: "c".into(), target_id: "t".into(), session_id: "s".into() };
        let mut frames = page.start_screencast(ScreencastOptions::default()).await.unwrap();

        let waiter = tokio::spawn(async move { frames.next().await.map(|f| f.seq) });
        tokio::time::sleep(Duration::from_millis(50)).await;
        keep.shutdown(std::net::Shutdown::Both).unwrap(); // the browser "exits"
        let got = tokio::time::timeout(Duration::from_secs(2), waiter).await.expect("must not wait forever").unwrap();
        assert_eq!(got, None);
        // And the page's own calls now fail fast instead of hanging.
        assert!(page.call("Page.reload", json!({})).await.is_err());
    }
}
