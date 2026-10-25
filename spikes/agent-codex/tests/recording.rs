use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use atrium_agents::AdapterInput;
use atrium_agents::fixture::{self, FixtureLine};
use atrium_protocol::agent::{AgentEvent, SessionCommand};
use serde_json::{Value, json};
use spike_agent_codex::{CodexAdapter, recording};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "live/recording-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn stdout(value: Value) -> FixtureLine {
    FixtureLine {
        ms: 0,
        input: AdapterInput::Stdout {
            line: value.to_string(),
        },
    }
}
fn notification(method: &str, params: Value) -> FixtureLine {
    stdout(json!({"method":method,"params":params}))
}
fn synthetic(host: &str) -> Vec<FixtureLine> {
    let mut lines = vec![
        notification("thread/started", json!({"thread":{"id":"thread-fixture"}})),
        // Copy identifiers before their structured definition to exercise the
        // whole-stream collection pass, including JSON-escaped hostnames.
        FixtureLine {
            ms: 0,
            input: AdapterInput::Stderr {
                line: format!("host={host} installation=11111111-2222-3333-4444-555555555555"),
            },
        },
        notification(
            "remoteControl/status/changed",
            json!({"serverName":host,"installationId":"11111111-2222-3333-4444-555555555555","environmentId":"private-environment","status":"disconnected"}),
        ),
        notification(
            "account/updated",
            json!({"authMode":"chatgpt","planType":"pro"}),
        ),
        notification(
            "mcpServer/startupStatus/updated",
            json!({"threadId":"thread-fixture","name":"personal-mcp","status":"starting","error":null}),
        ),
        notification(
            "mcpServer/startupStatus/updated",
            json!({"threadId":"thread-fixture","name":"personal-mcp","status":"failed","command":"/opt/private-account/bin/mcp --secret argument","error":format!("Cannot launch personal-mcp on {host} from /Volumes/private-data/config"),"futureDiagnostic":"private arbitrary diagnostic"}),
        ),
        notification(
            "mcpServer/startupStatus/updated",
            json!({"threadId":"thread-fixture","name":"second-personal-server","status":"ready"}),
        ),
    ];
    for (method, status) in [("hook/started", "running"), ("hook/completed", "failed")] {
        lines.push(notification(method,json!({"threadId":"thread-fixture","turnId":"turn-fixture","run":{"id":"private-hook-uuid","name":"personal-hook","displayOrder":0,"eventName":"sessionStart","executionMode":"sync","handlerType":"command","scope":"thread","source":"user","sourcePath":"/opt/private-account/hooks.json","command":"python /opt/private-account/hook.py","startedAt":0,"status":status,"statusMessage":"private hook description","entries":[{"kind":"error","text":"Private hook failed in /Volumes/private-data"}],"error":{"message":"private nested hook error"}}})));
    }
    lines.extend([
        notification("thread/readState/changed",json!({"threadId":"thread-fixture","readState":{"unread":false}})),
        notification("thread/status/changed",json!({"threadId":"thread-fixture","status":{"type":"idle"}})),
        notification("turn/moderationMetadata",json!({"threadId":"thread-fixture","turnId":"turn-fixture","metadata":{}})),
        notification("future/metadata",json!({"nested":[{"installationId":"another-installation","auth_mode":"apikey","plan_type":"enterprise"}]})),
        FixtureLine {ms:1,input:AdapterInput::Command {command:SessionCommand::SendUserMessage {text:format!("Hello from {host}")}}},
        FixtureLine {ms:2,input:AdapterInput::Exit {code:Some(0)}},
    ]);
    lines
}

