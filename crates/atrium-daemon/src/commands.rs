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
use atrium_core::{
    ApprovalLedger, LedgerError, NewThread, RequestKey, Signal, StorageStatus, Store,
};
use atrium_host::lifecycle;
use atrium_protocol::agent::SessionCommand;
use atrium_protocol::limits::REPLAY_PAGE_EVENTS;
use atrium_protocol::policy::{
    Preset, Rule, RuleDecision, RuleDraft, RuleId, RuleMatcher, RuleOrigin, RuleScope,
};
use atrium_protocol::thread::{Location, ThreadId, ThreadSummary};
use atrium_protocol::wire::{
    AgentLaunch, DaemonCommand, ErrorCode, EventId, ExportFormat, ProviderProbe, StoredEvent,
};
use serde_json::{Map, Value, json};

use crate::conn::Conn;
use crate::hub::inbox_value;
use crate::sessions::{Agent, Launch};
use crate::{Failure, Inner, Phase, failure, fake, now_ms, store_failure};

/// A thread's title until its first user message or a rename.
pub(crate) const UNTITLED: &str = "New thread";

/// Titles are cut to this many characters.
const MAX_TITLE_CHARS: usize = 120;

const MAX_SETTING_KEY_BYTES: usize = 128;

/// The setting behind "keep agents running after quitting" (contract §10).
pub const KEEP_RUNNING_SETTING: &str = "keep_agents_running";

/// Notification switches (contract §8). On unless explicitly set to `false`.
pub const NOTIFY_APPROVAL_SETTING: &str = "notify_approval";
pub const NOTIFY_QUESTION_SETTING: &str = "notify_question";
pub const NOTIFY_FAILED_SETTING: &str = "notify_failed";

