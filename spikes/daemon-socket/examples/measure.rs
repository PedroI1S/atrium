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
            .ok_or("usage: measure /path/to/spike-daemon [idle_seconds=60]")?,
    )
    .canonicalize()?;
    let idle = std::env::args()
        .nth(2)
        .unwrap_or("60".into())
        .parse::<u64>()?;
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
    let child = Process(
        Command::new(daemon)
            .arg("--dir")
            .arg(dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let socket = dir.path().join("daemon.sock");
    for _ in 0..200 {
        if Client::connect(&socket, "ready").await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
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
    c.subscribe(thread, None).await?;
    let start = Instant::now();
    let mut deltas = 0;
    loop {
        if let ServerMessage::Event { event } = c.next().await? {
            let done = matches!(event.envelope.event, AgentEvent::TurnCompleted { .. });
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
        "live_delta_events={deltas} elapsed_s={:.3} live_delta_events_per_second={:.1}",
        start.elapsed().as_secs_f64(),
        deltas as f64 / start.elapsed().as_secs_f64()
    );
    drop(c);
    println!("idle_wait_seconds={idle} daemon_pid={}", child.0.id());
    tokio::time::sleep(Duration::from_secs(idle)).await;
    let bytes = spike_daemon_socket::daemon::rss(child.0.id())?;
    println!(
        "daemon_idle_rss_bytes={bytes} daemon_idle_rss_mib={:.3}",
        bytes as f64 / 1024. / 1024.
    );
    Ok(())
}
