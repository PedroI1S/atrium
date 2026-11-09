//! `SessionGate`: one session's `ToolGate`. Resolves paths (its only
//! filesystem access), asks the engine, opens and redeems ledger entries.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use atrium_agents::{ApprovalTicket, BoxFuture, GateDecision, ToolCheck, ToolGate};
use atrium_protocol::agent::{Provider, ToolKind};
use atrium_protocol::policy::{
    HARD_DENY_DEFAULTS, PolicyContext, Preset, Rule, RuleMatcher, RuleOrigin, ToolSubject,
};
use atrium_protocol::thread::ThreadId;

use super::deny;
use super::shell;
use super::words::{expand_home, normalize};
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

const MAX_HOPS: u8 = 40;

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

/// Escapes glob metacharacters so the pattern matches only `p`.
fn escape_glob(p: &Path) -> String {
    let mut s = String::new();
    for c in p.to_string_lossy().chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            s.push('\\');
        }
        s.push(c);
    }
    s
}

fn deny_reason(id: &str) -> String {
    let d = HARD_DENY_DEFAULTS.iter().find(|d| d.id == id);
    format!("Always blocked ({id}): {}", d.map_or("", |d| d.description))
}

pub struct Evaluated {
    pub verdict: PolicyVerdict,
    pub resolved: Vec<PathBuf>,
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

    fn evaluate(&self, check: &ToolCheck, rules: &[Rule], preset: Preset) -> Evaluated {
        let c = &self.config;
        let mut input = PolicyInput {
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
        let analysis = deny::analyze_with(&input, &fs_resolve);
        let mut resolved = vec![];
        for u in &analysis.uses {
            resolved.extend(u.paths.iter().cloned());
        }
        input.resolved_paths = &resolved;
        let verdict = self.policy.evaluate(&input, rules);
        Evaluated { verdict, resolved }
    }

    /// The rule "always…" would create (contract §5, `SessionGate` docs).
    fn remember_matcher(&self, check: &ToolCheck, e: &Evaluated) -> Option<RuleMatcher> {
        match &check.subject {
            ToolSubject::Command { line, .. } => {
                let l = shell::lex(line);
                if !l.reasons.is_empty() || l.units.len() != 1 {
                    return None;
                }
                let u = &l.units[0];
                if !u.seg.assignments.is_empty()
                    || u.seg.argv.is_empty()
                    || !u.seg.redirects.iter().all(super::Redirect::harmless)
                {
                    return None;
                }
                Some(RuleMatcher::CommandPrefix {
                    argv: u.seg.argv.clone(),
                })
            }
            ToolSubject::Paths { paths, .. } => {
                let file_tool = matches!(
                    check.kind,
                    ToolKind::Read
                        | ToolKind::Edit
                        | ToolKind::Delete
                        | ToolKind::Move
                        | ToolKind::Search
                );
                if !file_tool || paths.len() != 1 || e.resolved.len() != 1 {
                    return None;
                }
                Some(RuleMatcher::PathGlob {
                    glob: escape_glob(&e.resolved[0]),
                })
            }
            _ => None,
        }
    }

    async fn do_check(&self, check: &ToolCheck) -> Result<GateDecision, String> {
        let (rev, rules) = self.rules.snapshot().await.ok_or("rules unavailable")?;
        let preset = self.preset().ok_or("preset unavailable")?;
        let e = self.evaluate(check, &rules, preset);
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
                    remember_matcher: self.remember_matcher(check, &e),
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
        let e = self.evaluate(check, &others, preset);
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
}
