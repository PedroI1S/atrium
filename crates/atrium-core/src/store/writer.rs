//! The writer: the only read-write connection, and the in-memory hashers.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use atrium_host::supervise::ProcessIdentity;
use atrium_protocol::agent::{AgentEvent, AgentEventEnvelope, MessageBody, MessageKind};
use atrium_protocol::limits;
use atrium_protocol::policy::{EnforcementDescriptor, Rule, RuleId};
use atrium_protocol::thread::{Cleanup, Location, ThreadId, ThreadState, ThreadSummary};
use atrium_protocol::wire::{EventId, StoredEvent};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::codec::{enum_str, is_closing, kind_name, state_of, summary, to_json};
use super::{StoreOptions, now_ms, sql_err};
use crate::attention::{AttentionInput, Pending};
use crate::{NewThread, OpenItems, OpenMessage, Signal, StoreError, StoreResult};

type MsgKey = (String, u32, String);

pub(crate) struct Writer {
    pub(super) conn: Connection,
    pub(super) opts: StoreOptions,
    hashers: HashMap<MsgKey, Sha256>,
    integrity: Arc<AtomicU64>,
    counter: u64,
}

/// The `threads` columns an append or signal changes.
#[derive(Clone)]
struct ThreadRow {
    state: ThreadState,
    incarnation: u32,
    unread: bool,
    pa: u32,
    pq: u32,
    since: Option<u64>,
    detail: Option<String>,
    last_event_id: Option<u64>,
    updated_ms: u64,
}

fn load_row(conn: &Connection, thread: &ThreadId) -> StoreResult<ThreadRow> {
    let r = conn
        .prepare_cached(
            "SELECT state, incarnation, unread, pending_approvals, pending_questions, \
             attention_since_ms, attention_detail, last_event_id, updated_ms \
             FROM threads WHERE thread_id = ?1",
        )
        .map_err(sql_err)?
        .query_row([&thread.0], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, u32>(3)?,
                r.get::<_, u32>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<i64>>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })
        .optional()
        .map_err(sql_err)?
        .ok_or_else(|| StoreError::UnknownThread(thread.clone()))?;
    Ok(ThreadRow {
        state: state_of(&r.0)?,
        incarnation: r.1,
        unread: r.2 != 0,
        pa: r.3,
        pq: r.4,
        since: r.5.map(|v| v as u64),
        detail: r.6,
        last_event_id: r.7.map(|v| v as u64),
        updated_ms: r.8 as u64,
    })
}

