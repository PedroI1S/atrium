//! What the daemon does on its own: recovery at start (contract §5, §10),
//! the storage watch and daily snapshots (contract §7), and posting
//! notifications (contract §8).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atrium_core::{ApprovalLedger, Signal, StorageStatus};
use atrium_host::lifecycle::secure_dir;
use atrium_host::supervise;
use atrium_protocol::thread::{Cleanup, ThreadId, ThreadState};

use crate::hub::{Notification, NotifyReason};
use crate::{Inner, Notifier, Phase, StartError, now_ms};

const STORAGE_CHECK_EVERY: Duration = Duration::from_secs(2);
const SNAPSHOT_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const SNAPSHOT_FIRST_CHECK: Duration = Duration::from_secs(60);

const RESTARTED: &str = "the daemon restarted";

/// What a previous daemon left behind, before any client is accepted or
/// any session starts:
/// 1. every recorded process is checked and its group killed
///    (`supervise::kill_recorded`);
/// 2. every listed thread whose current incarnation has no `SessionEnded`
///    is closed from the store's projections, whatever its state says (a
///    user's stop or full storage settles the state before the closing
///    events are stored). It becomes `Interrupted` with the cleanup's
///    result, unless its state was settled already;
/// 3. the ledger expires everything.
///
/// A closure that can't be stored fails the start: nothing is cleared, and
/// the next start tries again.
///
/// Not covered: an archived thread whose session never ended. The store
/// lists no archived threads (see the change requests); its requests are
/// expired with the rest, its open items stay open.
pub(crate) async fn recover(inner: &Arc<Inner>) -> Result<(), StartError> {
    // What became of each thread's current incarnation's process.
    let mut cleanups = HashMap::new();
    let mut records = Vec::new();
    for (thread, incarnation, process) in inner.store.live_processes().await? {
        let cleanup = tokio::task::spawn_blocking(move || supervise::kill_recorded(&process))
            .await
            .unwrap_or(Cleanup::Unverified);
        let current = inner
            .store
            .thread(&thread)
            .await?
            .is_some_and(|summary| summary.incarnation == incarnation);
        if current {
            cleanups.insert(thread.clone(), cleanup);
            // An archived thread isn't in the list below.
            close(inner, &thread, incarnation, cleanup).await?;
        }
        records.push((thread, incarnation));
    }
    for summary in inner.store.threads().await? {
        if summary.incarnation == 0 || cleanups.contains_key(&summary.thread_id) {
            continue;
        }
        let cleanup = match summary.state {
            // A process may have been started and not yet recorded: nothing
            // was killed, and nothing says it stopped.
            ThreadState::Starting => Cleanup::Unverified,
            _ => Cleanup::Verified,
        };
        close(inner, &summary.thread_id, summary.incarnation, cleanup).await?;
    }
    // Only now: a start that failed above finds the records again.
    for (thread, incarnation) in records {
        inner.store.set_process(&thread, incarnation, None).await?;
    }
    inner.ledger.expire_all().await;
    Ok(())
}

/// Closes one incarnation a previous daemon left unfinished.
async fn close(
    inner: &Arc<Inner>,
    thread: &ThreadId,
    incarnation: u32,
    cleanup: Cleanup,
) -> Result<(), StartError> {
    if inner.incarnation_ended(thread, incarnation).await {
        return Ok(());
    }
    // The signal first: the `SessionEnded` among the closing events settles
    // the thread's state, and an unverified cleanup must not be lost under
    // it.
    inner
        .store
        .signal(thread, Signal::Recovered { cleanup })
        .await?;
    inner
        .close_incarnation(thread, incarnation, None, Some(RESTARTED.into()), None)
        .await?;
    Ok(())
}

/// Returns once the daemon is stopping.
async fn stopping(inner: &Inner) {
    let mut phase = inner.phase.subscribe();
    let _ = phase.wait_for(|phase| *phase == Phase::Stopping).await;
}

/// Follows the store in and out of read-only (contract §7). While it is
/// read-only the inbox shows the storage-full item, threads with a running
/// turn are interrupted and their sessions stopped, and new turns are
/// refused (by the commands). It ends by itself when space returns.
pub(crate) async fn watch_storage(inner: Arc<Inner>) {
    let mut full = false;
    loop {
        let now_full = matches!(inner.store.storage_status(), StorageStatus::ReadOnly { .. });
        if now_full != full {
            full = now_full;
            inner.hub().set_storage_full(full.then(now_ms));
        }
        if full {
            let running: Vec<_> = inner
                .sessions()
                .iter()
                .filter(|(_, live)| live.turn_running())
                .map(|(thread, live)| (thread.clone(), live.clone()))
                .collect();
            for (thread, live) in running {
                if inner
                    .store
                    .signal(&thread, Signal::StorageReadOnly)
                    .await
                    .is_ok()
                {
                    inner.refresh_thread(&thread).await;
                }
                live.stop();
            }
        }
        tokio::select! {
            () = tokio::time::sleep(STORAGE_CHECK_EVERY) => {}
            () = stopping(&inner) => return,
        }
    }
}

/// When the newest snapshot was made.
fn last_snapshot(dir: &std::path::Path) -> Option<SystemTime> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok()?.metadata().ok()?.modified().ok())
        .max()
}

/// A snapshot once a day, when no turn is running (contract §7).
pub(crate) async fn snapshot_daily(inner: Arc<Inner>) {
    let dir = inner.options.paths.snapshots_dir.clone();
    let every = inner.options.snapshot_check_every;
    let mut wait = every.min(SNAPSHOT_FIRST_CHECK);
    loop {
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = stopping(&inner) => return,
        }
        wait = every;
        let due = last_snapshot(&dir).is_none_or(|made| {
            // A time in the future (a clock that went back) counts as due.
            made.elapsed().map_or(true, |age| age >= SNAPSHOT_EVERY)
        });
        if !due
            || inner.running_turns() > 0
            || matches!(inner.store.storage_status(), StorageStatus::ReadOnly { .. })
        {
            continue;
        }
        if let Err(error) = secure_dir(&dir) {
            eprintln!("atriumd: no snapshot: {error}");
            continue;
        }
        if let Err(error) = inner.store.snapshot(&dir).await {
            eprintln!("atriumd: the snapshot failed: {error}");
        }
    }
}

/// Posts notifications with the desktop's own command: `notify-send` on
/// Linux, `osascript` on macOS. The thread's title is passed as an
/// argument, never as part of a script. If the command is missing, nothing
/// is posted.
#[derive(Debug, Default)]
pub struct CommandNotifier;

fn body(reason: NotifyReason) -> &'static str {
    match reason {
        NotifyReason::Approval => "Needs your approval",
        NotifyReason::Question => "Has a question for you",
        NotifyReason::Failed => "Failed",
    }
}

impl Notifier for CommandNotifier {
    fn notify(&self, notification: &Notification) {
        let title = notification.title.as_str();
        let body = body(notification.reason);
        let mut command;
        if cfg!(target_os = "macos") {
            command = tokio::process::Command::new("/usr/bin/osascript");
            command.args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title \"Atrium\" subtitle (item 1 of argv)",
                "-e",
                "end run",
                "--",
                title,
                body,
            ]);
        } else {
            command = tokio::process::Command::new("notify-send");
            // The summary is plain text; only the body may be markup.
            command.args(["--app-name=Atrium", "--", title, body]);
        }
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        // Needs the runtime; outside one (it never is), nothing is posted.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if let Ok(mut child) = command.spawn() {
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(10), child.wait()).await;
            });
        }
    }
}
