//! Every `DaemonCommand` (`atrium_protocol::wire`). The connection's
//! authority was checked before any of this runs.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use atrium_agents::{
    ApprovalTicket, BoxFuture, GateDecision, PermissionPolicy, SessionConfig, ToolCheck, ToolGate,
};
use atrium_core::history::{self, Redactor};
use atrium_core::{ApprovalLedger, LedgerError, NewThread, RequestKey, Signal, StorageStatus, Store};
use atrium_host::lifecycle;
use atrium_protocol::agent::SessionCommand;
use atrium_protocol::limits::REPLAY_PAGE_EVENTS;
use atrium_protocol::policy::{Preset, Rule, RuleDraft, RuleId, RuleMatcher, RuleOrigin};
use atrium_protocol::thread::{Location, ThreadId, ThreadSummary};
use atrium_protocol::wire::{
    AgentLaunch, DaemonCommand, ErrorCode, EventId, ExportFormat, ProviderProbe, StoredEvent,
};
use serde_json::{Map, Value, json};

use crate::conn::Conn;
use crate::hub::inbox_value;
use crate::sessions::{Agent, Launch};
use crate::{Failure, Inner, Phase, fake, failure, now_ms, store_failure};

/// A thread's title until its first user message or a rename.
pub(crate) const UNTITLED: &str = "New thread";

/// Titles are cut to this many characters.
const MAX_TITLE_CHARS: usize = 120;

const MAX_SETTING_KEY_BYTES: usize = 128;

/// The setting behind "keep agents running after quitting" (contract §10).
pub const KEEP_RUNNING_SETTING: &str = "keep_agents_running";

/// How long one provider's probe may take.
const PROBE_PATIENCE: Duration = Duration::from_secs(15);

/// Commands that can take long: they run beside the connection's reader
/// instead of in it.
pub(crate) fn is_slow(command: &DaemonCommand) -> bool {
    matches!(
        command,
        DaemonCommand::ExportThread { .. } | DaemonCommand::ProbeProviders { .. }
    )
}

fn value<T: serde::Serialize>(of: &T) -> Result<Value, Failure> {
    serde_json::to_value(of).map_err(|error| failure(ErrorCode::Internal, error.to_string()))
}

fn unknown_thread() -> Failure {
    failure(ErrorCode::UnknownThread, "unknown thread")
}

/// A title from a user message: its first line with text, spaces collapsed,
/// cut to `MAX_TITLE_CHARS`.
fn title_from(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let words: Vec<&str> = line.split_whitespace().collect();
    Some(words.join(" ").chars().take(MAX_TITLE_CHARS).collect())
}

/// Every ledger refusal is "expired" to the client (contract §5); only a
/// store that can't be reached is something else.
fn ledger_failure(error: LedgerError) -> Failure {
    match error {
        LedgerError::Store => failure(ErrorCode::Internal, "the store is unavailable"),
        other => failure(ErrorCode::Expired, other.to_string()),
    }
}

/// The gate of a probe: nothing runs during one.
struct ProbeGate;

impl ToolGate for ProbeGate {
    fn check<'a>(&'a self, _: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async {
            GateDecision::Deny {
                reason: "probing".into(),
            }
        })
    }

    fn redeem<'a>(
        &'a self,
        _: &'a ApprovalTicket,
        _: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        Box::pin(async {
            GateDecision::Deny {
                reason: "probing".into(),
            }
        })
    }

    fn question_ticket(&self, _: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        Box::pin(async { None })
    }
}

pub(crate) fn permissions(preset: Preset) -> PermissionPolicy {
    match preset {
        Preset::Home => PermissionPolicy::ReadOnly,
        Preset::FullAccess => PermissionPolicy::FullAccess,
        Preset::Careful | Preset::Normal | Preset::Trusted => PermissionPolicy::Ask,
    }
}

impl Inner {
    /// A thread that is listed (exists and isn't archived).
    fn listed(&self, thread: &ThreadId) -> Result<ThreadSummary, Failure> {
        self.hub().thread(thread).cloned().ok_or_else(unknown_thread)
    }

    /// Whether new work can start: not while stopping, not while storage is
    /// full.
    fn can_start_turns(&self) -> Result<(), Failure> {
        if self.phase() != Phase::Running {
            return Err(failure(ErrorCode::Invalid, "the daemon is stopping"));
        }
        if let StorageStatus::ReadOnly { free_bytes } = self.store.storage_status() {
            return Err(failure(
                ErrorCode::StorageReadOnly,
                format!("storage is full ({free_bytes} bytes free)"),
            ));
        }
        Ok(())
    }

