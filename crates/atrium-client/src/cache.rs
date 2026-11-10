//! One thread's history, as the client holds it.
//!
//! The daemon stores every event; a long stream is millions of them. The
//! client keeps the same history **folded**, so its size follows the
//! transcript and not the event count:
//!
//! - a message's `MessageDelta`s become one delta with the text so far;
//! - a tool call's `ToolCallUpdated`s become one update (later fields win,
//!   `output` replaces, `output_delta` appends);
//! - `Usage` within a turn, `ContextUpdated` and `QuotaUpdated` keep only the
//!   latest within a segment (see below), where the segment's first one was.
//!
//! Applying the folded entries in order gives the same state as applying the
//! original events. An entry keeps the id of the first event folded into it,
//! so ids rise along the list.
//!
//! Entries are grouped into **segments**: an entry starts a segment when no
//! message, tool call or request was open before it. Snapshots, pages and
//! eviction work in whole segments, so none of them begins in the middle of
//! an item (a tool result without its call, a completion without its text).
//!
//! The cache holds at most `max_bytes` (counted as the events' size on the
//! wire). Past that the oldest segments are dropped and the cache is
//! `truncated`: that history is still in the daemon, but the wire has no way
//! to read a range of it (see the handoff).

use std::collections::{HashMap, HashSet, VecDeque};

use atrium_protocol::agent::AgentEvent;
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{EventId, StoredEvent};
use serde::Serialize;
use serde::ser::{SerializeSeq, Serializer};

struct Entry {
    event: StoredEvent,
    /// Its size as counted against budgets.
    bytes: usize,
    /// Starts a segment.
    boundary: bool,
}

/// What is open in the current incarnation, as entry numbers (the entry's
/// index plus the number of entries dropped before it).
#[derive(Default)]
struct Open {
    messages: HashMap<String, u64>,
    /// An open call, and its update entry once it has one.
    calls: HashMap<String, Option<u64>>,
    requests: HashSet<String>,
    usage: Option<u64>,
    context: Option<u64>,
    quota: Option<u64>,
}

impl Open {
    fn idle(&self) -> bool {
        self.messages.is_empty() && self.calls.is_empty() && self.requests.is_empty()
    }
}

/// A run of entries chosen for a snapshot or a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    start: usize,
    end: usize,
    pub(crate) bytes: usize,
}

pub(crate) struct ThreadCache {
    entries: VecDeque<Entry>,
    /// Entries dropped from the front.
    base: u64,
    /// The entry number that starts the last segment. Folding never reaches
    /// behind it: a snapshot of the newest segments must hold every value
    /// that arrived within them.
    segment_start: u64,
    bytes: usize,
    max_bytes: usize,
    last: Option<EventId>,
    truncated: bool,
    incarnation: u32,
    next_seq: Option<u64>,
    gaps: u64,
    open: Open,
}

