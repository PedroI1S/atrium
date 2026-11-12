//! M1-13: adversarial tests of remembered rules and the approval ledger.
//!
//! Rules are evaluated by `DefaultPolicy`; approvals run over the real
//! `SqliteStore` (the ledger's SQL rows), behind a `SessionGate` over a temp
//! folder with a fake HOME. Nothing is executed.

mod common_adversarial;

use std::sync::Arc;

use atrium_agents::{ApprovalTicket, GateDecision, ToolCheck, ToolGate};
use atrium_core::policy::{DefaultPolicy, Ledger, SessionGate, SessionGateConfig, StoreRules};
use atrium_core::store::{SqliteStore, StoreOptions};
use atrium_core::{
    ApprovalLedger, LedgerError, NewThread, PolicyVerdict, RequestKey, Signal, Store,
};
use atrium_protocol::agent::{ApprovalDecision, Provider, ToolKind};
use atrium_protocol::policy::{
    Preset, RememberScope, Rule, RuleDecision, RuleMatcher, RuleScope, ToolSubject,
};
use atrium_protocol::thread::{Location, ThreadId};
use common_adversarial::{
    Env, Tmp, classify, cmd_check, exact, paths_check, prefix, rule, run_table, Want,
};

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

fn allow_exact(argv: &[&str]) -> Rule {
    rule(
        RuleScope::Global,
        ToolKind::Execute,
        exact(argv),
        RuleDecision::Allow,
        "exact",
    )
}

/// An exact rule matches the exact argv and nothing longer, shorter, joined
/// to another command, decorated with an assignment, redirect or expansion.
#[test]
fn an_exact_rule_never_matches_a_longer_argv() {
    let env = Env::home();
    let rules = [allow_exact(&["rm", "-rf", "build"])];
    let same = [
        "rm -rf build",
        "rm  -rf   build",
        "rm -rf 'build'",
        "rm -rf \"build\"",
        "rm -rf bui\"\"ld",
        "rm -rf bu\\ild",
        "rm -rf build;",
        "ls; rm -rf build",
        "rm -rf build; ls",
        "rm -rf build | cat",
        "\\rm -rf build",
        "rm -rf build 2>&1",
        "rm -rf build >/dev/null",
    ];
    let not_same = [
        "rm -rf build ~/Documents",
        "rm -rf build x",
        "rm -rf build -v",
        "rm -rf -v build",
        "rm -rfv build",
        "rm -r -f build",
        "rm -rf build/",
        "rm -rf ./build",
        "rm -rf build ~",
        "rm -rf build; rm -rf x",
        "rm -rf build && rm -rf x",
        "rm -rf build || rm -rf x",
        "rm -rf build\nrm -rf x",
        "A=1 rm -rf build",
        "rm -rf build > out",
        "rm -rf build >> out",
        "rm -rf build < in",
        "rm -rf build &",
        "rm -rf build $X",
        "rm -rf build $(ls)",
        "rm -rf build `ls`",
        "rm -rf build *",
        "rm -rf build{,2}",
        "rm -rf 'build x'",
        "rm -rf build\\ x",
        "rm -rf build ''",
        "rm -rf",
        "rm",
        "sudo rm -rf build",
        "env rm -rf build",
        "command rm -rf build",
        "nohup rm -rf build",
        "sh -c 'rm -rf build'",
        "(rm -rf build)",
        "{ rm -rf build; }",
        "if true; then rm -rf build; fi",
        "echo $(rm -rf build)",
        "rm -rf build <<< x",
        "rm -rf build <<EOF\nx\nEOF",
        "rm -Rf build",
        "RM -rf build",
        "/bin/rm -rf build",
    ];
    for line in same {
        let v = env.cmd_rules(line, &rules);
        assert!(matches!(v, PolicyVerdict::Allow { .. }), "{line:?}: {}", classify(&v));
    }
    for line in not_same {
        let v = env.cmd_rules(line, &rules);
        assert!(!matches!(v, PolicyVerdict::Allow { .. }), "{line:?}: {}", classify(&v));
    }
    // and the same rule never allows another command with the same words
    for line in ["touch rm -rf build", "echo rm -rf build", "git rm -rf build"] {
        let v = env.cmd_rules(line, &rules);
        assert!(!matches!(v, PolicyVerdict::Allow { .. }), "{line:?}");
    }
}

