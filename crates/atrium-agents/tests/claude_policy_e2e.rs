//! Fix round 2, item 1: the subjects `tool_check` builds for `Glob` and
//! `Grep`, end to end through the real policy (`atrium_core::policy`
//! `SessionGate` + `DefaultPolicy`), in a temp layout with a home of
//! `<tmp>/home/ana` and a cwd of `<tmp>/home/ana/work`.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use atrium_agents::claude::tool_check;
use atrium_agents::{ApprovalTicket, BoxFuture, GateDecision, ToolGate};
use atrium_core::policy::{DefaultPolicy, RulesSource, SessionGate, SessionGateConfig};
use atrium_core::{ApprovalLedger, Binding, LedgerError, RequestKey};
use atrium_protocol::agent::{ApprovalDecision, Provider};
use atrium_protocol::policy::{Preset, RememberScope, Rule, RuleId, ToolSubject};
use atrium_protocol::thread::ThreadId;
use serde_json::{Value, json};

/// A ledger that only opens approvals (every `Ask` becomes ticket `ASK`).
struct AskLedger;

impl ApprovalLedger for AskLedger {
    fn open_approval(
        &self,
        _: &ThreadId,
        _: u32,
        _: Binding,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>> {
        Box::pin(async {
            Ok(ApprovalTicket {
                request_id: "ASK".into(),
                rememberable: false,
            })
        })
    }
    fn open_question(
        &self,
        _: &ThreadId,
        _: u32,
        _: Option<u32>,
    ) -> BoxFuture<'_, Result<ApprovalTicket, LedgerError>> {
        Box::pin(async { Err(LedgerError::Unknown) })
    }
    fn resolve(
        &self,
        _: &RequestKey,
        _: &ApprovalDecision,
        _: Option<RememberScope>,
    ) -> BoxFuture<'_, Result<Option<RuleId>, LedgerError>> {
        Box::pin(async { Err(LedgerError::Unknown) })
    }
    fn answer(&self, _: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Err(LedgerError::Unknown) })
    }
    fn redeem(
        &self,
        _: &RequestKey,
        _: &atrium_agents::ToolCheck,
    ) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Err(LedgerError::Unknown) })
    }
    fn withdraw(&self, _: &RequestKey) -> BoxFuture<'_, Result<(), LedgerError>> {
        Box::pin(async { Ok(()) })
    }
    fn expire_turn(&self, _: &ThreadId, _: u32, _: u32) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }
    fn expire_incarnation(&self, _: &ThreadId, _: u32) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }
    fn expire_all(&self) -> BoxFuture<'_, u64> {
        Box::pin(async { 0 })
    }
}

struct NoRules;

impl RulesSource for NoRules {
    fn snapshot(&self) -> BoxFuture<'_, Option<(u64, Vec<Rule>)>> {
        Box::pin(async { Some((0, vec![])) })
    }
}

struct Layout {
    root: PathBuf,
    home: PathBuf,
    cwd: PathBuf,
}

impl Layout {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("atrium-e2e-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(&root).unwrap();
        let home = root.join("home/ana");
        let cwd = home.join("work");
        for (file, text) in [
            (".ssh/id_ed25519", "secret"),
            (".aws/credentials", "secret"),
            ("work/src/mod.rs", "fn main() {}"),
            ("work/a.ts", "x"),
            ("work/b.tsx", "x"),
        ] {
            let p = home.join(file);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, text).unwrap();
        }
        Self { root, home, cwd }
    }

    fn gate(&self, preset: Preset) -> SessionGate {
        SessionGate::new(
            SessionGateConfig {
                thread: ThreadId("t1".into()),
                incarnation: 1,
                cwd: self.cwd.clone(),
                home: self.home.clone(),
                atrium_data_paths: vec![self.home.join(".local/share/atrium")],
                atrium_protected_paths: vec![self.home.join(".config/atrium")],
                preset,
                provider: Provider::Claude,
            },
            Arc::new(DefaultPolicy::new()),
            Arc::new(AskLedger),
            Arc::new(NoRules),
        )
    }

    /// The input, with `{home}` and `{cwd}` filled in.
    fn input(&self, tool: &str, raw: &str) -> Value {
        let text = raw
            .replace("{home}", &self.home.display().to_string())
            .replace("{cwd}", &self.cwd.display().to_string());
        let mut v: Value = serde_json::from_str(&text).unwrap();
        // Tools are given an explicit base unless the case says otherwise.
        if tool == "Glob" || tool == "Grep" {
            let _ = &mut v;
        }
        v
    }
}

impl Drop for Layout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

async fn verdict(
    l: &Layout,
    preset: Preset,
    tool: &str,
    input: Value,
) -> (GateDecision, ToolSubject) {
    let check = tool_check(Some("t1"), tool, &input, 1);
    let subject = check.subject.clone();
    (l.gate(preset).check(&check).await, subject)
}

fn hard_denied(d: &GateDecision) -> bool {
    matches!(d, GateDecision::Deny { reason } if reason.contains("Always blocked"))
}