impl ThreadCache {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            base: 0,
            segment_start: 0,
            bytes: 0,
            max_bytes,
            last: None,
            truncated: false,
            incarnation: 0,
            next_seq: None,
            gaps: 0,
            open: Open::default(),
        }
    }

    /// Forgets everything, to load the thread again from its start.
    pub(crate) fn reset(&mut self) {
        *self = Self::new(self.max_bytes);
    }

    /// The last event applied: the cursor to subscribe from.
    pub(crate) fn last(&self) -> Option<EventId> {
        self.last
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    /// Events whose sequence number didn't follow the one before, within an
    /// incarnation. Should stay 0; kept for diagnostics.
    pub(crate) fn gaps(&self) -> u64 {
        self.gaps
    }

    /// Applies one event; `bytes` is its size on the wire. `false` for an
    /// event at or before the cursor (a repeat), which changes nothing.
    pub(crate) fn append(&mut self, stored: StoredEvent, bytes: usize) -> bool {
        if self.last.is_some_and(|last| stored.id <= last) {
            return false;
        }
        self.last = Some(stored.id);
        if stored.envelope.incarnation != self.incarnation {
            // A new provider process: nothing of the old one can continue,
            // and its sequence starts again at 0. (The cache's very first
            // event is taken as it comes.)
            self.next_seq = (self.incarnation != 0).then_some(0);
            self.incarnation = stored.envelope.incarnation;
            self.open = Open::default();
        }
        if self
            .next_seq
            .is_some_and(|next| next != stored.envelope.seq)
        {
            self.gaps += 1;
        }
        self.next_seq = Some(stored.envelope.seq.saturating_add(1));

        // The hot path: one more delta of a message that is streaming.
        if let AgentEvent::MessageDelta {
            message_id, text, ..
        } = &stored.envelope.event
            && let Some(&number) = self.open.messages.get(message_id)
            && let Some(entry) = entry_mut(&mut self.entries, self.base, number)
            && let AgentEvent::MessageDelta { text: joined, .. } = &mut entry.event.envelope.event
        {
            joined.push_str(text);
            entry.bytes += text.len();
            self.bytes += text.len();
            self.trim();
            return true;
        }

        let boundary = self.open.idle();
        match kind_of(&stored.envelope.event) {
            Kind::Delta(id) => {
                let number = self.push(stored, bytes, boundary);
                self.open.messages.insert(id, number);
            }
            Kind::Completed(id) => {
                self.push(stored, bytes, boundary);
                self.open.messages.remove(&id);
            }
            Kind::CallStarted(id) => {
                self.push(stored, bytes, boundary);
                self.open.calls.insert(id, None);
            }
            Kind::CallUpdated(id) => {
                let update = self.open.calls.get(&id).copied().flatten();
                match update.and_then(|number| entry_mut(&mut self.entries, self.base, number)) {
                    Some(entry) => {
                        let (grown, shrunk) = merge_update(&mut entry.event.envelope.event, stored);
                        let merged = (entry.bytes + grown).saturating_sub(shrunk);
                        self.bytes = self.bytes.saturating_sub(entry.bytes) + merged;
                        entry.bytes = merged;
                    }
                    None => {
                        let number = self.push(stored, bytes, boundary);
                        self.open.calls.insert(id, Some(number));
                    }
                }
            }
            Kind::CallFinished(id) => {
                self.push(stored, bytes, boundary);
                self.open.calls.remove(&id);
            }
            Kind::RequestOpened(id) => {
                self.push(stored, bytes, boundary);
                self.open.requests.insert(id);
            }
            Kind::RequestResolved(id) => {
                self.push(stored, bytes, boundary);
                self.open.requests.remove(&id);
            }
            Kind::Latest(slot) => {
                // Only within the last segment: an older entry may lie
                // outside the snapshot a window is given, which would then
                // never show this value.
                let number = (*slot.of(&mut self.open)).filter(|n| *n >= self.segment_start);
                match number.and_then(|number| entry_mut(&mut self.entries, self.base, number)) {
                    Some(entry) => {
                        self.bytes = self.bytes.saturating_sub(entry.bytes) + bytes;
                        entry.bytes = bytes;
                        // Its place and id stay; the content is the newer one.
                        entry.event.envelope = stored.envelope;
                    }
                    None => {
                        let number = self.push(stored, bytes, boundary);
                        *slot.of(&mut self.open) = Some(number);
                    }
                }
            }
            Kind::TurnEdge => {
                // `Usage` is a running total per turn.
                self.open.usage = None;
                self.push(stored, bytes, boundary);
            }
            Kind::SessionEnded => {
                self.push(stored, bytes, boundary);
                self.open = Open::default();
            }
            Kind::Other => {
                self.push(stored, bytes, boundary);
            }
        }
        self.trim();
        true
    }

    fn push(&mut self, event: StoredEvent, bytes: usize, boundary: bool) -> u64 {
        let number = self.base + self.entries.len() as u64;
        // The first entry always starts a segment.
        let boundary = boundary || self.entries.is_empty();
        if boundary {
            self.segment_start = number;
        }
        self.entries.push_back(Entry {
            event,
            bytes,
            boundary,
        });
        self.bytes += bytes;
        number
    }

    /// Drops the oldest segments while the cache is over its budget. The
    /// last segment stays whatever its size: it may still be growing.
    fn trim(&mut self) {
        while self.bytes > self.max_bytes {
            let Some(next) = self.entries.iter().skip(1).position(|e| e.boundary) else {
                break;
            };
            for entry in self.entries.drain(..=next) {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
            self.base += next as u64 + 1;
            self.truncated = true;
        }
    }

    /// The newest whole segments that fit in `budget`, and at least one.
    pub(crate) fn tail(&self, budget: usize) -> Span {
        self.segments_before(self.entries.len(), budget)
    }

    /// The whole segments before the entry with id `before` that fit in
    /// `budget`, and at least one.
    pub(crate) fn before(&self, before: EventId, budget: usize) -> Span {
        let end = self.entries.partition_point(|e| e.event.id < before);
        self.segments_before(end, budget)
    }

    fn segments_before(&self, end: usize, budget: usize) -> Span {
        let mut span = Span {
            start: end,
            end,
            bytes: 0,
        };
        let mut segment = 0;
        for index in (0..end).rev() {
            let entry = &self.entries[index];
            segment += entry.bytes;
            if entry.boundary {
                if span.start != end && span.bytes + segment > budget {
                    break;
                }
                span.bytes += segment;
                span.start = index;
                segment = 0;
            }
        }
        span
    }

    /// Whether history older than `span` exists in the cache.
    pub(crate) fn has_older(&self, span: Span) -> bool {
        span.start > 0
    }

    /// Whether older history was dropped from the cache.
    pub(crate) fn truncated(&self) -> bool {
        self.truncated
    }

    pub(crate) fn events(&self, span: Span) -> Vec<StoredEvent> {
        self.entries
            .range(span.start..span.end)
            .map(|e| e.event.clone())
            .collect()
    }

    /// `Delivery::Snapshot` for `span`, as JSON, without copying the events.
    pub(crate) fn snapshot_json(&self, thread_id: &ThreadId, span: Span) -> Option<String> {
        #[derive(Serialize)]
        struct Snapshot<'a> {
            r#type: &'static str,
            thread_id: &'a ThreadId,
            events: Events<'a>,
            has_older: bool,
            truncated: bool,
            last: Option<EventId>,
        }
        struct Events<'a>(&'a ThreadCache, Span);
        impl Serialize for Events<'_> {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let Events(cache, span) = self;
                let mut seq = serializer.serialize_seq(Some(span.end - span.start))?;
                for entry in cache.entries.range(span.start..span.end) {
                    seq.serialize_element(&entry.event)?;
                }
                seq.end()
            }
        }
        serde_json::to_string(&Snapshot {
            r#type: "snapshot",
            thread_id,
            events: Events(self, span),
            has_older: self.has_older(span),
            truncated: self.truncated,
            last: self.last,
        })
        .ok()
    }
}