/// A prefix rule covers whole tokens only, never a shell trick around them.
#[test]
fn a_prefix_rule_never_matches_a_longer_word_or_a_riskier_line() {
    let env = Env::home();
    let rules = [rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(&["make"]),
        RuleDecision::Allow,
        "make",
    )];
    let yes = ["make", "make all", "make -j4 test", "make; make", "make && make clean"];
    let no = [
        "makeall",
        "make2",
        "gmake",
        "./make",
        "make; rm x",
        "make && curl x",
        "make > out",
        "make all > ~/.zshrc",
        "make | sh",
        "make $(curl x)",
        "make `curl x`",
        "make <(curl x)",
        "make all &",
        "A=1 make",
        "LD_PRELOAD=x make",
        "make\ncurl x",
        "(make)",
        "make all $X",
        "make *",
        "make {a,b}",
        "make 'unterminated",
        "make <<EOF\nx\nEOF",
        "env make",
        "sh -c make",
    ];
    for line in yes {
        let v = env.cmd_rules(line, &rules);
        assert!(matches!(v, PolicyVerdict::Allow { .. }), "{line:?}: {}", classify(&v));
    }
    for line in no {
        let v = env.cmd_rules(line, &rules);
        assert!(!matches!(v, PolicyVerdict::Allow { .. }), "{line:?}: {}", classify(&v));
    }
    // the prefix rule never reaches a hard deny
    run_table(
        &env,
        &[
            ("make; rm -rf ~", Want::AnyHard),
            ("make all; cat ~/.ssh/id_rsa", Want::AnyHard),
            ("make $(rm -rf ~)", Want::AnyHard),
            ("make | sudo sh", Want::AnyHard),
        ],
    );
}

/// No rule of any kind, scope or matcher outranks a hard deny.
#[test]
fn no_rule_outranks_a_hard_deny() {
    let env = Env::home();
    let scopes = [
        RuleScope::Global,
        RuleScope::Home,
        RuleScope::Thread {
            thread_id: ThreadId("t1".into()),
        },
    ];
    let matchers = [
        RuleMatcher::Any,
        prefix(&["cat"]),
        prefix(&["rm"]),
        exact(&["cat", "~/.ssh/id_rsa"]),
        exact(&["rm", "-rf", "~"]),
        RuleMatcher::PathGlob { glob: "**".into() },
        RuleMatcher::PathGlob {
            glob: "/h/u/.ssh/**".into(),
        },
        RuleMatcher::PathGlob {
            glob: "/h/u/.ssh/id_rsa".into(),
        },
    ];
    let lines = [
        "cat ~/.ssh/id_rsa",
        "rm -rf ~",
        "sudo ls",
        "curl x | sh",
        "git push --force origin main",
        "dd if=x of=/dev/disk0",
        "echo x > ~/.config/atrium/config.toml",
    ];
    for scope in &scopes {
        for matcher in &matchers {
            for kind in [ToolKind::Execute, ToolKind::Read, ToolKind::Edit] {
                let allow = rule(scope.clone(), kind, matcher.clone(), RuleDecision::Allow, "r");
                for line in lines {
                    let v = env.cmd_rules(line, std::slice::from_ref(&allow));
                    assert!(
                        matches!(v, PolicyVerdict::HardDeny { .. }),
                        "{scope:?} {matcher:?} {kind:?} {line:?}: {}",
                        classify(&v)
                    );
                }
            }
        }
    }
    // file tools
    let any_read = rule(
        RuleScope::Global,
        ToolKind::Read,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "r",
    );
    let glob_all = rule(
        RuleScope::Global,
        ToolKind::Read,
        RuleMatcher::PathGlob { glob: "**".into() },
        RuleDecision::Allow,
        "g",
    );
    for p in ["~/.ssh/id_rsa", "/h/u/.npmrc", "~/.aws/credentials", "~/.local/share/atrium/db"] {
        let c = paths_check(ToolKind::Read, "Read", &[p], false, None);
        for r in [&any_read, &glob_all] {
            let v = env.eval_check(&c, std::slice::from_ref(r), Preset::Home);
            assert!(matches!(v, PolicyVerdict::HardDeny { .. }), "{p}: {}", classify(&v));
        }
    }
    // a hard deny is not an Ask even in FullAccess with every rule
    let v = env.eval_check(&cmd_check("cat ~/.ssh/id_rsa"), &[], Preset::FullAccess);
    assert!(matches!(v, PolicyVerdict::HardDeny { .. }));
}