    /// The thread's session, if it is the given incarnation's.
    fn session_of(
        &self,
        thread: &ThreadId,
        incarnation: Option<u32>,
    ) -> Result<Arc<crate::sessions::Live>, Failure> {
        self.sessions()
            .get(thread)
            .filter(|live| incarnation.is_none_or(|wanted| live.incarnation() == wanted))
            .cloned()
            .ok_or_else(|| failure(ErrorCode::NotRunning, "the thread has no running session"))
    }
}

pub(crate) async fn run(
    inner: &Arc<Inner>,
    conn: &Arc<Conn>,
    command: DaemonCommand,
) -> Result<Value, Failure> {
    match command {
        DaemonCommand::StartThread { agent, message } => start_thread(inner, agent, message).await,
        DaemonCommand::ListThreads => {
            let threads = inner.store.threads().await.map_err(store_failure)?;
            value(&threads)
        }
        DaemonCommand::ProbeProviders { preset } => probe(inner, preset).await,
        DaemonCommand::RenameThread { thread_id, title } => {
            inner.listed(&thread_id)?;
            let title = title_from(&title)
                .ok_or_else(|| failure(ErrorCode::Invalid, "the title is empty"))?;
            inner
                .store
                .rename(&thread_id, &title)
                .await
                .map_err(store_failure)?;
            inner.refresh_thread(&thread_id).await;
            Ok(Value::Null)
        }
        DaemonCommand::GetThread { thread_id } => {
            let summary = inner
                .store
                .thread(&thread_id)
                .await
                .map_err(store_failure)?
                .ok_or_else(unknown_thread)?;
            value(&summary)
        }
        DaemonCommand::ThreadCommand { thread_id, command } => {
            thread_command(inner, &thread_id, command).await
        }
        DaemonCommand::ResolveApproval {
            thread_id,
            incarnation,
            request_id,
            decision,
            remember,
        } => {
            let key = RequestKey {
                thread: thread_id.clone(),
                incarnation,
                request_id: request_id.clone(),
            };
            // The ledger first: only what it accepts reaches the session.
            let rule_id = inner
                .ledger
                .resolve(&key, &decision, remember)
                .await
                .map_err(ledger_failure)?;
            inner
                .session_of(&thread_id, Some(incarnation))?
                .send(SessionCommand::ResolveApproval {
                    request_id,
                    decision,
                })
                .await?;
            Ok(json!({ "rule_id": rule_id }))
        }
        DaemonCommand::AnswerQuestion {
            thread_id,
            incarnation,
            request_id,
            answers,
        } => {
            let key = RequestKey {
                thread: thread_id.clone(),
                incarnation,
                request_id: request_id.clone(),
            };
            inner.ledger.answer(&key).await.map_err(ledger_failure)?;
            inner
                .session_of(&thread_id, Some(incarnation))?
                .send(SessionCommand::AnswerQuestion {
                    request_id,
                    answers,
                })
                .await?;
            Ok(Value::Null)
        }
        DaemonCommand::ResumeThread { thread_id } => resume_thread(inner, &thread_id).await,
        DaemonCommand::ArchiveThread { thread_id } => {
            inner.listed(&thread_id)?;
            // Its session, if any, ends on its own time; the thread leaves
            // the list now.
            if let Ok(live) = inner.session_of(&thread_id, None) {
                let _ = inner.store.signal(&thread_id, Signal::Stopped).await;
                live.stop();
            }
            inner
                .store
                .archive(&thread_id)
                .await
                .map_err(store_failure)?;
            inner.thread_removed(&thread_id).await;
            Ok(Value::Null)
        }
        DaemonCommand::MarkRead { thread_id } => {
            inner.listed(&thread_id)?;
            inner
                .store
                .mark_read(&thread_id)
                .await
                .map_err(store_failure)?;
            inner.refresh_thread(&thread_id).await;
            Ok(Value::Null)
        }
        DaemonCommand::SetFocus { thread_id } => {
            inner.hub().set_focus(conn.id, thread_id);
            Ok(Value::Null)
        }
        DaemonCommand::ListInbox => Ok(inbox_value(inner.hub().inbox())),
        DaemonCommand::ListRules => {
            let (_, rules) = inner.store.rules().await.map_err(store_failure)?;
            value(&rules)
        }
        DaemonCommand::CreateRule { rule } => create_rule(inner, rule).await,
        DaemonCommand::DeleteRule { rule_id } => {
            inner
                .store
                .delete_rule(&rule_id)
                .await
                .map_err(store_failure)?;
            Ok(Value::Null)
        }
        DaemonCommand::ExportThread {
            thread_id,
            format,
            path,
        } => export(inner, thread_id, format, path).await,
        DaemonCommand::GetSettings => {
            let settings = inner.store.settings().await.map_err(store_failure)?;
            Ok(Value::Object(settings.into_iter().collect::<Map<_, _>>()))
        }
        DaemonCommand::SetSetting { key, value } => set_setting(inner, key, value).await,
        DaemonCommand::ShutdownDaemon { force } => {
            if !force {
                let turns = inner.running_turns();
                if turns > 0 {
                    return Err(failure(
                        ErrorCode::TurnsRunning,
                        format!("{turns} turns are running"),
                    ));
                }
            }
            // The answer is written before the connection closes.
            inner.request_stop(true);
            Ok(Value::Null)
        }
    }
}

