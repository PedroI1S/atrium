//! The ledger's rows: `RequestStore` (contract §5).

use std::sync::Arc;

use atrium_agents::BoxFuture;
use atrium_protocol::policy::Rule;
use atrium_protocol::thread::ThreadId;
use rusqlite::{OptionalExtension, params};

use super::codec::{enum_str, from_json, str_enum, to_json};
use super::writer::Writer;
use super::{Shared, now_ms, sql_err};
use crate::{Binding, RequestKey, RequestStore, StoreError, StoreResult};

pub(crate) struct Requests {
    pub(super) shared: Arc<Shared>,
}

/// Statuses a request never leaves; moving there is cleanup and works while
/// the store is read-only.
fn is_final(status: &str) -> bool {
    matches!(
        status,
        "redeemed" | "denied" | "answered" | "cancelled" | "expired"
    )
}

impl Writer {
    fn open_row(
        &mut self,
        key: &RequestKey,
        kind: &str,
        turn: Option<u32>,
        binding: Option<&Binding>,
    ) -> StoreResult<()> {
        self.writable()?;
        let tx = self.tx()?;
        if !super::codec::thread_exists(&tx, &key.thread)? {
            return Err(StoreError::UnknownThread(key.thread.clone()));
        }
        let (tool_name, tool_kind, sha, context, title, matcher) = match binding {
            Some(b) => (
                Some(b.tool_name.clone()),
                Some(enum_str(&b.tool_kind)),
                Some(b.input_sha256.clone()),
                Some(to_json(&b.context)?),
                b.title.clone(),
                b.remember_matcher.as_ref().map(to_json).transpose()?,
            ),
            None => (None, None, None, None, None, None),
        };
        tx.execute(
            "INSERT INTO requests (thread_id, incarnation, request_id, kind, turn, tool_name, \
             tool_kind, input_sha256, context, title, remember_matcher, status, opened_ms) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'pending',?12)",
            params![
                key.thread.0,
                key.incarnation,
                key.request_id,
                kind,
                turn,
                tool_name,
                tool_kind,
                sha,
                context,
                title,
                matcher,
                now_ms() as i64
            ],
        )
        .map_err(sql_err)?;
        tx.commit().map_err(sql_err)
    }

    fn cas(
        tx: &rusqlite::Transaction<'_>,
        key: &RequestKey,
        from: &[String],
        to: &str,
    ) -> StoreResult<Option<String>> {
        let found: Option<String> = tx
            .query_row(
                "SELECT status FROM requests WHERE thread_id = ?1 AND incarnation = ?2 AND request_id = ?3",
                params![key.thread.0, key.incarnation, key.request_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)?;
        let Some(found) = found else { return Ok(None) };
        if from.contains(&found) {
            let resolved = if is_final(to) {
                Some(now_ms() as i64)
            } else {
                None
            };
            tx.execute(
                "UPDATE requests SET status = ?4, resolved_ms = COALESCE(?5, resolved_ms) \
                 WHERE thread_id = ?1 AND incarnation = ?2 AND request_id = ?3",
                params![key.thread.0, key.incarnation, key.request_id, to, resolved],
            )
            .map_err(sql_err)?;
        }
        Ok(Some(found))
    }

    fn req_transition(
        &mut self,
        key: &RequestKey,
        from: Vec<String>,
        to: String,
    ) -> StoreResult<Option<String>> {
        if !is_final(&to) {
            self.writable()?;
        }
        let tx = self.tx()?;
        let found = Self::cas(&tx, key, &from, &to)?;
        tx.commit().map_err(sql_err)?;
        Ok(found)
    }

    fn req_approve_with_rule(
        &mut self,
        key: &RequestKey,
        rule: Option<Rule>,
    ) -> StoreResult<Option<String>> {
        self.writable()?;
        let tx = self.tx()?;
        let found = Self::cas(&tx, key, &["pending".to_owned()], "approved")?;
        if found.as_deref() == Some("pending")
            && let Some(rule) = rule
        {
            {
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
            }
        }
        tx.commit().map_err(sql_err)?;
        Ok(found)
    }

    fn req_expire(
        &mut self,
        thread: Option<ThreadId>,
        incarnation: Option<u32>,
        turn: Option<u32>,
    ) -> StoreResult<u64> {
        let tx = self.tx()?;
        let n = tx
            .execute(
                "UPDATE requests SET status = 'expired', resolved_ms = ?4 \
                 WHERE status IN ('pending', 'approved') \
                 AND (?1 IS NULL OR thread_id = ?1) \
                 AND (?2 IS NULL OR incarnation = ?2) \
                 AND (?3 IS NULL OR turn = ?3)",
                params![thread.map(|t| t.0), incarnation, turn, now_ms() as i64],
            )
            .map_err(sql_err)?;
        tx.commit().map_err(sql_err)?;
        Ok(n as u64)
    }
}

impl RequestStore for Requests {
    fn current_incarnation(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<u32>>> {
        let thread = thread.clone();
        self.shared.read(move |c| {
            c.query_row(
                "SELECT incarnation FROM threads WHERE thread_id = ?1",
                [&thread.0],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql_err)
        })
    }

    fn open_approval(&self, key: &RequestKey, binding: &Binding) -> BoxFuture<'_, StoreResult<()>> {
        let (key, binding) = (key.clone(), binding.clone());
        self.shared
            .call(move |w| w.open_row(&key, "approval", Some(binding.turn), Some(&binding)))
    }

