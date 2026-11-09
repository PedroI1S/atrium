//! Numbered SQL migrations, one transaction each (contract §7).

use rusqlite::Connection;

use super::now_ms;
use crate::{StoreError, StoreResult};

/// One numbered migration.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

pub(crate) const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: include_str!("../../schema/0001_init.sql"),
}];

fn current_version(conn: &Connection) -> Result<u32, rusqlite::Error> {
    let has: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |r| r.get(0),
    )?;
    if !has {
        return Ok(0);
    }
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )
}

/// Applies every migration newer than the database's version. A failure rolls
/// that migration back and returns `StoreError::Migration`; the file is left
/// as it was before this call (earlier migrations of the same call, committed
/// one by one, stay: they are complete).
pub(crate) fn run(conn: &mut Connection, migrations: &[Migration]) -> StoreResult<()> {
    let fail = |version: u32, e: rusqlite::Error| StoreError::Migration {
        version,
        message: e.to_string(),
    };
    let current = current_version(conn).map_err(|e| fail(0, e))?;
    if let Some(newest) = migrations.iter().map(|m| m.version).max()
        && current > newest
    {
        return Err(StoreError::Migration {
            version: current,
            message: format!("database is at version {current}, newer than this build ({newest})"),
        });
    }
    let mut pending: Vec<&Migration> = migrations.iter().filter(|m| m.version > current).collect();
    pending.sort_by_key(|m| m.version);
    for m in pending {
        let tx = conn.transaction().map_err(|e| fail(m.version, e))?;
        tx.execute_batch(m.sql).map_err(|e| fail(m.version, e))?;
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_ms) VALUES (?1, ?2)",
            rusqlite::params![m.version, now_ms() as i64],
        )
        .map_err(|e| fail(m.version, e))?;
        tx.commit().map_err(|e| fail(m.version, e))?;
    }
    Ok(())
}