fn entry_mut(entries: &mut VecDeque<Entry>, base: u64, number: u64) -> Option<&mut Entry> {
    let index = usize::try_from(number.checked_sub(base)?).ok()?;
    entries.get_mut(index)
}

/// Which "latest value" an event replaces.
#[derive(Clone, Copy)]
enum Slot {
    Usage,
    Context,
    Quota,
}

impl Slot {
    fn of(self, open: &mut Open) -> &mut Option<u64> {
        match self {
            Slot::Usage => &mut open.usage,
            Slot::Context => &mut open.context,
            Slot::Quota => &mut open.quota,
        }
    }
}

/// What an event does to the fold, with the id it concerns.
enum Kind {
    Delta(String),
    Completed(String),
    CallStarted(String),
    CallUpdated(String),
    CallFinished(String),
    RequestOpened(String),
    RequestResolved(String),
    Latest(Slot),
    TurnEdge,
    SessionEnded,
    Other,
}

fn kind_of(event: &AgentEvent) -> Kind {
    match event {
        AgentEvent::MessageDelta { message_id, .. } => Kind::Delta(message_id.clone()),
        AgentEvent::MessageCompleted { message_id, .. } => Kind::Completed(message_id.clone()),
        AgentEvent::ToolCallStarted { call_id, .. } => Kind::CallStarted(call_id.clone()),
        AgentEvent::ToolCallUpdated { call_id, .. } => Kind::CallUpdated(call_id.clone()),
        AgentEvent::ToolCallFinished { call_id, .. } => Kind::CallFinished(call_id.clone()),
        AgentEvent::ApprovalRequested { request_id, .. }
        | AgentEvent::QuestionAsked { request_id, .. } => Kind::RequestOpened(request_id.clone()),
        AgentEvent::RequestResolved { request_id, .. } => Kind::RequestResolved(request_id.clone()),
        AgentEvent::Usage { .. } => Kind::Latest(Slot::Usage),
        AgentEvent::ContextUpdated { .. } => Kind::Latest(Slot::Context),
        AgentEvent::QuotaUpdated { .. } => Kind::Latest(Slot::Quota),
        AgentEvent::TurnStarted { .. } | AgentEvent::TurnCompleted { .. } => Kind::TurnEdge,
        AgentEvent::SessionEnded { .. } => Kind::SessionEnded,
        AgentEvent::SessionStarted { .. }
        | AgentEvent::EnforcementChanged { .. }
        | AgentEvent::PlanUpdated { .. }
        | AgentEvent::RateLimited { .. }
        | AgentEvent::Error { .. }
        | AgentEvent::Unmapped { .. } => Kind::Other,
    }
}

