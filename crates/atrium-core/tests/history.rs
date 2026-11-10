//! M1-11: export and reconstruction (contract §13).

use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

use atrium_core::history::{
    HistoryError, Redactor, create_export_file, export_thread, reconstruct_message,
};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, MessageBody, MessageKind, PlanEntry, PlanStatus, Provider,
    Question, QuestionOption, RequestOutcome, ToolKind, ToolStatus, TurnOutcome, Usage,
};
use atrium_protocol::policy::{
    ClassCoverage, Coverage, EnforcementDescriptor, EnforcementReason, ReasonCode, ToolClass,
};
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{EventId, ExportFormat, StoredEvent};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const HOME: &str = "/Users/pedro";

const ANTHROPIC: &str = "sk-ant-api03-AbCdEf0123456789_-xyzXYZ";
const OPENAI: &str = "sk-proj-abcdefghijklmnopqrstuvwx1234";
const GITHUB: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";
const GITHUB_PAT: &str = "github_pat_11ABCDEFG0abcdefghijkl_mnopqrstuvwxyz0123456789";
const GITLAB: &str = "glpat-abcdefghij0123456789";
const NPM: &str = "npm_abcdefghijklmnopqrstuvwxyz0123456789";
const AWS: &str = "AKIAIOSFODNN7EXAMPLE";
const GOOGLE: &str = "AIzaSyA-0123456789abcdefghijklmnopqrstuv";
const GOOGLE_OAUTH: &str = "ya29.a0AfH6SMBxabcdefghij-klmnop";
const SLACK: &str = "xoxb-123456789012-abcdefghijkl";
const STRIPE: &str = "sk_live_abcdefghijklmnop0123";
const JWT: &str =
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
const CANARY: &str = "CANARY-12345-user-secret";

const SHAPED: &[&str] = &[
    ANTHROPIC,
    OPENAI,
    GITHUB,
    GITHUB_PAT,
    GITLAB,
    NPM,
    AWS,
    GOOGLE,
    GOOGLE_OAUTH,
    SLACK,
    STRIPE,
    JWT,
];

fn redactor() -> Redactor {
    Redactor::new(HOME, &[CANARY])
}

