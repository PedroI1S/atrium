//! What the daemon does on its own: recovery at start (contract §5, §10),
//! the storage watch and daily snapshots (contract §7), and posting
//! notifications (contract §8).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use atrium_core::{ApprovalLedger, Signal, StorageStatus};
use atrium_host::lifecycle::secure_dir;
use atrium_host::supervise;
use atrium_protocol::thread::{Cleanup, ThreadState};

use crate::hub::{Notification, NotifyReason};
use crate::{Inner, Notifier, Phase, StartError, now_ms};

const STORAGE_CHECK_EVERY: Duration = Duration::from_secs(2);
const SNAPSHOT_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const SNAPSHOT_FIRST_CHECK: Duration = Duration::from_secs(60);
const SNAPSHOT_CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

const RESTARTED: &str = "the daemon restarted";

/// What a previous daemon left behind, before any client is accepted or
/// any session starts:
/// 1. every recorded process is checked and its group killed
///    (`supervise::kill_recorded`), its incarnation's closing events are
///    stored from the store's projections, and its thread becomes
///    `Interrupted` with the cleanup's result;
/// 2. a thread left in a live state without a recorded process (a synthetic
///    session, or a daemon that died while starting one) is closed the same
///    way;
/// 3. the ledger expires everything.
pub(crate) async fn recover(inner: &Arc<Inner>) -> Result<(), StartError> {
    let mut recovered = HashSet::new();
    for (thread, incarnation, process) in inner.store.live_processes().await? {
        let cleanup = tokio::task::spawn_blocking(move || supervise::kill_recorded(&process))
            .await
            .unwrap_or(Cleanup::Unverified);
        inner
            .close_incarnation(&thread, incarnation, 0, None, Some(RESTARTED.into()))
            .await;
        inner.store.set_process(&thread, incarnation, None).await?;
        inner
            .store
            .signal(&thread, Signal::Recovered { cleanup })
            .await?;
        recovered.insert(thread);
    }
    for summary in inner.store.threads().await? {
        if summary.incarnation == 0 || recovered.contains(&summary.thread_id) {
            continue;
        }
        let (live, cleanup) = match summary.state {
            // A process may have been started and not yet recorded: nothing
            // was killed, and nothing says it stopped.
            ThreadState::Starting => (true, Cleanup::Unverified),
            ThreadState::Running
            | ThreadState::NeedsApproval
            | ThreadState::NeedsInput
            | ThreadState::Idle => (true, Cleanup::Verified),
            // Kept through a session's end: only one that never ended is
            // interrupted.
            ThreadState::Paused { .. } => (false, Cleanup::Verified),
            ThreadState::Failed { .. } | ThreadState::Interrupted { .. } | ThreadState::Ended => {
                continue;
            }
        };
        let closed = inner
            .close_incarnation(
                &summary.thread_id,
                summary.incarnation,
                0,
                None,
                Some(RESTARTED.into()),
            )
            .await;
        if live || closed {
            inner
                .store
                .signal(&summary.thread_id, Signal::Recovered { cleanup })
                .await?;
        }
    }
    inner.ledger.expire_all().await;
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
        let now_full = matches!(
            inner.store.storage_status(),
            StorageStatus::ReadOnly { .. }
        );
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
    let mut wait = SNAPSHOT_FIRST_CHECK;
    loop {
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = stopping(&inner) => return,
        }
        wait = SNAPSHOT_CHECK_EVERY;
        let due = last_snapshot(&dir).is_none_or(|made| {
            // A time in the future (a clock that went back) counts as due.
            made.elapsed().map_or(true, |age| age >= SNAPSHOT_EVERY)
        });
        if !due
            || inner.running_turns() > 0
            || matches!(
                inner.store.storage_status(),
                StorageStatus::ReadOnly { .. }
            )
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