async fn start_thread(
    inner: &Arc<Inner>,
    agent: AgentLaunch,
    message: Option<String>,
) -> Result<Value, Failure> {
    inner.can_start_turns()?;
    let message = message.filter(|text| !text.trim().is_empty());
    let (agent, provider, model, effort, preset) = match agent {
        AgentLaunch::Native {
            provider,
            model,
            effort,
            preset,
        } => {
            let adapter = inner.adapters.get(&provider).cloned().ok_or_else(|| {
                failure(
                    ErrorCode::Invalid,
                    format!("no adapter for {provider:?} in this daemon"),
                )
            })?;
            (Agent::Native(adapter), provider, model, effort, preset)
        }
        AgentLaunch::Fake {
            tokens_per_second,
            chunk_chars,
            duration_ms,
            tool_call_every_ms,
        } => (
            Agent::Fake(fake::Params {
                tokens_per_second,
                chunk_chars,
                duration_ms,
                tool_call_every_ms,
            }),
            fake::PROVIDER,
            None,
            None,
            Preset::Home,
        ),
    };
    let created = inner
        .store
        .create_thread(NewThread {
            provider,
            model: model.clone(),
            effort: effort.clone(),
            preset,
            location: Location::Home,
            title: message
                .as_deref()
                .and_then(title_from)
                .unwrap_or_else(|| UNTITLED.to_owned()),
        })
        .await
        .map_err(store_failure)?;
    let thread = created.thread_id.clone();
    inner.thread_added(created).await;
    let launch = Launch {
        agent,
        provider,
        model,
        effort,
        preset,
        resume: None,
        message,
    };
    match inner.launch(&thread, launch).await {
        Ok(summary) => value(&summary),
        Err(error) => {
            // Nothing ever ran in it: it doesn't stay in the list.
            if inner.store.archive(&thread).await.is_ok() {
                inner.thread_removed(&thread).await;
            }
            Err(error)
        }
    }
}

async fn resume_thread(inner: &Arc<Inner>, thread: &ThreadId) -> Result<Value, Failure> {
    let summary = inner.listed(thread)?;
    inner.can_start_turns()?;
    let resume = inner
        .store
        .resume_id(thread)
        .await
        .map_err(store_failure)?
        .ok_or_else(|| failure(ErrorCode::Invalid, "the thread has no session to resume"))?;
    if resume.starts_with(fake::NATIVE_ID_PREFIX) {
        return Err(failure(
            ErrorCode::Invalid,
            "a synthetic thread can't be resumed",
        ));
    }
    let adapter = inner
        .adapters
        .get(&summary.provider)
        .cloned()
        .ok_or_else(|| {
            failure(
                ErrorCode::Invalid,
                format!("no adapter for {:?} in this daemon", summary.provider),
            )
        })?;
    // The store doesn't give back the effort and preset a thread was
    // started with (CR-08): Home, M1's only preset in use, and no effort.
    let launch = Launch {
        agent: Agent::Native(adapter),
        provider: summary.provider,
        model: summary.model,
        effort: None,
        preset: Preset::Home,
        resume: Some(resume),
        message: None,
    };
    value(&inner.launch(thread, launch).await?)
}

