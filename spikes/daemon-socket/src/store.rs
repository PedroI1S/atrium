//! A dedicated single writer thread owns the WAL connection. Readers use short,
//! paginated queries; replay never holds a writer lock or a long read transaction.
use crate::{agent::AgentEventEnvelope, error, wire::*};
use rusqlite::{Connection, params};
use std::{
    io,
    path::{Path, PathBuf},
};
use tokio::sync::{mpsc, oneshot, watch};
type Job = Box<dyn FnOnce(&mut Connection) -> bool + Send>;
#[derive(Clone)]
pub struct Store {
    jobs: mpsc::Sender<Job>,
    pub changed: watch::Receiver<u64>,
    pub path: PathBuf,
}
pub fn open_reader(path: &Path) -> io::Result<Connection> {
    let c = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(error)?;
    c.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(error)?;
    Ok(c)
}
impl Store {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut c = Connection::open(path).map_err(error)?;
        c.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(error)?;
        c.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-2048;
        CREATE TABLE IF NOT EXISTS threads(id TEXT PRIMARY KEY, running INTEGER NOT NULL, pid INTEGER, start TEXT, exe TEXT);
        CREATE TABLE IF NOT EXISTS events(id INTEGER PRIMARY KEY, thread_id TEXT NOT NULL, envelope_json TEXT NOT NULL);
        CREATE INDEX IF NOT EXISTS events_thread ON events(thread_id,id);").map_err(error)?;
        let (jobs, mut rx) = mpsc::channel::<Job>(256);
        let (notify, changed) = watch::channel(0u64);
        std::thread::spawn(move || {
            while let Some(job) = rx.blocking_recv() {
                if job(&mut c) { notify.send_modify(|n| *n = n.wrapping_add(1)); }
            }
        });
        Ok(Self {
            jobs,
            changed,
            path: path.into(),
        })
    }
    pub async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Connection) -> io::Result<T> + Send + 'static,
    ) -> io::Result<T> {
        self.call_inner(false, f).await
    }
    async fn call_inner<T: Send + 'static>(&self, notify: bool, f: impl FnOnce(&mut Connection) -> io::Result<T> + Send + 'static) -> io::Result<T> {
        let (tx, rx) = oneshot::channel();
        self.jobs
            .send(Box::new(move |c| {
                let result = f(c); let changed = notify && result.is_ok();
                let _ = tx.send(result); changed
            }))
            .await
            .map_err(error)?;
        rx.await.map_err(error)?
    }
    pub async fn append(
        &self,
        thread: ThreadId,
        envelopes: Vec<AgentEventEnvelope>,
    ) -> io::Result<()> {
        self.call_inner(true, move |c| {
            let tx = c.transaction().map_err(error)?;
            for envelope in envelopes {
                tx.execute(
                    "INSERT INTO events(thread_id,envelope_json) VALUES(?1,?2)",
                    params![thread.0, serde_json::to_string(&envelope).map_err(error)?],
                )
                .map_err(error)?;
            }
            tx.commit().map_err(error)
        })
        .await
    }
}
pub fn last(c: &Connection, thread: &ThreadId) -> io::Result<Option<EventId>> {
    c.query_row(
        "SELECT max(id) FROM events WHERE thread_id=?1",
        [&thread.0],
        |r| r.get::<_, Option<i64>>(0),
    )
    .map(|v| v.map(|id| EventId(id as u64)))
    .map_err(error)
}
pub fn page(
    c: &Connection,
    thread: &ThreadId,
    after: Option<EventId>,
    until: Option<EventId>,
) -> io::Result<Vec<StoredEvent>> {
    let mut q = c.prepare("SELECT id,envelope_json FROM events WHERE thread_id=?1 AND id>?2 AND id<=?3 ORDER BY id LIMIT 128").map_err(error)?;
    let rows = q
        .query_map(
            params![
                thread.0,
                after.map_or(0, |x| x.0.min(i64::MAX as u64) as i64),
                until.map_or(i64::MAX, |x| x.0.min(i64::MAX as u64) as i64)
            ],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )
        .map_err(error)?;
    rows.map(|row| {
        let (id, json) = row.map_err(error)?;
        Ok(StoredEvent {
            id: EventId(id as u64),
            thread_id: thread.clone(),
            envelope: serde_json::from_str(&json).map_err(error)?,
        })
    })
    .collect()
}