/// Folds a later `ToolCallUpdated` into an earlier one of the same call.
/// Returns the bytes of content the entry gained and those it lost, so the
/// entry is counted at what it holds and not at the frames that built it.
fn merge_update(into: &mut AgentEvent, from: StoredEvent) -> (usize, usize) {
    let (
        AgentEvent::ToolCallUpdated {
            title,
            kind,
            input,
            output_delta,
            output,
            ..
        },
        AgentEvent::ToolCallUpdated {
            title: new_title,
            kind: new_kind,
            input: new_input,
            output_delta: more,
            output: whole,
            ..
        },
    ) = (into, from.envelope.event)
    else {
        return (0, 0);
    };
    let text = |text: &Option<String>| text.as_ref().map_or(0, String::len);
    let (mut grown, mut shrunk) = (0, 0);
    if new_title.is_some() {
        grown += text(&new_title);
        shrunk += text(title);
        *title = new_title;
    }
    if new_kind.is_some() {
        *kind = new_kind;
    }
    if new_input.is_some() {
        grown += json_len(&new_input);
        shrunk += json_len(input);
        *input = new_input;
    }
    match whole {
        // "Replaces the whole output so far", then its own delta appends.
        Some(whole) => {
            grown += whole.len() + text(&more);
            shrunk += text(output) + text(output_delta);
            *output = Some(whole);
            *output_delta = more;
        }
        None => {
            if let Some(more) = more {
                grown += more.len();
                match output_delta {
                    Some(joined) => joined.push_str(&more),
                    None => *output_delta = Some(more),
                }
            }
        }
    }
    (grown, shrunk)
}

