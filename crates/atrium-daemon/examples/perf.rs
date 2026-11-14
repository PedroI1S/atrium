//! Contract §14's daemon numbers, against a real `atriumd` process:
//!
//! - reconnect with a cursor 1,000 events behind (20 runs: connect, `Hello`,
//!   `Subscribe`, every event, `ReplayDone`);
//! - a full replay of 100,000 events;
//! - idle RSS, 60 s after a 3,000,000-token synthetic session was closed
//!   (10 samples), with the peaks during the load and after the close.
//!
//! ```text
//! cargo build --profile perf -p atrium-daemon --bin atriumd --example perf
//! target/perf/examples/perf [--tokens 3000000] [--idle-secs 60]
//! ```
//!
//! The `perf` profile is `release` with debug assertions on, which is what
//! lets the daemon accept this harness as a `Dev` client (contract §4).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use atrium_protocol::PROTOCOL_VERSION;
use atrium_protocol::agent::SessionCommand;
use atrium_protocol::thread::{ThreadId, ThreadState, ThreadSummary};
use atrium_protocol::wire::{
    AgentLaunch, ClientMessage, ClientRole, CommandResult, DaemonCommand, EventId, ServerMessage,
};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

type Failure = Box<dyn std::error::Error>;

struct Wire {
    stream: BufReader<UnixStream>,
    next_request: u64,
}

impl Wire {
    async fn connect(socket: &Path) -> Result<Self, Failure> {
        let mut wire = Self {
            stream: BufReader::with_capacity(256 * 1024, UnixStream::connect(socket).await?),
            next_request: 0,
        };
        wire.send(&ClientMessage::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: "perf".into(),
            role: ClientRole::Dev,
        })
        .await?;
        match wire.read().await? {
            ServerMessage::Welcome { .. } => Ok(wire),
            other => Err(format!("not welcomed: {other:?}").into()),
        }
    }

    async fn send(&mut self, message: &ClientMessage) -> Result<(), Failure> {
        let payload = serde_json::to_vec(message)?;
        let mut frame = ((payload.len() + 1) as u32).to_be_bytes().to_vec();
        frame.push(0);
        frame.extend_from_slice(&payload);
        self.stream.get_mut().write_all(&frame).await?;
        Ok(())
    }

    /// One frame's payload.
    async fn frame(&mut self) -> Result<Vec<u8>, Failure> {
        let len = self.stream.read_u32().await? as usize;
        let _kind = self.stream.read_u8().await?;
        let mut payload = vec![0; len - 1];
        self.stream.read_exact(&mut payload).await?;
        Ok(payload)
    }

    async fn read(&mut self) -> Result<ServerMessage, Failure> {
        Ok(serde_json::from_slice(&self.frame().await?)?)
    }

    async fn command(&mut self, command: DaemonCommand) -> Result<Value, Failure> {
        self.next_request += 1;
        let request_id = self.next_request;
        self.send(&ClientMessage::Command {
            request_id,
            command,
        })
        .await?;
        loop {
            if let ServerMessage::Response {
                request_id: id,
                result,
            } = self.read().await?
                && id == request_id
            {
                return match result {
                    CommandResult::Ok { value } => Ok(value),
                    CommandResult::Err { code, message } => {
                        Err(format!("{code:?}: {message}").into())
                    }
                };
            }
        }
    }

    async fn thread(&mut self, thread: &ThreadId) -> Result<ThreadSummary, Failure> {
        let value = self
            .command(DaemonCommand::GetThread {
                thread_id: thread.clone(),
            })
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    /// Subscribes and reads to `ReplayDone`. Returns the events received.
    async fn replay(&mut self, thread: &ThreadId, after: Option<EventId>) -> Result<u64, Failure> {
        self.send(&ClientMessage::Subscribe {
            request_id: 1,
            thread_id: thread.clone(),
            after,
        })
        .await?;
        let mut events = 0;
        loop {
            // Events are counted, not decoded: this times the daemon and
            // the socket, and the client's own share is M1-06's number.
            let payload = self.frame().await?;
            if payload.starts_with(b"{\"type\":\"event\"") {
                events += 1;
            } else if payload.starts_with(b"{\"type\":\"replay_done\"") {
                return Ok(events);
            } else if let ServerMessage::Response {
                result: CommandResult::Err { code, message },
                ..
            } = serde_json::from_slice(&payload)?
            {
                return Err(format!("{code:?}: {message}").into());
            }
        }
    }
}

/// `(resident, peak resident)` of a process, in KiB.
fn rss_kib(pid: u32) -> (u64, u64) {
    let field = |status: &str, name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
    };
    if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
        let now = field(&status, "VmRSS:").unwrap_or(0);
        return (now, field(&status, "VmHWM:").unwrap_or(now));
    }
    // No procfs (macOS): `ps` gives the resident size only.
    let now = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok()?.trim().parse().ok())
        .unwrap_or(0);
    (now, now)
}

