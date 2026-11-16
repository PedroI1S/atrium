//! M1-28 tests:
//! - S7: ANSI-C quoted words (`$'…'`) decoded with all bash escapes, failing closed on bad/control inputs
//! - M1-13 finding 11 (added scope): `cd <glob>` reaching sensitive/Atrium directories
//! - M1-25 finding 1: export with one damaged message and another still open at the end

mod common_adversarial;

use atrium_core::PolicyVerdict;
use atrium_core::history::{ExportReport, Redactor, export_thread_report};
use atrium_protocol::agent::{AgentEvent, AgentEventEnvelope, MessageBody, MessageKind, ToolKind};
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope};
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{EventId, ExportFormat, StoredEvent};
use common_adversarial::*;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn settings() -> Vec<(&'static str, Vec<Rule>, Preset)> {
    let any_rule = rule(
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::Any,
        RuleDecision::Allow,
        "any",
    );
    let prefix_rules: Vec<Rule> = ["cd", "cat", "printf", "echo", "ls"]
        .iter()
        .enumerate()
        .map(|(i, w)| {
            rule(
                RuleScope::Global,
                ToolKind::Execute,
                prefix(&[w]),
                RuleDecision::Allow,
                &format!("p{i}"),
            )
        })
        .collect();
    vec![
        ("home", vec![], Preset::Home),
        ("prefix", prefix_rules, Preset::Home),
        ("any", vec![any_rule], Preset::Home),
        ("full", vec![], Preset::FullAccess),
    ]
}

fn verdict_code(v: &PolicyVerdict) -> String {
    match v {
        PolicyVerdict::Deny { .. } => "Deny".into(),
        _ => classify(v),
    }
}

