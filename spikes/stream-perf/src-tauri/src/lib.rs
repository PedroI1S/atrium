//! Atrium M0-07 stream-perf spike: a Tauri shell around the M0-05 client. All UI logic is in the
//! frontend; the Rust side owns the daemon connection, batching and encoding (`bridge`), and a
//! few harness commands.

pub mod bridge;
pub mod corpus;
pub mod encode;

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::State;

struct Boot {
    main_epoch_ms: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartupInfo {
    main_epoch_ms: f64,
    /// JSON scenario text from `ATRIUM_BENCH`, if the harness launched us.
    bench: Option<String>,
    dir: Option<String>,
    daemon_exe: Option<String>,
}

#[tauri::command]
fn startup_info(boot: State<'_, Boot>) -> StartupInfo {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    StartupInfo {
        main_epoch_ms: boot.main_epoch_ms,
        bench: env("ATRIUM_BENCH"),
        dir: env("ATRIUM_BENCH_DIR"),
        daemon_exe: env("ATRIUM_DAEMON_EXE"),
    }
}

/// Writes the harness report (JSON) to `ATRIUM_BENCH_OUT`, if set.
#[tauri::command]
fn write_report(json: String) -> Result<(), String> {
    if let Ok(path) = std::env::var("ATRIUM_BENCH_OUT") {
        std::fs::write(path, json).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// A batch of `events` synthetic deltas of `chars` characters, encoded as `bridge` would, returned as
/// bytes (the JSON encodings as UTF-8). Used by the in-webview decode microbenchmark only.
#[tauri::command]
fn encode_sample(encoding: String, events: usize, chars: usize) -> Result<tauri::ipc::Response, String> {
    use encode::{Encoding, Item, encode};
    use spike_daemon_socket::{agent::*, wire::*};
    use tauri::ipc::InvokeResponseBody;
    let enc = Encoding::parse(&encoding).ok_or("unknown encoding")?;
    let corpus = corpus::Corpus::new();
    let items: Vec<Item> = (0..events)
        .map(|i| Item {
            thread: 0,
            event: StoredEvent {
                id: EventId(i as u64 + 1),
                thread_id: ThreadId("sample-thread".into()),
                envelope: AgentEventEnvelope {
                    seq: i as u64,
                    at_ms: 1_760_000_000_000,
                    event: AgentEvent::MessageDelta {
                        message_id: "1".into(),
                        kind: MessageKind::Assistant,
                        text: corpus.slice(i * chars, chars),
                    },
                },
            },
        })
        .collect();
    let bytes = match encode(enc, 0, 0.0, &items) {
        InvokeResponseBody::Json(s) => s.into_bytes(),
        InvokeResponseBody::Raw(b) => b,
    };
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

pub fn run() {
    let main_epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0);
    tauri::Builder::default()
        .manage(Boot { main_epoch_ms })
        .manage(bridge::Bridge::default())
        .invoke_handler(tauri::generate_handler![
            startup_info,
            write_report,
            quit,
            encode_sample,
            bridge::bridge_start,
            bridge::bridge_ack,
            bridge::bridge_stats,
            bridge::bridge_stop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Atrium stream-perf spike");
}
