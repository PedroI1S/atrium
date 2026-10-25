use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use atrium_agents::{
    AdapterInput, AgentAdapter, BoxFuture, EVENT_BUFFER, GateDecision, MAX_QUEUED_MESSAGES,
    McpServer, PermissionPolicy, SHUTDOWN_GRACE, SessionConfig, ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{AgentEvent, ApprovalDecision, QuestionAnswer, SessionCommand};
use serde_json::json;
use spike_agent_agy::AgyAdapter;
use spike_agent_agy::bridge::{
    ApprovalMode, GateBridge, HookDecisionOutput, RetryGrantStore, create_private_session_dir,
    run_hook_client, write_session_hook_config, write_session_mcp_config,
};
use spike_agent_agy::normalizer::canonical_json;
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc;

struct CountingGate {
    calls: AtomicUsize,
    decision: GateDecision,
}

impl CountingGate {
    fn new(decision: GateDecision) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            decision,
        }
    }

    fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ToolGate for CountingGate {
    fn check<'a>(&'a self, _call: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let decision = self.decision.clone();
        Box::pin(async move { decision })
    }
}

fn temp_test_dir(label: &str) -> PathBuf {
    create_private_session_dir(&std::env::temp_dir(), &format!("atrium-agy-test-{label}"))
        .expect("create private session dir")
}

#[tokio::test]
async fn bridge_allow_and_deny() {
    // Allow
    {
        let dir = temp_test_dir("allow");
        let sock = dir.join("gate.sock");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate: Arc<dyn ToolGate> = Arc::new(CountingGate::new(GateDecision::Allow));
        let bridge = GateBridge::start(sock.clone(), "sess-1", gate, ApprovalMode::Block, tx)
            .expect("start bridge");

        let payload = br#"{"toolCall":{"name":"read_file","args":{"Path":"Cargo.toml"}}}"#;
        let out = run_hook_client(&sock, payload, Duration::from_secs(2)).await;
        assert_eq!(out.decision, "allow");

        let mut saw_req = false;
        let mut saw_gate = false;
        let mut saw_resp = false;
        while let Ok(inp) = rx.try_recv() {
            match inp {
                AdapterInput::HookRequest { .. } => saw_req = true,
                AdapterInput::Gate { decision, .. } => {
                    assert_eq!(decision, GateDecision::Allow);
                    saw_gate = true;
                }
                AdapterInput::HookResponse { .. } => saw_resp = true,
                _ => {}
            }
        }
        assert!(saw_req && saw_gate && saw_resp);
        drop(bridge);
        let _ = fs::remove_dir_all(&dir);
    }

    // Deny
    {
        let dir = temp_test_dir("deny");
        let sock = dir.join("gate.sock");
        let (tx, _rx) = mpsc::unbounded_channel();
        let gate: Arc<dyn ToolGate> = Arc::new(CountingGate::new(GateDecision::Deny {
            reason: "policy forbade this tool".to_string(),
        }));
        let bridge = GateBridge::start(sock.clone(), "sess-1", gate, ApprovalMode::Block, tx)
            .expect("start bridge");

        let payload = br#"{"toolCall":{"name":"run_command","args":{"CommandLine":"rm -rf /"}}}"#;
        let out = run_hook_client(&sock, payload, Duration::from_secs(2)).await;
        assert_eq!(out.decision, "deny");
        assert_eq!(out.reason.as_deref(), Some("policy forbade this tool"));
        drop(bridge);
        let _ = fs::remove_dir_all(&dir);
    }
}

