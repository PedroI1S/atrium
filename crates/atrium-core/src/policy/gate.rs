//! `SessionGate`: one session's `ToolGate`. Evaluates rev 4 policy using
//! `v4::paths::FsResolver` (its only filesystem access), opens and redeems
//! ledger entries.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use atrium_agents::{ApprovalTicket, BoxFuture, GateDecision, ToolCheck, ToolGate};
use atrium_protocol::agent::{Provider, UnreadNotice};
use atrium_protocol::policy::{
    HARD_DENY_DEFAULTS, PolicyContext, Preset, Rule, RuleMatcher, RuleOrigin, ToolSubject,
};
use atrium_protocol::thread::ThreadId;

use super::limits;
use super::model::{ComputeBudget, FoldRules, FsCaps};
use super::v4;
use super::{SessionGate, input_sha256};
use crate::{ApprovalLedger, Binding, PolicyEngine, PolicyInput, PolicyVerdict, RequestKey, Store};

/// Where a gate reads the current rules from: `(revision, rules)`. `None`
/// when they can't be read; the gate then denies.
pub trait RulesSource: Send + Sync {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>>;
}

/// [`RulesSource`] over the store.
pub struct StoreRules(pub Arc<dyn Store>);

impl RulesSource for StoreRules {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>> {
        Box::pin(async move { self.0.rules().await.ok() })
    }
}

/// What a gate is for: one thread, one incarnation.
#[derive(Debug, Clone)]
pub struct SessionGateConfig {
    pub thread: ThreadId,
    pub incarnation: u32,
    /// Absolute, symlinks resolved.
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub atrium_data_paths: Vec<PathBuf>,
    pub atrium_protected_paths: Vec<PathBuf>,
    pub preset: Preset,
    pub provider: Provider,
}

pub(super) const MAX_HOPS: u8 = 40;

fn expand_home(word: &str, home: &Path, cwd: &Path) -> String {
    let h = home.to_string_lossy();
    let c = cwd.to_string_lossy();
    if let Some(after) = word.strip_prefix('~') {
        let (name, rest) = match after.find('/') {
            Some(i) => (&after[..i], &after[i..]),
            None => (after, ""),
        };
        let user = home.file_name().map(|n| n.to_string_lossy().into_owned());
        if name.is_empty() || user.as_deref() == Some(name) {
            return format!("{h}{rest}");
        }
        if name == "+" || name == "-" {
            return format!("{c}{rest}");
        }
        return word.to_string();
    }
    for (var, val) in [
        ("$HOME", &*h),
        ("${HOME}", &*h),
        ("$PWD", &*c),
        ("${PWD}", &*c),
    ] {
        if let Some(rest) = word.strip_prefix(var)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            return format!("{val}{rest}");
        }
    }
    word.to_string()
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// Absolute path with the deepest existing ancestor canonicalized and the
/// rest appended (dangling symlinks are followed by hand).
pub fn fs_resolve(word: &str, cwd: &Path, home: &Path) -> PathBuf {
    let w = expand_home(word, home, cwd);
    let p = Path::new(&w);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    };
    resolve_fs(&abs, 0)
}

fn resolve_fs(p: &Path, hops: u8) -> PathBuf {
    let mut head = p.to_path_buf();
    let mut tail: Vec<OsString> = vec![];
    let base = loop {
        match std::fs::canonicalize(&head) {
            Ok(c) => break c,
            Err(_) => {
                if hops < MAX_HOPS
                    && let Ok(md) = std::fs::symlink_metadata(&head)
                    && md.is_symlink()
                    && let Ok(t) = std::fs::read_link(&head)
                {
                    let target = if t.is_absolute() {
                        t
                    } else {
                        head.parent().unwrap_or(Path::new("/")).join(t)
                    };
                    break resolve_fs(&normalize(&target), hops + 1);
                }
                match head.file_name() {
                    Some(n) => {
                        tail.push(n.to_owned());
                        head.pop();
                    }
                    None => {
                        let n = normalize(&head);
                        if n == head || hops >= MAX_HOPS {
                            break head;
                        }
                        // `missing/..`: start over on the cleaned-up path
                        let mut rebuilt = n;
                        for t in tail.into_iter().rev() {
                            rebuilt.push(t);
                        }
                        return resolve_fs(&rebuilt, hops + 1);
                    }
                }
            }
        }
    };
    let mut out = base;
    for t in tail.into_iter().rev() {
        out.push(t);
    }
    out
}

fn deny_reason(id: &str) -> String {
    let d = HARD_DENY_DEFAULTS.iter().find(|d| d.id == id);
    format!("Always blocked ({id}): {}", d.map_or("", |d| d.description))
}