fn save_row(conn: &Connection, thread: &ThreadId, row: &ThreadRow) -> StoreResult<()> {
    conn.prepare_cached(
        "UPDATE threads SET state = ?2, unread = ?3, pending_approvals = ?4, \
         pending_questions = ?5, attention_since_ms = ?6, attention_detail = ?7, \
         last_event_id = ?8, updated_ms = ?9 WHERE thread_id = ?1",
    )
    .map_err(sql_err)?
    .execute(params![
        thread.0,
        to_json(&row.state)?,
        row.unread as i64,
        row.pa,
        row.pq,
        row.since.map(|v| v as i64),
        row.detail,
        row.last_event_id.map(|v| v as i64),
        row.updated_ms as i64,
    ])
    .map_err(sql_err)?;
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reason {
    Approval,
    Question,
    Failed,
    Cleanup,
    Reply,
}

fn reason_of(row: &ThreadRow, fatal: Option<&str>) -> Option<Reason> {
    if row.pa > 0 {
        Some(Reason::Approval)
    } else if row.pq > 0 {
        Some(Reason::Question)
    } else if matches!(row.state, ThreadState::Failed { .. }) || fatal.is_some() {
        Some(Reason::Failed)
    } else if matches!(
        row.state,
        ThreadState::Interrupted {
            cleanup: Cleanup::Unverified
        }
    ) {
        Some(Reason::Cleanup)
    } else if row.unread {
        Some(Reason::Reply)
    } else {
        None
    }
}

/// Recomputes `attention_since_ms` / `attention_detail` for `row` (already
/// holding the new state and counters). `prev` is the row before the change.
fn refresh_attention(
    conn: &Connection,
    thread: &ThreadId,
    row: &mut ThreadRow,
    prev: &ThreadRow,
    fatal: Option<&str>,
    at: u64,
) -> StoreResult<()> {
    let new = reason_of(row, fatal);
    let old = reason_of(prev, None);
    let keep = |prev: &ThreadRow| {
        if old == new && prev.since.is_some() {
            prev.since
        } else {
            Some(at)
        }
    };
    let oldest = |kind: &str| -> StoreResult<(Option<u64>, Option<String>)> {
        let found: Option<(Option<String>, i64)> = conn
            .prepare_cached(
                "SELECT COALESCE(title, tool_name), opened_ms FROM requests \
                 WHERE thread_id = ?1 AND incarnation = ?2 AND kind = ?3 \
                 AND event_opened = 1 AND event_closed = 0 ORDER BY opened_ms, rowid LIMIT 1",
            )
            .map_err(sql_err)?
            .query_row(params![thread.0, row.incarnation, kind], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
            .map_err(sql_err)?;
        Ok(match found {
            Some((title, opened)) => (Some(opened as u64), title),
            None => (Some(at), None),
        })
    };
    let (since, detail) = match new {
        None => (None, None),
        Some(Reason::Approval) => oldest("approval")?,
        Some(Reason::Question) => oldest("question")?,
        Some(Reason::Failed) => {
            let msg = fatal.map(str::to_owned).or_else(|| match &row.state {
                ThreadState::Failed { message } => Some(message.clone()),
                _ => None,
            });
            (keep(prev), msg.or_else(|| prev.detail.clone()))
        }
        Some(Reason::Cleanup) | Some(Reason::Reply) => (keep(prev), None),
    };
    row.since = since;
    row.detail = detail;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}

fn hash_hex(h: &Sha256) -> String {
    hex(&h.clone().finalize())
}

/// The running hash of an open message: from memory, or recomputed from its
/// stored deltas (after a restart) with `event_id < before`.
fn ensure_hasher<'a>(
    conn: &Connection,
    hashers: &'a mut HashMap<MsgKey, Sha256>,
    key: &MsgKey,
    before: i64,
) -> StoreResult<&'a mut Sha256> {
    if !hashers.contains_key(key) {
        let mut h = Sha256::new();
        let deltas: Option<i64> = conn
            .prepare_cached(
                "SELECT deltas FROM open_messages \
                 WHERE thread_id = ?1 AND incarnation = ?2 AND message_id = ?3",
            )
            .map_err(sql_err)?
            .query_row(params![key.0, key.1, key.2], |r| r.get(0))
            .optional()
            .map_err(sql_err)?;
        if deltas.unwrap_or(0) > 0 {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT json_extract(payload, '$.event.text') FROM events \
                     WHERE thread_id = ?1 AND incarnation = ?2 AND kind = 'message_delta' \
                     AND json_extract(payload, '$.event.message_id') = ?3 AND event_id < ?4 \
                     ORDER BY event_id",
                )
                .map_err(sql_err)?;
            let mut rows = stmt
                .query(params![key.0, key.1, key.2, before])
                .map_err(sql_err)?;
            while let Some(r) = rows.next().map_err(sql_err)? {
                let text: String = r.get(0).map_err(sql_err)?;
                h.update(text.as_bytes());
            }
        }
        hashers.insert(key.clone(), h);
    }
    hashers
        .get_mut(key)
        .ok_or_else(|| StoreError::Sqlite("hasher vanished".into()))
}

struct Ctx {
    row: ThreadRow,
    fatal: Option<String>,
}

impl Writer {
    pub(super) fn new(conn: Connection, opts: StoreOptions, integrity: Arc<AtomicU64>) -> Self {
        Self {
            conn,
            opts,
            hashers: HashMap::new(),
            integrity,
            counter: 0,
        }
    }