/// Patterns that must never be allowed, in Home; the credential ones must be
/// hard-denied in Home and in FullAccess.
const ESCAPES: &[(&str, bool)] = &[
    // The review's sequences.
    (
        r#"{"path":"{cwd}","pattern":"{a,}{home}/.ssh/id_ed25519"}"#,
        true,
    ),
    (
        r#"{"path":"{cwd}","pattern":".{.,.}/.ssh/id_ed25519"}"#,
        true,
    ),
    (r#"{"path":"{cwd}","pattern":"{a,}/etc/passwd"}"#, false),
    (r#"{"path":"{cwd}","pattern":"../.aws/cred*"}"#, true),
    (r#"{"path":"{cwd}","pattern":"../.ss*/id_ed25519"}"#, true),
    (r#"{"path":"{cwd}","pattern":"~/.ss*/id_ed25519"}"#, true),
    (r#"{"path":"{cwd}","pattern":"~/.aws/*"}"#, true),
    // The first round's.
    (
        r#"{"path":"{cwd}","pattern":"{home}/.ssh/id_ed25519"}"#,
        true,
    ),
    (r#"{"pattern":"{home}/.ssh/id_ed25519"}"#, true),
    (r#"{"path":"{cwd}","pattern":"../../ana/.ssh/**"}"#, true),
    (r#"{"path":"{cwd}","pattern":"~/.ssh/*"}"#, true),
    (r#"{"path":"{cwd}","pattern":"/etc/pass*"}"#, false),
    // Variants of the same idea.
    (
        r#"{"path":"{cwd}","pattern":"{..,x}/.ssh/id_ed25519"}"#,
        true,
    ),
    (
        r#"{"path":"{cwd}","pattern":"{x,~}/.ssh/id_ed25519"}"#,
        true,
    ),
    (
        r#"{"path":"{cwd}","pattern":".{,}./.ssh/id_ed25519"}"#,
        true,
    ),
    (
        r#"{"path":"{cwd}","pattern":"src/{a,{..,b}}/../.ssh/*"}"#,
        true,
    ),
];

/// The review's `[.][.]` and friends: spelled so that we can't tell where
/// they point. `Unknown`, which the policy asks about and never allows.
const NOT_UNDERSTOOD: &[&str] = &[
    r#"{"path":"{cwd}","pattern":"[.][.]/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"[!a][!a]/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":".[.]/.ssh/id_ed25519"}"#,
    r#"{"path":"{cwd}","pattern":"[/]etc/passwd"}"#,
    r#"{"path":"{cwd}","pattern":"[+-0]etc/passwd"}"#,
    r#"{"path":"{cwd}","pattern":"a\\b"}"#,
    r#"{"path":"{cwd}","pattern":"{a,{b,c}"}"#,
    r#"{"path":"{cwd}","pattern":"**/../../etc/passwd"}"#,
    r#"{"path":"{cwd}","pattern":"[a"}"#,
];

#[tokio::test]
async fn patterns_we_cannot_bound_are_unknown_and_never_allowed() {
    let l = Layout::new("unknown");
    for raw in NOT_UNDERSTOOD {
        for preset in [Preset::Home, Preset::FullAccess] {
            let (d, subject) = verdict(&l, preset, "Glob", l.input("Glob", raw)).await;
            assert_eq!(subject, ToolSubject::Unknown, "{raw}");
            assert!(
                !matches!(d, GateDecision::Allow),
                "{preset:?} allowed {raw}"
            );
        }
    }
}

#[tokio::test]
async fn glob_escapes_are_never_allowed_and_credentials_are_hard_denied() {
    let l = Layout::new("glob");
    for (raw, credential) in ESCAPES {
        for (tool, input) in [
            ("Glob", l.input("Glob", raw)),
            // The same text as a Grep glob filter.
            ("Grep", {
                let mut v = l.input("Grep", raw);
                let pattern = v["pattern"].take();
                v["glob"] = pattern;
                v["pattern"] = json!("secret");
                v
            }),
        ] {
            let (home, subject) = verdict(&l, Preset::Home, tool, input.clone()).await;
            assert!(
                !matches!(home, GateDecision::Allow),
                "Home allowed {tool} {raw}: {subject:?}"
            );
            if *credential {
                assert!(
                    hard_denied(&home),
                    "Home did not hard-deny {tool} {raw}: {home:?} {subject:?}"
                );
                let (full, subject) = verdict(&l, Preset::FullAccess, tool, input).await;
                assert!(
                    hard_denied(&full),
                    "FullAccess did not hard-deny {tool} {raw}: {full:?} {subject:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn ordinary_patterns_keep_their_plain_subject_and_are_allowed_at_home() {
    let l = Layout::new("plain");
    let base = l.cwd.display().to_string();
    for pattern in [
        "**/*.rs",
        "src/**/mod.rs",
        "*.{ts,tsx}",
        "src/{a,b}/mod.rs",
        "*.[ch]",
    ] {
        let input = json!({"path": base, "pattern": pattern});
        let (d, subject) = verdict(&l, Preset::Home, "Glob", input).await;
        assert_eq!(
            subject,
            ToolSubject::Paths {
                paths: vec![base.clone()],
                glob: Some(pattern.into()),
                recursive: true
            },
            "{pattern}"
        );
        assert!(matches!(d, GateDecision::Allow), "{pattern}: {d:?}");
    }
    // The same without a `path` (the base is the cwd).
    let (d, subject) = verdict(&l, Preset::Home, "Glob", json!({"pattern": "**/*.rs"})).await;
    assert!(matches!(subject, ToolSubject::Paths { .. }));
    assert!(matches!(d, GateDecision::Allow), "{d:?}");
}

fn _unused(_: &Path) {}