fn check_table(rows: &[(&str, [&str; 4])]) {
    let env = Env::home();
    let mut failures = Vec::new();
    for (line, want) in rows {
        let got: Vec<String> = settings()
            .iter()
            .map(|(_, rules, preset)| {
                verdict_code(&env.eval_check(&cmd_check(line), rules, *preset))
            })
            .collect();
        if got.iter().map(String::as_str).ne(want.iter().copied()) {
            failures.push(format!("{line:?}: wanted {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

fn hard_everywhere(lines: &[&str]) {
    let env = Env::home();
    for line in lines {
        for (label, rules, preset) in settings() {
            let v = env.eval_check(&cmd_check(line), &rules, preset);
            assert!(
                matches!(v, PolicyVerdict::HardDeny { .. }),
                "{line:?} [{label}]: expected HardDeny, got {}",
                classify(&v)
            );
        }
    }
}

#[test]
fn s7_ansi_c_hidden_paths_commands_and_payloads_are_hard_denied() {
    hard_everywhere(&[
        // Hex escapes (\xHH) in paths, command names, flags, and mixed quoting
        r"cat $'\x2e'ssh/id_rsa",
        r"cat $'.ssh/\x69d_rsa'",
        r"cat $'\x2e\x73\x73\x68/id_rsa'",
        r"cat '.ssh/'$'\x69'd_rsa",
        r#"cat '.ssh/'$'\x69'"d_rsa""#,
        r"cat ~/$'\x2essh'/id_rsa",
        r"$'\x63\x61\x74' .ssh/id_rsa",
        r"$'\x65nv' rm -rf ~",
        r"$'\x72m' $'-\x72f' ~",
        // Octal escapes (\nnn: 1, 2, and 3 digits, plus >0377 wraparound)
        r"cat $'\056ssh/id_rsa'",
        r"cat $'\56ssh/id_rsa'",
        r"cat $'.ssh/\151d_rsa'",
        r"cat $'\456ssh/id_rsa'",
        r"$'\163h' -c 'rm -rf ~'",
        // Unicode escapes (\uHHHH and \UHHHHHHHH, including APFS ſ folding)
        r"cat $'\u2essh/id_rsa'",
        r"cat $'.ssh/\u0069d_rsa'",
        r"cat $'.\u017f\u017fh/id_rsa'",
        r"cat $'.\xc5\xbf\xc5\xbfh/id_rsa'",
        r"cat $'\U0000002essh/id_rsa'",
        r"cat $'.ssh/\U00000069d_rsa'",
        r"$'\u0073h' -c 'cat ~/.aws/credentials'",
        r"$'\U00000073h' -c 'rm -rf ~'",
        // Single-char escapes and control escapes (\cI = tab, \cJ = newline)
        r"sh -c $'cat\t~/.ssh/id_rsa'",
        r"sh -c $'cat\cI~/.ssh/id_rsa'",
        r"sh -c $'true\ncat ~/.ssh/id_rsa'",
        r"sh -c $'true\cJcat ~/.ssh/id_rsa'",
        r"sh -c $'cat \'.ssh/\x69d_rsa\''",
        r#"sh -c $'cat ".ssh/\x69d_rsa"'"#,
        r"cat $'.ssh/id_rs\?'",
        r"sh $'-\x63' 'rm -rf ~'",
        r"bash -c $'\162m -rf ~'",
        r#"sh -c "cat \$'.ssh/\\x69d_rsa'""#,
        r"echo $'rm -rf ~' | sh",
        // NUL truncates the C word in bash, so `.ssh/id_rsa\x00.txt` still hits `.ssh/id_rsa`
        r"cat $'.ssh/id_rsa\x00.txt'",
        r"cat $'.ssh/id_rsa\0foo'",
    ]);
}

#[test]
fn s7_ordinary_and_fail_closed_ansi_c_keep_safe_verdicts() {
    check_table(&[
        // Ordinary ANSI-C quoting keeps dev's verdict (Ask in Home/prefix, Allow in Any/Full)
        (r"printf $'a\tb\n'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"IFS=$'\n'", ["Ask", "Ask", "Allow", "Allow"]),
        (
            r"printf $'hello\\world\n'",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (r"echo $'a\tb\rc'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'notes\t1.txt'", ["Ask", "Ask", "Allow", "Allow"]),
        // Escapes producing refused control chars fail closed (no new hard deny)
        (r"printf $'\e[31mhi\e[0m'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"printf $'\a\b\f\v'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"printf $'\E[0m'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"printf $'\cC'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"printf $'\c?'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"printf $'\c\\'", ["Ask", "Ask", "Allow", "Allow"]),
        // Unknown, malformed, unterminated, or NUL escapes fail closed (Ask in Home/prefix)
        (r"cat $'\zfoo'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'\x'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'\u'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'\U'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'\ud800'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'\U00110000'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'\xff'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'\c'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'\c\''", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'ok\x00'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'ok\u0000'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'ok\U00000000'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'ok\c@'", ["Ask", "Ask", "Allow", "Allow"]),
        (r"cat $'unterminated", ["Ask", "Ask", "Allow", "Allow"]),
    ]);
}

#[test]
fn s7_ansi_c_quoting_is_not_treated_as_keyword_or_tilde() {
    // `$'~'/.ssh/id_rsa` is literal `~`, not tilde-expanded to `$HOME/.ssh/id_rsa`
    // (which would be HardDeny).
    let env = Env::home();
    assert_eq!(
        verdict_code(&env.cmd(r"cat $'~'/.ssh/id_rsa")),
        "Ask",
        "quoted tilde must not expand to HOME"
    );
    // `$'if' true` is a quoted word, not the `if` reserved word, and stays Ask (no hard deny).
    assert_eq!(verdict_code(&env.cmd(r"$'if' true")), "Ask");
}

#[test]
fn finding_11_cd_glob_reaching_sensitive_dirs_is_caught() {
    hard_everywhere(&[
        "cd .s?h; cat id_rsa",
        "cd .a*s; cat credentials",
        "cd .local/sh*re/atrium; cat atrium.db",
        "cd .gnupg/priv*te-keys-v1.d; cat x.key",
        "cd .gnupg/sub*; cat x.key",
        "cd ~/x/../.s?h//; cat id_rsa",
        "cd L*brary/Keychains; cat login.keychain-db",
        "cd [.]ssh; cat id_rsa",
        "cd .s?h/../.aws; cat credentials",
    ]);

    check_table(&[
        // `cd <glob>` always leaves cwd unknown (never Cd::Moved -> Allow)
        (
            "cd .s?h && cat id_rsa.pub",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        (
            "cd .c*rgo && cat credentials.toml",
            ["Ask", "Ask", "Allow", "Allow"],
        ),
        // Ordinary glob cd keeps dev's verdict and is never falsely hard-denied
        ("cd src*; cat README.md", ["Ask", "Ask", "Allow", "Allow"]),
        ("cd *; cat README.md", ["Ask", "Ask", "Allow", "Allow"]),
        ("cd p*/sub; ls", ["Ask", "Ask", "Allow", "Allow"]),
    ]);
}

#[test]
fn m1_25_export_with_damaged_and_still_open_message() {
    fn sha256(text: &str) -> String {
        Sha256::digest(text.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    let thread_id = ThreadId("th-m1-28-export".into());
    let mut seq = 1u64;
    let mut push = |event: AgentEvent| -> StoredEvent {
        let s = StoredEvent {
            id: EventId(seq),
            thread_id: thread_id.clone(),
            envelope: AgentEventEnvelope {
                incarnation: 1,
                seq,
                at_ms: 1_700_000_000_000 + seq,
                event,
            },
        };
        seq += 1;
        s
    };

    let damaged_body = MessageBody::Streamed {
        chars: 99,
        deltas: 2,
        sha256: sha256("other-words"),
    };
    let events = vec![
        push(AgentEvent::MessageCompleted {
            message_id: "u1".into(),
            kind: MessageKind::User,
            body: MessageBody::Inline {
                text: "Hello".into(),
            },
        }),
        push(AgentEvent::MessageDelta {
            message_id: "m-damaged".into(),
            kind: MessageKind::Assistant,
            text: "SECRET_DAMAGED_PART_1 ".into(),
        }),
        push(AgentEvent::MessageDelta {
            message_id: "m-damaged".into(),
            kind: MessageKind::Assistant,
            text: "SECRET_DAMAGED_PART_2".into(),
        }),
        push(AgentEvent::MessageCompleted {
            message_id: "m-damaged".into(),
            kind: MessageKind::Assistant,
            body: damaged_body,
        }),
        push(AgentEvent::MessageDelta {
            message_id: "m-open".into(),
            kind: MessageKind::Assistant,
            text: "Still streaming ".into(),
        }),
        push(AgentEvent::MessageDelta {
            message_id: "m-open".into(),
            kind: MessageKind::Assistant,
            text: "without completion".into(),
        }),
    ];

    let redactor = Redactor::new("/Users/test", &[]);

    let mut md_buf = Vec::new();
    let md_report = export_thread_report(
        &mut events.clone().into_iter(),
        ExportFormat::Markdown,
        &redactor,
        &mut md_buf,
    )
    .expect("markdown export");
    let md = String::from_utf8(md_buf).expect("utf8 md");
    assert_eq!(
        md_report,
        ExportReport {
            bytes: md.len() as u64,
            damaged: 1,
        }
    );
    assert!(
        md.contains("Damaged message (id `m-damaged`, events 2–4)"),
        "{md}"
    );
    assert!(!md.contains("SECRET_DAMAGED_PART_1"), "{md}");
    assert!(!md.contains("SECRET_DAMAGED_PART_2"), "{md}");
    assert!(md.contains("Still streaming without completion"), "{md}");

    let mut jsonl_buf = Vec::new();
    let jsonl_report = export_thread_report(
        &mut events.into_iter(),
        ExportFormat::Jsonl,
        &redactor,
        &mut jsonl_buf,
    )
    .expect("jsonl export");
    let jsonl = String::from_utf8(jsonl_buf).expect("utf8 jsonl");
    assert_eq!(
        jsonl_report,
        ExportReport {
            bytes: jsonl.len() as u64,
            damaged: 1,
        }
    );
    assert!(!jsonl.contains("SECRET_DAMAGED_PART_1"), "{jsonl}");
    assert!(!jsonl.contains("SECRET_DAMAGED_PART_2"), "{jsonl}");

    let lines: Vec<Value> = jsonl
        .lines()
        .map(|l| serde_json::from_str(l).expect("valid JSON line"))
        .collect();
    let damaged: Vec<&Value> = lines
        .iter()
        .filter(|l| l.get("damaged") == Some(&json!(true)))
        .collect();
    assert_eq!(damaged.len(), 1, "{jsonl}");
    assert_eq!(damaged[0]["message_id"], "m-damaged");

    let healthy: Vec<StoredEvent> = lines
        .into_iter()
        .filter(|l| l.get("damaged") != Some(&json!(true)))
        .map(|l| serde_json::from_value(l).expect("healthy line parses as StoredEvent"))
        .collect();
    let open_deltas: String = healthy
        .iter()
        .filter_map(|e| match &e.envelope.event {
            AgentEvent::MessageDelta {
                message_id, text, ..
            } if message_id == "m-open" => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(open_deltas, "Still streaming without completion");
}