    /// Ordinary writes are refused while the volume is nearly full.
    pub(super) fn writable(&self) -> StoreResult<()> {
        let free_bytes = (self.opts.free_space)();
        if free_bytes < self.opts.min_free_disk_bytes {
            Err(StoreError::ReadOnly { free_bytes })
        } else {
            Ok(())
        }
    }

    pub(super) fn tx(&mut self) -> StoreResult<Transaction<'_>> {
        self.conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)
    }

    pub(crate) fn create_thread(&mut self, new: NewThread) -> StoreResult<ThreadSummary> {
        self.writable()?;
        self.counter += 1;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let id = ThreadId(format!("th-{nanos:x}-{:x}", self.counter));
        let now = now_ms() as i64;
        let tx = self.tx()?;
        tx.execute(
            "INSERT INTO threads (thread_id, title, provider, model, effort, preset, location, \
             state, incarnation, unread, pending_approvals, pending_questions, archived, \
             created_ms, updated_ms) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,0,0,0,0,0,?9,?9)",
            params![
                id.0,
                new.title,
                enum_str(&new.provider),
                new.model,
                new.effort,
                enum_str(&new.preset),
                to_json::<Location>(&new.location)?,
                to_json(&ThreadState::Starting)?,
                now,
            ],
        )
        .map_err(sql_err)?;
        let s = summary(&tx, &id)?.ok_or_else(|| StoreError::UnknownThread(id.clone()))?;
        tx.commit().map_err(sql_err)?;
        Ok(s)
    }

    pub(crate) fn begin_incarnation(&mut self, thread: &ThreadId) -> StoreResult<u32> {
        self.writable()?;
        let tx = self.tx()?;
        let row = load_row(&tx, thread)?;
        let next = row.incarnation + 1;
        tx.execute(
            "UPDATE threads SET incarnation = ?2 WHERE thread_id = ?1",
            params![thread.0, next],
        )
        .map_err(sql_err)?;
        tx.execute(
            "INSERT INTO sessions (thread_id, incarnation, started_ms) VALUES (?1, ?2, ?3)",
            params![thread.0, next, now_ms() as i64],
        )
        .map_err(sql_err)?;
        tx.commit().map_err(sql_err)?;
        self.hashers.retain(|k, _| k.0 != thread.0);
        Ok(next)
    }

    pub(crate) fn set_process(
        &mut self,
        thread: &ThreadId,
        incarnation: u32,
        process: Option<ProcessIdentity>,
    ) -> StoreResult<()> {
        // Clearing is cleanup and works while read-only.
        if process.is_some() {
            self.writable()?;
        }
        let json = process.as_ref().map(to_json).transpose()?;
        let n = self
            .conn
            .execute(
                "UPDATE sessions SET process = ?3 WHERE thread_id = ?1 AND incarnation = ?2",
                params![thread.0, incarnation, json],
            )
            .map_err(sql_err)?;
        if n == 0 && !super::codec::thread_exists(&self.conn, thread)? {
            return Err(StoreError::UnknownThread(thread.clone()));
        }
        Ok(())
    }

    pub(crate) fn set_enforcement(
        &mut self,
        thread: &ThreadId,
        incarnation: u32,
        descriptor: EnforcementDescriptor,
    ) -> StoreResult<()> {
        self.writable()?;
        let n = self
            .conn
            .execute(
                "UPDATE sessions SET enforcement = ?3 WHERE thread_id = ?1 AND incarnation = ?2",
                params![thread.0, incarnation, to_json(&descriptor)?],
            )
            .map_err(sql_err)?;
        if n == 0 {
            return Err(StoreError::UnknownThread(thread.clone()));
        }
        Ok(())
    }

    pub(crate) fn append(
        &mut self,
        thread: &ThreadId,
        incarnation: u32,
        mut events: Vec<AgentEventEnvelope>,
    ) -> StoreResult<Vec<StoredEvent>> {
        let row = load_row(&self.conn, thread)?;
        if row.incarnation != incarnation {
            return Err(StoreError::StaleIncarnation {
                got: incarnation,
                current: row.incarnation,
            });
        }
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut payloads = Vec::with_capacity(events.len());
        for e in &mut events {
            e.incarnation = incarnation;
            let payload = to_json(e)?;
            if payload.len() > limits::MAX_EVENT_BYTES {
                return Err(StoreError::EventTooLarge {
                    bytes: payload.len(),
                });
            }
            payloads.push(payload);
        }
        if !events.iter().all(|e| is_closing(&e.event)) {
            self.writable()?;
        }
        let out = self.append_tx(thread, incarnation, row, events, payloads);
        if out.is_err() {
            // The in-memory hashes may be ahead of the rolled-back rows.
            self.hashers
                .retain(|k, _| !(k.0 == thread.0 && k.1 == incarnation));
        }
        out
    }

    fn append_tx(
        &mut self,
        thread: &ThreadId,
        incarnation: u32,
        row: ThreadRow,
        events: Vec<AgentEventEnvelope>,
        payloads: Vec<String>,
    ) -> StoreResult<Vec<StoredEvent>> {
        let Writer {
            conn,
            opts,
            hashers,
            integrity,
            ..
        } = self;
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sql_err)?;
        let prev = row.clone();
        let mut cx = Ctx { row, fatal: None };
        let mut stored = Vec::with_capacity(events.len());
        let mut at = cx.row.updated_ms;
        {
            let mut ins = tx
                .prepare_cached(
                    "INSERT INTO events (thread_id, incarnation, seq, at_ms, kind, payload) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )
                .map_err(sql_err)?;
            for (env, payload) in events.into_iter().zip(payloads) {
                ins.execute(params![
                    thread.0,
                    incarnation,
                    env.seq as i64,
                    env.at_ms as i64,
                    kind_name(&env.event),
                    payload
                ])
                .map_err(sql_err)?;
                let id = tx.last_insert_rowid();
                apply(
                    &tx,
                    hashers,
                    integrity,
                    thread,
                    incarnation,
                    &env,
                    id,
                    &mut cx,
                )?;
                cx.row.state = (opts.transition)(
                    &cx.row.state,
                    Pending {
                        approvals: cx.row.pa,
                        questions: cx.row.pq,
                    },
                    AttentionInput::Event(&env.event),
                );
                cx.row.last_event_id = Some(id as u64);
                at = env.at_ms;
                stored.push(StoredEvent {
                    id: EventId(id as u64),
                    thread_id: thread.clone(),
                    envelope: env,
                });
            }
        }
        cx.row.updated_ms = at;
        let fatal = cx.fatal.take();
        let mut row = cx.row;
        refresh_attention(&tx, thread, &mut row, &prev, fatal.as_deref(), at)?;
        save_row(&tx, thread, &row)?;
        tx.commit().map_err(sql_err)?;
        Ok(stored)
    }

    pub(crate) fn signal(
        &mut self,
        thread: &ThreadId,
        signal: Signal,
    ) -> StoreResult<ThreadSummary> {
        let transition = self.opts.transition;
        let tx = self.tx()?;
        let prev = load_row(&tx, thread)?;
        let now = now_ms();
        match &signal {
            Signal::Exited { code } => {
                tx.execute(
                    "UPDATE sessions SET ended_ms = COALESCE(ended_ms, ?3), \
                     exit_code = COALESCE(exit_code, ?4) WHERE thread_id = ?1 AND incarnation = ?2",
                    params![thread.0, prev.incarnation, now as i64, code],
                )
                .map_err(sql_err)?;
            }
            Signal::Recovered { cleanup } => {
                tx.execute(
                    "UPDATE sessions SET ended_ms = COALESCE(ended_ms, ?3), cleanup = ?4 \
                     WHERE thread_id = ?1 AND incarnation = ?2",
                    params![thread.0, prev.incarnation, now as i64, enum_str(cleanup)],
                )
                .map_err(sql_err)?;
            }
            Signal::Starting | Signal::StorageReadOnly | Signal::Stopped => {}
        }
        let mut row = prev.clone();
        row.state = transition(
            &prev.state,
            Pending {
                approvals: prev.pa,
                questions: prev.pq,
            },
            AttentionInput::Signal(&signal),
        );
        row.updated_ms = now;
        refresh_attention(&tx, thread, &mut row, &prev, None, now)?;
        save_row(&tx, thread, &row)?;
        let s = summary(&tx, thread)?.ok_or_else(|| StoreError::UnknownThread(thread.clone()))?;
        tx.commit().map_err(sql_err)?;
        Ok(s)
    }

    pub(crate) fn archive(&mut self, thread: &ThreadId) -> StoreResult<()> {
        self.writable()?;
        let n = self
            .conn
            .execute(
                "UPDATE threads SET archived = 1 WHERE thread_id = ?1",
                [&thread.0],
            )
            .map_err(sql_err)?;
        if n == 0 {
            return Err(StoreError::UnknownThread(thread.clone()));
        }
        Ok(())
    }

    pub(crate) fn rename(&mut self, thread: &ThreadId, title: &str) -> StoreResult<ThreadSummary> {
        self.writable()?;
        let tx = self.tx()?;
        let n = tx
            .execute(
                "UPDATE threads SET title = ?2, updated_ms = ?3 WHERE thread_id = ?1",
                params![thread.0, title, now_ms() as i64],
            )
            .map_err(sql_err)?;
        if n == 0 {
            return Err(StoreError::UnknownThread(thread.clone()));
        }
        let s = summary(&tx, thread)?.ok_or_else(|| StoreError::UnknownThread(thread.clone()))?;
        tx.commit().map_err(sql_err)?;
        Ok(s)
    }

    pub(crate) fn mark_read(&mut self, thread: &ThreadId) -> StoreResult<()> {
        self.writable()?;
        let tx = self.tx()?;
        let prev = load_row(&tx, thread)?;
        let mut row = prev.clone();
        row.unread = false;
        refresh_attention(&tx, thread, &mut row, &prev, None, now_ms())?;
        save_row(&tx, thread, &row)?;
        tx.commit().map_err(sql_err)
    }

    pub(crate) fn open_items(
        &mut self,
        thread: &ThreadId,
        incarnation: u32,
    ) -> StoreResult<OpenItems> {
        if !super::codec::thread_exists(&self.conn, thread)? {
            return Err(StoreError::UnknownThread(thread.clone()));
        }
        let Writer { conn, hashers, .. } = self;
        let mut items = OpenItems::default();
        let mut open: Vec<(String, String, i64, i64)> = Vec::new();
        {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT message_id, kind, chars, deltas FROM open_messages \
                     WHERE thread_id = ?1 AND incarnation = ?2 ORDER BY rowid",
                )
                .map_err(sql_err)?;
            let mut rows = stmt
                .query(params![thread.0, incarnation])
                .map_err(sql_err)?;
            while let Some(r) = rows.next().map_err(sql_err)? {
                open.push((
                    r.get(0).map_err(sql_err)?,
                    r.get(1).map_err(sql_err)?,
                    r.get(2).map_err(sql_err)?,
                    r.get(3).map_err(sql_err)?,
                ));
            }
        }
        for (message_id, kind, chars, deltas) in open {
            let key = (thread.0.clone(), incarnation, message_id.clone());
            let sha256 = hash_hex(ensure_hasher(conn, hashers, &key, i64::MAX)?);
            items.messages.push(OpenMessage {
                message_id,
                kind: super::codec::str_enum::<MessageKind>(&kind)?,
                chars: chars as u64,
                deltas: deltas as u64,
                sha256,
            });
        }
        let strings = |sql: &str| -> StoreResult<Vec<String>> {
            let mut stmt = conn.prepare_cached(sql).map_err(sql_err)?;
            let mut rows = stmt
                .query(params![thread.0, incarnation])
                .map_err(sql_err)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(sql_err)? {
                out.push(r.get(0).map_err(sql_err)?);
            }
            Ok(out)
        };
        items.tool_calls = strings(
            "SELECT call_id FROM open_tool_calls WHERE thread_id = ?1 AND incarnation = ?2 ORDER BY rowid",
        )?;
        items.requests = strings(
            "SELECT request_id FROM requests WHERE thread_id = ?1 AND incarnation = ?2 \
             AND event_opened = 1 AND event_closed = 0 ORDER BY opened_ms, rowid",
        )?;
        items.turn = conn
            .prepare_cached(
                "SELECT turn FROM turns WHERE thread_id = ?1 AND incarnation = ?2 \
                 AND completed_ms IS NULL ORDER BY turn DESC LIMIT 1",
            )
            .map_err(sql_err)?
            .query_row(params![thread.0, incarnation], |r| r.get(0))
            .optional()
            .map_err(sql_err)?;
        Ok(items)
    }

    pub(crate) fn create_rule(&mut self, rule: Rule) -> StoreResult<Rule> {
        self.writable()?;
        let tx = self.tx()?;
        tx.execute(
            "INSERT INTO rules (rule_id, rule, created_ms) VALUES (?1, ?2, ?3)",
            params![rule.id.0, to_json(&rule)?, rule.created_at_ms as i64],
        )
        .map_err(sql_err)?;
        tx.execute(
            "UPDATE rules_revision SET revision = revision + 1 WHERE id = 1",
            [],
        )
        .map_err(sql_err)?;
        tx.commit().map_err(sql_err)?;
        Ok(rule)
    }

    pub(crate) fn delete_rule(&mut self, rule: &RuleId) -> StoreResult<()> {
        self.writable()?;
        let tx = self.tx()?;
        let n = tx
            .execute("DELETE FROM rules WHERE rule_id = ?1", [&rule.0])
            .map_err(sql_err)?;
        if n > 0 {
            tx.execute(
                "UPDATE rules_revision SET revision = revision + 1 WHERE id = 1",
                [],
            )
            .map_err(sql_err)?;
        }
        tx.commit().map_err(sql_err)
    }

    pub(crate) fn set_setting(&mut self, key: &str, value: Value) -> StoreResult<()> {
        self.writable()?;
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, to_json(&value)?],
            )
            .map_err(sql_err)?;
        Ok(())
    }

    /// Records a snapshot file and prunes older ones (files and rows).
    pub(crate) fn record_snapshot(
        &mut self,
        dir: &std::path::Path,
        path: &std::path::Path,
        bytes: u64,
    ) -> StoreResult<()> {
        let tx = self.tx()?;
        tx.execute(
            "INSERT OR REPLACE INTO snapshots (path, taken_ms, bytes) VALUES (?1, ?2, ?3)",
            params![path.to_string_lossy(), now_ms() as i64, bytes as i64],
        )
        .map_err(sql_err)?;
        for old in super::snapshot::prune(dir)? {
            tx.execute(
                "DELETE FROM snapshots WHERE path = ?1",
                [old.to_string_lossy()],
            )
            .map_err(sql_err)?;
        }
        tx.commit().map_err(sql_err)
    }
}

