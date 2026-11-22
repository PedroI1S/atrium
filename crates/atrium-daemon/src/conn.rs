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
use tokio::sync::{Notify, watch};

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
    /// `SubscriptionLagged` for a thread: it belongs to the subscription
    /// that lagged, and goes with it when that thread is subscribed to
    /// again or unsubscribed from before it was written.
    Lagged(ThreadId),
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

    /// Removes a queued `SubscriptionLagged` of an earlier subscription to
    /// `thread`.
    fn drop_lagged(&mut self, thread: &ThreadId) {
        let key = Coalesce::Lagged(thread.clone());
        if let Some(at) = self.control.iter().position(|queued| queued.key == key)
            && let Some(old) = self.control.remove(at)
        {
            self.control_bytes -= old.frame.len();
        }
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

/// Result of trying to queue `ReplayDone` and go live atomically.
pub(crate) enum GoLive {
    Live,
    /// The subscription is already over (closed, replaced, unsubscribed).
    Gone,
    /// The event lane is full; wait for room without holding `Inner::publish`.
    Full(Frame),
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
    /// Becomes `true` once [`Conn::close`] closes the connection, without
    /// spurious wakeups from `room`.
    closed: watch::Sender<bool>,
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
            closed: watch::channel(false).0,
            max_event_bytes,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Outbox> {
        self.outbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
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
        self.closed.send_if_modified(|is_closed| {
            let changed = !*is_closed;
            *is_closed = true;
            changed
        });
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
        let mut closed = self.closed.subscribe();
        let _ = closed.wait_for(|is_closed| *is_closed).await;
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
        outbox.drop_lagged(thread);
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
        outbox.drop_lagged(thread);
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

    /// Queues `ReplayDone` and marks `sub` live under the outbox lock, so a
    /// client that sees `ReplayDone` is already live.
    pub(crate) fn go_live_with_done(&self, sub: SubId, done: Frame) -> GoLive {
        let mut outbox = self.lock();
        if outbox.closed || !outbox.subs.get(&sub).is_some_and(|state| !state.live) {
            return GoLive::Gone;
        }
        if !outbox.has_room(done.len(), self.max_event_bytes) {
            return GoLive::Full(done);
        }
        outbox.event_bytes += done.len();
        outbox.events.push_back(EventFrame {
            sub,
            id: None,
            frame: done,
        });
        if let Some(state) = outbox.subs.get_mut(&sub) {
            state.live = true;
        }
        drop(outbox);
        self.wake.notify_one();
        GoLive::Live
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
        let lagged = Coalesce::Lagged(ended.thread.clone());
        if let Some(frame) = codec::encode(&ServerMessage::SubscriptionLagged {
            thread_id: ended.thread,
            last_sent: ended.written,
        }) {
            outbox.push_control(frame, lagged);
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
            return Take::Frame(control.frame, None);
        }
        let Some(event) = outbox.events.pop_front() else {
            if outbox.finishing {
                return Take::Closed;
            }
            return Take::Empty;
        };
        outbox.event_bytes -= event.frame.len();
        drop(outbox);
        self.room.notify_waiters();
        Take::Frame(event.frame, Some((event.sub, event.id)))
    }

    /// Writes as much of a taken frame as the socket takes now. `Ok(None)`:
    /// the connection closed or the frame's subscription ended before any of
    /// it was written, so it must not be sent (D-058: nothing of a
    /// subscription follows its end, a frame the writer already holds
    /// included).
    ///
    /// The check and the first bytes happen under the outbox lock, the one
    /// a `Subscribe` or an `Unsubscribe` ends a subscription under. So a
    /// frame is either on its way before the subscription ends, or never
    /// sent. The write never waits: the socket was reported writable.
    fn write_some(
        &self,
        writer: &OwnedWriteHalf,
        frame: &[u8],
        first: bool,
        origin: Option<(SubId, Option<EventId>)>,
    ) -> std::io::Result<Option<usize>> {
        let mut outbox = self.lock();
        if first
            && (outbox.closed || origin.is_some_and(|(sub, _)| !outbox.subs.contains_key(&sub)))
        {
            return Ok(None);
        }
        let written = match writer.try_write(frame) {
            Ok(written) => written,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => 0,
            Err(error) => return Err(error),
        };
        // `last_sent` names an event only once some of it is on the wire.
        if first
            && written > 0
            && let Some((sub, Some(id))) = origin
            && let Some(state) = outbox.subs.get_mut(&sub)
        {
            state.written = Some(id);
        }
        Ok(Some(written))
    }

    #[cfg(test)]
    pub(crate) fn take_for_test(&self) -> Option<Frame> {
        match self.take() {
            Take::Frame(frame, origin) => {
                // As the writer would: the frame counts as sent.
                if let Some((sub, Some(id))) = origin
                    && let Some(state) = self.lock().subs.get_mut(&sub)
                {
                    state.written = Some(id);
                }
                Some(frame)
            }
            Take::Empty | Take::Closed => None,
        }
    }
}

enum Take {
    /// A frame and, for an event-lane frame, its subscription and event.
    Frame(Frame, Option<(SubId, Option<EventId>)>),
    Empty,
    Closed,
}

/// Writes the connection's frames until it closes or a write fails or
/// stalls. The connection is closed when this returns.
pub(crate) async fn write_loop(conn: &Conn, mut writer: OwnedWriteHalf) {
    let mut closed = conn.closed.subscribe();
    let mut since_yield = 0usize;
    loop {
        if *closed.borrow_and_update() {
            break;
        }
        let (frame, origin) = match conn.take() {
            Take::Frame(frame, origin) => (frame, origin),
            Take::Closed => break,
            Take::Empty => {
                since_yield = 0;
                // `notify_one` keeps a wake-up sent before this point.
                conn.wake.notified().await;
                continue;
            }
        };
        let mut written = 0;
        let mut io_err = false;
        // Fast path: write immediately ready frames without allocating a
        // `WRITE_STALL` timeout or awaiting `writable()` on every small event.
        while written < frame.len() {
            match conn.write_some(&writer, &frame[written..], written == 0, origin) {
                Ok(Some(0)) => break,
                Ok(Some(more)) => written += more,
                Ok(None) => {
                    // Its subscription ended first: not sent.
                    written = frame.len();
                    break;
                }
                Err(_) => {
                    io_err = true;
                    break;
                }
            }
        }
        if io_err {
            break;
        }
        if written < frame.len() {
            let writing = async {
                while written < frame.len() {
                    writer.writable().await?;
                    match conn.write_some(&writer, &frame[written..], written == 0, origin)? {
                        Some(more) => written += more,
                        // Its subscription ended first: not sent.
                        None => break,
                    }
                }
                std::io::Result::Ok(())
            };
            tokio::select! {
                biased;
                _ = closed.wait_for(|is_closed| *is_closed) => break,
                res = tokio::time::timeout(WRITE_STALL, writing) => match res {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) | Err(_) => break,
                },
            }
            since_yield = 0;
        } else {
            since_yield += 1;
            if since_yield >= 64 {
                since_yield = 0;
                tokio::task::yield_now().await;
            }
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
    async fn a_queued_lag_notice_goes_with_its_subscription() {
        for resubscribe in [false, true] {
            let conn = Conn::with_limit(1, ClientRole::Dev, 100);
            let sub = conn.subscribe(&t("a")).expect("a subscription");
            let other = conn.subscribe(&t("b")).expect("a subscription");
            assert!(conn.go_live(sub) && conn.go_live(other));
            assert_eq!(conn.push_live(sub, EventId(1), &frame(80)), Pushed::Queued);
            assert_eq!(
                conn.push_live(other, EventId(3), &frame(80)),
                Pushed::Lagged
            );
            assert_eq!(conn.push_live(sub, EventId(2), &frame(80)), Pushed::Lagged);
            // Both notices are queued. The thread is unsubscribed from (or
            // subscribed to again) before the writer gets to them.
            if resubscribe {
                conn.subscribe(&t("a")).expect("a subscription");
            } else {
                conn.unsubscribe(&t("a"));
            }
            // Only the other thread's notice is left.
            assert_eq!(
                message(&conn.take_for_test().expect("a notice")),
                ServerMessage::SubscriptionLagged {
                    thread_id: t("b"),
                    last_sent: None
                }
            );
            assert!(conn.take_for_test().is_none());
        }
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

    #[tokio::test]
    async fn a_frame_the_writer_holds_is_not_sent_once_its_subscription_ended() {
        use tokio::io::AsyncReadExt;
        for replaced in [false, true] {
            let (ours, mut theirs) = tokio::net::UnixStream::pair().expect("a pair");
            let (_reading, writer) = ours.into_split();
            let conn = Conn::with_limit(1, ClientRole::Dev, 1_000);
            let sub = conn.subscribe(&t("a")).expect("a subscription");
            assert!(conn.feed(sub, Some(EventId(1)), frame(10)).await);
            assert!(conn.feed(sub, Some(EventId(2)), frame(20)).await);
            // The writer takes event 1 and sends it.
            let Take::Frame(first, origin) = conn.take() else {
                panic!("a frame")
            };
            assert_eq!(
                conn.write_some(&writer, &first, true, origin)
                    .expect("writes"),
                Some(10)
            );
            // It takes event 2, and before any of it is written the thread
            // is unsubscribed from (or subscribed to again).
            let Take::Frame(second, origin) = conn.take() else {
                panic!("a frame")
            };
            if replaced {
                conn.subscribe(&t("a")).expect("a subscription");
            } else {
                conn.unsubscribe(&t("a"));
            }
            assert_eq!(
                conn.write_some(&writer, &second, true, origin)
                    .expect("checks"),
                None
            );
            drop(writer);
            let mut received = Vec::new();
            theirs.read_to_end(&mut received).await.expect("reads");
            assert_eq!(received.len(), 10, "event 1 only");
        }
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

    #[tokio::test]
    async fn write_loop_finishes_a_started_frame_and_drops_later_ones_when_subscription_ends() {
        use std::sync::Arc;
        use tokio::io::AsyncReadExt;

        for replaced in [false, true] {
            let (ours, mut theirs) = tokio::net::UnixStream::pair().expect("a pair");
            let (_reading, writer) = ours.into_split();
            let conn = Arc::new(Conn::with_limit(1, ClientRole::Dev, 8_000_000));
            let sub = conn.subscribe(&t("a")).expect("a subscription");
            let large: Frame = vec![0xAAu8; 4 * 1024 * 1024].into();
            let after: Frame = vec![0xBBu8; 1024].into();
            assert!(conn.feed(sub, Some(EventId(1)), large.clone()).await);
            assert!(conn.feed(sub, Some(EventId(2)), after).await);

            let writer_task = tokio::spawn({
                let conn = conn.clone();
                async move { write_loop(&conn, writer).await }
            });

            // Read the start of the large frame so we know it is in flight
            // (written > 0, and still incomplete because 4 MiB exceeds the
            // UnixStream buffer).
            let mut received = vec![0u8; 1024];
            theirs.read_exact(&mut received).await.expect("first bytes");
            assert!(received.len() < large.len());

            // Ending or replacing the subscription purges queued later
            // frames; the in-flight frame still finishes.
            if replaced {
                conn.subscribe(&t("a")).expect("replaces");
            } else {
                conn.unsubscribe(&t("a"));
            }
            conn.finish();

            let mut chunk = vec![0u8; 16 * 1024];
            loop {
                let n = theirs.read(&mut chunk).await.expect("reads");
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&chunk[..n]);
                tokio::task::yield_now().await;
            }
            writer_task.await.expect("writer task");

            assert_eq!(received.len(), large.len(), "whole first frame, never cut");
            assert_eq!(received, large.as_ref());
        }
    }

    #[tokio::test]
    async fn write_some_treats_first_as_true_only_before_any_byte_is_written() {
        use tokio::io::AsyncReadExt;

        let (ours, mut theirs) = tokio::net::UnixStream::pair().expect("a pair");
        let (_reading, writer) = ours.into_split();
        let conn = Conn::with_limit(1, ClientRole::Dev, 1_000);
        let sub = conn.subscribe(&t("a")).expect("a subscription");
        assert!(conn.feed(sub, Some(EventId(1)), frame(100)).await);
        let Take::Frame(first_frame, origin) = conn.take() else {
            panic!("a frame")
        };

        // Fill the socket buffer so try_write hits WouldBlock (0 bytes written).
        let mut filler_bytes = 0usize;
        let filler = [0u8; 4096];
        loop {
            match writer.try_write(&filler) {
                Ok(n) => filler_bytes += n,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => panic!("unexpected write error: {err}"),
            }
        }

        assert_eq!(
            conn.write_some(&writer, &first_frame, true, origin)
                .expect("would block"),
            Some(0)
        );
        // Zero bytes written: last_sent must still be None.
        assert_eq!(conn.lock().subs.get(&sub).expect("sub").written, None);

        // Drain the filler, end the subscription, and retry with first = true:
        // because no byte of the frame was written, it must be dropped.
        let mut drain = vec![0u8; filler_bytes];
        theirs.read_exact(&mut drain).await.expect("drains filler");
        writer.writable().await.expect("writable");
        conn.unsubscribe(&t("a"));
        assert_eq!(
            conn.write_some(&writer, &first_frame, true, origin)
                .expect("dropped"),
            None
        );

        // Once at least one byte is written (first = true), subsequent chunks
        // have first = false and finish even if the subscription ends.
        let sub2 = conn.subscribe(&t("b")).expect("a subscription");
        assert!(conn.feed(sub2, Some(EventId(2)), frame(100)).await);
        let Take::Frame(second_frame, origin2) = conn.take() else {
            panic!("a frame")
        };
        assert_eq!(
            conn.write_some(&writer, &second_frame[..40], true, origin2)
                .expect("first chunk"),
            Some(40)
        );
        assert_eq!(
            conn.lock().subs.get(&sub2).expect("sub2").written,
            Some(EventId(2))
        );
        conn.unsubscribe(&t("b"));
        assert_eq!(
            conn.write_some(&writer, &second_frame[40..], false, origin2)
                .expect("rest of frame"),
            Some(60)
        );

        drop(writer);
        let mut received = Vec::new();
        theirs.read_to_end(&mut received).await.expect("reads");
        assert_eq!(received.len(), 100);
    }

    #[tokio::test]
    async fn write_loop_does_not_close_when_stalled_reader_lags() {
        use std::sync::Arc;
        use tokio::io::AsyncReadExt;

        let (ours, mut theirs) = tokio::net::UnixStream::pair().expect("a pair");
        let (_reading, writer) = ours.into_split();

        // Fill the socket buffer so `write_loop` parks on `writer.writable()`.
        let mut filler_bytes = 0usize;
        let filler = [0xEEu8; 4096];
        loop {
            match writer.try_write(&filler) {
                Ok(n) => filler_bytes += n,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => panic!("unexpected write error: {err}"),
            }
        }

        let conn = Arc::new(Conn::with_limit(1, ClientRole::Dev, 100));
        let sub = conn.subscribe(&t("a")).expect("a subscription");
        assert!(conn.go_live(sub));
        let ev1 = codec::encode(&ServerMessage::ReplayDone {
            thread_id: t("a"),
            last: None,
        })
        .expect("encodes");
        assert_eq!(conn.push_live(sub, EventId(1), &ev1), Pushed::Queued);

        let writer_task = tokio::spawn({
            let conn = conn.clone();
            async move { write_loop(&conn, writer).await }
        });
        tokio::task::yield_now().await;

        // Exceed the event lane while `write_loop` is stalled: `room` wakes
        // feeders, but must not wake or close `write_loop`.
        assert_eq!(conn.push_live(sub, EventId(2), &frame(80)), Pushed::Queued);
        assert_eq!(conn.push_live(sub, EventId(3), &frame(80)), Pushed::Lagged);
        tokio::task::yield_now().await;
        assert!(!conn.is_closed());
        assert!(!writer_task.is_finished());

        conn.ok(42, Value::Null);
        conn.finish();

        let mut drain = vec![0u8; filler_bytes];
        theirs.read_exact(&mut drain).await.expect("drains filler");

        let mut messages = Vec::new();
        while let Ok(len) = theirs.read_u32().await {
            let kind = theirs.read_u8().await.expect("kind");
            assert_eq!(kind, 0);
            let mut payload = vec![0u8; len as usize - 1];
            theirs.read_exact(&mut payload).await.expect("payload");
            messages
                .push(serde_json::from_slice::<ServerMessage>(&payload).expect("server message"));
        }
        writer_task.await.expect("writer task");

        assert!(
            messages.iter().any(|m| matches!(
                m,
                ServerMessage::SubscriptionLagged { thread_id, .. } if *thread_id == t("a")
            )),
            "expected SubscriptionLagged, got {messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| matches!(m, ServerMessage::Response { request_id: 42, .. })),
            "expected Response 42, got {messages:?}"
        );
    }

    #[tokio::test]
    async fn write_loop_exits_promptly_when_closed_while_stalled() {
        use std::sync::Arc;
        use tokio::io::AsyncReadExt;

        let (ours, mut theirs) = tokio::net::UnixStream::pair().expect("a pair");
        let (_reading, writer) = ours.into_split();

        let mut filler_bytes = 0usize;
        let filler = [0xEEu8; 4096];
        loop {
            match writer.try_write(&filler) {
                Ok(n) => filler_bytes += n,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(err) => panic!("unexpected write error: {err}"),
            }
        }

        let conn = Arc::new(Conn::with_limit(1, ClientRole::Dev, 100));
        conn.ok(1, Value::Null);

        let writer_task = tokio::spawn({
            let conn = conn.clone();
            async move { write_loop(&conn, writer).await }
        });
        tokio::task::yield_now().await;

        // Overflow `MAX_CONTROL_BYTES` while the socket buffer is full.
        for _ in 0..9 {
            conn.control_frame(frame(1024 * 1024), Coalesce::Keep);
        }

        tokio::time::timeout(Duration::from_secs(2), writer_task)
            .await
            .expect("write_loop exited promptly")
            .expect("writer task");
        assert!(conn.is_closed());

        let mut drain = vec![0u8; filler_bytes];
        theirs.read_exact(&mut drain).await.expect("drains filler");
        let mut tail = [0u8; 16];
        assert_eq!(theirs.read(&mut tail).await.expect("eof"), 0);
    }

    #[tokio::test]
    async fn go_live_with_done_queues_done_and_marks_live_atomically() {
        let conn = Conn::with_limit(1, ClientRole::Dev, 100);
        let sub = conn.subscribe(&t("a")).expect("a subscription");
        assert!(conn.feed(sub, Some(EventId(1)), frame(80)).await);

        let done = codec::encode(&ServerMessage::ReplayDone {
            thread_id: t("a"),
            last: Some(EventId(1)),
        })
        .expect("encodes");
        assert!(matches!(
            conn.go_live_with_done(sub, done.clone()),
            GoLive::Full(_)
        ));
        assert_eq!(conn.push_live(sub, EventId(2), &frame(10)), Pushed::Gone);

        assert_eq!(conn.take_for_test().expect("replayed event").len(), 80);
        assert!(matches!(
            conn.go_live_with_done(sub, done.clone()),
            GoLive::Live
        ));
        assert!(matches!(conn.go_live_with_done(sub, done), GoLive::Gone));
        assert_eq!(conn.push_live(sub, EventId(2), &frame(10)), Pushed::Queued);
        assert!(matches!(
            message(&conn.take_for_test().expect("ReplayDone")),
            ServerMessage::ReplayDone {
                last: Some(EventId(1)),
                ..
            }
        ));
        assert_eq!(conn.take_for_test().expect("live event").len(), 10);
    }

    #[tokio::test]
    async fn write_loop_never_writes_a_later_frame_after_closing_mid_frame() {
        use std::sync::Arc;
        use tokio::io::AsyncReadExt;

        let (ours, mut theirs) = tokio::net::UnixStream::pair().expect("a pair");
        let (_reading, writer) = ours.into_split();

        let conn = Arc::new(Conn::with_limit(1, ClientRole::Dev, 100));
        let first_frame: Frame = vec![0x41u8; 4 * 1024 * 1024].into();
        conn.control_frame(first_frame.clone(), Coalesce::Keep);

        let writer_task = tokio::spawn({
            let conn = conn.clone();
            async move { write_loop(&conn, writer).await }
        });

        // Wait until `write_loop` has dequeued `first_frame`, written what
        // fits in the socket buffer, and parked on `writer.writable()`.
        tokio::time::timeout(Duration::from_secs(2), async {
            while !conn.lock().control.is_empty() {
                tokio::task::yield_now().await;
            }
            tokio::task::yield_now().await;
        })
        .await
        .expect("write_loop dequeued first_frame");

        // Overflow `MAX_CONTROL_BYTES` with frames carrying a distinct byte;
        // `write_loop` must close immediately without writing any byte of the
        // later frames after the partial `first_frame`.
        let later: Frame = vec![0x42u8; 1024 * 1024].into();
        for _ in 0..9 {
            conn.control_frame(later.clone(), Coalesce::Keep);
        }

        tokio::time::timeout(Duration::from_secs(2), writer_task)
            .await
            .expect("write_loop exited promptly")
            .expect("writer task");
        assert!(conn.is_closed());

        let mut received = Vec::new();
        theirs.read_to_end(&mut received).await.expect("reads");
        assert!(
            !received.is_empty() && received.len() < first_frame.len(),
            "expected a partial first frame before close, got {} bytes",
            received.len()
        );
        assert_eq!(
            received.as_slice(),
            &first_frame[..received.len()],
            "wire bytes must only be a prefix of the in-flight frame, never followed by another frame"
        );
    }
}