#[tokio::test]
async fn bridge_ask_block_allow_and_deny_and_question() {
    // Ask -> Allow in Block mode
    {
        let dir = temp_test_dir("ask-allow");
        let sock = dir.join("gate.sock");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate: Arc<dyn ToolGate> = Arc::new(CountingGate::new(GateDecision::Ask));
        let bridge = Arc::new(
            GateBridge::start(sock.clone(), "sess-ask", gate, ApprovalMode::Block, tx)
                .expect("start bridge"),
        );

        let sock_clone = sock.clone();
        let client_task = tokio::spawn(async move {
            let payload =
                br#"{"toolCall":{"name":"run_command","args":{"CommandLine":"echo ok"}}}"#;
            run_hook_client(&sock_clone, payload, Duration::from_secs(5)).await
        });

        let req_id = loop {
            match rx.recv().await.expect("bridge input") {
                AdapterInput::HookRequest { id, .. } => break id,
                _ => continue,
            }
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(bridge.resolve_approval(&req_id, &ApprovalDecision::Allow));

        let out = client_task.await.expect("client task");
        assert_eq!(out.decision, "allow");
        let _ = fs::remove_dir_all(&dir);
    }

    // Ask -> Deny in Block mode
    {
        let dir = temp_test_dir("ask-deny");
        let sock = dir.join("gate.sock");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate: Arc<dyn ToolGate> = Arc::new(CountingGate::new(GateDecision::Ask));
        let bridge = Arc::new(
            GateBridge::start(sock.clone(), "sess-ask-deny", gate, ApprovalMode::Block, tx)
                .expect("start bridge"),
        );

        let sock_clone = sock.clone();
        let client_task = tokio::spawn(async move {
            let payload =
                br#"{"toolCall":{"name":"run_command","args":{"CommandLine":"echo no"}}}"#;
            run_hook_client(&sock_clone, payload, Duration::from_secs(5)).await
        });

        let req_id = loop {
            match rx.recv().await.expect("bridge input") {
                AdapterInput::HookRequest { id, .. } => break id,
                _ => continue,
            }
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(bridge.resolve_approval(
            &req_id,
            &ApprovalDecision::Deny {
                message: Some("user clicked deny".to_string()),
            },
        ));

        let out = client_task.await.expect("client task");
        assert_eq!(out.decision, "deny");
        assert_eq!(out.reason.as_deref(), Some("user clicked deny"));
        let _ = fs::remove_dir_all(&dir);
    }

    // ask_question -> answer_question
    {
        let dir = temp_test_dir("question");
        let sock = dir.join("gate.sock");
        let (tx, mut rx) = mpsc::unbounded_channel();
        let gate: Arc<dyn ToolGate> = Arc::new(CountingGate::new(GateDecision::Allow));
        let bridge = Arc::new(
            GateBridge::start(sock.clone(), "sess-q", gate, ApprovalMode::Block, tx)
                .expect("start bridge"),
        );

        let sock_clone = sock.clone();
        let client_task = tokio::spawn(async move {
            let payload = br#"{"toolCall":{"name":"ask_question","args":{"questions":[{"question":"Profile?","options":["(Recommended) Debug","Release"],"is_multi_select":false}]}}}"#;
            run_hook_client(&sock_clone, payload, Duration::from_secs(5)).await
        });

        let req_id = loop {
            match rx.recv().await.expect("bridge input") {
                AdapterInput::HookRequest { id, .. } => break id,
                _ => continue,
            }
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(bridge.answer_question(
            &req_id,
            &[QuestionAnswer {
                question_id: "q-0".to_string(),
                selected: vec!["opt-0".to_string()],
                text: None,
            }],
        ));

        let out = client_task.await.expect("client task");
        assert_eq!(out.decision, "allow");
        let _ = fs::remove_dir_all(&dir);
    }
}

#[tokio::test]
async fn bridge_fails_closed_when_adapter_dropped_mid_wait() {
    let dir = temp_test_dir("drop-mid-wait");
    let sock = dir.join("gate.sock");
    let (tx, mut rx) = mpsc::unbounded_channel();
    let gate: Arc<dyn ToolGate> = Arc::new(CountingGate::new(GateDecision::Ask));
    let bridge = GateBridge::start(sock.clone(), "sess-drop", gate, ApprovalMode::Block, tx)
        .expect("start bridge");

    let sock_clone = sock.clone();
    let client_task = tokio::spawn(async move {
        let payload = br#"{"toolCall":{"name":"run_command","args":{"CommandLine":"echo wait"}}}"#;
        run_hook_client(&sock_clone, payload, Duration::from_secs(3)).await
    });

    loop {
        match rx.recv().await.expect("bridge input") {
            AdapterInput::HookRequest { .. } => break,
            _ => continue,
        }
    }
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(bridge);

    let out = client_task.await.expect("client task");
    assert_eq!(
        out.decision, "deny",
        "dropping adapter mid-wait must fail closed with deny"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn bridge_fails_closed_on_all_error_cases() {
    let dir = temp_test_dir("errors");
    let missing_sock = dir.join("missing.sock");

    // 1. Socket missing
    let out = run_hook_client(
        &missing_sock,
        br#"{"toolCall":{"name":"run_command","args":{}}}"#,
        Duration::from_millis(300),
    )
    .await;
    assert_eq!(out.decision, "deny");

    // 2. Empty stdin
    let out = run_hook_client(&missing_sock, b"   \n ", Duration::from_millis(300)).await;
    assert_eq!(out.decision, "deny");

    // 3. Invalid UTF-8 stdin
    let out = run_hook_client(
        &missing_sock,
        &[0xff, 0xfe, 0xfd],
        Duration::from_millis(300),
    )
    .await;
    assert_eq!(out.decision, "deny");

    // 4. Malformed JSON stdin
    let out = run_hook_client(&missing_sock, b"{bad json", Duration::from_millis(300)).await;
    assert_eq!(out.decision, "deny");

    // 5. Missing toolCall.name
    let out = run_hook_client(
        &missing_sock,
        br#"{"toolCall":{"args":{}}}"#,
        Duration::from_millis(300),
    )
    .await;
    assert_eq!(out.decision, "deny");

    // 6. Server closes connection immediately without replying
    let early_close_sock = dir.join("early-close.sock");
    let listener = UnixListener::bind(&early_close_sock).expect("bind early-close");
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });
    let out = run_hook_client(
        &early_close_sock,
        br#"{"toolCall":{"name":"run_command","args":{}}}"#,
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(out.decision, "deny");

    // 7. Server writes invalid JSON reply
    let bad_reply_sock = dir.join("bad-reply.sock");
    let listener = UnixListener::bind(&bad_reply_sock).expect("bind bad-reply");
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(b"not-json\n").await;
        }
    });
    let out = run_hook_client(
        &bad_reply_sock,
        br#"{"toolCall":{"name":"run_command","args":{}}}"#,
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(out.decision, "deny");

    // 8. Server writes unknown decision value ("maybe")
    let unknown_dec_sock = dir.join("unknown-dec.sock");
    let listener = UnixListener::bind(&unknown_dec_sock).expect("bind unknown-dec");
    tokio::spawn(async move {
        if let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(b"{\"decision\":\"maybe\"}\n").await;
        }
    });
    let out = run_hook_client(
        &unknown_dec_sock,
        br#"{"toolCall":{"name":"run_command","args":{}}}"#,
        Duration::from_secs(1),
    )
    .await;
    assert_eq!(out.decision, "deny");

    // 9. Server stalls past client timeout
    let timeout_sock = dir.join("timeout.sock");
    let listener = UnixListener::bind(&timeout_sock).expect("bind timeout");
    tokio::spawn(async move {
        if let Ok((_stream, _)) = listener.accept().await {
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });
    let out = run_hook_client(
        &timeout_sock,
        br#"{"toolCall":{"name":"run_command","args":{}}}"#,
        Duration::from_millis(150),
    )
    .await;
    assert_eq!(out.decision, "deny");

    // 10. Raw malformed JSON sent directly to GateBridge socket -> GateBridge replies deny
    let bridge_sock = dir.join("bridge.sock");
    let (tx, _rx) = mpsc::unbounded_channel();
    let bridge = GateBridge::start(
        bridge_sock.clone(),
        "sess-raw",
        Arc::new(CountingGate::new(GateDecision::Allow)),
        ApprovalMode::Block,
        tx,
    )
    .expect("start bridge");
    let mut raw_stream = UnixStream::connect(&bridge_sock).await.expect("connect");
    raw_stream
        .write_all(b"{\"toolCall\":{}}\n")
        .await
        .expect("write");
    let mut buf = String::new();
    tokio::io::AsyncBufReadExt::read_line(&mut tokio::io::BufReader::new(raw_stream), &mut buf)
        .await
        .expect("read");
    let parsed: HookDecisionOutput = serde_json::from_str(buf.trim()).expect("parse reply");
    assert_eq!(parsed.decision, "deny");
    drop(bridge);

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn retry_grant_single_use_and_canonical_args_and_argument_change() {
    let dir = temp_test_dir("retry-grants");
    let sock = dir.join("gate.sock");
    let (tx, mut rx) = mpsc::unbounded_channel();

    let counting_gate = Arc::new(CountingGate::new(GateDecision::Ask));
    let gate_dyn: Arc<dyn ToolGate> = counting_gate.clone();

    // Verify canonical_json sorts nested object keys deterministically
    let args_order_a = json!({"CommandLine": "echo hi", "meta": {"z": 1, "a": 2}});
    let args_order_b = json!({"meta": {"a": 2, "z": 1}, "CommandLine": "echo hi"});
    assert_eq!(canonical_json(&args_order_a), canonical_json(&args_order_b));

    let bridge = Arc::new(
        GateBridge::start(
            sock.clone(),
            "sess-retry",
            gate_dyn,
            ApprovalMode::DenyAndRetry,
            tx,
        )
        .expect("start bridge"),
    );

    // Step 1: First call in DenyAndRetry mode hits gate (count = 1) and returns deny immediately
    let payload_a =
        br#"{"toolCall":{"name":"run_command","args":{"CommandLine":"echo hi","meta":{"z":1,"a":2}}}}"#;
    let out1 = run_hook_client(&sock, payload_a, Duration::from_secs(2)).await;
    assert_eq!(out1.decision, "deny");
    assert_eq!(counting_gate.count(), 1);

    // Retrieve req_id and approve it -> issues a single-use RetryGrant
    let req_id = loop {
        match rx.recv().await.expect("bridge input") {
            AdapterInput::HookRequest { id, .. } => break id,
            _ => continue,
        }
    };
    assert!(bridge.resolve_approval(&req_id, &ApprovalDecision::Allow));
    assert_eq!(bridge.retry_grants().remaining(), 1);

    // Step 2: Retry with DIFFERENT arguments must NOT consume the grant and must hit the gate (count = 2)
    let payload_tampered =
        br#"{"toolCall":{"name":"run_command","args":{"CommandLine":"echo tampered","meta":{"z":1,"a":2}}}}"#;
    let out_tampered = run_hook_client(&sock, payload_tampered, Duration::from_secs(2)).await;
    assert_eq!(out_tampered.decision, "deny");
    assert_eq!(counting_gate.count(), 2);
    assert_eq!(
        bridge.retry_grants().remaining(),
        1,
        "grant for original args must still be unconsumed"
    );

    // Step 3: Retry with DIFFERENT tool_name must NOT consume the grant and must hit the gate (count = 3)
    let payload_diff_tool =
        br#"{"toolCall":{"name":"write_file","args":{"CommandLine":"echo hi","meta":{"z":1,"a":2}}}}"#;
    let out_diff_tool = run_hook_client(&sock, payload_diff_tool, Duration::from_secs(2)).await;
    assert_eq!(out_diff_tool.decision, "deny");
    assert_eq!(counting_gate.count(), 3);
    assert_eq!(bridge.retry_grants().remaining(), 1);

    // Step 4: Retry with matching arguments (even with reordered JSON keys!) consumes the grant once
    // and returns allow WITHOUT calling the gate again (count stays 3).
    let payload_b_reordered =
        br#"{"toolCall":{"name":"run_command","args":{"meta":{"a":2,"z":1},"CommandLine":"echo hi"}}}"#;
    let out_retry_ok = run_hook_client(&sock, payload_b_reordered, Duration::from_secs(2)).await;
    assert_eq!(out_retry_ok.decision, "allow");
    assert_eq!(
        counting_gate.count(),
        3,
        "gate must not be re-queried when a valid single-use RetryGrant is consumed"
    );
    assert_eq!(
        bridge.retry_grants().remaining(),
        0,
        "grant must be consumed after one use"
    );

    // Step 5: A second retry with the exact same arguments must NOT be allowed by the consumed grant
    // and must hit the gate again (count = 4 -> deny in DenyAndRetry mode).
    let out_second_retry = run_hook_client(&sock, payload_a, Duration::from_secs(2)).await;
    assert_eq!(out_second_retry.decision, "deny");
    assert_eq!(counting_gate.count(), 4);

    // Step 6: Also verify session_id scoping directly on RetryGrantStore
    let store = RetryGrantStore::new();
    store.insert("session-A", "run_command", &json!({"CommandLine": "pwd"}));
    assert!(
        !store.consume("session-B", "run_command", &json!({"CommandLine": "pwd"})),
        "RetryGrant must not match a different session_id"
    );
    assert!(
        store.consume("session-A", "run_command", &json!({"CommandLine": "pwd"})),
        "RetryGrant must match the issuing session_id"
    );

    drop(bridge);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn session_config_writers_set_strict_permissions_and_preserve_global_config() {
    let dir = temp_test_dir("config-writers");
    let dir_mode = fs::metadata(&dir)
        .expect("dir metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "session dir must have mode 0700");

    let helper = dir.join("spike-agy-hook");
    let sock = dir.join("gate.sock");
    let hooks_path =
        write_session_hook_config(&dir, &helper, &sock, 30).expect("write hook config");
    let hooks_mode = fs::metadata(&hooks_path)
        .expect("hooks metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(hooks_mode, 0o600, "hooks.json must have mode 0600");

    let hooks_text = fs::read_to_string(&hooks_path).expect("read hooks.json");
    assert!(hooks_text.contains("\"PreToolUse\""));
    assert!(hooks_text.contains("\"matcher\": \"*\""));

    let servers = vec![McpServer {
        name: "filesystem".to_string(),
        command: PathBuf::from("npx"),
        args: vec!["-y".to_string(), "mcp-fs".to_string()],
        env: vec![("MCP_TOKEN".to_string(), "redacted".to_string())],
    }];
    let mcp_path = write_session_mcp_config(&dir, &servers).expect("write mcp config");
    let mcp_mode = fs::metadata(&mcp_path)
        .expect("mcp metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mcp_mode, 0o600, "mcp.json must have mode 0600");
    let mcp_text = fs::read_to_string(&mcp_path).expect("read mcp.json");
    assert!(mcp_text.contains("\"mcpServers\""));
    assert!(mcp_text.contains("\"filesystem\""));

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn saturation_backpressure_interrupt_and_shutdown_complete_within_grace() {
    let dir = temp_test_dir("saturation");
    let fake_agy = dir.join("fake-agy.sh");

    // Write a fake `agy` script that floods > EVENT_BUFFER (2500) step_update lines
    // and then sleeps until signaled.
    let flood_count = EVENT_BUFFER * 2 + 400;
    let script = format!(
        "#!/bin/sh\n\
         printf '{{\"type\":\"init\",\"conversation_id\":\"sat-session-1\",\"init\":{{\"model\":\"gemini-3.8-flash-medium\",\"tools\":[\"run_command\"]}}}}\\n'\n\
         i=0\n\
         while [ $i -lt {flood_count} ]; do\n\
           printf '{{\"type\":\"step_update\",\"step_index\":1,\"step_type\":\"agent_response\",\"state\":\"ACTIVE\",\"text_delta\":\"x\"}}\\n'\n\
           i=$((i + 1))\n\
         done\n\
         trap 'exit 130' INT TERM\n\
         sleep 30 &\n\
         wait $!\n"
    );
    fs::write(&fake_agy, script).expect("write fake-agy.sh");
    fs::set_permissions(&fake_agy, fs::Permissions::from_mode(0o755)).expect("chmod +x");

    let adapter = AgyAdapter::new();
    let config = SessionConfig {
        cwd: dir.clone(),
        model: None,
        effort: None,
        permissions: PermissionPolicy::FullAccess,
        gate: Arc::new(CountingGate::new(GateDecision::Allow)),
        mcp_servers: Vec::new(),
        resume: None,
        env: Vec::new(),
        program: Some(fake_agy),
    };

    let mut session = adapter
        .start(config, Transport::Local)
        .await
        .expect("start fake-agy");

    // Wait long enough for the child to flood stdout and fill `session.events` (EVENT_BUFFER = 1024)
    // WITHOUT reading a single event from `session.events`.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // Also test sending > MAX_QUEUED_MESSAGES while turn is active and events channel is saturated
    for idx in 0..=(MAX_QUEUED_MESSAGES + 2) {
        session
            .commands
            .send(SessionCommand::SendUserMessage {
                text: format!("queued-{idx}"),
            })
            .await
            .expect("send queued message without blocking");
    }

    // Now send Interrupt and Shutdown while `session.events` is STILL completely unread!
    let start = Instant::now();
    session
        .commands
        .send(SessionCommand::Interrupt)
        .await
        .expect("send Interrupt while saturated");
    session
        .commands
        .send(SessionCommand::Shutdown)
        .await
        .expect("send Shutdown while saturated");

    // Drain `session.events` and verify the session terminates within SHUTDOWN_GRACE
    let mut saw_session_ended = false;
    let drain_deadline = Duration::from_secs(SHUTDOWN_GRACE.as_secs() + 2);
    let drain_res = tokio::time::timeout(drain_deadline, async {
        while let Some(env) = session.events.recv().await {
            if matches!(env.event, AgentEvent::SessionEnded { .. }) {
                saw_session_ended = true;
                break;
            }
        }
    })
    .await;

    assert!(
        drain_res.is_ok(),
        "session did not finish within SHUTDOWN_GRACE ({:?})",
        start.elapsed()
    );
    assert!(
        start.elapsed() < SHUTDOWN_GRACE + Duration::from_secs(1),
        "shutdown took {:?}, exceeding grace period",
        start.elapsed()
    );
    assert!(saw_session_ended, "must observe SessionEnded after drain");

    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn spike_agy_hook_binary_end_to_end_and_cli_args() {
    let dir = temp_test_dir("bin-e2e");
    let sock = dir.join("g.sock");
    let (tx, _rx) = mpsc::unbounded_channel();
    let bridge = GateBridge::start(
        sock.clone(),
        "sess-bin",
        Arc::new(CountingGate::new(GateDecision::Allow)),
        ApprovalMode::Block,
        tx,
    )
    .expect("start bridge");

    let hook_bin = PathBuf::from(env!("CARGO_BIN_EXE_spike-agy-hook"));
    assert!(hook_bin.exists(), "spike-agy-hook binary must exist");

    // 1. Valid request over real spike-agy-hook subprocess -> allow
    let mut child = tokio::process::Command::new(&hook_bin)
        .arg("--socket")
        .arg(&sock)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn spike-agy-hook");
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(br#"{"toolCall":{"name":"read_file","args":{"Path":"README.md"}}}"#)
            .await
            .expect("write stdin");
    }
    let out = child.wait_with_output().await.expect("wait spike-agy-hook");
    assert!(out.status.success());
    let parsed: HookDecisionOutput =
        serde_json::from_slice(&out.stdout).expect("parse hook stdout");
    assert_eq!(parsed.decision, "allow");

    // 2. Missing --socket argument -> spike-agy-hook still exits 0 with decision: deny (fail-closed)
    let out_missing = tokio::process::Command::new(&hook_bin)
        .output()
        .await
        .expect("run spike-agy-hook without args");
    assert!(out_missing.status.success());
    let parsed_missing: HookDecisionOutput =
        serde_json::from_slice(&out_missing.stdout).expect("parse hook stdout");
    assert_eq!(parsed_missing.decision, "deny");

    // 3. Pin exact CLI argv and stdin NDJSON shape verified against agy 1.4.7
    let default_adapter = AgyAdapter::new();
    let default_cfg = SessionConfig {
        cwd: dir.clone(),
        model: Some("gemini-3.8-flash-medium".to_string()),
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate: Arc::new(CountingGate::new(GateDecision::Allow)),
        mcp_servers: Vec::new(),
        resume: None,
        env: Vec::new(),
        program: None,
    };
    let pinned_args = default_adapter.build_cli_args(&default_cfg, &dir);
    assert_eq!(
        pinned_args,
        vec![
            "--print=",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--print-timeout",
            "0s",
            "--mode",
            "accept-edits",
            "--dangerously-skip-permissions",
            "--model",
            "gemini-3.8-flash-medium",
        ]
    );
    assert!(
        !pinned_args.contains(&"-p".to_string()),
        "-p without attached value must never be used; use --print="
    );

    let stdin_line = spike_agent_agy::StdinFormat::default().format_message("Reply with PONG.");
    assert_eq!(
        stdin_line,
        r#"{"event":"user","message":{"content":"Reply with PONG."}}"#
    );

    // 4. Verify AgyAdapter::build_cli_args with ReadOnly, resume, and pass_add_dir
    let mut adapter = AgyAdapter::new();
    adapter.pass_add_dir = true;
    let cfg = SessionConfig {
        cwd: dir.clone(),
        model: Some("gemini-3.8-flash-medium".to_string()),
        effort: None,
        permissions: PermissionPolicy::ReadOnly,
        gate: Arc::new(CountingGate::new(GateDecision::Allow)),
        mcp_servers: Vec::new(),
        resume: Some("prev-conv-123".to_string()),
        env: Vec::new(),
        program: None,
    };
    let args = adapter.build_cli_args(&cfg, &dir);
    assert_eq!(
        args,
        vec![
            "--print=".to_string(),
            "--input-format".to_string(),
            "stream-json".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--print-timeout".to_string(),
            "0s".to_string(),
            "--mode".to_string(),
            "plan".to_string(),
            "--dangerously-skip-permissions".to_string(),
            "--model".to_string(),
            "gemini-3.8-flash-medium".to_string(),
            "--conversation".to_string(),
            "prev-conv-123".to_string(),
            "--add-dir".to_string(),
            dir.display().to_string(),
        ]
    );

    // 5. End-to-end live session check against a mock agy binary that validates argv[1] == "--print="
    // and validates the stdin line has `"event":"user"` and `"message":{"content":"..."}`.
    let mock_agy = dir.join("mock-agy.sh");
    let mock_script = "#!/bin/sh\n\
        if [ \"$1\" != \"--print=\" ]; then\n\
          echo \"Error: -p took $2 as its prompt\" >&2\n\
          exit 2\n\
        fi\n\
        read -r line\n\
        case \"$line\" in\n\
          *'\"event\":\"user\"'*'\"message\":{\"content\":\"Reply with PONG.\"}'*) ;;\n\
          *) echo \"Error: missing the event field or unsupported event: $line\" >&2; exit 2 ;;\n\
        esac\n\
        printf '{\"event\":\"init\",\"conversation_id\":\"conv-live-1\",\"init\":{\"model\":\"gemini-3.8-flash-medium\",\"tools\":[\"run_command\"]}}\\n'\n\
        printf '{\"event\":\"step_update\",\"step_update\":{\"conversation_id\":\"conv-live-1\",\"step_index\":1,\"state\":\"DONE\",\"step_type\":\"agent_response\",\"text_delta\":\"PONG\"}}\\n'\n\
        printf '{\"event\":\"result\",\"result\":{\"conversation_id\":\"conv-live-1\",\"status\":\"SUCCESS\",\"response\":\"PONG\",\"num_turns\":1,\"usage\":{\"input_tokens\":10,\"output_tokens\":2}}}\\n'\n\
        exit 0\n";
    fs::write(&mock_agy, mock_script).expect("write mock-agy.sh");
    fs::set_permissions(&mock_agy, fs::Permissions::from_mode(0o755)).expect("chmod +x");

    let mut live_cfg = default_cfg;
    live_cfg.program = Some(mock_agy);
    let mut session = default_adapter
        .start(live_cfg, Transport::Local)
        .await
        .expect("start mock-agy");
    session
        .commands
        .send(SessionCommand::SendUserMessage {
            text: "Reply with PONG.".to_string(),
        })
        .await
        .expect("send user message");

    let mut saw_started = false;
    let mut saw_pong = false;
    let mut saw_completed = false;
    while let Some(env) = session.events.recv().await {
        match env.event {
            AgentEvent::SessionStarted {
                native_session_id, ..
            } => {
                assert_eq!(native_session_id, "conv-live-1");
                saw_started = true;
            }
            AgentEvent::MessageCompleted { text, .. } => {
                assert_eq!(text, "PONG");
                saw_pong = true;
            }
            AgentEvent::TurnCompleted { .. } => {
                saw_completed = true;
            }
            AgentEvent::SessionEnded { exit_code, .. } => {
                assert_eq!(exit_code, Some(0));
                break;
            }
            _ => {}
        }
    }
    assert!(saw_started && saw_pong && saw_completed);

    drop(bridge);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn record_and_hook_failures_harnesses_reject_broken_launches() {
    let dir = temp_test_dir("harnesses");

    // Create a broken agy binary that prints usage on stderr and exits 2 without emitting `init`
    let broken_agy = dir.join("broken-agy.sh");
    fs::write(
        &broken_agy,
        "#!/bin/sh\necho 'Error: bad flag' >&2\nexit 2\n",
    )
    .expect("write broken-agy.sh");
    fs::set_permissions(&broken_agy, fs::Permissions::from_mode(0o755)).expect("chmod +x");

    let hook_bin = PathBuf::from(env!("CARGO_BIN_EXE_spike-agy-hook"));
    let target_debug_dir = hook_bin.parent().expect("target/debug");
    let examples_dir = target_debug_dir.join("examples");

    // 1. Verify `examples/hook-failures` with broken agy reports INCONCLUSIVE and exits 2,
    // and NEVER reports `fail_closed: true`.
    let hf_bin = examples_dir.join("hook-failures");
    if hf_bin.exists() {
        let out = tokio::process::Command::new(&hf_bin)
            .arg("--binary")
            .arg(&broken_agy)
            .output()
            .await
            .expect("run hook-failures with broken agy");
        assert_eq!(
            out.status.code(),
            Some(2),
            "hook-failures must exit 2 when control case is inconclusive"
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("OVERALL RUN: INCONCLUSIVE"));
        assert!(stdout.contains("\"verdict\": \"inconclusive\""));
        assert!(!stdout.contains("\"fail_closed\": true"));
    }

    // 2. Verify `examples/record` with broken agy on a non-startup-error scenario ("simple")
    // fails loudly (non-zero exit) and refuses to write simple.jsonl into the target fixtures dir.
    let rec_bin = examples_dir.join("record");
    if rec_bin.exists() {
        let scratch_fixtures = dir.join("fixtures");
        fs::create_dir_all(&scratch_fixtures).expect("create scratch fixtures");
        let out = tokio::process::Command::new(&rec_bin)
            .arg("--binary")
            .arg(&broken_agy)
            .arg("--fixtures-dir")
            .arg(&scratch_fixtures)
            .arg("simple")
            .output()
            .await
            .expect("run record with broken agy");
        assert!(
            !out.status.success(),
            "record must fail loudly when simple does not reach SessionStarted"
        );
        assert!(
            !scratch_fixtures.join("simple.jsonl").exists(),
            "record must never write a live fixture when SessionStarted was not reached"
        );
    }

    let _ = fs::remove_dir_all(&dir);
}