#[test]
fn written_transcript_and_goldens_redact_identifiers_and_preserve_unmapped() {
    let dir = Scratch::new();
    let path = dir.0.join("noise.jsonl");
    let host = recording::hostname().expect("OS hostname");
    let raw = synthetic(&host);
    let original = raw.clone();
    let clean = recording::write(&path, &raw).unwrap();
    assert_eq!(raw, original, "recording must not mutate live session data");
    assert_eq!(fixture::read(&path).unwrap(), clean);
    let jsonl = std::fs::read_to_string(&path).unwrap();
    let golden = std::fs::read_to_string(path.with_extension("events.json")).unwrap();
    for text in [&jsonl, &golden] {
        assert!(
            !text.contains(&host),
            "recorded fixture leaked the current OS hostname"
        );
        for private in [
            "11111111-2222-3333-4444-555555555555",
            "another-installation",
            "private-environment",
            "personal-mcp",
            "second-personal-server",
            "private-hook-uuid",
            "personal-hook",
            "/opt/private-account",
            "/Volumes/private-data",
            "private arbitrary diagnostic",
            "private nested hook error",
        ] {
            assert!(!text.contains(private), "fixture leaked {private}");
        }
    }
    let events: Vec<AgentEvent> = serde_json::from_str(&golden).unwrap();
    let unknown = events
        .iter()
        .filter_map(|e| {
            if let AgentEvent::Unmapped { record_type, raw } = e {
                Some((record_type.as_str(), raw))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    for method in [
        "account/updated",
        "hook/started",
        "hook/completed",
        "mcpServer/startupStatus/updated",
        "remoteControl/status/changed",
        "thread/readState/changed",
        "thread/status/changed",
        "turn/moderationMetadata",
    ] {
        assert!(
            unknown.iter().any(|(m, _)| *m == method),
            "lost Unmapped {method}"
        );
    }
    let account = unknown
        .iter()
        .find(|(m, _)| *m == "account/updated")
        .unwrap()
        .1;
    assert_eq!(account["params"], json!({"authMode":null,"planType":null}));
    let remote = unknown
        .iter()
        .find(|(m, _)| *m == "remoteControl/status/changed")
        .unwrap()
        .1;
    assert_eq!(remote["params"]["serverName"], "<hostname>");
    assert_eq!(remote["params"]["installationId"], "<installation-id>");
    let mcps = unknown
        .iter()
        .filter(|(m, _)| *m == "mcpServer/startupStatus/updated")
        .map(|(_, v)| v["params"]["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(mcps, vec!["mcp-1", "mcp-1", "mcp-2"]);
    for (_, value) in unknown.iter().filter(|(m, _)| m.starts_with("hook/")) {
        assert_eq!(value["params"]["run"]["id"], "hook-1");
        assert_eq!(value["params"]["run"]["name"], "hook-1");
        assert_eq!(value["params"]["run"]["command"], "<hook-command>");
        assert_eq!(
            value["params"]["run"]["sourcePath"],
            "/fixture/hooks/hook-1"
        );
        assert_eq!(value["params"]["run"]["entries"][0]["kind"], "error");
    }
    fixture::assert_golden(&path, &mut spike_agent_codex::CodexNormalizer::default());
}

#[test]
fn installed_fixture_corpus_does_not_contain_current_hostname() {
    let host = recording::hostname().unwrap();
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/agents/codex");
    for entry in std::fs::read_dir(fixtures).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                !text.contains(&host),
                "{} contains current OS hostname",
                path.display()
            );
        }
    }
}

#[test]
fn overrides_disable_discovered_mcp_and_hooks_without_changing_auth() {
    let config = json!({"mcp_servers":{"personal.server":{"command":"/private/bin/server"},"quoted\"server":{"url":"https://private.invalid"}},"plugins":{"example@market":{"mcp_servers":{"plugin-server":{}}}},"forced_login_method":"chatgpt","cli_auth_credentials_store":"keyring"});
    let overrides = recording::config_overrides(&config);
    assert_eq!(
        overrides,
        vec![
            "features.hooks=false",
            "mcp_servers.\"personal.server\".enabled=false",
            "mcp_servers.\"quoted\\\"server\".enabled=false",
            "plugins.\"example@market\".mcp_servers.\"plugin-server\".enabled=false"
        ]
    );
    assert!(CodexAdapter::default().config_overrides.is_empty());
    assert_eq!(
        recording::config_overrides(&Value::Null),
        vec!["features.hooks=false"]
    );
    assert!(!overrides.iter().any(|arg| arg.contains("auth")
        || arg.contains("CODEX_HOME")
        || arg.contains("config.toml")));
}

#[test]
fn json_scalar_and_escaped_line_values_survive_sanitization() {
    let dir = Scratch::new();
    let path = dir.0.join("scalars.jsonl");
    let mut raw = synthetic("escaped\"host.local");
    raw.insert(0, stdout(json!("valid scalar")));
    let clean = recording::write(&path, &raw).unwrap();
    let AdapterInput::Stdout { line } = &clean[0].input else {
        panic!("stdout")
    };
    assert_eq!(
        serde_json::from_str::<Value>(line).unwrap(),
        json!("valid scalar")
    );
    let text = std::fs::read_to_string(path).unwrap();
    assert!(!text.contains("escaped"));
}
