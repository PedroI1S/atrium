//! Shared helpers for the M1-13 adversarial tests (each test crate file
//! includes this with `mod common_adversarial;`).
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use atrium_agents::{BoxFuture, ToolCheck};
use atrium_core::policy::{
    DefaultPolicy, Ledger, RulesSource, SessionGate, SessionGateConfig, input_sha256,
};
use atrium_core::{
    ApprovalLedger, Binding, PolicyEngine, PolicyInput, PolicyVerdict, RequestKey, RequestStore,
    StoreError, StoreResult,
};
use atrium_protocol::agent::{Provider, ToolKind};
use atrium_protocol::policy::{
    PolicyContext, Preset, Rule, RuleDecision, RuleId, RuleMatcher, RuleOrigin, RuleScope,
    ToolSubject,
};
use atrium_protocol::thread::ThreadId;
use serde_json::json;

pub const HOME: &str = "/h/u";

/// A virtual home: nothing on disk, resolved paths empty (lexical checks).
pub struct Env {
    pub thread: ThreadId,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub data: Vec<PathBuf>,
    pub prot: Vec<PathBuf>,
}

impl Env {
    pub fn home() -> Self {
        Self {
            thread: ThreadId("t1".into()),
            home: HOME.into(),
            cwd: HOME.into(),
            data: vec![format!("{HOME}/.local/share/atrium").into()],
            prot: vec![
                format!("{HOME}/.config/atrium").into(),
                "/Applications/Atrium.app".into(),
            ],
        }
    }
    pub fn eval_check(&self, check: &ToolCheck, rules: &[Rule], preset: Preset) -> PolicyVerdict {
        let input = PolicyInput {
            check,
            thread: &self.thread,
            provider: Provider::Claude,
            cwd: &self.cwd,
            home: &self.home,
            atrium_data_paths: &self.data,
            atrium_protected_paths: &self.prot,
            resolved_paths: &[],
            preset,
        };
        DefaultPolicy::new().evaluate(&input, rules)
    }
    pub fn cmd(&self, line: &str) -> PolicyVerdict {
        self.eval_check(&cmd_check(line), &[], Preset::Home)
    }
    pub fn cmd_rules(&self, line: &str, rules: &[Rule]) -> PolicyVerdict {
        self.eval_check(&cmd_check(line), rules, Preset::Home)
    }
}

pub fn cmd_check(line: &str) -> ToolCheck {
    ToolCheck {
        call_id: None,
        name: "Bash".into(),
        kind: ToolKind::Execute,
        title: None,
        input: json!({ "command": line }),
        subject: ToolSubject::Command {
            line: line.into(),
            cwd: None,
        },
        turn: 1,
    }
}

pub fn paths_check(
    kind: ToolKind,
    name: &str,
    paths: &[&str],
    recursive: bool,
    glob: Option<&str>,
) -> ToolCheck {
    ToolCheck {
        call_id: None,
        name: name.into(),
        kind,
        title: None,
        input: json!({ "paths": paths }),
        subject: ToolSubject::Paths {
            paths: paths.iter().map(|s| s.to_string()).collect(),
            glob: glob.map(str::to_string),
            recursive,
        },
        turn: 1,
    }
}

pub fn rule(
    scope: RuleScope,
    kind: ToolKind,
    matcher: RuleMatcher,
    decision: RuleDecision,
    id: &str,
) -> Rule {
    Rule {
        id: RuleId(id.into()),
        scope,
        tool_kind: kind,
        tool_name: None,
        matcher,
        path_scope: None,
        decision,
        origin: RuleOrigin::Settings,
        created_at_ms: 0,
    }
}

pub fn prefix(argv: &[&str]) -> RuleMatcher {
    RuleMatcher::CommandPrefix {
        argv: argv.iter().map(|s| s.to_string()).collect(),
    }
}

