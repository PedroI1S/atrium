//! Batch encodings compared by the spike. One batch is one `tauri::ipc::Channel` message.
//!
//! - `json`: the daemon's own `StoredEvent` JSON, as a Tauri JSON channel message (Tauri parses it).
//! - `json-slim`: the same, with delta events reduced to `{t,i,th,a,m,k,s}`.
//! - `msgpack` / `msgpack-slim`: the same two shapes as MessagePack (named maps) in a raw message.
//! - `bin`: a hand-rolled little-endian layout (see below) in a raw message; non-delta events carry
//!   their JSON.
//!
//! `bin` layout (all integers little-endian):
//! ```text
//! header : 0xB1 | seq u32 | sent f64 | count u32
//! delta  : 0x00 | id f64 | thread u16 | at f64 | kind u8 | mlen u8 | message_id | tlen u32 | text
//! event  : 0x01 | thread u16 | jlen u32 | StoredEvent as JSON
//! ```
//! `id` and `at` are f64 so JavaScript reads them with one `getFloat64` (both stay below 2^53).

use serde::{Serialize, Serializer};
use spike_daemon_socket::{agent::*, wire::*};
use tauri::ipc::InvokeResponseBody;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    Json,
    JsonSlim,
    Msgpack,
    MsgpackSlim,
    Bin,
}

impl Encoding {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "json" => Self::Json,
            "json-slim" => Self::JsonSlim,
            "msgpack" => Self::Msgpack,
            "msgpack-slim" => Self::MsgpackSlim,
            "bin" => Self::Bin,
            _ => return None,
        })
    }

    pub const ALL: [Encoding; 5] = [
        Self::Json,
        Self::JsonSlim,
        Self::Msgpack,
        Self::MsgpackSlim,
        Self::Bin,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::JsonSlim => "json-slim",
            Self::Msgpack => "msgpack",
            Self::MsgpackSlim => "msgpack-slim",
            Self::Bin => "bin",
        }
    }
}

/// A stored event plus the small integer the bridge gave its thread.
pub struct Item {
    pub thread: u16,
    pub event: StoredEvent,
}

#[derive(Serialize)]
#[serde(tag = "t")]
enum Slim<'a> {
    #[serde(rename = "d")]
    Delta {
        i: u64,
        th: u16,
        a: u64,
        m: &'a str,
        k: u8,
        s: &'a str,
    },
    #[serde(rename = "e")]
    Event { th: u16, e: &'a StoredEvent },
}

fn kind_byte(kind: MessageKind) -> u8 {
    match kind {
        MessageKind::Assistant => 0,
        MessageKind::Reasoning => 1,
    }
}

fn slim(item: &Item) -> Slim<'_> {
    match &item.event.envelope.event {
        AgentEvent::MessageDelta {
            message_id,
            kind,
            text,
        } => Slim::Delta {
            i: item.event.id.0,
            th: item.thread,
            a: item.event.envelope.at_ms,
            m: message_id,
            k: kind_byte(*kind),
            s: text,
        },
        _ => Slim::Event {
            th: item.thread,
            e: &item.event,
        },
    }
}

struct Full<'a>(&'a [Item]);
impl Serialize for Full<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.iter().map(|i| &i.event))
    }
}

struct SlimSeq<'a>(&'a [Item]);
impl Serialize for SlimSeq<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(self.0.iter().map(slim))
    }
}

#[derive(Serialize)]
struct Batch<E> {
    seq: u64,
    sent: f64,
    events: E,
}

