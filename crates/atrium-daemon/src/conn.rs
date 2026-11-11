//! One connection's outgoing side: two lanes, and its subscriptions.
//!
//! The **control lane** (responses, thread and inbox updates,
//! `SubscriptionLagged`) is always written before the **event lane** (replay
//! pages, `ReplayDone`, live events), so a command is never stuck behind a
//! replay.
//!
//! The event lane is bounded in bytes. A subscription is fed in one of two
//! ways:
//! - **from the store** (a replay, and the catching up after it): the feeder
//!   waits for room, so however far behind the client is, it is never
//!   dropped for being slow;
//! - **live** (pushed as events are stored): there is nobody to wait, so an
//!   event that doesn't fit drops the subscription with `SubscriptionLagged`
//!   and the client subscribes again from its cursor, which feeds it from
//!   the store.
//!
//! Every queued event frame carries its subscription's id. Replacing or
//! ending a subscription removes its queued frames, so once a `Subscribe` is
//! answered nothing of the earlier subscription is sent (D-058).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use atrium_protocol::limits::MAX_SUBSCRIPTIONS_PER_CONNECTION;
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{ClientRole, CommandResult, ErrorCode, EventId, ServerMessage};
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::Notify;

use crate::codec::{self, Frame};

pub(crate) type ConnId = u64;
pub(crate) type SubId = u64;

/// The control lane holds responses and coalesced updates, so it only grows
/// when a client sends commands without reading. Past this the connection is
/// closed.
const MAX_CONTROL_BYTES: usize = 8 * 1024 * 1024;

/// A write that makes no progress for this long closes the connection. The
/// client reconnects and continues from its cursor.
const WRITE_STALL: Duration = Duration::from_secs(30);

/// Which queued control frame a newer one replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Coalesce {
    /// A response, or anything else that must be delivered as it is.
    Keep,
    /// The whole inbox.
    Inbox,
    /// One thread's summary (or its removal).
    Thread(ThreadId),
}

struct ControlFrame {
    frame: Frame,
    key: Coalesce,
}

struct EventFrame {
    sub: SubId,
    /// `None` for `ReplayDone`.
    id: Option<EventId>,
    frame: Frame,
}

struct Sub {
    thread: ThreadId,
    live: bool,
    /// The last event of this subscription written to the socket.
    written: Option<EventId>,
}

#[derive(Default)]
struct Outbox {
    control: VecDeque<ControlFrame>,
    control_bytes: usize,
    events: VecDeque<EventFrame>,
    event_bytes: usize,
    subs: HashMap<SubId, Sub>,
    by_thread: HashMap<ThreadId, SubId>,
    closed: bool,
    /// Closing once the control lane is written (see [`Conn::finish`]).
    finishing: bool,
}

impl Outbox {
    fn has_room(&self, bytes: usize, max: usize) -> bool {
        // An empty lane takes anything, or an event larger than the bound
        // could never be sent.
        self.event_bytes == 0 || self.event_bytes.saturating_add(bytes) <= max
    }

    fn end_sub(&mut self, sub: SubId) -> Option<Sub> {
        let ended = self.subs.remove(&sub)?;
        if self.by_thread.get(&ended.thread) == Some(&sub) {
            self.by_thread.remove(&ended.thread);
        }
        let mut freed = 0;
        self.events.retain(|queued| {
            if queued.sub == sub {
                freed += queued.frame.len();
                false
            } else {
                true
            }
        });
        self.event_bytes -= freed;
        Some(ended)
    }

    fn push_control(&mut self, frame: Frame, key: Coalesce) {
        if key != Coalesce::Keep
            && let Some(at) = self.control.iter().position(|queued| queued.key == key)
            && let Some(old) = self.control.remove(at)
        {
            self.control_bytes -= old.frame.len();
        }
        self.control_bytes += frame.len();
        self.control.push_back(ControlFrame { frame, key });
        if self.control_bytes > MAX_CONTROL_BYTES {
            self.closed = true;
        }
    }
}

