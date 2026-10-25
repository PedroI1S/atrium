//! Rust-side cost and size of each batch encoding, plus golden fixtures for the TypeScript decoders.
//!
//!   cargo run --release --example stream-perf-encode-bench            # size + encode time table
//!   cargo run --release --example stream-perf-encode-bench -- --fixtures ../test-fixtures

use std::{path::PathBuf, time::Instant};

use atrium_stream_perf_lib::{
    corpus::Corpus,
    encode::{Encoding, Item, encode},
};
use spike_daemon_socket::{agent::*, wire::*};
use tauri::ipc::InvokeResponseBody;

fn stored(id: u64, thread: &str, event: AgentEvent) -> StoredEvent {
    StoredEvent {
        id: EventId(id),
        thread_id: ThreadId(thread.into()),
        envelope: AgentEventEnvelope {
            seq: id,
            at_ms: 1_760_000_000_000 + id,
            event,
        },
    }
}

fn delta(id: u64, thread: &str, text: String) -> StoredEvent {
    stored(
        id,
        thread,
        AgentEvent::MessageDelta {
            message_id: "1".into(),
            kind: MessageKind::Assistant,
            text,
        },
    )
}

fn body_len(b: &InvokeResponseBody) -> usize {
    match b {
        InvokeResponseBody::Json(s) => s.len(),
        InvokeResponseBody::Raw(v) => v.len(),
    }
}

fn fixtures(dir: PathBuf) {
    std::fs::create_dir_all(&dir).unwrap();
    let events = vec![
        stored(1, "t-a", AgentEvent::TurnStarted { turn: 1 }),
        delta(2, "t-a", "Hello, \"wor\\ld\"\n\n".into()),
        delta(3, "t-b", "caf\u{e9} \u{1f600} ok".into()),
        stored(
            4,
            "t-a",
            AgentEvent::ToolCallStarted {
                call_id: "c1".into(),
                name: "fake_read".into(),
                kind: ToolKind::Read,
                title: Some("read file".into()),
                input: serde_json::json!({"path": "a.rs", "n": 3}),
            },
        ),
        stored(
            5,
            "t-a",
            AgentEvent::ToolCallFinished {
                call_id: "c1".into(),
                status: ToolStatus::Succeeded,
                output: Some("done".into()),
                error: None,
            },
        ),
        stored(
            6,
            "t-b",
            AgentEvent::MessageCompleted {
                message_id: "1".into(),
                kind: MessageKind::Assistant,
                text: "caf\u{e9} \u{1f600} ok".into(),
            },
        ),
    ];
    std::fs::write(dir.join("events.json"), serde_json::to_string_pretty(&events).unwrap()).unwrap();
    let items: Vec<Item> = events
        .into_iter()
        .map(|event| Item {
            thread: u16::from(event.thread_id.0 == "t-b"),
            event,
        })
        .collect();
    for enc in Encoding::ALL {
        let path = dir.join(format!("batch.{}", enc.name()));
        match encode(enc, 42, 1_760_000_000_500.25, &items) {
            InvokeResponseBody::Json(s) => std::fs::write(path, s).unwrap(),
            InvokeResponseBody::Raw(b) => std::fs::write(path, b).unwrap(),
        }
    }
    println!("fixtures written to {}", dir.display());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--fixtures") {
        fixtures(PathBuf::from(&args[i + 1]));
        return;
    }
    let corpus = Corpus::new();
    println!("| delta chars | events/batch | encoding | bytes/batch | bytes/event | encode us/batch | encode ns/event |");
    println!("|---:|---:|---|---:|---:|---:|---:|");
    for chars in [4usize, 40, 200] {
        for count in [1usize, 16, 160, 1000] {
            let items: Vec<Item> = (0..count)
                .map(|i| Item {
                    thread: 0,
                    event: delta(i as u64 + 1, "t-a", corpus.slice(i * chars, chars)),
                })
                .collect();
            for enc in Encoding::ALL {
                let iters = (400_000 / count).max(200);
                let mut size = 0;
                let t = Instant::now();
                for n in 0..iters {
                    size = body_len(&encode(enc, n as u64, 1.0, &items));
                }
                let per_batch = t.elapsed().as_secs_f64() / iters as f64;
                println!(
                    "| {chars} | {count} | {} | {size} | {:.1} | {:.2} | {:.0} |",
                    enc.name(),
                    size as f64 / count as f64,
                    per_batch * 1e6,
                    per_batch * 1e9 / count as f64
                );
            }
        }
    }
}