fn is_notify_setting(key: &str) -> bool {
    matches!(
        key,
        NOTIFY_APPROVAL_SETTING | NOTIFY_QUESTION_SETTING | NOTIFY_FAILED_SETTING
    )
}

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
        self.hub()
            .thread(thread)
            .cloned()
            .ok_or_else(unknown_thread)
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
            // Both under the session's request lock, so neither happens
            // after an event that ends the request was stored.
            let live = inner.session_of(&thread_id, Some(incarnation));
            let _resolving = match &live {
                Ok(live) => Some(live.requests.lock().await),
                Err(_) => None,
            };
            let rule_id = inner
                .ledger
                .resolve(&key, &decision, remember)
                .await
                .map_err(ledger_failure)?;
            live.as_ref()
                .map_err(Clone::clone)?
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
            let live = inner.session_of(&thread_id, Some(incarnation));
            let _resolving = match &live {
                Ok(live) => Some(live.requests.lock().await),
                Err(_) => None,
            };
            inner.ledger.answer(&key).await.map_err(ledger_failure)?;
            live.as_ref()
                .map_err(Clone::clone)?
                .send(SessionCommand::AnswerQuestion {
                    request_id,
                    answers,
                })
                .await?;
            Ok(Value::Null)
        }
        DaemonCommand::DeclineQuestion {
            thread_id,
            incarnation,
            request_id,
        } => {
            let key = RequestKey {
                thread: thread_id.clone(),
                incarnation,
                request_id: request_id.clone(),
            };
            let live = inner.session_of(&thread_id, Some(incarnation));
            let _resolving = match &live {
                Ok(live) => Some(live.requests.lock().await),
                Err(_) => None,
            };
            let requests = inner.store.requests();
            match requests.current_incarnation(&thread_id).await {
                Ok(Some(cur)) if incarnation == cur => {}
                Ok(Some(_)) => return Err(ledger_failure(LedgerError::StaleIncarnation)),
                Ok(None) => return Err(ledger_failure(LedgerError::Unknown)),
                Err(_) => return Err(ledger_failure(LedgerError::Store)),
            }
            // Questions have no binding; an approval must not be dismissed through this door.
            if requests
                .binding(&key)
                .await
                .map_err(|_| ledger_failure(LedgerError::Store))?
                .is_some()
            {
                return Err(ledger_failure(LedgerError::Unknown));
            }
            inner.ledger.withdraw(&key).await.map_err(ledger_failure)?;
            live.as_ref()
                .map_err(Clone::clone)?
                .send(SessionCommand::DeclineQuestion { request_id })
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
            let listed: Vec<ListedRule<'_>> = rules
                .iter()
                .map(|rule| ListedRule {
                    invalid_reason: validate_rule_v4(
                        &rule.matcher,
                        rule.decision,
                        rule.path_scope.as_deref(),
                    )
                    .err(),
                    rule,
                })
                .collect();
            value(&listed)
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
            // Accepted. The connection's reader stops the daemon once this
            // answer is queued (`server::handle`).
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
            approval_at_ms,
            question_at_ms,
        } => (
            Agent::Fake(fake::Params {
                tokens_per_second,
                chunk_chars,
                duration_ms,
                tool_call_every_ms,
                approval_at_ms,
                question_at_ms,
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
    let stored = inner
        .store
        .launch(thread)
        .await
        .map_err(store_failure)?
        .ok_or_else(|| {
            failure(
                ErrorCode::UnknownThread,
                format!("unknown thread {thread:?}"),
            )
        })?;
    let adapter = inner
        .adapters
        .get(&stored.provider)
        .cloned()
        .ok_or_else(|| {
            failure(
                ErrorCode::Invalid,
                format!("no adapter for {:?} in this daemon", stored.provider),
            )
        })?;
    // Wait for any already-ending session on this thread to complete its final
    // teardown before checking/closing open items or launching the next one.
    inner.wait_if_ending(thread).await?;
    // The incarnation before is closed first. It is, unless storing its
    // closing events failed when its session ended: that is tried again
    // here, and the thread isn't resumed until it succeeds, or the open
    // items of that incarnation would never be closed.
    if summary.incarnation > 0 && inner.session_of(thread, None).is_err() {
        inner
            .close_incarnation(
                thread,
                summary.incarnation,
                None,
                Some("closed before a resume".into()),
                None,
            )
            .await
            .map_err(store_failure)?;
    }
    let launch = Launch {
        agent: Agent::Native(adapter),
        provider: stored.provider,
        model: stored.model,
        effort: stored.effort,
        preset: stored.preset,
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
            // The user asked, so the exit that follows is `Ended`. Mark
            // terminal before publishing `Ended` so an immediate resume waits
            // for teardown instead of racing with it.
            live.mark_terminal();
            if inner.store.signal(thread, Signal::Stopped).await.is_ok() {
                inner.refresh_thread(thread).await;
            }
            live.stop();
        }
        // These have their own commands, so the ledger sees them first.
        SessionCommand::ResolveApproval { .. }
        | SessionCommand::AnswerQuestion { .. }
        | SessionCommand::DeclineQuestion { .. } => {
            return Err(failure(
                ErrorCode::Invalid,
                "use ResolveApproval, AnswerQuestion, or DeclineQuestion",
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

#[derive(serde::Serialize)]
struct ListedRule<'a> {
    #[serde(flatten)]
    rule: &'a Rule,
    /// Present only when a stored rule is invalid under policy rev 4 §7
    /// (§11 item 5: stored invalid rules stay listed, marked invalid, and
    /// stop covering anything).
    #[serde(skip_serializing_if = "Option::is_none")]
    invalid_reason: Option<String>,
}

async fn create_rule(inner: &Arc<Inner>, draft: RuleDraft) -> Result<Value, Failure> {
    if let RuleScope::Thread { thread_id } = &draft.scope
        && thread_id.0.is_empty()
    {
        return Err(failure(
            ErrorCode::Invalid,
            "the rule's thread scope is empty",
        ));
    }
    if let Err(reason) =
        validate_rule_v4(&draft.matcher, draft.decision, draft.path_scope.as_deref())
    {
        return Err(failure(ErrorCode::Invalid, reason));
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
    let rule = inner.store.create_rule(rule).await.map_err(store_failure)?;
    value(&rule)
}

// ---------------------------------------------------------------------------
// Policy rev 4 §7 rule validation mirror (CR-P3-1: P0's `policy::v4` is
// crate-private inside `atrium-core`; once `atrium_core::policy` re-exports
// `validate_rule_matcher`, replace this module-local mirror with a direct call)
// ---------------------------------------------------------------------------

fn validate_rule_v4(
    matcher: &RuleMatcher,
    decision: RuleDecision,
    path_scope: Option<&str>,
) -> Result<(), String> {
    if let Some(scope) = path_scope {
        validate_glob_v4(scope)?;
    }
    match matcher {
        RuleMatcher::Any => Ok(()),
        RuleMatcher::PathGlob { glob } => validate_glob_v4(glob),
        RuleMatcher::CommandExact { argv } => {
            if argv.is_empty() || argv[0].is_empty() {
                return Err("The rule's command is empty.".to_owned());
            }
            Ok(())
        }
        RuleMatcher::CommandPrefix { argv } => {
            if argv.is_empty() || argv[0].is_empty() {
                return Err("The rule's command is empty.".to_owned());
            }
            if decision == RuleDecision::Deny {
                return Ok(());
            }
            validate_allow_command_prefix_v4(argv)
        }
    }
}

/// Pure `globset 0.4.21`-compatible glob syntax check (matches `v4::rules::validate_glob_syntax`
/// and `app/src/chat/rule-form.ts::globSyntaxError`).
fn validate_glob_v4(glob: &str) -> Result<(), String> {
    if glob.is_empty() {
        return Err("Invalid glob syntax: empty glob.".to_owned());
    }
    let cs: Vec<char> = glob.chars().collect();
    let mut depth: isize = 0;
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '\\' => {
                if i + 1 >= cs.len() {
                    return Err(format!(
                        "Invalid glob syntax: `{glob}` (trailing backslash)."
                    ));
                }
                i += 2;
            }
            '{' => {
                depth += 1;
                i += 1;
            }
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return Err(format!("Invalid glob syntax: `{glob}` (unmatched `}}`)."));
                }
                i += 1;
            }
            '[' => {
                let mut k = i + 1;
                if matches!(cs.get(k), Some('!' | '^')) {
                    k += 1;
                }
                let mut first = true;
                let mut in_range = false;
                let mut last: Option<char> = None;
                loop {
                    let Some(&d) = cs.get(k) else {
                        return Err(format!("Invalid glob syntax: `{glob}` (unclosed `[`)."));
                    };
                    k += 1;
                    if d == ']' && !first {
                        break;
                    }
                    if d == '-' && !first && !in_range {
                        in_range = true;
                    } else if in_range {
                        if let Some(prev) = last
                            && d < prev
                        {
                            return Err(format!(
                                "Invalid glob syntax: `{glob}` (descending range `{prev}-{d}`)."
                            ));
                        }
                        in_range = false;
                    } else {
                        last = Some(d);
                    }
                    first = false;
                }
                i = k;
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return Err(format!("Invalid glob syntax: `{glob}` (unclosed `{{`)."));
    }
    Ok(())
}

/// CR-19 command-word fold: basename after last `/`, Unicode lowercase (which
/// also folds U+212A Kelvin sign to `k`), and `ſ` (U+017F) → `s`.
fn fold_cmd_v4(word: &str) -> String {
    let base = match word.rfind('/') {
        Some(pos) => &word[pos + 1..],
        None => word,
    };
    if base.is_ascii() {
        base.to_ascii_lowercase()
    } else {
        base.to_lowercase().replace('\u{017F}', "s")
    }
}

fn validate_allow_command_prefix_v4(argv: &[String]) -> Result<(), String> {
    const OUTSIDE_NO_PIN: &[&str] = &[
        // Outside-model shells
        "zsh",
        "ksh",
        "fish",
        "ash",
        "csh",
        "tcsh",
        "mksh",
        "yash",
        "rbash",
        "posh",
        "elvish",
        "xonsh",
        // Outside-model wrappers
        "nohup",
        "builtin",
        "setsid",
        "unbuffer",
        "caffeinate",
        "sandbox-exec",
        "ionice",
        "stdbuf",
        "busybox",
        "toybox",
        "chrt",
        "taskset",
        "arch",
        "chroot",
        "flock",
        "script",
    ];
    const NESTED_SHELLS: &[&str] = &["sh", "bash", "dash"];
    const FIRST_OP_RUNNERS: &[&str] = &[
        "python", "python3", "perl", "node", "ruby", "source", ".", "npx",
    ];
    const FLAG_VAL_RUNNERS: &[&str] = &["awk", "gawk", "mawk"];
    const NO_PIN_RUNNERS: &[&str] = &[
        "eval",
        "xargs",
        "ssh",
        "mosh",
        "autossh",
        "nsenter",
        "unshare",
        "docker",
        "podman",
        "nerdctl",
        "kubectl",
        "oc",
        "lxc",
        "incus",
        "machinectl",
        "systemd-run",
        "systemd-nspawn",
        "firejail",
        "bwrap",
        "flatpak-spawn",
        "toolbox",
        "distrobox",
        "distrobox-enter",
        "multipass",
        "vagrant",
        "limactl",
        "colima",
        "parallel",
        "su",
        "runuser",
        "setpriv",
    ];

    let folded = fold_cmd_v4(&argv[0]);
    let name = folded.as_str();

    if OUTSIDE_NO_PIN.contains(&name) {
        return Err(format!(
            "An Allow prefix rule can't end at `{name}` without a modeled command."
        ));
    }
    if NESTED_SHELLS.contains(&name) {
        if argv.len() >= 2 && !argv[1].is_empty() && !argv[1].starts_with('-') {
            return Ok(());
        }
        return Err(format!(
            "An Allow prefix rule can't end at `{name}` without pinning a script file right after it (e.g. `sh script.sh`)."
        ));
    }
    if FIRST_OP_RUNNERS.contains(&name) {
        if argv.len() >= 2 && !argv[1].is_empty() && !argv[1].starts_with('-') {
            return Ok(());
        }
        return Err(format!(
            "An Allow prefix rule can't end at `{name}` without pinning a script or package right after it (e.g. `python build.py`)."
        ));
    }
    if FLAG_VAL_RUNNERS.contains(&name) {
        if argv.len() >= 3
            && (argv[1] == "-f" || argv[1] == "--file")
            && !argv[2].is_empty()
            && argv[2] != "-"
            && !has_competing_program_flags_v4(&argv[3..])
        {
            return Ok(());
        }
        return Err(format!(
            "An Allow prefix rule can't end at `{name}` without pinning a program file with `-f <file>` or `--file <file>`."
        ));
    }
    if name == "cargo" {
        if argv.len() >= 2 && !argv[1].is_empty() && !argv[1].starts_with('-') {
            return Ok(());
        }
        return Err(
            "An Allow prefix rule can't end at `cargo` without pinning a subcommand (e.g. `cargo test`)."
                .to_owned(),
        );
    }
    if name == "pipx" {
        if argv.len() >= 3 && argv[1] == "run" && !argv[2].is_empty() && !argv[2].starts_with('-') {
            return Ok(());
        }
        return Err(
            "An Allow prefix rule can't end at `pipx` without pinning `pipx run <package>`."
                .to_owned(),
        );
    }
    if NO_PIN_RUNNERS.contains(&name) {
        return Err(format!(
            "An Allow prefix rule can't end at `{name}` without a pinned program (use an exact command rule instead)."
        ));
    }
    if name == "find" {
        return Err(
            "An Allow prefix rule can't end at `find` (it would allow appending `-exec`; use an exact command rule instead)."
                .to_owned(),
        );
    }
    if name == "git" {
        return validate_git_prefix_v4(argv);
    }
    if name == "sed" {
        return validate_sed_prefix_v4(argv);
    }
    Ok(())
}

fn has_competing_program_flags_v4(rest: &[String]) -> bool {
    const FORBIDDEN: &[&str] = &["-f", "--file", "-e", "--source", "--eval", "--expression"];
    for tok in rest {
        if tok == "--" {
            break;
        }
        if !tok.starts_with('-') || tok == "-" {
            continue;
        }
        for &f in FORBIDDEN {
            if let Some(long) = f.strip_prefix("--") {
                if let Some(t) = tok.strip_prefix("--") {
                    let name = t.split('=').next().unwrap_or(t);
                    if !name.is_empty() && long.starts_with(name) {
                        return true;
                    }
                }
            } else if f.len() == 2
                && let Some(ch) = f.chars().nth(1)
                && !tok.starts_with("--")
                && tok[1..].contains(ch)
            {
                return true;
            }
        }
    }
    false
}

fn matches_git_flag_v4(tok: &str, trigger: &str) -> bool {
    if let Some(long) = trigger.strip_prefix("--") {
        let Some(rest) = tok.strip_prefix("--") else {
            return false;
        };
        let name = rest.split('=').next().unwrap_or(rest);
        return name == long;
    }
    if trigger.len() == 2
        && trigger.starts_with('-')
        && let Some(ch) = trigger.chars().nth(1)
        && let Some(cluster) = tok.strip_prefix('-')
        && !cluster.is_empty()
        && !tok.starts_with("--")
    {
        return cluster.contains(ch);
    }
    tok == trigger
}

fn validate_git_prefix_v4(argv: &[String]) -> Result<(), String> {
    if argv.len() < 2 || argv[1].is_empty() || argv[1].starts_with('-') {
        return Err(
            "An Allow prefix rule can't end at `git` without pinning a non-code-running subcommand (e.g. `git status`)."
                .to_owned(),
        );
    }
    let sub = argv[1].as_str();
    let rest = &argv[2..];
    if matches!(sub, "merge-index" | "filter-branch") {
        return Err(format!(
            "An Allow prefix rule can't allow `git {sub}`: it runs external programs."
        ));
    }
    if let Some(bad_subarg) = match sub {
        "bisect" => Some("run"),
        "submodule" => Some("foreach"),
        _ => None,
    } {
        let first_non_flag = rest
            .iter()
            .take_while(|a| a.as_str() != "--")
            .find(|a| !a.starts_with('-'))
            .map(String::as_str);
        if first_non_flag.is_none() || first_non_flag == Some(bad_subarg) {
            return Err(format!(
                "An Allow prefix rule on `git {sub}` must pin a non-code-running action."
            ));
        }
    }
    let trigger_flags: &[&str] = match sub {
        "rebase" => &["-x", "--exec"],
        "difftool" | "mergetool" => &["-x", "--tool-cmd"],
        "diff" => &["--ext-diff"],
        "push" => &["--receive-pack", "--exec"],
        _ => &[],
    };
    for arg in rest {
        if arg == "--" {
            break;
        }
        if trigger_flags.iter().any(|f| matches_git_flag_v4(arg, f)) {
            return Err(format!(
                "An Allow prefix rule can't allow `git {sub}` with code-running flag `{arg}`."
            ));
        }
    }
    Ok(())
}

fn validate_sed_prefix_v4(argv: &[String]) -> Result<(), String> {
    if argv.len() < 2 {
        return Err(
            "An Allow prefix rule can't end at `sed` without pinning a program file (`sed -f script.sed`) or a safe inline script."
                .to_owned(),
        );
    }
    if argv[1] == "-f" || argv[1] == "--file" {
        if argv.len() >= 3
            && !argv[2].is_empty()
            && argv[2] != "-"
            && !has_competing_program_flags_v4(&argv[3..])
        {
            return Ok(());
        }
        return Err(
            "An Allow prefix rule can't end at `sed` without pinning a program file (`sed -f script.sed`)."
                .to_owned(),
        );
    }
    let mut scripts: Vec<&str> = Vec::new();
    let mut first_pos: Option<&str> = None;
    let mut i = 1usize;
    let mut dd = false;
    while i < argv.len() {
        let tok = argv[i].as_str();
        if dd || !tok.starts_with('-') || tok == "-" {
            if first_pos.is_none() {
                first_pos = Some(tok);
            }
            i += 1;
            continue;
        }
        if tok == "--" {
            dd = true;
            i += 1;
            continue;
        }
        if let Some(rest) = tok.strip_prefix("--") {
            let (name, val) = match rest.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (rest, None),
            };
            match name {
                "expression" => {
                    if let Some(v) = val {
                        scripts.push(v);
                        i += 1;
                    } else if i + 1 < argv.len() {
                        scripts.push(argv[i + 1].as_str());
                        i += 2;
                    } else {
                        return Err(
                            "Invalid `sed` prefix: `--expression` missing script.".to_owned()
                        );
                    }
                }
                "quiet" | "silent" | "regexp-extended" | "separate" | "unbuffered"
                | "null-data" | "posix" | "sandbox"
                    if val.is_none() =>
                {
                    i += 1;
                }
                _ => {
                    return Err(format!(
                        "An Allow prefix rule on `sed` can't use unmodeled flag `{tok}`."
                    ));
                }
            }
        } else {
            let mut consumed_next = false;
            for (byte_off, ch) in tok[1..].char_indices() {
                let rest_start = 1 + byte_off + ch.len_utf8();
                match ch {
                    'e' => {
                        if rest_start < tok.len() {
                            scripts.push(&tok[rest_start..]);
                        } else if i + 1 < argv.len() {
                            scripts.push(argv[i + 1].as_str());
                            consumed_next = true;
                        } else {
                            return Err("Invalid `sed` prefix: `-e` missing script.".to_owned());
                        }
                        break;
                    }
                    'n' | 'E' | 'r' | 's' | 'u' | 'z' => {}
                    _ => {
                        return Err(format!(
                            "An Allow prefix rule on `sed` can't use unmodeled flag `{tok}`."
                        ));
                    }
                }
            }
            i += if consumed_next { 2 } else { 1 };
        }
    }
    if scripts.is_empty() {
        let Some(s) = first_pos else {
            return Err(
                "An Allow prefix rule on `sed` must pin a script or `-f <file>`.".to_owned(),
            );
        };
        scripts.push(s);
    }
    if scripts.into_iter().all(is_sed_script_safe_v4) {
        Ok(())
    } else {
        Err(
            "An Allow prefix rule on `sed` can't use `e`/`w`/`r`/`W`/`R` or unparsed script syntax."
                .to_owned(),
        )
    }
}

fn is_sed_script_safe_v4(script: &str) -> bool {
    let bytes = script.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    let skip_regex = |pos: &mut usize, delim: u8| -> bool {
        let mut in_class = false;
        while *pos < n {
            let b = bytes[*pos];
            if b == b'\\' {
                if *pos + 1 >= n {
                    return false;
                }
                *pos += 2;
                continue;
            }
            if in_class {
                if b == b']' {
                    in_class = false;
                }
                *pos += 1;
                continue;
            }
            if b == b'[' {
                in_class = true;
                *pos += 1;
                continue;
            }
            if b == delim {
                *pos += 1;
                return true;
            }
            *pos += 1;
        }
        false
    };
    let skip_addr = |pos: &mut usize| -> i8 {
        if *pos >= n {
            return 0;
        }
        match bytes[*pos] {
            b'0'..=b'9' => {
                while *pos < n && bytes[*pos].is_ascii_digit() {
                    *pos += 1;
                }
                if *pos < n && bytes[*pos] == b'~' {
                    *pos += 1;
                    if *pos >= n || !bytes[*pos].is_ascii_digit() {
                        return -1;
                    }
                    while *pos < n && bytes[*pos].is_ascii_digit() {
                        *pos += 1;
                    }
                }
                1
            }
            b'$' => {
                *pos += 1;
                1
            }
            b'/' => {
                *pos += 1;
                if !skip_regex(pos, b'/') {
                    return -1;
                }
                while *pos < n && matches!(bytes[*pos], b'I' | b'M') {
                    *pos += 1;
                }
                1
            }
            b'\\' => {
                if *pos + 1 >= n || bytes[*pos + 1] == b'\n' {
                    return -1;
                }
                let d = bytes[*pos + 1];
                *pos += 2;
                if !skip_regex(pos, d) {
                    return -1;
                }
                while *pos < n && matches!(bytes[*pos], b'I' | b'M') {
                    *pos += 1;
                }
                1
            }
            _ => 0,
        }
    };

    let mut depth = 0usize;
    while i < n {
        while i < n && matches!(bytes[i], b' ' | b'\t' | b';' | b'\n' | b'\r') {
            i += 1;
        }
        if i >= n {
            break;
        }
        if bytes[i] == b'#' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i] == b'}' {
            if depth == 0 {
                return false;
            }
            depth -= 1;
            i += 1;
            continue;
        }
        let a1 = skip_addr(&mut i);
        if a1 < 0 {
            return false;
        }
        if a1 > 0 {
            while i < n && matches!(bytes[i], b' ' | b'\t') {
                i += 1;
            }
            if i < n && bytes[i] == b',' {
                i += 1;
                while i < n && matches!(bytes[i], b' ' | b'\t') {
                    i += 1;
                }
                if i < n && matches!(bytes[i], b'+' | b'~') {
                    i += 1;
                    if i >= n || !bytes[i].is_ascii_digit() {
                        return false;
                    }
                    while i < n && bytes[i].is_ascii_digit() {
                        i += 1;
                    }
                } else if skip_addr(&mut i) <= 0 {
                    return false;
                }
            }
        }
        while i < n && matches!(bytes[i], b' ' | b'\t' | b'!') {
            i += 1;
        }
        if i >= n {
            return false;
        }
        let cmd = bytes[i];
        i += 1;
        match cmd {
            b'{' => depth += 1,
            b'e' | b'w' | b'r' | b'W' | b'R' => return false,
            b's' => {
                if i >= n || matches!(bytes[i], b'\n' | b'\\') {
                    return false;
                }
                let delim = bytes[i];
                i += 1;
                if !skip_regex(&mut i, delim) {
                    return false;
                }
                let mut closed = false;
                while i < n {
                    if bytes[i] == b'\\' {
                        if i + 1 >= n {
                            return false;
                        }
                        i += 2;
                        continue;
                    }
                    if bytes[i] == delim {
                        i += 1;
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    return false;
                }
                while i < n {
                    match bytes[i] {
                        b';' | b'\n' | b'\r' | b'}' => break,
                        b' ' | b'\t' => i += 1,
                        b'e' | b'w' | b'W' => return false,
                        b'g' | b'p' | b'I' | b'i' | b'M' | b'm' | b'0'..=b'9' => i += 1,
                        _ => return false,
                    }
                }
            }
            b'y' => {
                if i >= n || matches!(bytes[i], b'\n' | b'\\') {
                    return false;
                }
                let delim = bytes[i];
                i += 1;
                for _ in 0..2 {
                    let mut closed = false;
                    while i < n {
                        if bytes[i] == b'\\' {
                            if i + 1 >= n {
                                return false;
                            }
                            i += 2;
                            continue;
                        }
                        if bytes[i] == delim {
                            i += 1;
                            closed = true;
                            break;
                        }
                        i += 1;
                    }
                    if !closed {
                        return false;
                    }
                }
            }
            b'd' | b'D' | b'g' | b'G' | b'h' | b'H' | b'l' | b'n' | b'N' | b'p' | b'P' | b'x'
            | b'=' => {}
            b'q' | b'Q' => {
                while i < n && matches!(bytes[i], b' ' | b'\t') {
                    i += 1;
                }
                while i < n && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            b':' | b'b' | b't' | b'T' => {
                while i < n && !matches!(bytes[i], b';' | b'\n' | b'\r' | b'}') {
                    i += 1;
                }
            }
            b'a' | b'i' | b'c' => {
                while i < n {
                    if bytes[i] == b'\\' {
                        if i + 1 >= n {
                            return false;
                        }
                        i += 2;
                        continue;
                    }
                    if bytes[i] == b'\n' {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            _ => return false,
        }
    }
    depth == 0
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
            // One change at a time, so the setting and the service can't
            // end up saying different things.
            let _changing = inner.service.lock().await;
            // The setting first, then the service, so a store that refuses
            // leaves the service as it was. If the service can't be
            // changed, the setting goes back to what it was.
            let before = inner
                .store
                .settings()
                .await
                .map_err(store_failure)?
                .into_iter()
                .find(|(name, _)| name == KEEP_RUNNING_SETTING)
                .map_or(Value::Bool(false), |(_, value)| value);
            inner
                .store
                .set_setting(&key, Value::Bool(keep))
                .await
                .map_err(store_failure)?;
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
            if let Err(error) = changed {
                let restored = inner.store.set_setting(&key, before).await;
                return Err(failure(
                    ErrorCode::Internal,
                    match restored {
                        Ok(()) => format!("the login service couldn't be changed: {error}"),
                        Err(store) => format!(
                            "the login service couldn't be changed ({error}), and the setting \
                             couldn't be put back ({store})"
                        ),
                    },
                ));
            }
            return Ok(Value::Null);
        }
    }
    if is_notify_setting(&key) {
        let Value::Bool(enabled) = new else {
            return Err(failure(
                ErrorCode::Invalid,
                format!("{key} is true or false"),
            ));
        };
        inner
            .store
            .set_setting(&key, Value::Bool(enabled))
            .await
            .map_err(store_failure)?;
        inner.hub().set_notify_setting(&key, enabled);
        return Ok(Value::Null);
    }
    inner
        .store
        .set_setting(&key, new)
        .await
        .map_err(store_failure)?;
    Ok(Value::Null)
}

/// Makes the login service say what the stored setting says. At start: a
/// daemon that died between storing the setting and changing the service
/// left them apart.
pub(crate) async fn reconcile_service(inner: &Arc<Inner>) {
    if !inner.options.manage_service {
        return;
    }
    let _changing = inner.service.lock().await;
    let Ok(settings) = inner.store.settings().await else {
        return;
    };
    let Some((_, Value::Bool(keep))) = settings
        .into_iter()
        .find(|(name, _)| name == KEEP_RUNNING_SETTING)
    else {
        // Never set: nothing was ever installed by this setting.
        return;
    };
    let changed = tokio::task::spawn_blocking(move || {
        if keep {
            lifecycle::install_keep_running(&std::env::current_exe()?)
        } else {
            lifecycle::remove_keep_running()
        }
    })
    .await
    .unwrap_or_else(|error| Err(io::Error::other(error)));
    if let Err(error) = changed {
        eprintln!("atriumd: the login service doesn't match its setting: {error}");
    }
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

fn write_export(
    mut pages: Pages,
    format: ExportFormat,
    path: &Path,
    home: &str,
) -> Result<history::ExportReport, Failure> {
    let file = history::create_export_file(path).map_err(|error| export_error(&error))?;
    let redactor = Redactor::new(home, &[]);
    let mut out = io::BufWriter::new(file);
    let written = history::export_thread_report(&mut pages, format, &redactor, &mut out)
        .map_err(|error| match error {
            history::HistoryError::Io(error) => export_error(&error),
            integrity => failure(ErrorCode::Internal, integrity.to_string()),
        })
        .and_then(|report| match pages.failed.take() {
            Some(failed) => Err(failed),
            None => Ok(report),
        })
        .and_then(|report| {
            out.flush()
                .map(|()| report)
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
        return Err(failure(
            ErrorCode::Invalid,
            "the export path isn't absolute",
        ));
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
    let report = tokio::task::spawn_blocking(move || write_export(pages, format, &path, &home))
        .await
        .map_err(|error| failure(ErrorCode::Internal, error.to_string()))??;
    Ok(json!({ "bytes": report.bytes, "damaged": report.damaged }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_first_line_with_text() {
        assert_eq!(
            title_from("  \n\n  fix   the\tbuild \nmore"),
            Some("fix the build".into())
        );
        assert_eq!(title_from(" \n \t"), None);
        let long = "é".repeat(500);
        assert_eq!(
            title_from(&long).map(|title| title.chars().count()),
            Some(MAX_TITLE_CHARS)
        );
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