/// What happened to an event pushed for a subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pushed {
    Queued,
    /// The subscription is over (replaced, unsubscribed, connection closed).
    Gone,
    /// It didn't fit: the subscription was dropped and the client told.
    Lagged,
}

pub(crate) struct Conn {
    pub(crate) id: ConnId,
    pub(crate) role: ClientRole,
    outbox: Mutex<Outbox>,
    /// Wakes the writer.
    wake: Notify,
    /// Wakes feeders waiting for room, and tells them of ended subscriptions.
    room: Notify,
    max_event_bytes: usize,
}

static SUBS: AtomicU64 = AtomicU64::new(1);

impl Conn {
    pub(crate) fn with_limit(id: ConnId, role: ClientRole, max_event_bytes: usize) -> Self {
        Self {
            id,
            role,
            outbox: Mutex::new(Outbox::default()),
            wake: Notify::new(),
            room: Notify::new(),
            max_event_bytes,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Outbox> {
        self.outbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Drops everything queued and ends every subscription.
    pub(crate) fn close(&self) {
        let mut outbox = self.lock();
        outbox.closed = true;
        outbox.control.clear();
        outbox.control_bytes = 0;
        outbox.events.clear();
        outbox.event_bytes = 0;
        outbox.subs.clear();
        outbox.by_thread.clear();
        drop(outbox);
        self.wake.notify_one();
        self.room.notify_waiters();
    }

    /// Closes the connection once what is on its control lane has been
    /// written (the daemon is stopping, and the answer to `ShutdownDaemon`
    /// should still arrive). Events and subscriptions end now.
    pub(crate) fn finish(&self) {
        let mut outbox = self.lock();
        if outbox.closed {
            return;
        }
        outbox.finishing = true;
        outbox.events.clear();
        outbox.event_bytes = 0;
        outbox.subs.clear();
        outbox.by_thread.clear();
        drop(outbox);
        self.wake.notify_one();
        self.room.notify_waiters();
    }

    /// Returns once the connection is closed.
    pub(crate) async fn closed(&self) {
        loop {
            let notified = self.room.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_closed() {
                return;
            }
            notified.await;
        }
    }

    /// Queues a control-lane message.
    pub(crate) fn control(&self, message: &ServerMessage, key: Coalesce) {
        let Some(frame) = codec::encode(message) else {
            return;
        };
        self.control_frame(frame, key);
    }

    pub(crate) fn control_frame(&self, frame: Frame, key: Coalesce) {
        let mut outbox = self.lock();
        if outbox.closed {
            return;
        }
        outbox.push_control(frame, key);
        let closed = outbox.closed;
        drop(outbox);
        if closed {
            self.close();
        }
        self.wake.notify_one();
    }

    /// Answers a request. A value too large for a frame becomes an error.
    pub(crate) fn respond(&self, request_id: u64, result: CommandResult) {
        let frame = codec::encode(&ServerMessage::Response { request_id, result }).or_else(|| {
            codec::encode(&ServerMessage::Response {
                request_id,
                result: CommandResult::Err {
                    code: ErrorCode::Internal,
                    message: "the answer is too large for one frame".into(),
                },
            })
        });
        if let Some(frame) = frame {
            self.control_frame(frame, Coalesce::Keep);
        }
    }

    pub(crate) fn ok(&self, request_id: u64, value: Value) {
        self.respond(request_id, CommandResult::Ok { value });
    }

    pub(crate) fn err(&self, request_id: u64, code: ErrorCode, message: impl Into<String>) {
        self.respond(
            request_id,
            CommandResult::Err {
                code,
                message: message.into(),
            },
        );
    }

    /// Starts a subscription to `thread`, ending the one before it (its
    /// queued frames go too). `None` when the connection has too many, or is
    /// closed. The caller queues the response next, under no other lock, so
    /// nothing of the old subscription can follow it.
    pub(crate) fn subscribe(&self, thread: &ThreadId) -> Option<SubId> {
        let mut outbox = self.lock();
        if outbox.closed {
            return None;
        }
        if let Some(old) = outbox.by_thread.get(thread).copied() {
            outbox.end_sub(old);
        } else if outbox.subs.len() >= MAX_SUBSCRIPTIONS_PER_CONNECTION {
            return None;
        }
        let sub = SUBS.fetch_add(1, Ordering::Relaxed);
        outbox.subs.insert(
            sub,
            Sub {
                thread: thread.clone(),
                live: false,
                written: None,
            },
        );
        outbox.by_thread.insert(thread.clone(), sub);
        drop(outbox);
        // The old subscription's feeder, if it was waiting, sees it is over.
        self.room.notify_waiters();
        Some(sub)
    }

    pub(crate) fn unsubscribe(&self, thread: &ThreadId) {
        let mut outbox = self.lock();
        if let Some(sub) = outbox.by_thread.get(thread).copied() {
            outbox.end_sub(sub);
        }
        drop(outbox);
        self.room.notify_waiters();
    }

    /// Queues a frame read from the store for `sub`, waiting for room.
    /// `false` when the subscription is over.
    pub(crate) async fn feed(&self, sub: SubId, id: Option<EventId>, frame: Frame) -> bool {
        loop {
            let notified = self.room.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut outbox = self.lock();
                if outbox.closed || !outbox.subs.contains_key(&sub) {
                    return false;
                }
                if outbox.has_room(frame.len(), self.max_event_bytes) {
                    outbox.event_bytes += frame.len();
                    outbox.events.push_back(EventFrame { sub, id, frame });
                    drop(outbox);
                    self.wake.notify_one();
                    return true;
                }
            }
            notified.await;
        }
    }

    /// The subscription has everything stored: from now on it is pushed to.
    /// `false` when it is over.
    pub(crate) fn go_live(&self, sub: SubId) -> bool {
        let mut outbox = self.lock();
        match outbox.subs.get_mut(&sub) {
            Some(state) if !state.live => {
                state.live = true;
                true
            }
            _ => false,
        }
    }

    /// Queues a live event for `sub`.
    pub(crate) fn push_live(&self, sub: SubId, id: EventId, frame: &Frame) -> Pushed {
        let mut outbox = self.lock();
        if outbox.closed || !outbox.subs.get(&sub).is_some_and(|state| state.live) {
            return Pushed::Gone;
        }
        if outbox.has_room(frame.len(), self.max_event_bytes) {
            outbox.event_bytes += frame.len();
            outbox.events.push_back(EventFrame {
                sub,
                id: Some(id),
                frame: frame.clone(),
            });
            drop(outbox);
            self.wake.notify_one();
            return Pushed::Queued;
        }
        let Some(ended) = outbox.end_sub(sub) else {
            return Pushed::Gone;
        };
        if let Some(frame) = codec::encode(&ServerMessage::SubscriptionLagged {
            thread_id: ended.thread,
            last_sent: ended.written,
        }) {
            outbox.push_control(frame, Coalesce::Keep);
        }
        let closed = outbox.closed;
        drop(outbox);
        if closed {
            self.close();
        }
        self.wake.notify_one();
        self.room.notify_waiters();
        Pushed::Lagged
    }

    /// The next frame to write: control before events.
    fn take(&self) -> Take {
        let mut outbox = self.lock();
        if outbox.closed {
            return Take::Closed;
        }
        if let Some(control) = outbox.control.pop_front() {
            outbox.control_bytes -= control.frame.len();
            return Take::Frame(control.frame);
        }
        let Some(event) = outbox.events.pop_front() else {
            if outbox.finishing {
                return Take::Closed;
            }
            return Take::Empty;
        };
        outbox.event_bytes -= event.frame.len();
        if let Some(id) = event.id
            && let Some(sub) = outbox.subs.get_mut(&event.sub)
        {
            sub.written = Some(id);
        }
        drop(outbox);
        self.room.notify_waiters();
        Take::Frame(event.frame)
    }

    #[cfg(test)]
    pub(crate) fn take_for_test(&self) -> Option<Frame> {
        match self.take() {
            Take::Frame(frame) => Some(frame),
            Take::Empty | Take::Closed => None,
        }
    }
}

enum Take {
    Frame(Frame),
    Empty,
    Closed,
}

/// Writes the connection's frames until it closes or a write fails or
/// stalls. The connection is closed when this returns.
pub(crate) async fn write_loop(conn: &Conn, mut writer: OwnedWriteHalf) {
    loop {
        let frame = match conn.take() {
            Take::Frame(frame) => frame,
            Take::Closed => break,
            Take::Empty => {
                // `notify_one` keeps a wake-up sent before this point.
                conn.wake.notified().await;
                continue;
            }
        };
        match tokio::time::timeout(WRITE_STALL, writer.write_all(&frame)).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) | Err(_) => break,
        }
    }
    conn.close();
    let _ = writer.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(name: &str) -> ThreadId {
        ThreadId(name.into())
    }