fn mib(kib: u64) -> f64 {
    kib as f64 / 1024.0
}

fn fake(tokens: u64, tokens_per_second: u32) -> AgentLaunch {
    AgentLaunch::Fake {
        tokens_per_second,
        // One token (four characters) a delta, as in the M0 measurements.
        chunk_chars: 4,
        duration_ms: tokens * 1000 / u64::from(tokens_per_second),
        tool_call_every_ms: 0,
    }
}

/// Starts a synthetic thread and waits until its turn is over. Returns the
/// thread, how long the turn took, and the daemon's peak RSS meanwhile.
async fn load(
    wire: &mut Wire,
    daemon: u32,
    tokens: u64,
) -> Result<(ThreadSummary, Duration, u64), Failure> {
    let began = Instant::now();
    let started = wire
        .command(DaemonCommand::StartThread {
            agent: fake(tokens, 200_000),
            message: Some("load".into()),
        })
        .await?;
    let thread: ThreadSummary = serde_json::from_value(started)?;
    let mut peak = 0;
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        peak = peak.max(rss_kib(daemon).0);
        let summary = wire.thread(&thread.thread_id).await?;
        if summary.state == ThreadState::Idle {
            return Ok((summary, began.elapsed(), peak));
        }
    }
}

async fn close(wire: &mut Wire, thread: &ThreadId) -> Result<(), Failure> {
    wire.command(DaemonCommand::ThreadCommand {
        thread_id: thread.clone(),
        command: SessionCommand::Shutdown,
    })
    .await?;
    // `Ended` is set at once; the closing events follow.
    let mut last = None;
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let summary = wire.thread(thread).await?;
        if summary.state == ThreadState::Ended && summary.last_event == last {
            return Ok(());
        }
        last = summary.last_event;
    }
}

struct Daemon {
    child: Child,
    home: tempfile::TempDir,
}

impl Daemon {
    fn socket(&self) -> PathBuf {
        self.home.path().join("daemon.sock")
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_daemon() -> Result<Daemon, Failure> {
    let me = std::env::current_exe()?;
    let binary = me
        .parent()
        .and_then(Path::parent)
        .ok_or("no target directory")?
        .join("atriumd");
    if !binary.exists() {
        return Err(format!("{} isn't built", binary.display()).into());
    }
    use std::os::unix::fs::PermissionsExt;
    // The daemon refuses a runtime directory that others can enter.
    let home = tempfile::Builder::new()
        .prefix("atrium-perf")
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")?;
    std::fs::create_dir_all(home.path().join("user"))?;
    let child = Command::new(binary)
        .arg("--dev-socket")
        .env("ATRIUM_HOME", home.path())
        .env("HOME", home.path().join("user"))
        .stdin(Stdio::null())
        .spawn()?;
    Ok(Daemon { child, home })
}

fn argument(name: &str, default: u64) -> u64 {
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == name {
            return arguments
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default);
        }
    }
    default
}

struct SnapshotInfo {
    name: String,
    bytes: u64,
    modified: SystemTime,
}