pub fn exact(argv: &[&str]) -> RuleMatcher {
    RuleMatcher::CommandExact {
        argv: argv.iter().map(|s| s.to_string()).collect(),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Want {
    Hard(&'static str),
    Allow,
    Ask,
    /// Hard deny of any id.
    AnyHard,
    /// Anything but Allow (Ask, a Deny for size, or a hard deny).
    NotAllow,
}

pub fn classify(v: &PolicyVerdict) -> String {
    match v {
        PolicyVerdict::HardDeny { id } => format!("Hard({id})"),
        PolicyVerdict::Allow { .. } => "Allow".into(),
        PolicyVerdict::Ask => "Ask".into(),
        PolicyVerdict::Deny { reason, .. } => format!("Deny({reason})"),
    }
}

pub fn matches_want(v: &PolicyVerdict, w: &Want) -> bool {
    match (w, v) {
        (Want::Hard(id), PolicyVerdict::HardDeny { id: got }) => id == got,
        (Want::Allow, PolicyVerdict::Allow { .. }) => true,
        (Want::Ask, PolicyVerdict::Ask) => true,
        (Want::AnyHard, PolicyVerdict::HardDeny { .. }) => true,
        (Want::NotAllow, v) => !matches!(v, PolicyVerdict::Allow { .. }),
        _ => false,
    }
}

pub fn run_table(env: &Env, table: &[(&str, Want)]) {
    let mut failures = vec![];
    for (line, want) in table {
        let v = env.cmd(line);
        if !matches_want(&v, want) {
            failures.push(format!("{line:?}: wanted {want:?}, got {}", classify(&v)));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

pub fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

static N: AtomicU64 = AtomicU64::new(0);

/// A temp folder with a fake HOME inside it (canonical path). Removed on drop.
pub struct Tmp {
    pub root: PathBuf,
    pub home: PathBuf,
}

impl Tmp {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "atrium-m113-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("home")).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".local/share/atrium")).unwrap();
        std::fs::create_dir_all(home.join(".config/atrium")).unwrap();
        Self { root, home }
    }
    pub fn write(&self, rel: &str, body: &str) {
        let p = self.home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    pub fn gate_with(
        &self,
        thread: &str,
        incarnation: u32,
        cwd: &Path,
        preset: Preset,
        rules: Vec<Rule>,
        store: Option<Arc<MemStore>>,
    ) -> TestGate {
        let store = store.unwrap_or_default();
        {
            let mut g = store.0.lock().unwrap();
            g.current.insert(ThreadId(thread.into()), incarnation);
            if !rules.is_empty() {
                g.rules = rules;
            }
        }
        let ledger: Arc<dyn ApprovalLedger> = Arc::new(Ledger::new(store.clone()));
        let gate = SessionGate::new(
            SessionGateConfig {
                thread: ThreadId(thread.into()),
                incarnation,
                cwd: cwd.to_path_buf(),
                home: self.home.clone(),
                atrium_data_paths: vec![self.home.join(".local/share/atrium")],
                atrium_protected_paths: vec![self.home.join(".config/atrium")],
                preset,
                provider: Provider::Claude,
            },
            Arc::new(DefaultPolicy::new()),
            ledger.clone(),
            Arc::new(MemRules(store.clone())),
        );
        TestGate {
            gate,
            ledger,
            store,
        }
    }
    pub fn gate(&self, preset: Preset, rules: Vec<Rule>) -> TestGate {
        self.gate_with("t1", 1, &self.home, preset, rules, None)
    }
}

impl Default for Tmp {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub struct TestGate {
    pub gate: SessionGate,
    pub ledger: Arc<dyn ApprovalLedger>,
    pub store: Arc<MemStore>,
}

pub fn key(thread: &str, incarnation: u32, t: &atrium_agents::ApprovalTicket) -> RequestKey {
    RequestKey {
        thread: ThreadId(thread.into()),
        incarnation,
        request_id: t.request_id.clone(),
    }
}

pub fn binding(c: &ToolCheck, matcher: Option<RuleMatcher>) -> Binding {
    Binding {
        turn: c.turn,
        tool_name: c.name.clone(),
        tool_kind: c.kind,
        input_sha256: input_sha256(c),
        context: PolicyContext {
            rules_revision: 0,
            preset: Preset::Home,
        },
        title: None,
        remember_matcher: matcher,
    }
}

// ---------------------------------------------------------------------------
// In-memory RequestStore (same semantics as tests/ledger.rs)
// ---------------------------------------------------------------------------

pub type Row = (String, Option<Binding>, Option<u32>);

#[derive(Default)]
pub struct Inner {
    pub rows: HashMap<RequestKey, Row>,
    pub rules: Vec<Rule>,
    pub rev: u64,
    pub current: HashMap<ThreadId, u32>,
}

#[derive(Default)]
pub struct MemStore(pub Mutex<Inner>);

impl MemStore {
    pub fn status(&self, key: &RequestKey) -> Option<String> {
        self.0.lock().unwrap().rows.get(key).map(|r| r.0.clone())
    }
    pub fn set_current(&self, thread: &ThreadId, inc: u32) {
        self.0.lock().unwrap().current.insert(thread.clone(), inc);
    }
}

impl RequestStore for MemStore {
    fn current_incarnation(&self, thread: &ThreadId) -> BoxFuture<'_, StoreResult<Option<u32>>> {
        let thread = thread.clone();
        Box::pin(async move { Ok(self.0.lock().unwrap().current.get(&thread).copied()) })
    }
    fn open_approval(&self, key: &RequestKey, binding: &Binding) -> BoxFuture<'_, StoreResult<()>> {
        let (key, binding) = (key.clone(), binding.clone());
        Box::pin(async move {
            let mut g = self.0.lock().unwrap();
            if g.rows.contains_key(&key) {
                return Err(StoreError::Sqlite("duplicate request id".into()));
            }
            g.rows.insert(
                key,
                ("pending".into(), Some(binding.clone()), Some(binding.turn)),
            );
            Ok(())
        })
    }
    fn open_question(&self, key: &RequestKey, turn: Option<u32>) -> BoxFuture<'_, StoreResult<()>> {
        let key = key.clone();
        Box::pin(async move {
            let mut g = self.0.lock().unwrap();
            if g.rows.contains_key(&key) {
                return Err(StoreError::Sqlite("duplicate request id".into()));
            }
            g.rows.insert(key, ("pending".into(), None, turn));
            Ok(())
        })
    }
    fn transition(
        &self,
        key: &RequestKey,
        from: &[&str],
        to: &str,
    ) -> BoxFuture<'_, StoreResult<Option<String>>> {
        let key = key.clone();
        let from: Vec<String> = from.iter().map(|s| s.to_string()).collect();
        let to = to.to_string();
        Box::pin(async move {
            tokio::task::yield_now().await;
            let mut g = self.0.lock().unwrap();
            Ok(g.rows.get_mut(&key).map(|r| {
                let found = r.0.clone();
                if from.contains(&found) {
                    r.0 = to;
                }
                found
            }))
        })
    }
    fn open_count(&self, thread: &ThreadId, incarnation: u32) -> BoxFuture<'_, StoreResult<u64>> {
        let thread = thread.clone();
        Box::pin(async move {
            let g = self.0.lock().unwrap();
            Ok(g.rows
                .iter()
                .filter(|(k, v)| {
                    k.thread == thread
                        && k.incarnation == incarnation
                        && matches!(v.0.as_str(), "pending" | "approved")
                })
                .count() as u64)
        })
    }
    fn approve_with_rule(
        &self,
        key: &RequestKey,
        rule: Option<Rule>,
    ) -> BoxFuture<'_, StoreResult<Option<String>>> {
        let key = key.clone();
        Box::pin(async move {
            tokio::task::yield_now().await;
            let mut g = self.0.lock().unwrap();
            let Some(r) = g.rows.get_mut(&key) else {
                return Ok(None);
            };
            let found = r.0.clone();
            if found == "pending" {
                r.0 = "approved".into();
                if let Some(rule) = rule {
                    g.rules.push(rule);
                    g.rev += 1;
                }
            }
            Ok(Some(found))
        })
    }
    fn binding(&self, key: &RequestKey) -> BoxFuture<'_, StoreResult<Option<(String, Binding)>>> {
        let key = key.clone();
        Box::pin(async move {
            let g = self.0.lock().unwrap();
            Ok(g.rows
                .get(&key)
                .and_then(|r| r.1.clone().map(|b| (r.0.clone(), b))))
        })
    }
    fn expire(
        &self,
        thread: Option<&ThreadId>,
        incarnation: Option<u32>,
        turn: Option<u32>,
    ) -> BoxFuture<'_, StoreResult<u64>> {
        let thread = thread.cloned();
        Box::pin(async move {
            let mut g = self.0.lock().unwrap();
            let mut n = 0;
            for (k, v) in g.rows.iter_mut() {
                if thread.as_ref().is_some_and(|t| *t != k.thread)
                    || incarnation.is_some_and(|i| i != k.incarnation)
                    || turn.is_some_and(|t| v.2 != Some(t))
                {
                    continue;
                }
                if matches!(v.0.as_str(), "pending" | "approved") {
                    v.0 = "expired".into();
                    n += 1;
                }
            }
            Ok(n)
        })
    }
}

pub struct MemRules(pub Arc<MemStore>);

impl RulesSource for MemRules {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>> {
        Box::pin(async move {
            let g = self.0.0.lock().unwrap();
            Some((g.rev, g.rules.clone()))
        })
    }
}