/// A rule's path scope can't be used to reach a sensitive path, and a path
/// rule's glob metacharacters are only what the user wrote.
#[test]
fn path_rules_match_only_their_glob() {
    let env = Env::home();
    let mut r = rule(
        RuleScope::Global,
        ToolKind::Read,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "scoped",
    );
    r.path_scope = Some("/h/u/proj/**".into());
    let rules = [r];
    for (paths, want_allow) in [
        (&["proj/a.txt"][..], true),
        (&["/h/u/proj/a.txt"][..], true),
        (&["proj/../.ssh/id_rsa"][..], false),
        (&["proj/../other/a.txt"][..], false),
        (&["proj/a.txt", "other/b.txt"][..], false),
        (&["other/b.txt", "proj/a.txt"][..], false),
        (&["proj/a.txt", "~/.ssh/id_rsa"][..], false),
        (&["/h/u/projects/a.txt"][..], false),
        (&["/h/u/proj"][..], false),
        (&[][..], false),
    ] {
        let c = paths_check(ToolKind::Read, "Read", paths, false, None);
        let v = env.eval_check(&c, &rules, Preset::Home);
        // `Allow` can also come from Home's own list for reads in the folder;
        // the rule's scope is what is under test, so compare to a no-rule run.
        let base = env.eval_check(&c, &[], Preset::Home);
        if want_allow {
            assert!(matches!(v, PolicyVerdict::Allow { .. }), "{paths:?}: {}", classify(&v));
        } else {
            assert_eq!(classify(&v), classify(&base), "{paths:?}: the rule changed the verdict");
        }
    }
}

// ---------------------------------------------------------------------------
// The ledger over the real store
// ---------------------------------------------------------------------------

struct Sql {
    _dir: tempfile::TempDir,
    store: Arc<SqliteStore>,
    ledger: Arc<Ledger>,
    tmp: Tmp,
}

impl Sql {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(
            SqliteStore::open(&dir.path().join("atrium.db"), StoreOptions::default())
                .await
                .unwrap(),
        );
        let ledger = Arc::new(Ledger::new(store.requests()));
        Self {
            _dir: dir,
            store,
            ledger,
            tmp: Tmp::new(),
        }
    }

    /// A new thread with its first incarnation started.
    async fn thread(&self) -> (ThreadId, u32) {
        let s = self
            .store
            .create_thread(NewThread {
                provider: Provider::Claude,
                model: None,
                effort: None,
                preset: Preset::Home,
                location: Location::Home,
                title: "t".into(),
            })
            .await
            .unwrap();
        let inc = self.store.begin_incarnation(&s.thread_id).await.unwrap();
        self.store.signal(&s.thread_id, Signal::Starting).await.unwrap();
        (s.thread_id, inc)
    }

    async fn next_incarnation(&self, t: &ThreadId) -> u32 {
        self.store.begin_incarnation(t).await.unwrap()
    }

    fn gate(&self, thread: &ThreadId, incarnation: u32) -> Arc<SessionGate> {
        let home = self.tmp.home.clone();
        Arc::new(SessionGate::new(
            SessionGateConfig {
                thread: thread.clone(),
                incarnation,
                cwd: home.clone(),
                home: home.clone(),
                atrium_data_paths: vec![home.join(".local/share/atrium")],
                atrium_protected_paths: vec![home.join(".config/atrium")],
                preset: Preset::Home,
                provider: Provider::Claude,
            },
            Arc::new(DefaultPolicy::new()),
            self.ledger.clone(),
            Arc::new(StoreRules(self.store.clone())),
        ))
    }
}

fn key(thread: &ThreadId, incarnation: u32, t: &ApprovalTicket) -> RequestKey {
    RequestKey {
        thread: thread.clone(),
        incarnation,
        request_id: t.request_id.clone(),
    }
}

async fn ask(gate: &SessionGate, c: &ToolCheck) -> ApprovalTicket {
    match gate.check(c).await {
        GateDecision::Ask { ticket } => ticket,
        other => panic!("expected Ask, got {other:?}"),
    }
}