fn integrity_error(integrity: &AtomicU64, thread: &ThreadId, message_id: &str, what: &str) {
    integrity.fetch_add(1, Ordering::Relaxed);
    eprintln!(
        "atrium-store: integrity error in {} message {message_id}: {what}",
        thread.0
    );
}

/// Projects one stored event (already inserted as `event_id`).
#[allow(clippy::too_many_arguments)]
fn apply(
    tx: &Connection,
    hashers: &mut HashMap<MsgKey, Sha256>,
    integrity: &AtomicU64,
    thread: &ThreadId,
    inc: u32,
    env: &AgentEventEnvelope,
    event_id: i64,
    cx: &mut Ctx,
) -> StoreResult<()> {
    let t = thread.0.as_str();
    match &env.event {
        AgentEvent::SessionStarted {
            native_session_id,
            enforcement,
            ..
        } => {
            tx.prepare_cached(
                "UPDATE sessions SET native_session_id = ?3, enforcement = ?4 \
                 WHERE thread_id = ?1 AND incarnation = ?2",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, native_session_id, to_json(enforcement)?])
            .map_err(sql_err)?;
        }
        AgentEvent::EnforcementChanged { enforcement } => {
            tx.prepare_cached(
                "UPDATE sessions SET enforcement = ?3 WHERE thread_id = ?1 AND incarnation = ?2",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, to_json(enforcement)?])
            .map_err(sql_err)?;
        }
        AgentEvent::TurnStarted { turn } => {
            tx.prepare_cached(
                "INSERT OR IGNORE INTO turns (thread_id, incarnation, turn, started_ms) \
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, turn, env.at_ms as i64])
            .map_err(sql_err)?;
        }
        AgentEvent::MessageDelta {
            message_id,
            kind,
            text,
        } => {
            let key = (t.to_owned(), inc, message_id.clone());
            ensure_hasher(tx, hashers, &key, event_id)?.update(text.as_bytes());
            tx.prepare_cached(
                "INSERT INTO open_messages (thread_id, incarnation, message_id, kind, chars, deltas) \
                 VALUES (?1, ?2, ?3, ?4, ?5, 1) ON CONFLICT (thread_id, incarnation, message_id) \
                 DO UPDATE SET chars = chars + excluded.chars, deltas = deltas + 1",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, message_id, enum_str(kind), text.chars().count() as i64])
            .map_err(sql_err)?;
            if *kind != MessageKind::User {
                cx.row.unread = true;
            }
        }
        AgentEvent::MessageCompleted {
            message_id,
            kind,
            body,
        } => {
            let key = (t.to_owned(), inc, message_id.clone());
            let open: Option<(i64, i64)> = tx
                .prepare_cached(
                    "SELECT chars, deltas FROM open_messages \
                     WHERE thread_id = ?1 AND incarnation = ?2 AND message_id = ?3",
                )
                .map_err(sql_err)?
                .query_row(params![t, inc, message_id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()
                .map_err(sql_err)?;
            match body {
                MessageBody::Streamed {
                    chars,
                    deltas,
                    sha256,
                } => {
                    let (c, d) = open.unwrap_or((0, 0));
                    let hash = hash_hex(ensure_hasher(tx, hashers, &key, event_id)?);
                    if c as u64 != *chars {
                        integrity_error(integrity, thread, message_id, "chars differ");
                    } else if d as u64 != *deltas {
                        integrity_error(integrity, thread, message_id, "delta count differs");
                    } else if hash != *sha256 {
                        integrity_error(integrity, thread, message_id, "sha256 differs");
                    }
                }
                MessageBody::Inline { .. } => {
                    if open.is_some() {
                        integrity_error(integrity, thread, message_id, "inline body after deltas");
                    }
                }
            }
            hashers.remove(&key);
            tx.prepare_cached(
                "DELETE FROM open_messages WHERE thread_id = ?1 AND incarnation = ?2 AND message_id = ?3",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, message_id])
            .map_err(sql_err)?;
            if *kind != MessageKind::User {
                cx.row.unread = true;
            }
        }
        AgentEvent::ToolCallStarted { call_id, name, .. } => {
            tx.prepare_cached(
                "INSERT OR REPLACE INTO open_tool_calls (thread_id, incarnation, call_id, name) \
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, call_id, name])
            .map_err(sql_err)?;
        }
        AgentEvent::ToolCallFinished { call_id, .. } => {
            tx.prepare_cached(
                "DELETE FROM open_tool_calls WHERE thread_id = ?1 AND incarnation = ?2 AND call_id = ?3",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, call_id])
            .map_err(sql_err)?;
        }
        AgentEvent::ApprovalRequested {
            request_id,
            name,
            title,
            ..
        } => {
            let n = tx
                .prepare_cached(
                    "UPDATE requests SET event_opened = 1, title = COALESCE(title, ?4) \
                     WHERE thread_id = ?1 AND incarnation = ?2 AND request_id = ?3 \
                     AND kind = 'approval' AND event_opened = 0",
                )
                .map_err(sql_err)?
                .execute(params![
                    t,
                    inc,
                    request_id,
                    title.clone().unwrap_or_else(|| name.clone())
                ])
                .map_err(sql_err)?;
            if n > 0 {
                cx.row.pa += 1;
            }
        }
        AgentEvent::QuestionAsked {
            request_id,
            questions,
        } => {
            let n = tx
                .prepare_cached(
                    "UPDATE requests SET event_opened = 1, title = ?4 \
                     WHERE thread_id = ?1 AND incarnation = ?2 AND request_id = ?3 \
                     AND kind = 'question' AND event_opened = 0",
                )
                .map_err(sql_err)?
                .execute(params![
                    t,
                    inc,
                    request_id,
                    questions.first().map(|q| q.prompt.clone())
                ])
                .map_err(sql_err)?;
            if n > 0 {
                cx.row.pq += 1;
            }
        }
        AgentEvent::RequestResolved { request_id, .. } => {
            let kind: Option<String> = tx
                .prepare_cached(
                    "UPDATE requests SET event_closed = 1 \
                     WHERE thread_id = ?1 AND incarnation = ?2 AND request_id = ?3 \
                     AND event_opened = 1 AND event_closed = 0 RETURNING kind",
                )
                .map_err(sql_err)?
                .query_row(params![t, inc, request_id], |r| r.get(0))
                .optional()
                .map_err(sql_err)?;
            match kind.as_deref() {
                Some("approval") => cx.row.pa = cx.row.pa.saturating_sub(1),
                Some("question") => cx.row.pq = cx.row.pq.saturating_sub(1),
                _ => {}
            }
        }
        AgentEvent::Usage { usage } => {
            tx.prepare_cached(
                "UPDATE turns SET usage = ?3 WHERE thread_id = ?1 AND incarnation = ?2 AND turn = \
                 (SELECT MAX(turn) FROM turns WHERE thread_id = ?1 AND incarnation = ?2 \
                  AND completed_ms IS NULL)",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, to_json(usage)?])
            .map_err(sql_err)?;
        }
        AgentEvent::TurnCompleted {
            turn,
            outcome,
            usage,
        } => {
            let usage = usage.as_ref().map(to_json).transpose()?;
            let n = tx
                .prepare_cached(
                    "UPDATE turns SET completed_ms = ?4, outcome = ?5, usage = COALESCE(?6, usage) \
                     WHERE thread_id = ?1 AND incarnation = ?2 AND turn = ?3",
                )
                .map_err(sql_err)?
                .execute(params![
                    t,
                    inc,
                    turn,
                    env.at_ms as i64,
                    enum_str(outcome),
                    usage
                ])
                .map_err(sql_err)?;
            if n == 0 {
                tx.prepare_cached(
                    "INSERT INTO turns (thread_id, incarnation, turn, started_ms, completed_ms, \
                     outcome, usage) VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6)",
                )
                .map_err(sql_err)?
                .execute(params![
                    t,
                    inc,
                    turn,
                    env.at_ms as i64,
                    enum_str(outcome),
                    usage
                ])
                .map_err(sql_err)?;
            }
        }
        AgentEvent::Error { message, fatal, .. } => {
            if *fatal {
                cx.fatal = Some(message.clone());
            }
        }
        AgentEvent::SessionEnded { exit_code, .. } => {
            tx.prepare_cached(
                "UPDATE sessions SET ended_ms = COALESCE(ended_ms, ?3), \
                 exit_code = COALESCE(exit_code, ?4) WHERE thread_id = ?1 AND incarnation = ?2",
            )
            .map_err(sql_err)?
            .execute(params![t, inc, env.at_ms as i64, exit_code])
            .map_err(sql_err)?;
        }
        AgentEvent::ToolCallUpdated { .. }
        | AgentEvent::PlanUpdated { .. }
        | AgentEvent::ContextUpdated { .. }
        | AgentEvent::QuotaUpdated { .. }
        | AgentEvent::RateLimited { .. }
        | AgentEvent::Unmapped { .. } => {}
    }
    Ok(())
}
