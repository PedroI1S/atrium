//! The frontend's delivery types (`app/src/daemon/types.ts`) are written by
//! hand, and its reducer is tested against the examples in
//! `app/src/daemon/delivery-shapes.json`. This test ties those examples to
//! the Rust types: each one must decode as a [`Delivery`] and encode back to
//! exactly the same JSON, and together they must cover every variant. A
//! change to `Delivery`, `ConnectionState`, a thread summary or an inbox item
//! fails here until the examples (and the TypeScript types) follow.

use std::collections::BTreeSet;

use atrium_client::Delivery;
use serde_json::Value;

const SHAPES: &str = include_str!("../../src/daemon/delivery-shapes.json");

fn kind(value: &Value) -> String {
    let outer = value["type"].as_str().unwrap_or("?");
    match value["state"]["type"].as_str() {
        Some(state) if outer == "connection" => format!("connection/{state}"),
        _ => outer.to_string(),
    }
}

#[test]
fn every_example_is_a_delivery_and_encodes_back_unchanged() {
    let examples: Vec<Value> = serde_json::from_str(SHAPES).expect("the examples are a JSON array");
    for example in &examples {
        let delivery: Delivery = serde_json::from_value(example.clone())
            .unwrap_or_else(|error| panic!("{} is not a delivery: {error}", kind(example)));
        let encoded = serde_json::to_value(&delivery).expect("a delivery encodes");
        assert_eq!(&encoded, example, "{} changed shape", kind(example));
    }
}

#[test]
fn the_examples_cover_every_variant() {
    let examples: Vec<Value> = serde_json::from_str(SHAPES).expect("the examples are a JSON array");
    let seen: BTreeSet<String> = examples.iter().map(kind).collect();
    let expected = [
        "connection/closed",
        "connection/connected",
        "connection/connecting",
        "connection/failed",
        "connection/incompatible",
        "event",
        "inbox",
        "inbox_updated",
        "snapshot",
        "thread_failed",
        "thread_removed",
        "thread_updated",
    ];
    assert_eq!(seen, expected.iter().map(|name| name.to_string()).collect());

    // The list above is checked against the types: a new variant doesn't
    // compile here until it is given an example.
    for example in &examples {
        let delivery: Delivery = serde_json::from_value(example.clone()).expect("a delivery");
        match delivery {
            Delivery::Connection { state } => match state {
                atrium_client::ConnectionState::Connecting { .. }
                | atrium_client::ConnectionState::Connected { .. }
                | atrium_client::ConnectionState::Incompatible { .. }
                | atrium_client::ConnectionState::Failed { .. }
                | atrium_client::ConnectionState::Closed => {}
            },
            Delivery::Inbox { .. }
            | Delivery::ThreadUpdated { .. }
            | Delivery::InboxUpdated { .. }
            | Delivery::ThreadRemoved { .. }
            | Delivery::Snapshot { .. }
            | Delivery::Event { .. }
            | Delivery::ThreadFailed { .. } => {}
        }
    }
}