    fn open_question(&self, key: &RequestKey, turn: Option<u32>) -> BoxFuture<'_, StoreResult<()>> {
        let key = key.clone();
        self.shared
            .call(move |w| w.open_row(&key, "question", turn, None))
    }

    fn transition(
        &self,
        key: &RequestKey,
        from: &[&str],
        to: &str,
    ) -> BoxFuture<'_, StoreResult<Option<String>>> {
        let key = key.clone();
        let from: Vec<String> = from.iter().map(|s| (*s).to_owned()).collect();
        let to = to.to_owned();
        self.shared.call(move |w| w.req_transition(&key, from, to))
    }

    fn open_count(&self, thread: &ThreadId, incarnation: u32) -> BoxFuture<'_, StoreResult<u64>> {
        let thread = thread.clone();
        self.shared.read(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM requests WHERE thread_id = ?1 AND incarnation = ?2 \
                 AND status IN ('pending', 'approved')",
                params![thread.0, incarnation],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u64)
            .map_err(sql_err)
        })
    }

    fn approve_with_rule(
        &self,
        key: &RequestKey,
        rule: Option<Rule>,
    ) -> BoxFuture<'_, StoreResult<Option<String>>> {
        let key = key.clone();
        self.shared
            .call(move |w| w.req_approve_with_rule(&key, rule))
    }

    fn binding(&self, key: &RequestKey) -> BoxFuture<'_, StoreResult<Option<(String, Binding)>>> {
        let key = key.clone();
        self.shared.read(move |c| {
            let row = c
                .query_row(
                    "SELECT status, turn, tool_name, tool_kind, input_sha256, context, title, \
                     remember_matcher FROM requests WHERE thread_id = ?1 AND incarnation = ?2 \
                     AND request_id = ?3 AND kind = 'approval'",
                    params![key.thread.0, key.incarnation, key.request_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, Option<u32>>(1)?,
                            r.get::<_, Option<String>>(2)?,
                            r.get::<_, Option<String>>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, Option<String>>(5)?,
                            r.get::<_, Option<String>>(6)?,
                            r.get::<_, Option<String>>(7)?,
                        ))
                    },
                )
                .optional()
                .map_err(sql_err)?;
            let Some((status, turn, name, kind, sha, context, title, matcher)) = row else {
                return Ok(None);
            };
            let missing = || StoreError::Sqlite("approval row is missing its binding".into());
            Ok(Some((
                status,
                Binding {
                    turn: turn.unwrap_or(0),
                    tool_name: name.ok_or_else(missing)?,
                    tool_kind: str_enum(&kind.ok_or_else(missing)?)?,
                    input_sha256: sha.ok_or_else(missing)?,
                    context: from_json(&context.ok_or_else(missing)?)?,
                    title,
                    remember_matcher: matcher.map(|m| from_json(&m)).transpose()?,
                },
            )))
        })
    }

    fn expire(
        &self,
        thread: Option<&ThreadId>,
        incarnation: Option<u32>,
        turn: Option<u32>,
    ) -> BoxFuture<'_, StoreResult<u64>> {
        let thread = thread.cloned();
        self.shared
            .call(move |w| w.req_expire(thread, incarnation, turn))
    }
}