async fn thread_command(
    inner: &Arc<Inner>,
    thread: &ThreadId,
    command: SessionCommand,
) -> Result<Value, Failure> {
    let summary = inner.listed(thread)?;
    match command {
        SessionCommand::SendUserMessage { text } => {
            if text.trim().is_empty() {
                return Err(failure(ErrorCode::Invalid, "the message is empty"));
            }
            inner.can_start_turns()?;
            let live = inner.session_of(thread, None)?;
            let title = (summary.title == UNTITLED)
                .then(|| title_from(&text))
                .flatten();
            live.send(SessionCommand::SendUserMessage { text }).await?;
            if let Some(title) = title
                && inner.store.rename(thread, &title).await.is_ok()
            {
                inner.refresh_thread(thread).await;
            }
        }
        SessionCommand::Interrupt => {
            inner
                .session_of(thread, None)?
                .send(SessionCommand::Interrupt)
                .await?;
        }
        SessionCommand::Shutdown => {
            let live = inner.session_of(thread, None)?;
            // The user asked, so the exit that follows is `Ended`.
            if inner.store.signal(thread, Signal::Stopped).await.is_ok() {
                inner.refresh_thread(thread).await;
            }
            live.stop();
        }
        // These have their own commands, so the ledger sees them first.
        SessionCommand::ResolveApproval { .. } | SessionCommand::AnswerQuestion { .. } => {
            return Err(failure(
                ErrorCode::Invalid,
                "use ResolveApproval or AnswerQuestion",
            ));
        }
    }
    Ok(Value::Null)
}

async fn probe(inner: &Arc<Inner>, preset: Preset) -> Result<Value, Failure> {
    let mut probes = Vec::new();
    for adapter in inner.adapters.values() {
        let config = SessionConfig {
            cwd: inner.home.clone(),
            model: None,
            effort: None,
            permissions: permissions(preset),
            gate: Arc::new(ProbeGate),
            mcp_servers: Vec::new(),
            resume: None,
            env: Vec::new(),
            program: None,
            on_spawn: None,
        };
        let probed = tokio::time::timeout(PROBE_PATIENCE, adapter.probe(&config)).await;
        probes.push(match probed {
            Ok(enforcement) => ProviderProbe {
                provider: adapter.provider(),
                enforcement: Some(enforcement),
                error: None,
            },
            Err(_) => ProviderProbe {
                provider: adapter.provider(),
                enforcement: None,
                error: Some("the probe took too long".into()),
            },
        });
    }
    probes.sort_by_key(|probe| probe.provider as u8);
    value(&probes)
}

async fn create_rule(inner: &Arc<Inner>, draft: RuleDraft) -> Result<Value, Failure> {
    if let RuleMatcher::CommandPrefix { argv } | RuleMatcher::CommandExact { argv } =
        &draft.matcher
        && argv.is_empty()
    {
        return Err(failure(ErrorCode::Invalid, "the rule's command is empty"));
    }
    let now = now_ms();
    let rule = Rule {
        id: RuleId(format!(
            "rule_s{now:x}_{}",
            inner.rule_ids.fetch_add(1, Ordering::Relaxed)
        )),
        scope: draft.scope,
        tool_kind: draft.tool_kind,
        tool_name: draft.tool_name,
        matcher: draft.matcher,
        path_scope: draft.path_scope,
        decision: draft.decision,
        origin: RuleOrigin::Settings,
        created_at_ms: now,
    };
    let rule = inner
        .store
        .create_rule(rule)
        .await
        .map_err(store_failure)?;
    value(&rule)
}

async fn set_setting(inner: &Arc<Inner>, key: String, new: Value) -> Result<Value, Failure> {
    if key.is_empty() || key.len() > MAX_SETTING_KEY_BYTES {
        return Err(failure(ErrorCode::Invalid, "invalid setting key"));
    }
    if key == KEEP_RUNNING_SETTING {
        let Value::Bool(keep) = new else {
            return Err(failure(
                ErrorCode::Invalid,
                format!("{KEEP_RUNNING_SETTING} is true or false"),
            ));
        };
        if inner.options.manage_service {
            // Only Atrium's own service file is written or removed; the
            // change takes effect at the next login.
            let changed = tokio::task::spawn_blocking(move || {
                if keep {
                    lifecycle::install_keep_running(&std::env::current_exe()?)
                } else {
                    lifecycle::remove_keep_running()
                }
            })
            .await
            .unwrap_or_else(|error| Err(io::Error::other(error)));
            changed.map_err(|error| {
                failure(
                    ErrorCode::Internal,
                    format!("the login service couldn't be changed: {error}"),
                )
            })?;
        }
    }
    inner
        .store
        .set_setting(&key, new)
        .await
        .map_err(store_failure)?;
    Ok(Value::Null)
}

