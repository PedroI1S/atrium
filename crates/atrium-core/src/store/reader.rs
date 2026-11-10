//! The read-only connection pool and the read queries.

use std::path::Path;
use std::sync::{Condvar, Mutex};

use atrium_host::supervise::ProcessIdentity;
use atrium_protocol::policy::Rule;
use atrium_protocol::thread::{ThreadId, ThreadSummary};
use atrium_protocol::wire::{EventId, StoredEvent};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::Value;

use super::codec::{from_json, stored_event};
use super::sql_err;
use crate::StoreResult;
use atrium_protocol::limits;

pub(crate) struct ReaderPool {
    conns: Mutex<Vec<Connection>>,
    available: Condvar,
    /// One permit per connection, taken by async callers before they spawn.
    pub(crate) permits: std::sync::Arc<tokio::sync::Semaphore>,
}

impl ReaderPool {
    pub(crate) fn open(path: &Path, n: usize) -> StoreResult<Self> {
        let mut conns = Vec::with_capacity(n);
        for _ in 0..n.max(1) {
            let c = Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(sql_err)?;
            c.pragma_update(None, "cache_size", -(limits::SQLITE_CACHE_KIB as i64))
                .map_err(sql_err)?;
            conns.push(c);
        }
        let permits = std::sync::Arc::new(tokio::sync::Semaphore::new(conns.len()));
        Ok(Self {
            conns: Mutex::new(conns),
            available: Condvar::new(),
            permits,
        })
    }

    pub(crate) fn with<T>(&self, f: impl FnOnce(&Connection) -> StoreResult<T>) -> StoreResult<T> {
        let mut guard = self.conns.lock().unwrap_or_else(|e| e.into_inner());
        let conn = loop {
            if let Some(c) = guard.pop() {
                break c;
            }
            guard = self
                .available
                .wait(guard)
                .unwrap_or_else(|e| e.into_inner());
        };
        drop(guard);
        let out = f(&conn);
        self.conns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(conn);
        self.available.notify_one();
        out
    }
}

pub(crate) fn events_after(
    c: &Connection,
    thread: &ThreadId,
    after: Option<EventId>,
    limit: usize,
) -> StoreResult<Vec<StoredEvent>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut stmt = c
        .prepare_cached(
            "SELECT event_id, payload FROM events WHERE thread_id = ?1 AND event_id > ?2 \
             ORDER BY event_id LIMIT ?3",
        )
        .map_err(sql_err)?;
    let after = after.map_or(0, |e| i64::try_from(e.0).unwrap_or(i64::MAX));
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let mut rows = stmt
        .query(rusqlite::params![thread.0, after, limit])
        .map_err(sql_err)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(sql_err)? {
        let id: i64 = r.get(0).map_err(sql_err)?;
        let payload: String = r.get(1).map_err(sql_err)?;
        out.push(stored_event(id, &thread.0, &payload)?);
    }
    Ok(out)
}

pub(crate) fn last_event(c: &Connection, thread: &ThreadId) -> StoreResult<Option<EventId>> {
    let v: Option<Option<i64>> = c
        .query_row(
            "SELECT last_event_id FROM threads WHERE thread_id = ?1",
            [&thread.0],
            |r| r.get(0),
        )
        .optional()
        .map_err(sql_err)?;
    Ok(v.flatten().map(|v| EventId(v as u64)))
}

pub(crate) fn threads(c: &Connection) -> StoreResult<Vec<ThreadSummary>> {
    super::codec::summaries(c)
}

pub(crate) fn resume_id(c: &Connection, thread: &ThreadId) -> StoreResult<Option<String>> {
    c.query_row(
        "SELECT native_session_id FROM sessions WHERE thread_id = ?1 \
         AND native_session_id IS NOT NULL ORDER BY incarnation DESC LIMIT 1",
        [&thread.0],
        |r| r.get(0),
    )
    .optional()
    .map_err(sql_err)
}

pub(crate) fn live_processes(c: &Connection) -> StoreResult<Vec<(ThreadId, u32, ProcessIdentity)>> {
    let mut stmt = c
        .prepare_cached(
            "SELECT thread_id, incarnation, process FROM sessions \
             WHERE process IS NOT NULL AND ended_ms IS NULL ORDER BY thread_id, incarnation",
        )
        .map_err(sql_err)?;
    let mut rows = stmt.query([]).map_err(sql_err)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(sql_err)? {
        let process: String = r.get(2).map_err(sql_err)?;
        out.push((
            ThreadId(r.get(0).map_err(sql_err)?),
            r.get(1).map_err(sql_err)?,
            from_json(&process)?,
        ));
    }
    Ok(out)
}

pub(crate) fn rules(c: &Connection) -> StoreResult<(u64, Vec<Rule>)> {
    // One read transaction: revision and rules come from the same snapshot.
    let c = &c.unchecked_transaction().map_err(sql_err)?;
    let revision: i64 = c
        .query_row(
            "SELECT revision FROM rules_revision WHERE id = 1",
            [],
            |r| r.get(0),
        )
        .map_err(sql_err)?;
    let mut stmt = c
        .prepare_cached("SELECT rule FROM rules ORDER BY created_ms, rowid")
        .map_err(sql_err)?;
    let mut rows = stmt.query([]).map_err(sql_err)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(sql_err)? {
        let rule: String = r.get(0).map_err(sql_err)?;
        out.push(from_json(&rule)?);
    }
    Ok((revision as u64, out))
}

pub(crate) fn settings(c: &Connection) -> StoreResult<Vec<(String, Value)>> {
    let mut stmt = c
        .prepare_cached("SELECT key, value FROM settings ORDER BY key")
        .map_err(sql_err)?;
    let mut rows = stmt.query([]).map_err(sql_err)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(sql_err)? {
        let value: String = r.get(1).map_err(sql_err)?;
        out.push((r.get(0).map_err(sql_err)?, from_json(&value)?));
    }
    Ok(out)
}