/// Waits for `upkeep::snapshot_daily`'s first snapshot to appear and settle.
async fn wait_for_snapshot(dir: &Path, timeout: Duration) -> Result<SnapshotInfo, Failure> {
    let deadline = Instant::now() + timeout;
    let mut stable: Option<(String, u64, SystemTime, u8)> = None;
    loop {
        if Instant::now() >= deadline {
            return Err("timed out waiting for the first daily snapshot".into());
        }
        let newest = std::fs::read_dir(dir).ok().and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let meta = e.metadata().ok()?;
                    if !meta.is_file() || meta.len() == 0 {
                        return None;
                    }
                    let modified = meta.modified().ok()?;
                    let name = e.file_name().to_string_lossy().into_owned();
                    Some((name, meta.len(), modified))
                })
                .max_by_key(|(_, _, modified)| *modified)
        });
        match (newest, &mut stable) {
            (Some((name, bytes, modified)), Some((s_name, s_bytes, s_mod, count)))
                if name == *s_name && bytes == *s_bytes && modified == *s_mod =>
            {
                *count += 1;
                if *count >= 4 {
                    // Let `record_snapshot` finish writing to SQLite after `VACUUM INTO`.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    return Ok(SnapshotInfo {
                        name,
                        bytes,
                        modified,
                    });
                }
            }
            (Some((name, bytes, modified)), _) => {
                stable = Some((name, bytes, modified, 1));
            }
            (None, _) => {
                stable = None;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Failure> {
    let tokens = argument("--tokens", 3_000_000);
    let idle_secs = argument("--idle-secs", 60);
    let daemon_started = Instant::now();
    let daemon = start_daemon()?;
    let pid = daemon.child.id();
    let socket = daemon.socket();
    let waited = Instant::now();
    while !socket.exists() {
        if waited.elapsed() > Duration::from_secs(10) {
            return Err("the daemon didn't start".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut wire = Wire::connect(&socket).await?;
    println!(
        "daemon pid {pid}, RSS at start {:.1} MiB",
        mib(rss_kib(pid).0)
    );

    // ---- A full replay of 100,000 events ----
    let (small, took, _) = load(&mut wire, pid, 100_000).await?;
    println!("\n100k-token thread: stored in {:.2} s", took.as_secs_f64());
    for run in 1..=3 {
        let began = Instant::now();
        let mut fresh = Wire::connect(&socket).await?;
        let events = fresh.replay(&small.thread_id, None).await?;
        let took = began.elapsed();
        println!(
            "full replay, run {run}: {events} events in {:.1} ms ({:.0} events/s)",
            took.as_secs_f64() * 1000.0,
            events as f64 / took.as_secs_f64()
        );
    }

    // ---- The large session ----
    let (large, took, peak_load) = load(&mut wire, pid, tokens).await?;
    let last = large.last_event.ok_or("the thread has no events")?;
    let (after_load, high_water_load) = rss_kib(pid);
    println!(
        "\n{tokens}-token thread: stored in {:.1} s ({:.0} events/s), peak RSS during the load {:.1} MiB (sampled every 250 ms), RSS right after {:.1} MiB, the process's peak so far {:.1} MiB",
        took.as_secs_f64(),
        tokens as f64 / took.as_secs_f64(),
        mib(peak_load),
        mib(after_load),
        mib(high_water_load)
    );

    // ---- Reconnect with a cursor 1,000 events behind ----
    let cursor = EventId(last.0 - 1000);
    let mut runs = Vec::new();
    for _ in 0..20 {
        let began = Instant::now();
        let mut fresh = Wire::connect(&socket).await?;
        let events = fresh.replay(&large.thread_id, Some(cursor)).await?;
        runs.push(began.elapsed());
        if events != 1000 {
            return Err(format!("expected 1000 events, got {events}").into());
        }
        drop(fresh);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    runs.sort();
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
    println!(
        "\nreconnect, cursor 1,000 behind, 20 runs: min {:.2} ms, median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        ms(runs[0]),
        ms(runs[runs.len() / 2]),
        ms(runs[(runs.len() * 95).div_ceil(100) - 1]),
        ms(runs[runs.len() - 1])
    );

    // ---- Close, wait for the first daily snapshot, then idle ----
    close(&mut wire, &large.thread_id).await?;
    close(&mut wire, &small.thread_id).await?;
    drop(wire);
    let (after_close, high_water) = rss_kib(pid);
    println!(
        "\nsessions closed, clients gone: RSS {:.1} MiB (the process's peak so far {:.1} MiB)",
        mib(after_close),
        mib(high_water)
    );
    let snapshot = wait_for_snapshot(
        &daemon.home.path().join("snapshots"),
        Duration::from_secs(180),
    )
    .await?;
    let window_start = SystemTime::now();
    let snapshot_ms = snapshot
        .modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let window_ms = window_start
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let (after_snapshot, high_water_snapshot) = rss_kib(pid);
    println!(
        "snapshot finished: {} ({:.0} MiB, modified {} ms); RSS {:.1} MiB (peak {:.1} MiB)",
        snapshot.name,
        snapshot.bytes as f64 / (1024.0 * 1024.0),
        snapshot_ms,
        mib(after_snapshot),
        mib(high_water_snapshot)
    );
    println!(
        "idle window starts at {} ms ({:.1} s after daemon start), waiting {idle_secs} s…",
        window_ms,
        daemon_started.elapsed().as_secs_f64()
    );
    tokio::time::sleep(Duration::from_secs(idle_secs)).await;
    let mut samples = Vec::new();
    for _ in 0..10 {
        samples.push(rss_kib(pid).0);
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let shown: Vec<String> = samples
        .iter()
        .map(|kib| format!("{:.1}", mib(*kib)))
        .collect();
    samples.sort_unstable();
    println!(
        "idle RSS, 10 samples from {idle_secs} s after the last activity (snapshot included): median {:.1} MiB, max {:.1} MiB [{}]",
        mib((samples[4] + samples[5]) / 2),
        mib(samples[9]),
        shown.join(", ")
    );
    let store = daemon.home.path().join("atrium.db");
    if let Ok(meta) = std::fs::metadata(&store) {
        println!(
            "store file: {:.0} MiB",
            meta.len() as f64 / (1024.0 * 1024.0)
        );
    }

    let mut wire = Wire::connect(&socket).await?;
    wire.command(DaemonCommand::ShutdownDaemon { force: true })
        .await?;
    Ok(())
}