fn is_deny(d: &GateDecision) -> bool {
    matches!(d, GateDecision::Deny { .. })
}

#[tokio::test]
async fn an_approval_is_executed_once_and_only_for_its_call() {
    let sql = Sql::new().await;
    let (t, inc) = sql.thread().await;
    let g = sql.gate(&t, inc);
    let c = cmd_check("touch notes.txt");
    let ticket = ask(&g, &c).await;
    let k = key(&t, inc, &ticket);

    // not approved yet: a redeem is refused
    assert!(is_deny(&g.redeem(&ticket, &c).await));
    sql.ledger.resolve(&k, &ApprovalDecision::Allow, None).await.unwrap();

    // another call, a longer call, another turn, another tool name: refused,
    // and none of them burns the approval
    let mut other_turn = c.clone();
    other_turn.turn = 2;
    let mut other_name = c.clone();
    other_name.name = "Run".into();
    let mut other_kind = c.clone();
    other_kind.kind = ToolKind::Edit;
    let longer = cmd_check("touch notes.txt other.txt");
    for wrong in [&other_turn, &other_name, &other_kind, &longer] {
        assert!(is_deny(&g.redeem(&ticket, wrong).await), "{:?}", wrong.input);
    }
    // the right call: once
    assert_eq!(g.redeem(&ticket, &c).await, GateDecision::Allow);
    // replay: never again, whatever the call
    for _ in 0..3 {
        assert!(is_deny(&g.redeem(&ticket, &c).await));
    }
    assert_eq!(sql.ledger.redeem(&k, &c).await, Err(LedgerError::AlreadyResolved));

    // an approved ticket redeemed with a call that is now a hard deny is
    // refused and withdrawn: the approval can't be used for anything after
    let ticket = ask(&g, &c).await;
    sql.ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    for hard in ["touch ~/.ssh/authorized_keys", "rm -rf ~"] {
        assert!(is_deny(&g.redeem(&ticket, &cmd_check(hard)).await), "{hard}");
    }
    assert!(is_deny(&g.redeem(&ticket, &c).await));
}

#[tokio::test]
async fn a_denied_cancelled_or_expired_approval_never_runs() {
    let sql = Sql::new().await;
    let (t, inc) = sql.thread().await;
    let g = sql.gate(&t, inc);
    // denied
    let c = cmd_check("touch a");
    let ticket = ask(&g, &c).await;
    sql.ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Deny { message: None }, None)
        .await
        .unwrap();
    assert!(is_deny(&g.redeem(&ticket, &c).await));
    // allowing a denied one is refused, and still can't run
    assert!(sql
        .ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, None)
        .await
        .is_err());
    assert!(is_deny(&g.redeem(&ticket, &c).await));
    // withdrawn by the provider after the user allowed it
    let c = cmd_check("touch b");
    let ticket = ask(&g, &c).await;
    sql.ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    sql.ledger.withdraw(&key(&t, inc, &ticket)).await.unwrap();
    assert!(is_deny(&g.redeem(&ticket, &c).await));
    // the turn ended
    let c = cmd_check("touch c");
    let ticket = ask(&g, &c).await;
    sql.ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    sql.ledger.expire_turn(&t, inc, 1).await;
    assert!(is_deny(&g.redeem(&ticket, &c).await));
    assert!(sql
        .ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, None)
        .await
        .is_err());
    // the session ended
    let c = cmd_check("touch d");
    let ticket = ask(&g, &c).await;
    sql.ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    sql.ledger.expire_incarnation(&t, inc).await;
    assert!(is_deny(&g.redeem(&ticket, &c).await));
    // the daemon restarted
    let (t2, inc2) = sql.thread().await;
    let g2 = sql.gate(&t2, inc2);
    let c = cmd_check("touch e");
    let ticket = ask(&g2, &c).await;
    sql.ledger
        .resolve(&key(&t2, inc2, &ticket), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    sql.ledger.expire_all().await;
    assert!(is_deny(&g2.redeem(&ticket, &c).await));
}