    fn frame(bytes: usize) -> Frame {
        vec![7u8; bytes].into()
    }

    fn message(frame: &Frame) -> ServerMessage {
        serde_json::from_slice(&frame[5..]).expect("a server message")
    }

    #[tokio::test]
    async fn control_is_written_before_events() {
        let conn = Conn::with_limit(1, ClientRole::Dev, 1_000);
        let sub = conn.subscribe(&t("a")).expect("a subscription");
        assert!(conn.feed(sub, Some(EventId(1)), frame(10)).await);
        conn.ok(5, Value::Null);
        assert!(matches!(
            message(&conn.take_for_test().expect("the response")),
            ServerMessage::Response { request_id: 5, .. }
        ));
        assert_eq!(conn.take_for_test().expect("the event").len(), 10);
        assert!(conn.take_for_test().is_none());
    }

    #[tokio::test]
    async fn a_replaced_subscription_leaves_nothing_queued() {
        let conn = Conn::with_limit(1, ClientRole::Dev, 1_000);
        let old = conn.subscribe(&t("a")).expect("a subscription");
        let other = conn.subscribe(&t("b")).expect("a subscription");
        assert!(conn.feed(old, Some(EventId(1)), frame(10)).await);
        assert!(conn.feed(other, Some(EventId(2)), frame(20)).await);
        assert!(conn.feed(old, None, frame(30)).await);

        let new = conn.subscribe(&t("a")).expect("replaces");
        assert_ne!(new, old);
        // The old subscription's feeder and live pushes are refused.
        assert!(!conn.feed(old, Some(EventId(3)), frame(10)).await);
        assert_eq!(conn.push_live(old, EventId(3), &frame(10)), Pushed::Gone);
        assert_eq!(conn.take_for_test().expect("b's event").len(), 20);
        assert!(conn.take_for_test().is_none());

        // Unsubscribing does the same.
        assert!(conn.feed(new, Some(EventId(4)), frame(10)).await);
        conn.unsubscribe(&t("a"));
        assert!(conn.take_for_test().is_none());
        assert!(!conn.feed(new, Some(EventId(5)), frame(10)).await);
    }