#[cfg(unix)]
fn path_to_bytes(p: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    p.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_to_bytes(p: &Path) -> Vec<u8> {
    p.as_os_str().as_encoded_bytes().to_vec()
}

fn precheck_too_large(check: &ToolCheck) -> bool {
    match &check.subject {
        ToolSubject::Command { line, .. } => line.len() > limits::MAX_BYTES,
        ToolSubject::Paths { paths, glob, .. } => {
            paths.len() > limits::MAX_USES
                || glob.as_deref().is_some_and(|g| g.len() > limits::MAX_BYTES)
        }
        ToolSubject::Fetch { url } => url.len() > limits::MAX_BYTES,
        ToolSubject::Mcp { .. } | ToolSubject::Unknown => {
            v4::evaluate::json_input_too_large(&check.input)
        }
        ToolSubject::None => false,
    }
}

pub struct Evaluated {
    pub verdict: PolicyVerdict,
    pub remember_matcher: Option<RuleMatcher>,
    pub unread: Option<UnreadNotice>,
}

fn evaluate_sync(
    c: &SessionGateConfig,
    _policy: &dyn PolicyEngine,
    check: &ToolCheck,
    rules: &[Rule],
    preset: Preset,
) -> Result<Evaluated, String> {
    if precheck_too_large(check) {
        return Ok(Evaluated {
            verdict: PolicyVerdict::Deny {
                reason: "too large to check".into(),
                rule: None,
            },
            remember_matcher: None,
            unread: None,
        });
    }

    let input = PolicyInput {
        check,
        thread: &c.thread,
        provider: c.provider,
        cwd: &c.cwd,
        home: &c.home,
        atrium_data_paths: &c.atrium_data_paths,
        atrium_protected_paths: &c.atrium_protected_paths,
        resolved_paths: &[],
        preset,
    };

    let resolver = v4::paths::FsResolver::new();
    // Give `build_protected_set` its own `FsCaps` and `ComputeBudget` so its
    // discovery/protected-root walks stay strictly bounded without starving the
    // call's own caps or compute budget (contract §8).
    let mut protected_caps = FsCaps::with_default_limits();
    let mut protected_budget = ComputeBudget::with_default_limits();
    let home_bytes = path_to_bytes(&c.home);
    let cwd_bytes = path_to_bytes(&c.cwd);
    let data_bytes: Vec<Vec<u8>> = c
        .atrium_data_paths
        .iter()
        .map(|p| path_to_bytes(p))
        .collect();
    let prot_bytes: Vec<Vec<u8>> = c
        .atrium_protected_paths
        .iter()
        .map(|p| path_to_bytes(p))
        .collect();

    let protected = match v4::paths::sensitive::build_protected_set(
        &home_bytes,
        &cwd_bytes,
        data_bytes,
        prot_bytes,
        FoldRules::REV4,
        &resolver,
        &mut protected_caps,
        &mut protected_budget,
    ) {
        Ok(p) => p,
        Err(_) => {
            return Ok(Evaluated {
                verdict: PolicyVerdict::Ask,
                remember_matcher: None,
                unread: Some(UnreadNotice::Exhausted),
            });
        }
    };

    let eval = v4::evaluate::evaluate_call(&input, rules, &protected, &resolver);
    let unread = v4::evaluate::unread_notice_for_outcome(&eval.outcome);
    let remember_matcher = if unread.is_some() {
        None
    } else {
        eval.remember_matcher
    };

    Ok(Evaluated {
        verdict: eval.verdict,
        remember_matcher,
        unread,
    })
}

impl SessionGate {
    pub fn new(
        config: SessionGateConfig,
        policy: Arc<dyn PolicyEngine>,
        ledger: Arc<dyn ApprovalLedger>,
        rules: Arc<dyn RulesSource>,
    ) -> Self {
        let preset = Mutex::new(config.preset);
        Self {
            config,
            preset,
            policy,
            ledger,
            rules,
            opened: Mutex::new(HashMap::new()),
        }
    }

    /// Changes the preset for later checks. Approvals opened under another
    /// preset are re-evaluated when redeemed.
    pub fn set_preset(&self, preset: Preset) {
        if let Ok(mut p) = self.preset.lock() {
            *p = preset;
        }
    }

    fn preset(&self) -> Option<Preset> {
        self.preset.lock().ok().map(|p| *p)
    }

    /// Path resolution touches the filesystem, so it runs on the blocking
    /// pool; a panic anywhere in it becomes an error (the caller denies).
    async fn evaluate(
        &self,
        check: &ToolCheck,
        rules: &[Rule],
        preset: Preset,
    ) -> Result<Evaluated, String> {
        let cfg = self.config.clone();
        let policy = self.policy.clone();
        let check = check.clone();
        let rules = rules.to_vec();
        tokio::task::spawn_blocking(move || {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                evaluate_sync(&cfg, policy.as_ref(), &check, &rules, preset)
            }))
            .map_err(|_| "internal error while checking".to_string())?
        })
        .await
        .map_err(|e| format!("internal error while checking: {e}"))?
    }

    async fn do_check(&self, check: &ToolCheck) -> Result<GateDecision, String> {
        let (rev, rules) = self.rules.snapshot().await.ok_or("rules unavailable")?;
        let preset = self.preset().ok_or("preset unavailable")?;
        let e = self.evaluate(check, &rules, preset).await?;
        match &e.verdict {
            PolicyVerdict::Allow { .. } => Ok(GateDecision::Allow),
            PolicyVerdict::Deny { reason, .. } => Ok(GateDecision::Deny {
                reason: reason.clone(),
            }),
            PolicyVerdict::HardDeny { id } => Ok(GateDecision::Deny {
                reason: deny_reason(id),
            }),
            PolicyVerdict::Ask => {
                let context = PolicyContext {
                    rules_revision: rev,
                    preset,
                };
                let binding = Binding {
                    turn: check.turn,
                    tool_name: check.name.clone(),
                    tool_kind: check.kind,
                    input_sha256: input_sha256(check),
                    context,
                    title: check.title.clone(),
                    remember_matcher: e.remember_matcher,
                    unread: e.unread,
                };
                let ticket = self
                    .ledger
                    .open_approval(&self.config.thread, self.config.incarnation, binding)
                    .await
                    .map_err(|e| e.to_string())?;
                self.opened
                    .lock()
                    .map_err(|_| "gate state poisoned")?
                    .insert(ticket.request_id.clone(), context);
                Ok(GateDecision::Ask { ticket })
            }
        }
    }

    async fn do_redeem(
        &self,
        ticket: &ApprovalTicket,
        check: &ToolCheck,
    ) -> Result<GateDecision, String> {
        let key = RequestKey {
            thread: self.config.thread.clone(),
            incarnation: self.config.incarnation,
            request_id: ticket.request_id.clone(),
        };
        let (_rev, rules) = self.rules.snapshot().await.ok_or("rules unavailable")?;
        let preset = self.preset().ok_or("preset unavailable")?;
        let known = self
            .opened
            .lock()
            .map_err(|_| "gate state poisoned")?
            .get(&ticket.request_id)
            .copied();
        if known.is_none() {
            return Err("unknown approval".into());
        }
        // Without the rule this approval's own "always…" created.
        let others: Vec<Rule> = rules
            .into_iter()
            .filter(|r| !matches!(&r.origin, RuleOrigin::Approval { request_id } if *request_id == ticket.request_id))
            .collect();
        // Always evaluated, so a symlink swapped since the approval, a deny
        // rule added meanwhile or a changed preset can't reach a blocked
        // place. `Ask` is fine: the user already approved this call.
        let e = self.evaluate(check, &others, preset).await?;
        match e.verdict {
            PolicyVerdict::HardDeny { id } => {
                let _ = self.ledger.withdraw(&key).await;
                return Ok(GateDecision::Deny {
                    reason: deny_reason(id),
                });
            }
            PolicyVerdict::Deny { reason, .. } => {
                let _ = self.ledger.withdraw(&key).await;
                return Ok(GateDecision::Deny { reason });
            }
            PolicyVerdict::Allow { .. } | PolicyVerdict::Ask => {}
        }
        self.ledger
            .redeem(&key, check)
            .await
            .map_err(|e| e.to_string())?;
        if let Ok(mut m) = self.opened.lock() {
            m.remove(&ticket.request_id);
        }
        Ok(GateDecision::Allow)
    }

    /// Withdraws an open ticket as cancelled, freeing its ledger slot and
    /// preventing future redemption. Idempotent: resolved, redeemed, or
    /// unknown tickets are a no-op and never fail the turn.
    pub async fn withdraw(&self, ticket: &ApprovalTicket) {
        let key = RequestKey {
            thread: self.config.thread.clone(),
            incarnation: self.config.incarnation,
            request_id: ticket.request_id.clone(),
        };
        if let Ok(mut m) = self.opened.lock() {
            m.remove(&ticket.request_id);
        }
        let _ = self.ledger.withdraw(&key).await;
    }
}

impl ToolGate for SessionGate {
    fn check<'a>(&'a self, check: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            self.do_check(check)
                .await
                .unwrap_or_else(|reason| GateDecision::Deny { reason })
        })
    }

    fn redeem<'a>(
        &'a self,
        ticket: &'a ApprovalTicket,
        check: &'a ToolCheck,
    ) -> BoxFuture<'a, GateDecision> {
        Box::pin(async move {
            self.do_redeem(ticket, check)
                .await
                .unwrap_or_else(|reason| GateDecision::Deny { reason })
        })
    }

    fn question_ticket(&self, turn: Option<u32>) -> BoxFuture<'_, Option<ApprovalTicket>> {
        Box::pin(async move {
            self.ledger
                .open_question(&self.config.thread, self.config.incarnation, turn)
                .await
                .ok()
        })
    }

    fn withdraw<'a>(&'a self, ticket: &'a ApprovalTicket) -> BoxFuture<'a, ()> {
        Box::pin(self.withdraw(ticket))
    }
}
