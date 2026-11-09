//! Row <-> type conversions shared by the writer and the readers.

use atrium_protocol::agent::{AgentEvent, AgentEventEnvelope};
use atrium_protocol::policy::EnforcementDescriptor;
use atrium_protocol::thread::{ThreadId, ThreadState, ThreadSummary};
use atrium_protocol::wire::{EventId, StoredEvent};
use rusqlite::{Connection, OptionalExtension, Row};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::sql_err;
use crate::{StoreError, StoreResult};

/// A unit-variant enum as its serde string (`"claude"`).
pub(crate) fn enum_str<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

pub(crate) fn str_enum<T: DeserializeOwned>(s: &str) -> StoreResult<T> {
    serde_json::from_value(serde_json::Value::String(s.to_owned()))
        .map_err(|e| StoreError::Sqlite(format!("bad stored value {s:?}: {e}")))
}

pub(crate) fn to_json<T: Serialize>(v: &T) -> StoreResult<String> {
    serde_json::to_string(v).map_err(|e| StoreError::Sqlite(format!("serialize: {e}")))
}

pub(crate) fn from_json<T: DeserializeOwned>(s: &str) -> StoreResult<T> {
    serde_json::from_str(s).map_err(|e| StoreError::Sqlite(format!("bad stored json: {e}")))
}

/// The `kind` column: the event's serde `type` tag.
pub(crate) fn kind_name(e: &AgentEvent) -> &'static str {
    use AgentEvent::*;
    match e {
        SessionStarted { .. } => "session_started",
        EnforcementChanged { .. } => "enforcement_changed",
        TurnStarted { .. } => "turn_started",
        MessageDelta { .. } => "message_delta",
        MessageCompleted { .. } => "message_completed",
        ToolCallStarted { .. } => "tool_call_started",
        ToolCallUpdated { .. } => "tool_call_updated",
        ToolCallFinished { .. } => "tool_call_finished",
        ApprovalRequested { .. } => "approval_requested",
        QuestionAsked { .. } => "question_asked",
        RequestResolved { .. } => "request_resolved",
        PlanUpdated { .. } => "plan_updated",
        Usage { .. } => "usage",
        ContextUpdated { .. } => "context_updated",
        QuotaUpdated { .. } => "quota_updated",
        RateLimited { .. } => "rate_limited",
        TurnCompleted { .. } => "turn_completed",
        Error { .. } => "error",
        Unmapped { .. } => "unmapped",
        SessionEnded { .. } => "session_ended",
    }
}

/// Events that close something and so are accepted while read-only.
pub(crate) fn is_closing(e: &AgentEvent) -> bool {
    use AgentEvent::*;
    matches!(
        e,
        MessageCompleted { .. }
            | ToolCallFinished { .. }
            | RequestResolved { .. }
            | TurnCompleted { .. }
            | Error { .. }
            | SessionEnded { .. }
    )
}

const SUMMARY_SQL: &str = "SELECT t.thread_id, t.title, t.provider, t.model, t.location, t.state, \
    t.incarnation, t.last_event_id, t.unread, t.pending_approvals, t.pending_questions, \
    t.attention_since_ms, t.attention_detail, t.created_ms, t.updated_ms, \
    s.native_session_id, s.enforcement \
    FROM threads t LEFT JOIN sessions s \
      ON s.thread_id = t.thread_id AND s.incarnation = t.incarnation";

pub(crate) fn summary_from_row(r: &Row<'_>) -> StoreResult<ThreadSummary> {
    let g = |e: rusqlite::Error| sql_err(e);
    let provider: String = r.get(2).map_err(g)?;
    let location: String = r.get(4).map_err(g)?;
    let state: String = r.get(5).map_err(g)?;
    let enforcement: Option<String> = r.get(16).map_err(g)?;
    Ok(ThreadSummary {
        thread_id: ThreadId(r.get(0).map_err(g)?),
        title: r.get(1).map_err(g)?,
        provider: str_enum(&provider)?,
        model: r.get(3).map_err(g)?,
        location: from_json(&location)?,
        state: from_json(&state)?,
        enforcement: match enforcement {
            Some(s) => Some(from_json::<EnforcementDescriptor>(&s)?),
            None => None,
        },
        incarnation: r.get(6).map_err(g)?,
        native_session_id: r.get(15).map_err(g)?,
        last_event: r
            .get::<_, Option<i64>>(7)
            .map_err(g)?
            .map(|v| EventId(v as u64)),
        unread: r.get::<_, i64>(8).map_err(g)? != 0,
        pending_approvals: r.get(9).map_err(g)?,
        pending_questions: r.get(10).map_err(g)?,
        attention_since_ms: r.get::<_, Option<i64>>(11).map_err(g)?.map(|v| v as u64),
        attention_detail: r.get(12).map_err(g)?,
        created_at_ms: r.get::<_, i64>(13).map_err(g)? as u64,
        updated_at_ms: r.get::<_, i64>(14).map_err(g)? as u64,
    })
}

pub(crate) fn summary(conn: &Connection, thread: &ThreadId) -> StoreResult<Option<ThreadSummary>> {
    let sql = format!("{SUMMARY_SQL} WHERE t.thread_id = ?1");
    let mut stmt = conn.prepare_cached(&sql).map_err(sql_err)?;
    let mut rows = stmt.query([&thread.0]).map_err(sql_err)?;
    match rows.next().map_err(sql_err)? {
        Some(r) => Ok(Some(summary_from_row(r)?)),
        None => Ok(None),
    }
}

pub(crate) fn summaries(conn: &Connection) -> StoreResult<Vec<ThreadSummary>> {
    let sql =
        format!("{SUMMARY_SQL} WHERE t.archived = 0 ORDER BY t.created_ms DESC, t.rowid DESC");
    let mut stmt = conn.prepare_cached(&sql).map_err(sql_err)?;
    let mut rows = stmt.query([]).map_err(sql_err)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(sql_err)? {
        out.push(summary_from_row(r)?);
    }
    Ok(out)
}

pub(crate) fn stored_event(id: i64, thread: &str, payload: &str) -> StoreResult<StoredEvent> {
    Ok(StoredEvent {
        id: EventId(id as u64),
        thread_id: ThreadId(thread.to_owned()),
        envelope: from_json::<AgentEventEnvelope>(payload)?,
    })
}

pub(crate) fn thread_exists(conn: &Connection, thread: &ThreadId) -> StoreResult<bool> {
    conn.query_row(
        "SELECT 1 FROM threads WHERE thread_id = ?1",
        [&thread.0],
        |_| Ok(()),
    )
    .optional()
    .map(|o| o.is_some())
    .map_err(sql_err)
}

pub(crate) fn state_of(s: &str) -> StoreResult<ThreadState> {
    from_json(s)
}