    #[tokio::test]
    async fn a_feeder_waits_for_room_and_a_live_push_lags() {
        let conn = std::sync::Arc::new(Conn::with_limit(1, ClientRole::Dev, 100));
        let fed = conn.subscribe(&t("a")).expect("a subscription");
        let live = conn.subscribe(&t("b")).expect("a subscription");
        assert!(conn.go_live(live));
        assert!(!conn.go_live(live), "only once");
        assert!(conn.feed(fed, Some(EventId(1)), frame(80)).await);

        // The feeder waits; it isn't dropped.
        let waiting = tokio::spawn({
            let conn = conn.clone();
            async move { conn.feed(fed, Some(EventId(2)), frame(80)).await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());

        // The live subscription has nobody to wait: it lags.
        assert_eq!(conn.push_live(live, EventId(9), &frame(80)), Pushed::Lagged);
        assert_eq!(conn.push_live(live, EventId(10), &frame(1)), Pushed::Gone);
        let lagged = message(&conn.take_for_test().expect("told at once"));
        assert_eq!(
            lagged,
            ServerMessage::SubscriptionLagged {
                thread_id: t("b"),
                last_sent: None
            }
        );

        // Writing the first event makes room for the feeder.
        assert_eq!(conn.take_for_test().expect("event 1").len(), 80);
        assert!(waiting.await.expect("joins"));
        assert_eq!(conn.take_for_test().expect("event 2").len(), 80);
    }

    #[tokio::test]
    async fn an_event_larger_than_the_lane_is_still_sent() {
        let conn = Conn::with_limit(1, ClientRole::Dev, 100);
        let sub = conn.subscribe(&t("a")).expect("a subscription");
        assert!(conn.go_live(sub));
        assert_eq!(conn.push_live(sub, EventId(1), &frame(500)), Pushed::Queued);
        assert_eq!(conn.take_for_test().expect("the event").len(), 500);
        // `last_sent` is what was written.
        assert_eq!(conn.push_live(sub, EventId(2), &frame(60)), Pushed::Queued);
        assert_eq!(conn.push_live(sub, EventId(3), &frame(60)), Pushed::Lagged);
        assert_eq!(
            message(&conn.take_for_test().expect("lagged")),
            ServerMessage::SubscriptionLagged {
                thread_id: t("a"),
                last_sent: Some(EventId(1))
            }
        );
        assert!(conn.take_for_test().is_none(), "event 2 went with it");
    }

    #[test]
    fn updates_replace_the_queued_ones_and_a_flooded_control_lane_closes() {
        let conn = Conn::with_limit(1, ClientRole::Dev, 100);
        conn.control_frame(frame(10), Coalesce::Inbox);
        conn.control_frame(frame(11), Coalesce::Thread(t("a")));
        conn.control_frame(frame(12), Coalesce::Thread(t("b")));
        conn.control_frame(frame(13), Coalesce::Inbox);
        conn.control_frame(frame(14), Coalesce::Thread(t("a")));
        let lens: Vec<usize> = std::iter::from_fn(|| conn.take_for_test())
            .map(|frame| frame.len())
            .collect();
        assert_eq!(lens, [12, 13, 14]);

        for _ in 0..9 {
            conn.control_frame(frame(1024 * 1024), Coalesce::Keep);
        }
        assert!(conn.is_closed());
        assert!(conn.subscribe(&t("a")).is_none());
    }

    #[tokio::test]
    async fn finishing_writes_the_control_lane_and_drops_the_events() {
        let conn = Conn::with_limit(1, ClientRole::Dev, 1_000);
        let sub = conn.subscribe(&t("a")).expect("a subscription");
        assert!(conn.feed(sub, Some(EventId(1)), frame(10)).await);
        conn.ok(7, Value::Null);
        conn.finish();
        assert!(!conn.is_closed());
        assert!(!conn.feed(sub, Some(EventId(2)), frame(10)).await);
        assert!(matches!(
            message(&conn.take_for_test().expect("the response")),
            ServerMessage::Response { request_id: 7, .. }
        ));
        assert!(matches!(conn.take(), Take::Closed));
    }

    #[test]
    fn subscriptions_are_limited() {
        let conn = Conn::with_limit(1, ClientRole::Dev, 1_000);
        for n in 0..MAX_SUBSCRIPTIONS_PER_CONNECTION {
            assert!(conn.subscribe(&t(&n.to_string())).is_some());
        }
        assert!(conn.subscribe(&t("one more")).is_none());
        // Replacing one that exists is not one more.
        assert!(conn.subscribe(&t("0")).is_some());
    }
}
