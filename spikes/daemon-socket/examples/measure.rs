//! Reproducible transport benchmark; seed outside the measured daemon so fixture
//! construction is not counted as daemon memory. Uses the real socket client.
use spike_daemon_socket::{Client, agent::*, store::Store, wire::*};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
struct Process(std::process::Child);
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let daemon = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: measure /path/to/spike-daemon [runs=3]")?,
    )
    .canonicalize()?;
    let runs = std::env::args()
        .nth(2)
        .unwrap_or("3".into())
        .parse::<usize>()?;
    for run in 1..=runs {
        println!("run={run}");
        measure(&daemon).await?;
    }
    Ok(())
}
fn memory(pid: u32, phase: &str) -> std::io::Result<()> {
    let rss = spike_daemon_socket::daemon::rss(pid)?;
    let footprint = spike_daemon_socket::daemon::phys_footprint(pid)?;
    println!(
        "phase={phase} rss_bytes={rss} rss_mib={:.3} phys_footprint_bytes={footprint:?}",
        rss as f64 / 1048576.
    );
    Ok(())
}
async fn measure(daemon: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::Builder::new()
        .prefix("a5-bench-")
        .tempdir_in("/tmp")?;
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700))?;
    let s = Store::open(&dir.path().join("events.sqlite"))?;
    s.call(|c| {
        c.execute("INSERT INTO threads(id,running) VALUES('bench',0)", [])
            .map_err(std::io::Error::other)?;
        Ok(())
    })
    .await?;
    for batch in 0..100 {
        s.append(
            ThreadId("bench".into()),
            (batch * 1000..(batch + 1) * 1000)
                .map(|seq| AgentEventEnvelope {
                    seq,
                    at_ms: 0,
                    event: AgentEvent::MessageDelta {
                        message_id: "m".into(),
                        kind: MessageKind::Assistant,
                        text: "x".repeat(200),
                    },
                })
                .collect(),
        )
        .await?;
    }
    drop(s);
    let diagnostics = std::fs::File::create(dir.path().join("memory.log"))?;
    let mut child = Process(
        Command::new(daemon)
            .arg("--dir")
            .arg(dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(diagnostics)
            .env("ATRIUM_MEMORY_DIAGNOSTICS", "1")
            .spawn()?,
    );
    let socket = dir.path().join("daemon.sock");
    use std::io::BufRead;
    let mut ready = String::new();
    std::io::BufReader::new(child.0.stdout.take().ok_or("no stdout")?).read_line(&mut ready)?;
    if ready.trim() != "READY" {
        return Err("daemon failed before ready".into());
    }
    let mut through = 0.;
    for after in [Some(EventId(99_000)), None] {
        let start = Instant::now();
        let mut c = Client::connect(&socket, "measure").await?;
        let r = c.subscribe(ThreadId("bench".into()), after).await?;
        if !matches!(r, CommandResult::Ok { .. }) {
            return Err(format!("{r:?}").into());
        }
        let mut count = 0;
        loop {
            match c.next().await? {
                ServerMessage::Event { event } => {
                    count += 1;
                    c.applied(&event.thread_id, event.id);
                }
                ServerMessage::ReplayDone { .. } => break,
                _ => (),
            }
        }
        let elapsed = start.elapsed();
        assert_eq!(count, if after.is_some() { 1000 } else { 100_000 });
        println!(
            "after={after:?} events={count} connect_to_caught_up_ms={:.3}",
            elapsed.as_secs_f64() * 1000.
        );
        if after.is_none() {
            through = count as f64 / elapsed.as_secs_f64();
        }
    }
    println!(
        "stored_event_sustained_delivery_events_per_second={through:.1} payload_chars_per_event=200"
    );
    memory(child.0.id(), "after_replay")?;
    // Also measure append + live delivery from a real fake child, at a target
    // 10,000 200-character deltas/sec. SQLite transactions may be the limiter.
    let mut c = Client::connect(&socket, "live-measure").await?;
    let CommandResult::Ok { value } = c
        .command(DaemonCommand::StartThread {
            agent: AgentLaunch::Fake {
                tokens_per_second: 500_000,
                chunk_chars: 200,
                duration_ms: 5000,
                tool_call_every_ms: 0,
            },
            cwd: "/tmp".into(),
        })
        .await?
    else {
        return Err("fake launch failed".into());
    };
    let thread = ThreadId(value["thread_id"].as_str().ok_or("thread missing")?.into());
    c.subscribe(thread.clone(), None).await?;
    let start = Instant::now();
    let mut deltas = 0;
    loop {
        if let ServerMessage::Event { event } = c.next().await? {
            let done = matches!(event.envelope.event, AgentEvent::SessionEnded { .. });
            if matches!(event.envelope.event, AgentEvent::MessageDelta { .. }) {
                deltas += 1;
            }
            c.applied(&event.thread_id, event.id);
            if done {
                break;
            }
        }
    }
    println!(
        "generator_target=10000_deltas_per_second live_delta_events={deltas} elapsed_s={:.3} observed_delivery_per_second={:.1}",
        start.elapsed().as_secs_f64(),
        deltas as f64 / start.elapsed().as_secs_f64()
    );
    // Wait on the durable lifecycle projection, not a guessed settling sleep.
    loop {
        let CommandResult::Ok { value } = c.command(DaemonCommand::ListThreads).await? else {
            return Err("ListThreads failed".into());
        };
        if value
            .as_array()
            .ok_or("invalid threads")?
            .iter()
            .any(|row| row["thread_id"] == thread.0 && row["running"] == false)
        {
            break;
        }
        tokio::task::yield_now().await;
    }
    drop(c);
    let idle_start = tokio::time::Instant::now();
    for seconds in [0, 3, 60] {
        tokio::time::sleep_until(idle_start + Duration::from_secs(seconds)).await;
        memory(child.0.id(), &format!("idle_{seconds}s"))?;
    }
    println!(
        "diagnostics:\n{}",
        std::fs::read_to_string(dir.path().join("memory.log"))?
    );
    Ok(())
}