/// A thread's events from its first one, a page at a time, for the export's
/// synchronous iterator. Runs on a blocking thread.
struct Pages {
    store: Arc<dyn Store>,
    runtime: tokio::runtime::Handle,
    thread: ThreadId,
    cursor: Option<EventId>,
    page: VecDeque<StoredEvent>,
    finished: bool,
    failed: Option<Failure>,
}

impl Iterator for Pages {
    type Item = StoredEvent;

    fn next(&mut self) -> Option<StoredEvent> {
        if self.page.is_empty() && !self.finished {
            let read = self.runtime.block_on(self.store.events_after(
                &self.thread,
                self.cursor,
                REPLAY_PAGE_EVENTS,
            ));
            match read {
                Ok(page) => {
                    self.finished = page.len() < REPLAY_PAGE_EVENTS;
                    self.page = page.into();
                }
                Err(error) => {
                    self.finished = true;
                    self.failed = Some(store_failure(error));
                }
            }
        }
        let event = self.page.pop_front()?;
        self.cursor = Some(event.id);
        Some(event)
    }
}

fn export_error(error: &io::Error) -> Failure {
    let code = match error.kind() {
        io::ErrorKind::AlreadyExists
        | io::ErrorKind::PermissionDenied
        | io::ErrorKind::InvalidInput
        | io::ErrorKind::NotFound
        | io::ErrorKind::NotADirectory => ErrorCode::Invalid,
        _ => ErrorCode::Internal,
    };
    let message = match error.kind() {
        io::ErrorKind::AlreadyExists => "something already exists at the export path".to_owned(),
        io::ErrorKind::PermissionDenied => {
            "the export folder can't be written to, or belongs to another user".to_owned()
        }
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => {
            "the export folder doesn't exist".to_owned()
        }
        _ => format!("the export file couldn't be written: {error}"),
    };
    failure(code, message)
}

fn write_export(mut pages: Pages, format: ExportFormat, path: &Path, home: &str) -> Result<u64, Failure> {
    let file = history::create_export_file(path).map_err(|error| export_error(&error))?;
    let redactor = Redactor::new(home, &[]);
    let mut out = io::BufWriter::new(file);
    let written = history::export_thread(&mut pages, format, &redactor, &mut out)
        .map_err(|error| match error {
            history::HistoryError::Io(error) => export_error(&error),
            integrity => failure(ErrorCode::Internal, integrity.to_string()),
        })
        .and_then(|bytes| match pages.failed.take() {
            Some(failed) => Err(failed),
            None => Ok(bytes),
        })
        .and_then(|bytes| {
            out.flush()
                .map(|()| bytes)
                .map_err(|error| export_error(&error))
        });
    if written.is_err() {
        // What was written before the failure is in the file: it goes.
        drop(out);
        let _ = std::fs::remove_file(path);
    }
    written
}

async fn export(
    inner: &Arc<Inner>,
    thread: ThreadId,
    format: ExportFormat,
    path: String,
) -> Result<Value, Failure> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return Err(failure(ErrorCode::Invalid, "the export path isn't absolute"));
    }
    inner
        .store
        .thread(&thread)
        .await
        .map_err(store_failure)?
        .ok_or_else(unknown_thread)?;
    let pages = Pages {
        store: inner.store.clone(),
        runtime: tokio::runtime::Handle::current(),
        thread,
        cursor: None,
        page: VecDeque::new(),
        finished: false,
        failed: None,
    };
    let home = inner.home.to_string_lossy().into_owned();
    let bytes = tokio::task::spawn_blocking(move || write_export(pages, format, &path, &home))
        .await
        .map_err(|error| failure(ErrorCode::Internal, error.to_string()))??;
    Ok(json!({ "bytes": bytes }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_first_line_with_text() {
        assert_eq!(title_from("  \n\n  fix   the\tbuild \nmore"), Some("fix the build".into()));
        assert_eq!(title_from(" \n \t"), None);
        let long = "é".repeat(500);
        assert_eq!(title_from(&long).map(|title| title.chars().count()), Some(MAX_TITLE_CHARS));
    }

    #[test]
    fn ledger_refusals_are_expired() {
        for refusal in [
            LedgerError::Unknown,
            LedgerError::AlreadyResolved,
            LedgerError::Expired,
            LedgerError::StaleIncarnation,
        ] {
            assert_eq!(ledger_failure(refusal).0, ErrorCode::Expired);
        }
        assert_eq!(ledger_failure(LedgerError::Store).0, ErrorCode::Internal);
    }
}