fn redact(text: &str) -> String {
    redactor().text(text)
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn streamed(parts: &[&str]) -> MessageBody {
    let text = parts.concat();
    MessageBody::Streamed {
        chars: text.chars().count() as u64,
        deltas: parts.len() as u64,
        sha256: sha256(&text),
    }
}

/// Full coverage unless `reasons` are given.
fn enforcement(reasons: Vec<EnforcementReason>) -> EnforcementDescriptor {
    let coverage = ToolClass::ALL
        .into_iter()
        .map(|class| ClassCoverage {
            class,
            coverage: Coverage::Intercepted,
            evidence: None,
        })
        .collect();
    EnforcementDescriptor::evaluate(
        Provider::Claude,
        Some("2.1.0".into()),
        true,
        coverage,
        Vec::new(),
        reasons,
    )
}

/// Builds a thread's stored events. Ids count from 1; an event's `seq` is its
/// id and its time is one second after the previous event's.
struct Thread {
    events: Vec<StoredEvent>,
    incarnation: u32,
}

const T0_MS: u64 = 1_762_696_201_000;

impl Thread {
    fn new() -> Self {
        Self {
            events: Vec::new(),
            incarnation: 1,
        }
    }

    fn push(&mut self, event: AgentEvent) -> &mut Self {
        let id = self.events.len() as u64 + 1;
        self.events.push(StoredEvent {
            id: EventId(id),
            thread_id: ThreadId("t-1".into()),
            envelope: AgentEventEnvelope {
                incarnation: self.incarnation,
                seq: id,
                at_ms: T0_MS + id * 1000,
                event,
            },
        });
        self
    }

    fn start(&mut self, reasons: Vec<EnforcementReason>) -> &mut Self {
        self.push(AgentEvent::SessionStarted {
            provider: Provider::Claude,
            native_session_id: "abc-123".into(),
            model: Some("claude-opus".into()),
            tools: Vec::new(),
            enforcement: enforcement(reasons),
        })
    }

    fn deltas(&mut self, id: &str, kind: MessageKind, parts: &[&str]) -> &mut Self {
        for part in parts {
            self.push(AgentEvent::MessageDelta {
                message_id: id.into(),
                kind,
                text: (*part).into(),
            });
        }
        self
    }

    /// The deltas and their completion.
    fn message(&mut self, id: &str, kind: MessageKind, parts: &[&str]) -> &mut Self {
        self.deltas(id, kind, parts);
        self.push(AgentEvent::MessageCompleted {
            message_id: id.into(),
            kind,
            body: streamed(parts),
        })
    }

    fn inline(&mut self, id: &str, kind: MessageKind, text: &str) -> &mut Self {
        self.push(AgentEvent::MessageCompleted {
            message_id: id.into(),
            kind,
            body: MessageBody::Inline { text: text.into() },
        })
    }

    fn tool(&mut self, call: &str, title: Option<&str>, input: Value) -> &mut Self {
        self.push(AgentEvent::ToolCallStarted {
            call_id: call.into(),
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: title.map(Into::into),
            input,
        })
    }

    fn output(&mut self, call: &str, parts: &[&str]) -> &mut Self {
        for part in parts {
            self.push(AgentEvent::ToolCallUpdated {
                call_id: call.into(),
                title: None,
                kind: None,
                input: None,
                output_delta: Some((*part).into()),
                output: None,
            });
        }
        self
    }

    fn finish(&mut self, call: &str, output: Option<&str>) -> &mut Self {
        self.push(AgentEvent::ToolCallFinished {
            call_id: call.into(),
            status: ToolStatus::Succeeded,
            output: output.map(Into::into),
            error: None,
            truncated_bytes: 0,
        })
    }

    fn end(&mut self) -> &mut Self {
        self.push(AgentEvent::SessionEnded {
            exit_code: Some(0),
            reason: None,
        })
    }

    fn try_export(&self, format: ExportFormat) -> Result<String, HistoryError> {
        let mut out = Vec::new();
        let bytes = export_thread(
            &mut self.events.clone().into_iter(),
            format,
            &redactor(),
            &mut out,
        )?;
        assert_eq!(bytes, out.len() as u64, "returns the bytes written");
        Ok(String::from_utf8(out).expect("exports are UTF-8"))
    }

    fn export(&self, format: ExportFormat) -> String {
        self.try_export(format).expect("exports")
    }

    fn jsonl(&self) -> Vec<StoredEvent> {
        self.export(ExportFormat::Jsonl)
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line is a stored event"))
            .collect()
    }
}

fn ids(events: &[StoredEvent]) -> Vec<u64> {
    events.iter().map(|e| e.id.0).collect()
}

const BOTH: [ExportFormat; 2] = [ExportFormat::Markdown, ExportFormat::Jsonl];

// ---------------------------------------------------------------------------
// Reconstruction
// ---------------------------------------------------------------------------

#[test]
fn reconstruct_rejects_hash_or_length_mismatch() {
    let parts = ["héllo ", "wörld"];
    let body = streamed(&parts);
    assert_eq!(
        reconstruct_message("m1", &parts, &body).expect("matches"),
        "héllo wörld"
    );
    assert_eq!(
        reconstruct_message("m1", &[], &streamed(&[])).expect("an empty message"),
        ""
    );
    let inline = MessageBody::Inline { text: "hi".into() };
    assert_eq!(
        reconstruct_message("m1", &[], &inline).expect("inline"),
        "hi"
    );

    let MessageBody::Streamed {
        chars,
        deltas,
        sha256,
    } = body.clone()
    else {
        unreachable!()
    };
    let cases: Vec<(&str, Vec<&str>, MessageBody)> = vec![
        ("a lost delta", vec!["héllo "], body.clone()),
        ("reordered deltas", vec!["wörld", "héllo "], body.clone()),
        ("a changed delta", vec!["hello ", "wörld"], body.clone()),
        // Same text, cut differently: the count is part of the completion.
        (
            "a different delta count",
            vec!["héllo", " ", "wörld"],
            body.clone(),
        ),
        (
            "a wrong hash",
            parts.to_vec(),
            MessageBody::Streamed {
                chars,
                deltas,
                sha256: sha256.replace(|c: char| c.is_ascii_digit(), "0") + "0",
            },
        ),
        (
            "an uppercase hash",
            parts.to_vec(),
            MessageBody::Streamed {
                chars,
                deltas,
                sha256: sha256.to_uppercase(),
            },
        ),
        (
            // The length in bytes, not in characters.
            "a wrong length",
            parts.to_vec(),
            MessageBody::Streamed {
                chars: parts.concat().len() as u64,
                deltas,
                sha256: sha256.clone(),
            },
        ),
        ("an inline body after deltas", vec!["hi"], inline),
    ];
    for (what, deltas, body) in cases {
        match reconstruct_message("m1", &deltas, &body) {
            Err(HistoryError::Integrity { message_id }) => assert_eq!(message_id, "m1", "{what}"),
            other => panic!("{what}: expected an integrity error, got {other:?}"),
        }
    }
}

#[test]
fn export_fails_on_a_message_that_does_not_match_its_completion() {
    let mut thread = Thread::new();
    thread
        .start(Vec::new())
        .deltas("m1", MessageKind::Assistant, &["one ", "two"])
        .push(AgentEvent::MessageCompleted {
            message_id: "m1".into(),
            kind: MessageKind::Assistant,
            body: streamed(&["one ", "two", " three"]),
        });
    for format in BOTH {
        match thread.try_export(format) {
            Err(HistoryError::Integrity { message_id }) => assert_eq!(message_id, "m1"),
            other => panic!("expected an integrity error, got {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

#[test]
fn every_token_shape_is_redacted() {
    for secret in SHAPED {
        assert_eq!(redact(secret), "[redacted]", "{secret}");
        assert_eq!(
            redact(&format!("see ({secret}), then")),
            "see ([redacted]), then",
            "{secret}"
        );
    }
}

#[test]
fn private_key_blocks_are_redacted_whole() {
    assert_eq!(
        redact(
            "key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\nq1w2e3\n-----END RSA PRIVATE KEY-----\nafter"
        ),
        "key:\n[redacted]\nafter"
    );
    // Cut off (a truncated tool output): everything after the header goes.
    assert_eq!(
        redact("x -----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXk"),
        "x [redacted]"
    );
    // With its newlines still escaped, as inside a JSON string.
    assert_eq!(
        redact(r"-----BEGIN PRIVATE KEY-----\nMIIE\n-----END PRIVATE KEY-----\n"),
        r"[redacted]\n"
    );
    let public = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----";
    assert_eq!(redact(public), public);
}

#[test]
fn credentials_are_redacted_by_where_they_are() {
    let cases = [
        // Authorization headers and bearer tokens.
        ("Authorization: Bearer abc.def", "Authorization: [redacted]"),
        (
            "curl -H 'Authorization: token abc123' https://x",
            "curl -H 'Authorization: [redacted]' https://x",
        ),
        (
            r#"{"Authorization": "Basic dXNlcjpwYXNz"}"#,
            r#"{"Authorization": "[redacted]"}"#,
        ),
        (
            "use Bearer abcdefghijklmnop123456 here",
            "use Bearer [redacted] here",
        ),
        // Keys that name a secret.
        ("GITHUB_TOKEN=abc123", "GITHUB_TOKEN=[redacted]"),
        (
            "export API_KEY='p@ss word' next",
            "export API_KEY='[redacted]' next",
        ),
        (r#"password = "hunter2""#, r#"password = "[redacted]""#),
        ("db.password=hunter2", "db.password=[redacted]"),
        (
            r#"{"client_secret": "s3cr3t", "id": 1}"#,
            r#"{"client_secret": "[redacted]", "id": 1}"#,
        ),
        (
            r#"{'api-key': 'abc', 'n': 2}"#,
            r#"{'api-key': '[redacted]', 'n': 2}"#,
        ),
        (
            "https://x/?access_token=abc&x=1 y",
            "https://x/?access_token=[redacted] y",
        ),
        (r#":token=>"abc""#, r#":token=>"[redacted]""#),
        ("password: hunter22x", "password: [redacted]"),
        (
            "  - api_key: abcd1234efgh\nnext: 1",
            "  - api_key: [redacted]\nnext: 1",
        ),
        (
            "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "aws_secret_access_key = [redacted]",
        ),
        // Headers.
        (
            r#"curl -H "X-Api-Key: abcdef" https://x"#,
            r#"curl -H "X-Api-Key: [redacted]" https://x"#,
        ),
        (
            "X-Auth-Token: opaque value here\nnext",
            "X-Auth-Token: [redacted]\nnext",
        ),
        ("Cookie: session=abc123; theme=dark", "Cookie: [redacted]"),
        (
            r#"{"cookie": "sid=abc", "Authorization": "ApiKey abc"}"#,
            r#"{"cookie": "[redacted]", "Authorization": "[redacted]"}"#,
        ),
        // No scheme: one random-looking word.
        (
            "Authorization: a1b2c3d4e5f6\nHost: x",
            "Authorization: [redacted]\nHost: x",
        ),
        (
            "proxy-authorization=Basic dXNlcjpwYXNz",
            "proxy-authorization=[redacted]",
        ),
        (
            "Set-Cookie: sid=xyz; HttpOnly\n",
            "Set-Cookie: [redacted]\n",
        ),
        // Flags.
        (
            "curl -u pedro:hunter2 https://x",
            "curl -u pedro:[redacted] https://x",
        ),
        (
            "curl --user pedro:hunter2 x",
            "curl --user pedro:[redacted] x",
        ),
        (
            r#"curl -u "pedro:hunter 2" https://x"#,
            r#"curl -u "pedro:[redacted]" https://x"#,
        ),
        (
            "curl --user 'pedro:hunter2' x",
            "curl --user 'pedro:[redacted]' x",
        ),
        (
            "curl --user=pedro:hunter2 x",
            "curl --user=pedro:[redacted] x",
        ),
        (
            "mysql --password hunter2 -u root",
            "mysql --password [redacted] -u root",
        ),
        (
            r#"tool --api-key "abc def" run"#,
            r#"tool --api-key "[redacted]" run"#,
        ),
        // URLs.
        (
            "git clone https://pedro:hunter2@github.com/x/y.git",
            "git clone https://pedro:[redacted]@github.com/x/y.git",
        ),
    ];
    for (text, expected) in cases {
        assert_eq!(redact(text), expected, "{text}");
        assert_eq!(redact(expected), expected, "redacting twice: {text}");
    }
}

#[test]
fn ordinary_text_and_code_are_left_alone() {
    let cases = [
        "let token = lexer.next();",
        "token = lexer.next();",
        "secret = other_secret",
        // A known gap, not a goal: too short to tell from code.
        "password = hunter2",
        "docker run -u root image and tar -u archive.tar",
        "the cookies are in the jar, and a cookie-cutter",
        // `cookie` and `authorization` are identifiers too.
        "fn parse(cookie: &str) -> bool",
        "struct Req { cookie: String, path: String }",
        "    cookie: String,",
        "if (cookie == null) { cookie = getCookie(); }",
        "my_cookie: String and cookie = jar.get()",
        "cookie: str = request.cookies.get('session')",
        "document.cookie = \"a=b\"",
        "fn f(authorization: &str) and authorization: String,",
        "authorization = get_auth() or authorization: Option<String> = None",
        "authorization: str = Header(None)",
        "authorization: string | undefined;",
        "Authorization: required",
        "reauthorization: Bearer of news",
        "sort -u file and su -u pedro",
        "token: String,",
        "secret: Vec<u8>,",
        "    password: string;",
        "max_tokens=100",
        "if token == other {",
        "password=null and use_token: true",
        "password: must be at least 8 characters",
        "PWD=/tmp/x bypass=1 compass=north",
        "the task-force-one-two-three-four-five met",
        "class sk-spinner-rotating-plane-animation",
        "ghp_short sk-ant-short AIzaShort xoxb-1",
        "AKIAIOSFODNN7EXAMPLEX XAKIAIOSFODNN7EXAMPLE",
        "the bearer of bad news is here",
        "authorization_endpoint is https://example.com/auth",
        "cmd --token --verbose",
        "http://localhost:8080/a@b and https://example.com/path",
        "eyJhbGciOiJIUzI1NiJ9 alone is not a token",
        "C:\\Users\\pedro\\new and a \\n in a regex",
        "an em-dash — and “quotes” and 日本語",
        "",
    ];
    for text in cases {
        assert_eq!(redact(text), text);
    }
}

#[test]
fn home_becomes_a_tilde_only_where_it_is_the_home_directory() {
    let cases = [
        ("/Users/pedro/Harness/src", "~/Harness/src"),
        ("/Users/pedro", "~"),
        ("cd /Users/pedro && ls", "cd ~ && ls"),
        ("it is in /Users/pedro.", "it is in ~."),
        ("file:///Users/pedro/x", "file://~/x"),
        (r"\/Users\/pedro\/x", r"~\/x"),
        ("/Users/pedro2/x", "/Users/pedro2/x"),
        ("/Users/pedro.bak/x", "/Users/pedro.bak/x"),
        ("/Users/pedro-old/x", "/Users/pedro-old/x"),
        ("/mnt/Users/pedro/x", "/mnt/Users/pedro/x"),
        ("/Users/pedroé/x", "/Users/pedroé/x"),
    ];
    for (text, expected) in cases {
        assert_eq!(redact(text), expected, "{text}");
    }
    assert_eq!(
        Redactor::new("/Users/pedro/", &[]).text("/Users/pedro/x"),
        "~/x"
    );
    // No usable home: nothing is replaced.
    for home in ["", "/", "relative/home"] {
        let redactor = Redactor::new(home, &[]);
        assert_eq!(redactor.text("/a/relative/home/x"), "/a/relative/home/x");
    }
}

#[test]
fn escaped_and_nested_secrets_are_found_and_the_rest_is_untouched() {
    // A JSON document inside a string, with the token's first letter as a
    // `\u` escape and no telling key next to it.
    let escaped = format!(r#"{{\"note\":\"\u0067{}\"}}"#, &GITHUB[1..]);
    assert_eq!(redact(&escaped), r#"{\"note\":\"[redacted]\"}"#);
    // Two layers down.
    assert_eq!(
        redact(r#"\\\"password\\\":\\\"hunter2\\\""#),
        r#"\\\"password\\\":\\\"[redacted]\\\""#
    );
    // A literal secret that JSON has to escape.
    let quoted = Redactor::new("", &["p\"ss\\word"]);
    assert_eq!(
        quoted.text(r#"say "p\"ss\\word" now"#),
        r#"say "[redacted]" now"#
    );
    // Surrogate pairs and `\x` decode; the text around a hole is kept as is.
    let canary = Redactor::new("", &["k😀y-A"]);
    assert_eq!(
        canary.text(r"a\tb k\uD83D\uDE00y-\x41 c\n"),
        r"a\tb [redacted] c\n"
    );
}

#[test]
fn debug_does_not_print_secrets() {
    let printed = format!("{:?}", redactor());
    assert!(!printed.contains(CANARY), "{printed}");
    assert!(printed.contains("1 hidden"), "{printed}");
}

#[test]
fn redaction_never_panics_and_never_lets_a_literal_secret_through() {
    const PIECES: &[&str] = &[
        "\\",
        "\"",
        "'",
        "=",
        ":",
        " ",
        "\n",
        "\r",
        "\t",
        "\\u",
        "\\uD83D",
        "\\u00e9",
        "\\x4",
        "\\u{1F600}",
        "é",
        "😀",
        "token",
        "password",
        "x-api-key",
        "cookie",
        "--",
        "-u ",
        "--user ",
        "a:b",
        " = ",
        ": ",
        "-----BEGIN ",
        "PRIVATE KEY-----",
        "-----END ",
        "sk-",
        "sk-ant-",
        "ghp_",
        "eyJ",
        ".",
        "AKIA",
        "Authorization",
        "Bearer ",
        "://",
        "@",
        "/Users/pedro",
        "0123456789abcdefghij",
        "ABCDEFGHIJKLMNOP",
        "`",
        "{",
        "}",
        ",",
    ];
    // xorshift64: the same texts on every run.
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move |below: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % below as u64) as usize
    };
    let redactor = redactor();
    let rounds: usize = std::env::var("ATRIUM_FUZZ_ROUNDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(3000);
    for _ in 0..rounds {
        let len = 1 + next(40);
        let canary_at = next(len);
        let mut text = String::new();
        for i in 0..len {
            if i == canary_at {
                text.push_str(CANARY);
            }
            text.push_str(PIECES[next(PIECES.len())]);
        }
        let redacted = redactor.text(&text);
        assert!(!redacted.contains(CANARY), "{text:?} -> {redacted:?}");
    }
}

#[test]
fn redaction_stays_linear_on_one_long_line() {
    // Agent output is untrusted: a scanner that walked the rest of the line
    // from every match would take minutes here ("token=" and
    // "authorization:" once did), not milliseconds.
    let redactor = redactor();
    for piece in [
        "token=",
        "token: ",
        "token=\"",
        "authorization:",
        "cookie:",
        "cookie: a=",
        "authorization:abc_defg1 ",
        "authorization:abcdefgh(1",
        "authorization: a ",
        "x-token: ",
        "-u 'a:",
        "-u a:b",
        "bearer x",
        "--password x",
        "://a:",
        "eyJ",
        "-----BEGIN ",
        "\\\\\\\"",
    ] {
        let text = piece.repeat(512 * 1024 / piece.len());
        let started = std::time::Instant::now();
        let _ = redactor.text(&text);
        let took = started.elapsed();
        assert!(took.as_secs() < 10, "{piece:?} took {took:?}");
    }
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

#[test]
fn export_redacts_secrets_split_across_deltas_and_escaped() {
    const OPAQUE: &str = "0123456789abcdefOPAQUE";
    const PLAIN: &str = "plain-value-with-no-shape";
    let mut thread = Thread::new();
    thread
        .start(Vec::new())
        .push(AgentEvent::TurnStarted { turn: 1 })
        .inline(
            "u1",
            MessageKind::User,
            &format!("my password is {CANARY}, remember it"),
        )
        // A key cut in three by the stream.
        .message(
            "m1",
            MessageKind::Assistant,
            &[
                "Your key ",
                &ANTHROPIC[..4],
                &ANTHROPIC[4..17],
                &ANTHROPIC[17..],
                " is set.",
            ],
        )
        .tool(
            "c1",
            Some(&format!("curl -H 'Authorization: Bearer {OPAQUE}' x")),
            json!({
                "command": format!("curl -H \"Authorization: Bearer {OPAQUE}\" https://api.example.com"),
                "env": { "GITHUB_TOKEN": PLAIN },
                // JSON in a string, the token's first letter `\u`-escaped.
                "payload": format!("{{\"note\":\"\\u0067{}\"}}", &GITHUB[1..]),
                "cwd": "/Users/pedro/project",
            }),
        )
        // A token and the home directory, each cut by the stream.
        .output(
            "c1",
            &[
                "export GH=",
                &GITHUB[..14],
                &GITHUB[14..],
                "\ncd /Users/pe",
                "dro/project\n",
            ],
        )
        .finish("c1", None)
        .end();

    for format in BOTH {
        let out = thread.export(format);
        for secret in [
            CANARY,
            ANTHROPIC,
            &ANTHROPIC[4..17],
            &GITHUB[1..],
            &GITHUB[14..],
            OPAQUE,
            PLAIN,
            HOME,
        ] {
            assert!(!out.contains(secret), "{format:?} leaks {secret}:\n{out}");
        }
        assert!(out.contains("my password is [redacted], remember it"));
        assert!(out.contains("Your key [redacted] is set."));
        assert!(out.contains("~/project"));
    }
    // The values around the holes are intact.
    let events = thread.jsonl();
    let AgentEvent::ToolCallStarted { input, .. } = &events[4].envelope.event else {
        panic!("{:?}", events[4]);
    };
    assert_eq!(
        *input,
        json!({
            "command": "curl -H \"Authorization: [redacted]\" https://api.example.com",
            "env": { "GITHUB_TOKEN": "[redacted]" },
            "payload": "{\"note\":\"[redacted]\"}",
            "cwd": "~/project",
        })
    );
    let AgentEvent::ToolCallFinished { output, .. } = &events[5].envelope.event else {
        panic!("{:?}", events[5]);
    };
    assert_eq!(
        output.as_deref(),
        Some("export GH=[redacted]\ncd ~/project\n")
    );
}

#[test]
fn a_secret_cut_anywhere_by_the_stream_is_redacted() {
    for secret in SHAPED.iter().chain(&[CANARY]) {
        let text = format!("before {secret} after");
        let middle = &secret[secret.len() / 2 - 4..secret.len() / 2 + 4];
        for cut in (1..text.len()).filter(|i| text.is_char_boundary(*i)) {
            let parts = [&text[..cut], &text[cut..]];
            let mut thread = Thread::new();
            thread
                .message("m1", MessageKind::Assistant, &parts)
                .tool("c1", None, Value::Null)
                .output("c1", &parts)
                .finish("c1", None);
            for format in BOTH {
                let out = thread.export(format);
                assert!(
                    !out.contains(middle),
                    "{format:?}, cut at {cut}: {secret} leaks:\n{out}"
                );
                assert_eq!(out.matches("before [redacted] after").count(), 2);
            }
        }
    }
}

#[test]
fn markdown_is_a_readable_transcript() {
    let mut thread = Thread::new();
    thread
        .start(vec![EnforcementReason {
            code: ReasonCode::ExternalHooks,
            message: "Hooks in /Users/pedro/.claude/settings.json run outside Atrium".into(),
        }])
        .push(AgentEvent::TurnStarted { turn: 1 })
        .inline("u1", MessageKind::User, "List the files")
        .message("m1", MessageKind::Assistant, &["I'll ", "look."])
        .tool("c1", Some("ls"), json!({ "command": "ls" }))
        .push(AgentEvent::ApprovalRequested {
            request_id: "r1".into(),
            call_id: Some("c1".into()),
            name: "Bash".into(),
            kind: ToolKind::Execute,
            title: Some("ls".into()),
            input: json!({ "command": "ls" }),
            reason: Some("Not in the read-only list".into()),
            rememberable: true,
        })
        .push(AgentEvent::RequestResolved {
            request_id: "r1".into(),
            outcome: RequestOutcome::Approved,
        })
        .output("c1", &["a.txt\n", "b.txt\n"])
        .finish("c1", None)
        .push(AgentEvent::PlanUpdated {
            entries: vec![
                PlanEntry {
                    content: "List".into(),
                    status: PlanStatus::Completed,
                },
                PlanEntry {
                    content: "Summarize".into(),
                    status: PlanStatus::InProgress,
                },
                PlanEntry {
                    content: "Report".into(),
                    status: PlanStatus::Pending,
                },
            ],
        })
        .push(AgentEvent::QuestionAsked {
            request_id: "q1".into(),
            questions: vec![Question {
                id: "q".into(),
                prompt: "Which\nformat?".into(),
                options: vec![
                    QuestionOption {
                        id: "0".into(),
                        label: "Markdown".into(),
                        description: Some("for people".into()),
                    },
                    QuestionOption {
                        id: "1".into(),
                        label: "JSONL".into(),
                        description: None,
                    },
                ],
                multi_select: false,
                allow_text: false,
            }],
        })
        .push(AgentEvent::RequestResolved {
            request_id: "q1".into(),
            outcome: RequestOutcome::Answered,
        })
        .push(AgentEvent::Error {
            message: "The model is overloaded".into(),
            fatal: false,
            details: vec!["status 529".into(), "retrying".into()],
        })
        .push(AgentEvent::RateLimited {
            resets_at_ms: Some(T0_MS + 3_600_000),
            message: Some("5h window".into()),
        })
        .push(AgentEvent::Usage {
            usage: Usage::default(),
        })
        .push(AgentEvent::TurnCompleted {
            turn: 1,
            outcome: TurnOutcome::Completed,
            usage: Some(Usage {
                input_tokens: Some(120),
                output_tokens: Some(45),
                cost_usd: Some(0.0123),
                ..Usage::default()
            }),
        })
        .end();

    let expected = r#"# Thread t-1

## Session 1: Claude Code, claude-opus

- Started: 2025-11-09 13:50:02 UTC
- Native session: `abc-123`
- Coverage: partial (some tool calls could run without Atrium seeing them first)
  - Hooks in ~/.claude/settings.json run outside Atrium

### User

List the files

### Assistant

I'll look.

**Tool** `Bash` (execute): ls

```json
{
  "command": "ls"
}
```

**Approval requested** `Bash` (execute): ls

Reason: Not in the read-only list

**Approval** approved

**Result** `Bash`: succeeded

```text
a.txt
b.txt
```

**Plan**

- [x] List
- [ ] Summarize (in progress)
- [ ] Report

**Question**

1. Which format?
   - Markdown: for people
   - JSONL

**Question** answered

**Error**: The model is overloaded

```text
status 529
retrying
```

**Rate limited** until 2025-11-09 14:50:01 UTC: 5h window

_Turn 1 completed, 120 input tokens, 45 output tokens, $0.0123_

**Session ended**, exit code 0

"#;
    assert_eq!(thread.export(ExportFormat::Markdown), expected);
}

#[test]
fn jsonl_is_the_folded_event_stream() {
    let mut thread = Thread::new();
    thread
        .start(Vec::new())
        .push(AgentEvent::TurnStarted { turn: 1 })
        .inline("u1", MessageKind::User, "List the files")
        .message("m1", MessageKind::Assistant, &["I'll ", "look."])
        .tool("c1", Some("ls"), json!({ "command": "ls" }))
        .output("c1", &["a.txt\n"])
        // An update that renames the call and adds output: the title stays.
        .push(AgentEvent::ToolCallUpdated {
            call_id: "c1".into(),
            title: Some("ls -a".into()),
            kind: None,
            input: None,
            output_delta: Some("b.txt\n".into()),
            output: None,
        })
        .finish("c1", None)
        // A whole replacement, then a final output that replaces it all.
        .tool("c2", None, Value::Null)
        .output("c2", &["dropped"])
        .push(AgentEvent::ToolCallUpdated {
            call_id: "c2".into(),
            title: None,
            kind: None,
            input: None,
            output_delta: Some(" more".into()),
            output: Some("kept".into()),
        })
        .finish("c2", None)
        .tool("c3", None, Value::Null)
        .output("c3", &["streamed"])
        .finish("c3", Some("final"))
        .push(AgentEvent::Unmapped {
            record_type: "debug".into(),
            raw: json!({
                "path": "/Users/pedro/x",
                "api_key": "abc",
                // Everything under a key that names a secret.
                "token": { "value": "abc", "ttl": 60, "old": ["def"] },
                "/Users/pedro/key": true,
            }),
        })
        .end();

    let events = thread.jsonl();
    // Deltas (4, 5), and updates that only carried output (8, 12, 13, 16), are folded away.
    assert_eq!(ids(&events), [1, 2, 3, 6, 7, 9, 10, 11, 14, 15, 17, 18, 19]);
    let event = |id: u64| {
        let stored = events.iter().find(|e| e.id.0 == id).expect("exported");
        // Everything but the event is the stored event's own.
        assert_eq!(stored.envelope.seq, id);
        assert_eq!(stored.envelope.at_ms, T0_MS + id * 1000);
        assert_eq!(stored.thread_id, ThreadId("t-1".into()));
        stored.envelope.event.clone()
    };
    assert_eq!(
        event(6),
        AgentEvent::MessageCompleted {
            message_id: "m1".into(),
            kind: MessageKind::Assistant,
            body: MessageBody::Inline {
                text: "I'll look.".into()
            },
        }
    );
    assert_eq!(
        event(9),
        AgentEvent::ToolCallUpdated {
            call_id: "c1".into(),
            title: Some("ls -a".into()),
            kind: None,
            input: None,
            output_delta: None,
            output: None,
        }
    );
    let output = |id: u64| match event(id) {
        AgentEvent::ToolCallFinished { output, .. } => output,
        other => panic!("{other:?}"),
    };
    assert_eq!(output(10).as_deref(), Some("a.txt\nb.txt\n"));
    assert_eq!(output(14).as_deref(), Some("kept more"));
    assert_eq!(output(17).as_deref(), Some("final"));
    assert_eq!(
        event(18),
        AgentEvent::Unmapped {
            record_type: "debug".into(),
            raw: json!({
                "path": "~/x",
                "api_key": "[redacted]",
                "token": { "value": "[redacted]", "ttl": 60, "old": ["[redacted]"] },
                "~/key": true,
            }),
        }
    );
}

#[test]
fn what_is_still_open_is_exported_as_far_as_it_got() {
    let mut thread = Thread::new();
    thread
        .start(Vec::new())
        .tool("c1", None, json!({ "command": "make" }))
        .output("c1", &["compiling ", GITHUB])
        .deltas("m1", MessageKind::Assistant, &["Half a ", "sent"])
        // A call that started and has no output yet has nothing more to say.
        .tool("c2", None, Value::Null);

    // In event order, then what was open, each under its latest piece's id.
    let events = thread.jsonl();
    assert_eq!(ids(&events), [1, 2, 7, 4, 6]);
    assert_eq!(
        events[3].envelope.event,
        AgentEvent::ToolCallUpdated {
            call_id: "c1".into(),
            title: None,
            kind: None,
            input: None,
            output_delta: None,
            output: Some("compiling [redacted]".into()),
        }
    );
    assert_eq!(
        events[4].envelope.event,
        AgentEvent::MessageDelta {
            message_id: "m1".into(),
            kind: MessageKind::Assistant,
            text: "Half a sent".into(),
        }
    );

    let markdown = thread.export(ExportFormat::Markdown);
    assert!(
        markdown.contains("**Output so far** `Bash`\n\n```text\ncompiling [redacted]\n```\n"),
        "{markdown}"
    );
    assert!(
        markdown.ends_with("```\n\n### Assistant (incomplete)\n\nHalf a sent\n\n"),
        "{markdown}"
    );
}

#[test]
fn a_session_that_ends_closes_what_it_left_open() {
    let mut thread = Thread::new();
    thread
        .start(Vec::new())
        .deltas("m1", MessageKind::Assistant, &["cut ", "off"])
        .end();
    // The same message id in the next session is a different message.
    thread.incarnation = 2;
    thread
        .start(Vec::new())
        .message("m1", MessageKind::Assistant, &["second ", "session"])
        .end();

    let events = thread.jsonl();
    assert_eq!(ids(&events), [1, 3, 4, 5, 8, 9]);
    assert_eq!(
        events[1].envelope.event,
        AgentEvent::MessageDelta {
            message_id: "m1".into(),
            kind: MessageKind::Assistant,
            text: "cut off".into(),
        }
    );
    assert_eq!(events[1].envelope.incarnation, 1);
    assert_eq!(
        events[4].envelope.event,
        AgentEvent::MessageCompleted {
            message_id: "m1".into(),
            kind: MessageKind::Assistant,
            body: MessageBody::Inline {
                text: "second session".into()
            },
        }
    );
    assert_eq!(events[4].envelope.incarnation, 2);

    let markdown = thread.export(ExportFormat::Markdown);
    let first = markdown
        .find("### Assistant (incomplete)\n\ncut off\n\n**Session ended**")
        .expect("the first session's message, before its end");
    let second = markdown
        .find("## Session 2: Claude Code")
        .expect("the second session");
    assert!(first < second, "{markdown}");
    assert!(markdown.contains("### Assistant\n\nsecond session\n\n"));
}

#[test]
fn a_session_that_never_ended_keeps_its_open_items() {
    let mut thread = Thread::new();
    thread
        .start(Vec::new())
        .deltas("m1", MessageKind::Assistant, &["lost ", "session"]);
    thread.incarnation = 2;
    thread.start(Vec::new()).end();
    let events = thread.jsonl();
    assert_eq!(ids(&events), [1, 3, 4, 5]);
    assert_eq!(
        events[1].envelope.event,
        AgentEvent::MessageDelta {
            message_id: "m1".into(),
            kind: MessageKind::Assistant,
            text: "lost session".into(),
        }
    );
}

#[test]
fn markdown_cannot_be_broken_out_of_by_what_it_quotes() {
    let mut thread = Thread::new();
    thread
        .tool("c1", Some("echo `date`\nrm -rf x"), Value::Null)
        .finish("c1", Some("```\n# not a heading\n````\n"));
    let markdown = thread.export(ExportFormat::Markdown);
    assert_eq!(
        markdown,
        "# Thread t-1\n\n**Tool** `Bash` (execute): echo `date` rm -rf x\n\n\
         **Result** `Bash`: succeeded\n\n`````text\n```\n# not a heading\n````\n`````\n\n"
    );
}

#[test]
fn a_message_of_many_deltas_is_joined_once() {
    let parts: Vec<String> = (0..200_000).map(|i| format!("w{i} ")).collect();
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    let mut thread = Thread::new();
    thread.message("m1", MessageKind::Assistant, &parts);
    let events = thread.jsonl();
    assert_eq!(ids(&events), [200_001]);
    let AgentEvent::MessageCompleted {
        body: MessageBody::Inline { text },
        ..
    } = &events[0].envelope.event
    else {
        panic!("{:?}", events[0].envelope.event);
    };
    assert_eq!(*text, parts.concat());
}

#[test]
fn an_empty_thread_exports_nothing() {
    for format in BOTH {
        assert_eq!(Thread::new().export(format), "");
    }
}

#[test]
fn a_failing_writer_fails_the_export() {
    struct Full;
    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(std::io::ErrorKind::StorageFull))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut thread = Thread::new();
    thread.start(Vec::new());
    let result = export_thread(
        &mut thread.events.into_iter(),
        ExportFormat::Jsonl,
        &redactor(),
        &mut Full,
    );
    assert!(matches!(result, Err(HistoryError::Io(_))), "{result:?}");
}

// ---------------------------------------------------------------------------
// Export files
// ---------------------------------------------------------------------------

#[test]
fn create_export_file_is_0600_new_and_nofollow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("thread.md");

    let mut file = create_export_file(&path).expect("creates");
    file.write_all(b"exported").expect("is writable");
    drop(file);
    let meta = std::fs::metadata(&path).expect("exists");
    assert_eq!(meta.permissions().mode() & 0o7777, 0o600);
    assert_eq!(std::fs::read(&path).expect("reads"), b"exported");

    // New files only: an existing one is never opened or truncated.
    let again = create_export_file(&path).expect_err("exists");
    assert_eq!(again.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&path).expect("reads"), b"exported");

    // A symlink is not followed, whether or not it points anywhere.
    let target = dir.path().join("target.txt");
    std::fs::write(&target, b"untouched").expect("writes");
    let link = dir.path().join("link.md");
    symlink(&target, &link).expect("symlinks");
    let through = create_export_file(&link).expect_err("a symlink");
    assert_eq!(through.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&target).expect("reads"), b"untouched");

    let nowhere = dir.path().join("nowhere.txt");
    let dangling = dir.path().join("dangling.md");
    symlink(&nowhere, &dangling).expect("symlinks");
    let through = create_export_file(&dangling).expect_err("a dangling symlink");
    assert_eq!(through.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(!nowhere.exists());

    // The folder has to exist, be a folder, and the path needs a file name.
    let missing =
        create_export_file(&dir.path().join("no-such-folder/thread.md")).expect_err("no folder");
    assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    let under_a_file = create_export_file(&path.join("thread.md")).expect_err("not a folder");
    assert_eq!(under_a_file.kind(), std::io::ErrorKind::NotADirectory);
    let unnamed = create_export_file(&dir.path().join("..")).expect_err("no file name");
    assert_eq!(unnamed.kind(), std::io::ErrorKind::InvalidInput);

    // A tighter umask-style mode can't be asked for, and a looser one can't
    // happen: the file is 0600 whatever the folder allows.
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).expect("chmod");
    let open_folder = dir.path().join("open.md");
    drop(create_export_file(&open_folder).expect("creates"));
    let mode = std::fs::metadata(&open_folder)
        .expect("exists")
        .permissions()
        .mode();
    assert_eq!(mode & 0o7777, 0o600);
}

#[test]
fn create_export_file_refuses_a_folder_owned_by_someone_else() {
    let me = std::fs::metadata(tempfile::tempdir().expect("tempdir").path())
        .expect("stat")
        .uid();
    // World-writable and owned by root everywhere we run, so only the
    // ownership check can refuse it.
    let shared = std::path::Path::new("/tmp");
    if std::fs::metadata(shared).map(|m| m.uid()).ok() == Some(me) {
        return; // running as /tmp's owner (root in a container)
    }
    let path = shared.join(format!("atrium-export-test-{}.md", std::process::id()));
    let result = create_export_file(&path);
    let created = path.exists();
    let _ = std::fs::remove_file(&path);
    let error = result.expect_err("someone else's folder");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(!created);
}

#[test]
fn an_export_lands_in_its_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("thread.jsonl");
    let mut thread = Thread::new();
    thread
        .start(Vec::new())
        .message("m1", MessageKind::Assistant, &["he", "llo"])
        .end();
    let mut file = create_export_file(&path).expect("creates");
    let bytes = export_thread(
        &mut thread.events.clone().into_iter(),
        ExportFormat::Jsonl,
        &redactor(),
        &mut file,
    )
    .expect("exports");
    drop(file);
    let mut written = String::new();
    std::fs::File::open(&path)
        .expect("opens")
        .read_to_string(&mut written)
        .expect("reads");
    assert_eq!(bytes, written.len() as u64);
    assert_eq!(written, thread.export(ExportFormat::Jsonl));
    assert_eq!(written.lines().count(), 3);
}