#[tokio::test]
async fn approvals_do_not_cross_threads_or_incarnations() {
    let sql = Sql::new().await;
    let (a, inc_a) = sql.thread().await;
    let (b, inc_b) = sql.thread().await;
    let ga = sql.gate(&a, inc_a);
    let gb = sql.gate(&b, inc_b);
    let c = cmd_check("touch notes.txt");
    let ticket = ask(&ga, &c).await;
    // thread B's window resolves A's request id under B's key, or under A's
    // key with the wrong incarnation: refused
    for wrong in [
        RequestKey {
            thread: b.clone(),
            incarnation: inc_b,
            request_id: ticket.request_id.clone(),
        },
        RequestKey {
            thread: b.clone(),
            incarnation: inc_a,
            request_id: ticket.request_id.clone(),
        },
        RequestKey {
            thread: a.clone(),
            incarnation: inc_a + 1,
            request_id: ticket.request_id.clone(),
        },
        RequestKey {
            thread: a.clone(),
            incarnation: 0,
            request_id: ticket.request_id.clone(),
        },
        RequestKey {
            thread: ThreadId("nope".into()),
            incarnation: 1,
            request_id: ticket.request_id.clone(),
        },
        RequestKey {
            thread: a.clone(),
            incarnation: inc_a,
            request_id: format!("{}x", ticket.request_id),
        },
        RequestKey {
            thread: a.clone(),
            incarnation: inc_a,
            request_id: String::new(),
        },
    ] {
        assert!(
            sql.ledger.resolve(&wrong, &ApprovalDecision::Allow, None).await.is_err(),
            "{wrong:?}"
        );
    }
    // still pending: A's real key works exactly once
    sql.ledger
        .resolve(&key(&a, inc_a, &ticket), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    // B's gate never honours A's ticket, with the same call
    assert!(is_deny(&gb.redeem(&ticket, &c).await));
    // and a ledger redeem under B's key finds nothing
    let under_b = RequestKey {
        thread: b.clone(),
        incarnation: inc_b,
        request_id: ticket.request_id.clone(),
    };
    assert_eq!(sql.ledger.redeem(&under_b, &c).await, Err(LedgerError::Unknown));
    // a new incarnation of A: the old approval is stale for the new gate, and
    // the old gate's ticket no longer redeems either
    let inc_a2 = sql.next_incarnation(&a).await;
    assert!(inc_a2 > inc_a);
    let ga2 = sql.gate(&a, inc_a2);
    assert!(is_deny(&ga2.redeem(&ticket, &c).await));
    let stale = sql
        .ledger
        .resolve(&key(&a, inc_a, &ticket), &ApprovalDecision::Allow, None)
        .await;
    assert!(stale.is_err(), "{stale:?}");
    // the redeem by the old gate (the old session is gone): the ledger
    // refuses an approval from an older incarnation
    sql.ledger.expire_incarnation(&a, inc_a).await;
    assert!(is_deny(&ga.redeem(&ticket, &c).await));
}

#[tokio::test]
async fn an_unknown_or_foreign_ticket_is_never_redeemed() {
    let sql = Sql::new().await;
    let (t, inc) = sql.thread().await;
    let g = sql.gate(&t, inc);
    let c = cmd_check("touch a");
    // a ticket that was never opened
    let forged = ApprovalTicket {
        request_id: "req_0_0".into(),
        rememberable: false,
    };
    assert!(is_deny(&g.redeem(&forged, &c).await));
    // a ticket opened by another gate over the same store (same thread): this
    // gate never opened it
    let other = sql.gate(&t, inc);
    let ticket = ask(&other, &c).await;
    sql.ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, None)
        .await
        .unwrap();
    assert!(is_deny(&g.redeem(&ticket, &c).await));
    assert_eq!(other.redeem(&ticket, &c).await, GateDecision::Allow);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_redeems_of_one_ticket_redeem_once() {
    for round in 0..20 {
        let sql = Sql::new().await;
        let (t, inc) = sql.thread().await;
        let g = sql.gate(&t, inc);
        let c = cmd_check("touch notes.txt");
        let ticket = ask(&g, &c).await;
        sql.ledger
            .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, None)
            .await
            .unwrap();
        let mut tasks = vec![];
        for _ in 0..8 {
            let (g, ticket, c) = (g.clone(), ticket.clone(), c.clone());
            tasks.push(tokio::spawn(async move { g.redeem(&ticket, &c).await }));
        }
        // plus ledger-level redeems racing the gate's
        for _ in 0..4 {
            let (l, k, c) = (sql.ledger.clone(), key(&t, inc, &ticket), c.clone());
            tasks.push(tokio::spawn(async move {
                match l.redeem(&k, &c).await {
                    Ok(()) => GateDecision::Allow,
                    Err(e) => GateDecision::Deny {
                        reason: e.to_string(),
                    },
                }
            }));
        }
        let mut allowed = 0;
        for task in tasks {
            if task.await.unwrap() == GateDecision::Allow {
                allowed += 1;
            }
        }
        assert_eq!(allowed, 1, "round {round}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_allow_and_deny_leave_one_outcome() {
    for round in 0..20 {
        let sql = Sql::new().await;
        let (t, inc) = sql.thread().await;
        let g = sql.gate(&t, inc);
        let c = cmd_check("touch notes.txt");
        let ticket = ask(&g, &c).await;
        let k = key(&t, inc, &ticket);
        let (l1, k1) = (sql.ledger.clone(), k.clone());
        let (l2, k2) = (sql.ledger.clone(), k.clone());
        let allow = tokio::spawn(async move {
            l1.resolve(&k1, &ApprovalDecision::Allow, None).await.is_ok()
        });
        let deny = tokio::spawn(async move {
            l2.resolve(&k2, &ApprovalDecision::Deny { message: None }, None)
                .await
                .is_ok()
        });
        let (allow_won, deny_won) = (allow.await.unwrap(), deny.await.unwrap());
        assert!(allow_won ^ deny_won, "round {round}: exactly one resolves");
        let ran = g.redeem(&ticket, &c).await == GateDecision::Allow;
        assert_eq!(ran, allow_won, "round {round}: it runs only if the allow won");
    }
}

/// "Always…" creates an exact rule; it is used for the same command and for
/// nothing longer, and never reaches a hard deny.
#[tokio::test]
async fn a_remembered_command_allows_only_itself() {
    let sql = Sql::new().await;
    let (t, inc) = sql.thread().await;
    let g = sql.gate(&t, inc);
    let c = cmd_check("touch notes.txt");
    let ticket = ask(&g, &c).await;
    let id = sql
        .ledger
        .resolve(&key(&t, inc, &ticket), &ApprovalDecision::Allow, Some(RememberScope::Global))
        .await
        .unwrap();
    assert!(id.is_some());
    assert_eq!(g.redeem(&ticket, &c).await, GateDecision::Allow);
    // the next identical call is allowed without asking
    assert_eq!(g.check(&c).await, GateDecision::Allow);
    // a longer one, one with more after it, another one: asked, or blocked
    for line in [
        "touch notes.txt other.txt",
        "touch notes.txt; touch other",
        "touch notes.txt && rm -rf x",
        "touch notes.txt > out",
        "touch notes.txt $X",
        "touch other.txt",
        "touch ~/.ssh/authorized_keys",
        "touch notes.txt ~/.ssh/authorized_keys",
        "touch notes.txt; rm -rf ~",
    ] {
        let d = g.check(&cmd_check(line)).await;
        assert!(
            !matches!(d, GateDecision::Allow),
            "{line:?}: {d:?}"
        );
    }
    // another thread has the global rule too (it asked for Global), and the
    // same limits
    let (t2, inc2) = sql.thread().await;
    let g2 = sql.gate(&t2, inc2);
    assert_eq!(g2.check(&c).await, GateDecision::Allow);
    assert!(!matches!(
        g2.check(&cmd_check("touch notes.txt other.txt")).await,
        GateDecision::Allow
    ));
}

/// A remembered path with glob characters in its name allows that file only.
#[tokio::test]
async fn a_remembered_path_with_glob_characters_allows_only_that_file() {
    let sql = Sql::new().await;
    sql.tmp.write("proj/a[1].txt", "x");
    sql.tmp.write("proj/a1.txt", "x");
    sql.tmp.write("proj/b{1,2}.txt", "x");
    sql.tmp.write("proj/b1.txt", "x");
    sql.tmp.write("proj/c*.txt", "x");
    sql.tmp.write("proj/cdd.txt", "x");
    let (t, inc) = sql.thread().await;
    let g = sql.gate(&t, inc);
    for (odd, plain) in [
        ("proj/a[1].txt", "proj/a1.txt"),
        ("proj/b{1,2}.txt", "proj/b1.txt"),
        ("proj/c*.txt", "proj/cdd.txt"),
    ] {
        let c = paths_check(ToolKind::Edit, "Edit", &[odd], false, None);
        let ticket = ask(&g, &c).await;
        sql.ledger
            .resolve(
                &key(&t, inc, &ticket),
                &ApprovalDecision::Allow,
                Some(RememberScope::Thread),
            )
            .await
            .unwrap();
        assert_eq!(g.redeem(&ticket, &c).await, GateDecision::Allow, "{odd}");
        assert_eq!(g.check(&c).await, GateDecision::Allow, "{odd}");
        let other = paths_check(ToolKind::Edit, "Edit", &[plain], false, None);
        assert!(
            matches!(g.check(&other).await, GateDecision::Ask { .. }),
            "{odd} must not allow {plain}"
        );
    }
}

/// The hard deny list holds through the gate with stored rules of every
/// kind, and a rule made by "always…" doesn't change that.
#[tokio::test]
async fn stored_rules_never_allow_a_hard_deny_through_the_gate() {
    let sql = Sql::new().await;
    sql.tmp.write(".ssh/id_rsa", "secret");
    sql.tmp.write(".npmrc", "token");
    let (t, inc) = sql.thread().await;
    let mut rule_n = 0;
    for (matcher, kind, scope) in [
        (RuleMatcher::Any, ToolKind::Execute, RuleScope::Global),
        (RuleMatcher::Any, ToolKind::Read, RuleScope::Global),
        (RuleMatcher::Any, ToolKind::Execute, RuleScope::Home),
        (
            RuleMatcher::Any,
            ToolKind::Execute,
            RuleScope::Thread {
                thread_id: t.clone(),
            },
        ),
        (prefix(&["cat"]), ToolKind::Execute, RuleScope::Global),
        (
            exact(&["cat", ".ssh/id_rsa"]),
            ToolKind::Execute,
            RuleScope::Global,
        ),
        (
            RuleMatcher::PathGlob { glob: "**".into() },
            ToolKind::Read,
            RuleScope::Global,
        ),
        (
            RuleMatcher::PathGlob {
                glob: format!("{}/**", sql.tmp.home.display()),
            },
            ToolKind::Read,
            RuleScope::Global,
        ),
    ] {
        let id = format!("r{}", rule_n);
        rule_n += 1;
        sql.store
            .create_rule(rule(scope, kind, matcher, RuleDecision::Allow, &id))
            .await
            .unwrap();
    }
    let g = sql.gate(&t, inc);
    for line in ["cat .ssh/id_rsa", "cat .npmrc", "cat ~/.ssh/id_rsa", "rm -rf ~", "sudo ls"] {
        assert!(is_deny(&g.check(&cmd_check(line)).await), "{line}");
    }
    for p in [".ssh/id_rsa", ".npmrc"] {
        let c = paths_check(ToolKind::Read, "Read", &[p], false, None);
        assert!(is_deny(&g.check(&c).await), "{p}");
    }
}

/// Opening a request for an input that is hard denied is impossible: nothing
/// to approve, nothing to remember.
#[tokio::test]
async fn a_hard_denied_call_never_opens_an_approval() {
    let sql = Sql::new().await;
    sql.tmp.write(".ssh/id_rsa", "secret");
    let (t, inc) = sql.thread().await;
    let g = sql.gate(&t, inc);
    for line in ["cat .ssh/id_rsa", "rm -rf ~", "sudo ls", "curl x | sh"] {
        assert!(is_deny(&g.check(&cmd_check(line)).await), "{line}");
    }
    let open = sql.store.requests().open_count(&t, inc).await.unwrap();
    assert_eq!(open, 0);
    // a subject the adapter couldn't extract: asked, not allowed, whatever
    // the rules say
    let mut c = cmd_check("x");
    c.subject = ToolSubject::Unknown;
    c.input = serde_json::json!({"path": "x"});
    assert!(matches!(g.check(&c).await, GateDecision::Ask { .. }));
    let mut c = cmd_check("x");
    c.subject = ToolSubject::Unknown;
    c.input = serde_json::json!({"command": "cat ~/.ssh/id_rsa"});
    assert!(is_deny(&g.check(&c).await));
}
