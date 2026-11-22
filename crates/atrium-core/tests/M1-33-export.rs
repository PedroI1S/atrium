//! M1-25 finding 1: export with one damaged message and another still open at the end.
//! Moved from M1-28's test file by M1-33 (D-136).

use atrium_core::history::{ExportReport, Redactor, export_thread_report};
use atrium_protocol::agent::{AgentEvent, AgentEventEnvelope, MessageBody, MessageKind};
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{EventId, ExportFormat, StoredEvent};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[test]
fn m1_25_export_with_damaged_and_still_open_message() {
    fn sha256(text: &str) -> String {
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    let thread_id = ThreadId("th-m1-28-export".into());
    let mut seq = 1u64;
    let mut push = |event: AgentEvent| -> StoredEvent {
        let s = StoredEvent {
            id: EventId(seq),
            thread_id: thread_id.clone(),
            envelope: AgentEventEnvelope {
                incarnation: 1,
                seq,
                at_ms: 1_700_000_000_000 + seq,
                event,
            },
        };
        seq += 1;
        s
    };

    let damaged_body = MessageBody::Streamed {
        chars: 99,
        deltas: 2,
        sha256: sha256("other-words"),
    };
    let events = vec![
        push(AgentEvent::MessageCompleted {
            message_id: "u1".into(),
            kind: MessageKind::User,
            body: MessageBody::Inline {
                text: "Hello".into(),
            },
        }),
        push(AgentEvent::MessageDelta {
            message_id: "m-damaged".into(),
            kind: MessageKind::Assistant,
            text: "SECRET_DAMAGED_PART_1 ".into(),
        }),
        push(AgentEvent::MessageDelta {
            message_id: "m-damaged".into(),
            kind: MessageKind::Assistant,
            text: "SECRET_DAMAGED_PART_2".into(),
        }),
        push(AgentEvent::MessageCompleted {
            message_id: "m-damaged".into(),
            kind: MessageKind::Assistant,
            body: damaged_body,
        }),
        push(AgentEvent::MessageDelta {
            message_id: "m-open".into(),
            kind: MessageKind::Assistant,
            text: "Still streaming ".into(),
        }),
        push(AgentEvent::MessageDelta {
            message_id: "m-open".into(),
            kind: MessageKind::Assistant,
            text: "without completion".into(),
        }),
    ];

    let redactor = Redactor::new("/Users/test", &[]);

    let mut md_buf = Vec::new();
    let md_report = export_thread_report(
        &mut events.clone().into_iter(),
        ExportFormat::Markdown,
        &redactor,
        &mut md_buf,
    )
    .expect("markdown export");
    let md = String::from_utf8(md_buf).expect("utf8 md");
    assert_eq!(
        md_report,
        ExportReport {
            bytes: md.len() as u64,
            damaged: 1,
        }
    );
    assert!(
        md.contains("Damaged message (id `m-damaged`, events 2–4)"),
        "{md}"
    );
    assert!(!md.contains("SECRET_DAMAGED_PART_1"), "{md}");
    assert!(!md.contains("SECRET_DAMAGED_PART_2"), "{md}");
    assert!(md.contains("Still streaming without completion"), "{md}");

    let mut jsonl_buf = Vec::new();
    let jsonl_report = export_thread_report(
        &mut events.into_iter(),
        ExportFormat::Jsonl,
        &redactor,
        &mut jsonl_buf,
    )
    .expect("jsonl export");
    let jsonl = String::from_utf8(jsonl_buf).expect("utf8 jsonl");
    assert_eq!(
        jsonl_report,
        ExportReport {
            bytes: jsonl.len() as u64,
            damaged: 1,
        }
    );
    assert!(!jsonl.contains("SECRET_DAMAGED_PART_1"), "{jsonl}");
    assert!(!jsonl.contains("SECRET_DAMAGED_PART_2"), "{jsonl}");

    let lines: Vec<Value> = jsonl
        .lines()
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect();
    let damaged: Vec<&Value> = lines
        .iter()
        .filter(|l| l.get("damaged") == Some(&json!(true)))
        .collect();
    assert_eq!(damaged.len(), 1, "{jsonl}");
    assert_eq!(damaged[0]["message_id"], "m-damaged");

    let healthy: Vec<StoredEvent> = lines
        .into_iter()
        .filter(|l| l.get("damaged") != Some(&json!(true)))
        .map(|l| serde_json::from_value(l).expect("healthy line parses as StoredEvent"))
        .collect();
    let open_deltas: String = healthy
        .iter()
        .filter_map(|e| match &e.envelope.event {
            AgentEvent::MessageDelta {
                message_id, text, ..
            } if message_id == "m-open" => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(open_deltas, "Still streaming without completion");
}