pub fn encode(enc: Encoding, seq: u64, sent_ms: f64, items: &[Item]) -> InvokeResponseBody {
    match enc {
        Encoding::Json => InvokeResponseBody::Json(
            serde_json::to_string(&Batch {
                seq,
                sent: sent_ms,
                events: Full(items),
            })
            .expect("json"),
        ),
        Encoding::JsonSlim => InvokeResponseBody::Json(
            serde_json::to_string(&Batch {
                seq,
                sent: sent_ms,
                events: SlimSeq(items),
            })
            .expect("json"),
        ),
        Encoding::Msgpack => InvokeResponseBody::Raw(
            rmp_serde::to_vec_named(&Batch {
                seq,
                sent: sent_ms,
                events: Full(items),
            })
            .expect("msgpack"),
        ),
        Encoding::MsgpackSlim => InvokeResponseBody::Raw(
            rmp_serde::to_vec_named(&Batch {
                seq,
                sent: sent_ms,
                events: SlimSeq(items),
            })
            .expect("msgpack"),
        ),
        Encoding::Bin => InvokeResponseBody::Raw(encode_bin(seq, sent_ms, items)),
    }
}

fn encode_bin(seq: u64, sent_ms: f64, items: &[Item]) -> Vec<u8> {
    let hint: usize = items
        .iter()
        .map(|i| match &i.event.envelope.event {
            AgentEvent::MessageDelta { text, .. } => 32 + text.len(),
            _ => 256,
        })
        .sum();
    let mut out = Vec::with_capacity(17 + hint);
    out.push(0xB1);
    out.extend_from_slice(&(seq as u32).to_le_bytes());
    out.extend_from_slice(&sent_ms.to_le_bytes());
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for item in items {
        match &item.event.envelope.event {
            AgentEvent::MessageDelta {
                message_id,
                kind,
                text,
            } if message_id.len() <= 255 => {
                out.push(0);
                out.extend_from_slice(&(item.event.id.0 as f64).to_le_bytes());
                out.extend_from_slice(&item.thread.to_le_bytes());
                out.extend_from_slice(&(item.event.envelope.at_ms as f64).to_le_bytes());
                out.push(kind_byte(*kind));
                out.push(message_id.len() as u8);
                out.extend_from_slice(message_id.as_bytes());
                out.extend_from_slice(&(text.len() as u32).to_le_bytes());
                out.extend_from_slice(text.as_bytes());
            }
            _ => {
                let json = serde_json::to_vec(&item.event).expect("json");
                out.push(1);
                out.extend_from_slice(&item.thread.to_le_bytes());
                out.extend_from_slice(&(json.len() as u32).to_le_bytes());
                out.extend_from_slice(&json);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(id: u64, text: &str) -> Item {
        Item {
            thread: 3,
            event: StoredEvent {
                id: EventId(id),
                thread_id: ThreadId("t-1".into()),
                envelope: AgentEventEnvelope {
                    seq: id,
                    at_ms: 1_700_000_000_123,
                    event: AgentEvent::MessageDelta {
                        message_id: "1".into(),
                        kind: MessageKind::Assistant,
                        text: text.into(),
                    },
                },
            },
        }
    }

    #[test]
    fn bin_header_and_delta_layout() {
        let items = [delta(7, "abcd")];
        let InvokeResponseBody::Raw(b) = encode(Encoding::Bin, 5, 1.5, &items) else {
            panic!("raw")
        };
        assert_eq!(b[0], 0xB1);
        assert_eq!(u32::from_le_bytes(b[1..5].try_into().unwrap()), 5);
        assert_eq!(f64::from_le_bytes(b[5..13].try_into().unwrap()), 1.5);
        assert_eq!(u32::from_le_bytes(b[13..17].try_into().unwrap()), 1);
        assert_eq!(b[17], 0);
        assert_eq!(f64::from_le_bytes(b[18..26].try_into().unwrap()), 7.0);
        assert_eq!(&b[b.len() - 4..], b"abcd");
    }

    #[test]
    fn every_encoding_produces_output_and_slim_is_smaller() {
        let items: Vec<Item> = (0..16).map(|i| delta(i, "abcd")).collect();
        let size = |e| match encode(e, 1, 2.0, &items) {
            InvokeResponseBody::Json(s) => s.len(),
            InvokeResponseBody::Raw(b) => b.len(),
        };
        assert!(size(Encoding::JsonSlim) < size(Encoding::Json));
        assert!(size(Encoding::MsgpackSlim) < size(Encoding::Msgpack));
        assert!(size(Encoding::Bin) < size(Encoding::MsgpackSlim));
    }
}