/// The length of a value's JSON, without building it.
fn json_len<T: Serialize>(value: &Option<T>) -> usize {
    struct Count(usize);
    impl std::io::Write for Count {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let Some(value) = value else {
        return 0;
    };
    let mut count = Count(0);
    match serde_json::to_writer(&mut count, value) {
        Ok(()) => count.0,
        Err(_) => 0,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use atrium_protocol::agent::{
        AgentEventEnvelope, MessageBody, MessageKind, ToolKind, ToolStatus, TurnOutcome, Usage,
    };
    use serde_json::json;

    use crate::Delivery;

    pub(crate) fn stored(id: u64, event: AgentEvent) -> StoredEvent {
        stored_in(id, 1, event)
    }

    pub(crate) fn stored_in(id: u64, incarnation: u32, event: AgentEvent) -> StoredEvent {
        StoredEvent {
            id: EventId(id),
            thread_id: ThreadId("t".into()),
            envelope: AgentEventEnvelope {
                incarnation,
                seq: id,
                at_ms: 1_000 + id,
                event,
            },
        }
    }

    pub(crate) fn delta(message: &str, text: &str) -> AgentEvent {
        AgentEvent::MessageDelta {
            message_id: message.into(),
            kind: MessageKind::Assistant,
            text: text.into(),
        }
    }

    pub(crate) fn completed(message: &str) -> AgentEvent {
        AgentEvent::MessageCompleted {
            message_id: message.into(),
            kind: MessageKind::Assistant,
            body: MessageBody::Streamed {
                chars: 0,
                deltas: 0,
                sha256: String::new(),
            },
        }
    }

    fn call(id: &str) -> AgentEvent {
        AgentEvent::ToolCallStarted {
            call_id: id.into(),
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: None,
            input: json!({}),
        }
    }

    fn update(
        id: &str,
        title: Option<&str>,
        delta: Option<&str>,
        whole: Option<&str>,
    ) -> AgentEvent {
        AgentEvent::ToolCallUpdated {
            call_id: id.into(),
            title: title.map(Into::into),
            kind: None,
            input: None,
            output_delta: delta.map(Into::into),
            output: whole.map(Into::into),
        }
    }

    fn finished(id: &str) -> AgentEvent {
        AgentEvent::ToolCallFinished {
            call_id: id.into(),
            status: ToolStatus::Succeeded,
            output: None,
            error: None,
            truncated_bytes: 0,
        }
    }

    fn usage(input: u64) -> AgentEvent {
        AgentEvent::Usage {
            usage: Usage {
                input_tokens: Some(input),
                ..Usage::default()
            },
        }
    }

    pub(crate) fn turn(n: u32) -> AgentEvent {
        AgentEvent::TurnStarted { turn: n }
    }

    fn turn_done(n: u32) -> AgentEvent {
        AgentEvent::TurnCompleted {
            turn: n,
            outcome: TurnOutcome::Completed,
            usage: None,
        }
    }

    /// Appends events numbered from `first`, each counted as 10 bytes.
    fn fill(cache: &mut ThreadCache, first: u64, events: Vec<AgentEvent>) {
        for (offset, event) in events.into_iter().enumerate() {
            assert!(cache.append(stored(first + offset as u64, event), 10));
        }
    }

    fn all(cache: &ThreadCache) -> Vec<StoredEvent> {
        cache.events(cache.tail(usize::MAX))
    }

    fn ids(events: &[StoredEvent]) -> Vec<u64> {
        events.iter().map(|e| e.id.0).collect()
    }

    #[test]
    fn deltas_and_tool_updates_fold_and_usage_keeps_the_latest() {
        let mut cache = ThreadCache::new(usize::MAX);
        fill(
            &mut cache,
            1,
            vec![
                turn(1),
                delta("m1", "Hel"),
                usage(1),
                delta("m1", "lo "),
                delta("m1", "there"),
                completed("m1"),
                call("c1"),
                update("c1", None, Some("a"), None),
                update("c1", Some("ls"), Some("b"), None),
                usage(2),
                update("c1", None, Some("d"), Some("c")),
                update("c1", None, Some("e"), None),
                finished("c1"),
                turn_done(1),
                turn(2),
                usage(3),
            ],
        );
        let events = all(&cache);
        // 4, 5 joined 2; 9, 11, 12 joined 8; 10 is in another segment than
        // 3, so it stays.
        assert_eq!(ids(&events), [1, 2, 3, 6, 7, 8, 10, 13, 14, 15, 16]);
        assert_eq!(events[1].envelope.event, delta("m1", "Hello there"));
        assert_eq!(events[2].envelope.event, usage(1));
        assert_eq!(
            events[5].envelope.event,
            update("c1", Some("ls"), Some("de"), Some("c"))
        );
        assert_eq!(events[6].envelope.event, usage(2));
        // A new turn's usage is a new entry.
        assert_eq!(events[10].envelope.event, usage(3));
        assert_eq!(cache.last(), Some(EventId(16)));
        assert_eq!(cache.gaps(), 0);
    }

    #[test]
    fn latest_values_fold_only_within_a_segment() {
        let mut cache = ThreadCache::new(usize::MAX);
        fill(
            &mut cache,
            1,
            vec![
                turn(1),          // 1
                delta("m1", "a"), // 2  starts a segment
                usage(1),         // 3
                usage(2),         // 4  replaces 3: same segment
                completed("m1"),  // 5
                call("c1"),       // 6  starts a segment
                usage(3),         // 7  a new entry: 3 is in an earlier segment
                usage(4),         // 8  replaces 7
                finished("c1"),   // 9
            ],
        );
        let events = all(&cache);
        assert_eq!(ids(&events), [1, 2, 3, 5, 6, 7, 9]);
        assert_eq!(events[2].envelope.event, usage(2));
        assert_eq!(events[2].envelope.seq, 4);
        assert_eq!(events[5].envelope.event, usage(4));
        // A snapshot of the last segment alone has the turn's latest usage.
        let tail = cache.events(cache.tail(1));
        assert_eq!(ids(&tail), [6, 7, 9]);
        assert_eq!(tail[1].envelope.event, usage(4));
        assert_eq!(cache.bytes(), 70);
    }

    #[test]
    fn a_folded_tool_call_is_counted_at_what_it_holds() {
        let with_input = |input: serde_json::Value| AgentEvent::ToolCallUpdated {
            call_id: "c1".into(),
            title: None,
            kind: None,
            input: Some(input),
            output_delta: None,
            output: None,
        };
        let mut cache = ThreadCache::new(usize::MAX);
        cache.append(stored(1, call("c1")), 100);
        // The first update is an entry at its wire size.
        cache.append(stored(2, update("c1", None, Some("0123456789"), None)), 200);
        assert_eq!(cache.bytes(), 300);
        // A thousand more chunks cost their text, not their frames.
        for id in 3..1_003 {
            cache.append(
                stored(id, update("c1", None, Some("0123456789"), None)),
                200,
            );
        }
        assert_eq!(cache.bytes(), 300 + 10_000);
        // A large input stays counted when the output is then replaced.
        let input = json!({ "command": "x".repeat(50_000) });
        let input_len = serde_json::to_vec(&input).expect("json").len();
        cache.append(stored(1_003, with_input(input)), 50_200);
        assert_eq!(cache.bytes(), 300 + 10_000 + input_len);
        cache.append(
            stored(1_004, update("c1", None, Some("!"), Some("ok"))),
            200,
        );
        // The 10,010 bytes of output so far went; "ok" and "!" came.
        assert_eq!(cache.bytes(), 300 - 10 + input_len + 3);
        // A title replaced by a shorter one gives bytes back.
        cache.append(
            stored(1_005, update("c1", Some("a long title"), None, None)),
            200,
        );
        cache.append(stored(1_006, update("c1", Some("short"), None, None)), 200);
        assert_eq!(cache.bytes(), 300 - 10 + input_len + 3 + 5);
        assert_eq!(all(&cache).len(), 2);
    }

    #[test]
    fn a_repeat_changes_nothing() {
        let mut cache = ThreadCache::new(usize::MAX);
        fill(&mut cache, 1, vec![turn(1), delta("m1", "a")]);
        assert!(!cache.append(stored(2, delta("m1", "a")), 10));
        assert!(!cache.append(stored(1, turn(1)), 10));
        assert_eq!(all(&cache)[1].envelope.event, delta("m1", "a"));
        assert_eq!(cache.bytes(), 20);
    }

    #[test]
    fn a_message_of_a_million_deltas_costs_its_text() {
        let mut cache = ThreadCache::new(usize::MAX);
        for id in 1..=1_000_000u64 {
            cache.append(stored(id, delta("m1", "word ")), 150);
        }
        let events = all(&cache);
        assert_eq!(events.len(), 1);
        // The first delta at its wire size, the rest at their text's.
        assert_eq!(cache.bytes(), 150 + 999_999 * 5);
        let AgentEvent::MessageDelta { text, .. } = &events[0].envelope.event else {
            panic!("a delta");
        };
        assert_eq!(text.len(), 5_000_000);
    }

    #[test]
    fn segments_start_only_where_nothing_is_open() {
        let mut cache = ThreadCache::new(usize::MAX);
        fill(
            &mut cache,
            1,
            vec![
                turn(1),          // 1  starts a segment
                delta("m1", "a"), // 2  starts a segment
                call("c1"),       // 3  inside m1
                completed("m1"),  // 4
                finished("c1"),   // 5
                delta("m2", "b"), // 6  starts a segment
                completed("m2"),  // 7
                turn_done(1),     // 8  starts a segment
            ],
        );
        // 10 bytes each. A budget of one entry still takes a whole segment.
        let tail = cache.tail(1);
        assert_eq!(ids(&cache.events(tail)), [8]);
        assert!(cache.has_older(tail));
        // 30 bytes: the last two segments (8, and 6..=7).
        assert_eq!(ids(&cache.events(cache.tail(30))), [6, 7, 8]);
        // 60 bytes isn't enough to add the four-entry segment 2..=5.
        assert_eq!(ids(&cache.events(cache.tail(60))), [6, 7, 8]);
        assert_eq!(ids(&cache.events(cache.tail(70))), [2, 3, 4, 5, 6, 7, 8]);

        // Paging back from what a window has.
        let page = cache.before(EventId(6), 1);
        assert_eq!(ids(&cache.events(page)), [2, 3, 4, 5]);
        assert!(cache.has_older(page));
        let page = cache.before(EventId(2), 1_000);
        assert_eq!(ids(&cache.events(page)), [1]);
        assert!(!cache.has_older(page));
        let page = cache.before(EventId(1), 1_000);
        assert!(cache.events(page).is_empty());
        assert!(!cache.has_older(page));
    }

    #[test]
    fn eviction_drops_whole_segments_from_the_front() {
        let mut cache = ThreadCache::new(45);
        fill(
            &mut cache,
            1,
            vec![
                turn(1),
                delta("m1", "a"),
                completed("m1"),
                call("c1"),
                finished("c1"),
            ],
        );
        // 50 bytes > 45: the first segment (the turn start) goes.
        assert_eq!(ids(&all(&cache)), [2, 3, 4, 5]);
        assert!(cache.truncated());
        assert_eq!(cache.bytes(), 40);

        // An open item keeps its segment whole, over the budget if need be.
        fill(&mut cache, 6, vec![delta("m2", "b")]);
        assert_eq!(ids(&all(&cache)), [4, 5, 6]);
        for id in 7..100 {
            cache.append(stored(id, delta("m2", "0123456789")), 10);
        }
        assert_eq!(ids(&all(&cache)), [6]);
        assert!(cache.bytes() > 45);
        // Joining still finds the entry after earlier ones were dropped.
        let AgentEvent::MessageDelta { text, .. } = &all(&cache)[0].envelope.event else {
            panic!("a delta");
        };
        assert_eq!(text.len(), 1 + 93 * 10);
    }

    #[test]
    fn a_new_incarnation_closes_what_the_old_one_left_open() {
        let mut cache = ThreadCache::new(usize::MAX);
        fill(&mut cache, 1, vec![turn(1), delta("m1", "cut")]);
        // The same message id in the next session is another message.
        cache.append(stored_in(3, 2, delta("m1", "new")), 10);
        cache.append(stored_in(4, 2, delta("m1", "er")), 10);
        let events = all(&cache);
        assert_eq!(ids(&events), [1, 2, 3]);
        assert_eq!(events[1].envelope.event, delta("m1", "cut"));
        assert_eq!(events[2].envelope.event, delta("m1", "newer"));
        // The new session's first entry starts a segment.
        assert_eq!(ids(&cache.events(cache.tail(1))), [3]);
    }

    #[test]
    fn a_sequence_gap_is_counted() {
        let mut cache = ThreadCache::new(usize::MAX);
        cache.append(stored(5, turn(1)), 10);
        cache.append(stored(6, delta("m1", "a")), 10);
        assert_eq!(cache.gaps(), 0);
        cache.append(stored(8, delta("m1", "b")), 10);
        assert_eq!(cache.gaps(), 1);
        // A new incarnation starts at 0.
        let mut first = stored_in(9, 2, turn(1));
        first.envelope.seq = 0;
        cache.append(first, 10);
        assert_eq!(cache.gaps(), 1);
    }

    #[test]
    fn the_snapshot_json_is_a_snapshot_delivery() {
        let mut cache = ThreadCache::new(usize::MAX);
        fill(
            &mut cache,
            1,
            vec![turn(1), delta("m1", "a"), delta("m1", "b"), completed("m1")],
        );
        let span = cache.tail(20);
        let json = cache
            .snapshot_json(&ThreadId("t".into()), span)
            .expect("serializes");
        let delivery: Delivery = serde_json::from_str(&json).expect("a delivery");
        assert_eq!(
            delivery,
            Delivery::Snapshot {
                thread_id: ThreadId("t".into()),
                events: vec![stored(2, delta("m1", "ab")), stored(4, completed("m1"))],
                has_older: true,
                truncated: false,
                last: Some(EventId(4)),
            }
        );
        assert_eq!(span.bytes, 10 + 1 + 10);
    }
}
