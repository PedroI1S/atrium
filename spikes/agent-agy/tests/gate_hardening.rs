//! Fail-open hardening (Fix round 2).
//!
//! Live finding on `agy` 1.4.7: an unreadable (mode 0000) or malformed hook config makes `agy`
//! ignore the hooks and run the tool under `--dangerously-skip-permissions`. These tests pin
//! what the adapter does about it: validate the config before spawning, refuse to start when it
//! cannot be trusted, and abort a running session when a tool runs without a covering hook.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use atrium_agents::{
    AdapterError, AgentAdapter, BoxFuture, GateDecision, PermissionPolicy, SessionConfig,
    ToolCheck, ToolGate, Transport,
};
use atrium_protocol::agent::{AgentEvent, SessionCommand, ToolStatus};
use serde_json::json;
use spike_agent_agy::AgyAdapter;
use spike_agent_agy::bridge::{
    DirRule, GateSetup, HookConfigError, HookConfigExpectation, agents_dir_name,
    check_helper_binary, create_private_session_dir, install_cwd_hook, validate_hook_config,
    write_session_hook_config,
};

struct AllowGate;

impl ToolGate for AllowGate {
    fn check<'a>(&'a self, _call: &'a ToolCheck) -> BoxFuture<'a, GateDecision> {
        Box::pin(async { GateDecision::Allow })
    }
}

fn hook_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_spike-agy-hook"))
}

fn temp_dir(label: &str) -> PathBuf {
    create_private_session_dir(&std::env::temp_dir(), &format!("atrium-agy-hard-{label}"))
        .expect("create private dir")
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

/// A session dir with a freshly written, valid config, and the matching expectation.
fn valid_session(label: &str) -> (PathBuf, HookConfigExpectation) {
    let dir = temp_dir(label);
    let exp = HookConfigExpectation {
        helper_bin: hook_bin(),
        socket_path: dir.join("g.sock"),
        timeout_secs: 30,
    };
    write_session_hook_config(&dir, &exp.helper_bin, &exp.socket_path, exp.timeout_secs)
        .expect("write hook config");
    (dir, exp)
}

fn setup_for(dir: &Path, exp: &HookConfigExpectation, cwd_hooks: Option<PathBuf>) -> GateSetup {
    GateSetup {
        session_dir: dir.to_path_buf(),
        cwd_hooks,
        expectation: exp.clone(),
    }
}

fn cleanup(dir: &Path) {
    // Make sure nothing is left unreadable before removing.
    let _ = fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------------------------
// Config validation
// ---------------------------------------------------------------------------------------------

#[test]
fn valid_config_passes_validation() {
    let (dir, exp) = valid_session("valid");
    validate_hook_config(&dir.join("hooks.json"), &exp, DirRule::Private).expect("valid");
    setup_for(&dir, &exp, None).verify().expect("whole setup");
    cleanup(&dir);
}

#[test]
fn unreadable_mode_0000_config_is_rejected() {
    // The exact live fail-open case: agy ignores a hooks.json it cannot read.
    let (dir, exp) = valid_session("mode0000");
    let hooks = dir.join("hooks.json");
    set_mode(&hooks, 0o000);
    let err = validate_hook_config(&hooks, &exp, DirRule::Private).expect_err("must reject");
    assert!(
        matches!(err, HookConfigError::BadFileMode { actual: 0, .. }),
        "got {err}"
    );
    // The nested copy (the one in <agents dir>) is checked as well.
    set_mode(&hooks, 0o600);
    let nested = dir.join(agents_dir_name()).join("hooks.json");
    set_mode(&nested, 0o000);
    let err = setup_for(&dir, &exp, None).verify().expect_err("nested");
    assert!(
        matches!(err, HookConfigError::BadFileMode { .. }),
        "got {err}"
    );
    set_mode(&nested, 0o600);
    cleanup(&dir);
}

#[test]
fn malformed_json_config_is_rejected() {
    // The other live fail-open case: agy ignores a hooks.json that does not parse.
    let (dir, exp) = valid_session("malformed");
    let hooks = dir.join("hooks.json");
    for bad in ["{ malformed json ", "", "not json at all", "{\"atrium\": "] {
        fs::write(&hooks, bad).expect("write");
        set_mode(&hooks, 0o600);
        let err = validate_hook_config(&hooks, &exp, DirRule::Private).expect_err(bad);
        assert!(
            matches!(err, HookConfigError::Malformed { .. }),
            "{bad:?}: {err}"
        );
    }
    fs::write(&hooks, "[1,2,3]").expect("write");
    set_mode(&hooks, 0o600);
    let err = validate_hook_config(&hooks, &exp, DirRule::Private).expect_err("array");
    assert!(
        matches!(err, HookConfigError::Malformed { .. }),
        "got {err}"
    );
    cleanup(&dir);
}

#[test]
fn config_without_the_gate_hook_is_rejected() {
    let (dir, exp) = valid_session("nogate");
    let hooks = dir.join("hooks.json");
    let good = fs::read_to_string(&hooks).expect("read");
    let mutations: Vec<(&str, String)> = vec![
        ("empty object", "{}".to_owned()),
        (
            "no PreToolUse",
            json!({"g": {"PostToolUse": []}}).to_string(),
        ),
        ("other command", good.replace("--socket", "--other-flag")),
        ("other socket", good.replace("g.sock", "evil.sock")),
        (
            "narrow matcher",
            good.replace("\"matcher\": \"*\"", "\"matcher\": \"run_command\""),
        ),
        (
            "other timeout",
            good.replace("\"timeout\": 30", "\"timeout\": 31"),
        ),
        (
            "non-command hook",
            good.replace("\"type\": \"command\"", "\"type\": \"log\""),
        ),
    ];
    for (name, text) in mutations {
        fs::write(&hooks, &text).expect("write");
        set_mode(&hooks, 0o600);
        let err = validate_hook_config(&hooks, &exp, DirRule::Private).expect_err(name);
        assert!(
            matches!(err, HookConfigError::GateHookAbsent { .. }),
            "{name}: {err}"
        );
    }
    cleanup(&dir);
}

#[test]
fn file_and_dir_modes_must_be_strict() {
    let (dir, exp) = valid_session("modes");
    let hooks = dir.join("hooks.json");
    for mode in [0o644, 0o640, 0o604, 0o400, 0o700, 0o666] {
        set_mode(&hooks, mode);
        let err = validate_hook_config(&hooks, &exp, DirRule::Private).expect_err("file mode");
        assert!(
            matches!(err, HookConfigError::BadFileMode { .. }),
            "{mode:o}: {err}"
        );
    }
    set_mode(&hooks, 0o600);

    // Private directories must be exactly 0700.
    for mode in [0o755, 0o750, 0o770, 0o500] {
        set_mode(&dir, mode);
        let err = validate_hook_config(&hooks, &exp, DirRule::Private).expect_err("dir mode");
        assert!(
            matches!(err, HookConfigError::BadDirMode { .. }),
            "{mode:o}: {err}"
        );
    }
    // A project's own config dir may be 0755 but not writable by group or others.
    set_mode(&dir, 0o755);
    validate_hook_config(&hooks, &exp, DirRule::NoGroupOtherWrite).expect("0755 is fine");
    for mode in [0o775, 0o757, 0o777] {
        set_mode(&dir, mode);
        let err = validate_hook_config(&hooks, &exp, DirRule::NoGroupOtherWrite)
            .expect_err("writable by others");
        assert!(
            matches!(err, HookConfigError::BadDirMode { .. }),
            "{mode:o}: {err}"
        );
    }
    set_mode(&dir, 0o700);
    cleanup(&dir);
}

#[test]
fn symlinks_and_missing_files_are_rejected() {
    let (dir, exp) = valid_session("links");
    let hooks = dir.join("hooks.json");

    // hooks.json replaced by a symlink to a perfectly valid file elsewhere.
    let elsewhere = temp_dir("links-elsewhere");
    let real = elsewhere.join("real.json");
    fs::copy(&hooks, &real).expect("copy");
    set_mode(&real, 0o600);
    fs::remove_file(&hooks).expect("rm");
    symlink(&real, &hooks).expect("symlink");
    let err = validate_hook_config(&hooks, &exp, DirRule::Private).expect_err("symlink file");
    assert!(
        matches!(err, HookConfigError::NotRegularFile { .. }),
        "got {err}"
    );

    // Missing file.
    fs::remove_file(&hooks).expect("rm link");
    let err = validate_hook_config(&hooks, &exp, DirRule::Private).expect_err("missing");
    assert!(matches!(err, HookConfigError::Missing { .. }), "got {err}");

    // A symlinked config directory.
    let linked_dir = dir.join("linked");
    symlink(&elsewhere, &linked_dir).expect("symlink dir");
    let err = validate_hook_config(
        &linked_dir.join("real.json"),
        &exp,
        DirRule::NoGroupOtherWrite,
    )
    .expect_err("symlink dir");
    assert!(
        matches!(err, HookConfigError::NotADirectory { .. }),
        "got {err}"
    );

    cleanup(&dir);
    cleanup(&elsewhere);
}

#[test]
fn helper_binary_must_exist_and_be_executable() {
    check_helper_binary(&hook_bin()).expect("the real helper is fine");

    let dir = temp_dir("helper");
    let missing = dir.join("no-such-helper");
    assert!(matches!(
        check_helper_binary(&missing),
        Err(HookConfigError::HelperUnusable { .. })
    ));
    let not_exec = dir.join("plain-file");
    fs::write(&not_exec, "#!/bin/sh\n").expect("write");
    set_mode(&not_exec, 0o600);
    let err = check_helper_binary(&not_exec).expect_err("not executable");
    assert!(err.to_string().contains("not executable"), "got {err}");
    assert!(matches!(
        check_helper_binary(Path::new("surely-not-on-path-spike-agy-hook-xyz")),
        Err(HookConfigError::HelperUnusable { .. })
    ));
    cleanup(&dir);

    // The full setup check reports a missing helper too.
    let (sdir, mut exp) = valid_session("helper-setup");
    exp.helper_bin = sdir.join("gone");
    write_session_hook_config(&sdir, &exp.helper_bin, &exp.socket_path, exp.timeout_secs)
        .expect("rewrite");
    let err = setup_for(&sdir, &exp, None)
        .verify()
        .expect_err("helper gone");
    assert!(
        matches!(err, HookConfigError::HelperUnusable { .. }),
        "got {err}"
    );
    cleanup(&sdir);
}

#[test]
fn cwd_install_creates_strict_files_and_refuses_existing_configs() {
    let (sdir, exp) = valid_session("cwd-install");
    let session_hooks = sdir.join("hooks.json");

    // Fresh cwd: installed with mode 0600 in a 0700 dir, passes the full check, removable.
    let cwd = temp_dir("cwd-fresh");
    let install = install_cwd_hook(&cwd, &session_hooks).expect("install");
    let mode = fs::metadata(&install.hooks_path)
        .expect("meta")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    assert!(install.created_dir.is_some());
    setup_for(&sdir, &exp, Some(install.hooks_path.clone()))
        .verify()
        .expect("cwd copy validates");
    install.remove();
    assert!(!cwd.join(agents_dir_name()).exists(), "created dir removed");
    cleanup(&cwd);

    // Pre-existing configs of every kind are refused and left exactly as they were.
    let variants: Vec<(&str, String, u32)> = vec![
        (
            "other valid hook",
            json!({"x":{"PreToolUse":[]}}).to_string(),
            0o644,
        ),
        ("malformed", "{ malformed".to_owned(), 0o644),
        ("unreadable", "{}".to_owned(), 0o000),
    ];
    for (name, contents, mode) in variants {
        let cwd = temp_dir("cwd-existing");
        let agents = cwd.join(agents_dir_name());
        fs::create_dir_all(&agents).expect("mkdir");
        let existing = agents.join("hooks.json");
        fs::write(&existing, &contents).expect("write");
        set_mode(&existing, mode);
        let err = install_cwd_hook(&cwd, &session_hooks).expect_err(name);
        assert!(
            matches!(err, HookConfigError::CwdHooksPresent { .. }),
            "{name}: {err}"
        );
        assert_eq!(
            fs::metadata(&existing).expect("meta").permissions().mode() & 0o777,
            mode,
            "{name}: mode untouched"
        );
        set_mode(&existing, 0o600);
        assert_eq!(
            fs::read_to_string(&existing).expect("read"),
            contents,
            "{name}"
        );
        cleanup(&cwd);
    }
    cleanup(&sdir);
}

// ---------------------------------------------------------------------------------------------
// Adapter: refuses to start
// ---------------------------------------------------------------------------------------------

fn config(cwd: &Path, program: &Path) -> SessionConfig {
    SessionConfig {
        cwd: cwd.to_path_buf(),
        model: None,
        effort: None,
        permissions: PermissionPolicy::Ask,
        gate: Arc::new(AllowGate),
        mcp_servers: Vec::new(),
        resume: None,
        env: Vec::new(),
        program: Some(program.to_path_buf()),
    }
}

fn write_script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}")).expect("write script");
    set_mode(path, 0o755);
}

fn adapter_in(base: &Path) -> AgyAdapter {
    let mut adapter = AgyAdapter::new();
    adapter.session_base_dir = base.join("sessions");
    adapter.helper_bin = Some(hook_bin());
    adapter.shutdown_grace = Duration::from_secs(2);
    adapter
}

fn start_error(result: Result<atrium_agents::Session, AdapterError>) -> String {
    match result {
        Ok(_) => panic!("start must refuse"),
        Err(e) => {
            assert!(matches!(e, AdapterError::Handshake(_)), "got {e:?}");
            e.to_string()
        }
    }
}

#[tokio::test]
async fn adapter_refuses_to_start_when_the_cwd_already_has_a_hook_config() {
    for (name, contents, mode) in [
        (
            "other valid hook",
            json!({"x":{"PreToolUse":[]}}).to_string(),
            0o600,
        ),
        ("malformed", "{ malformed json ".to_owned(), 0o600),
        ("mode 0000", "{}".to_owned(), 0o000),
    ] {
        let base = temp_dir("refuse-cwd");
        let cwd = base.join("work");
        let agents = cwd.join(agents_dir_name());
        fs::create_dir_all(&agents).expect("mkdir");
        let existing = agents.join("hooks.json");
        fs::write(&existing, &contents).expect("write");
        set_mode(&existing, mode);

        let program = base.join("fake-agy.sh");
        let marker = base.join("spawned");
        write_script(&program, &format!("touch '{}'\n", marker.display()));

        let adapter = adapter_in(&base);
        let msg = start_error(
            adapter
                .start(config(&cwd, &program), Transport::Local)
                .await,
        );
        assert!(msg.contains("already exists"), "{name}: {msg}");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!marker.exists(), "{name}: agy must not have been spawned");
        let leftover = fs::read_dir(base.join("sessions")).map_or(0, Iterator::count);
        assert_eq!(leftover, 0, "{name}: session dir must be cleaned up");
        set_mode(&existing, 0o600);
        assert_eq!(
            fs::read_to_string(&existing).expect("read"),
            contents,
            "{name}"
        );
        cleanup(&base);
    }
}

#[tokio::test]
async fn adapter_refuses_to_start_when_the_helper_is_missing() {
    let base = temp_dir("refuse-helper");
    let cwd = base.join("work");
    fs::create_dir_all(&cwd).expect("mkdir");
    let program = base.join("fake-agy.sh");
    let marker = base.join("spawned");
    write_script(&program, &format!("touch '{}'\n", marker.display()));

    let mut adapter = adapter_in(&base);
    adapter.helper_bin = Some(base.join("no-such-helper"));
    let msg = start_error(
        adapter
            .start(config(&cwd, &program), Transport::Local)
            .await,
    );
    assert!(msg.contains("helper"), "{msg}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!marker.exists(), "agy must not have been spawned");
    assert!(
        !cwd.join(agents_dir_name()).exists(),
        "the cwd hook config must be removed again"
    );
    cleanup(&base);
}

// ---------------------------------------------------------------------------------------------
// Adapter: runtime tripwire
// ---------------------------------------------------------------------------------------------

const INIT: &str =
    r#"{"event":"init","conversation_id":"c-1","init":{"model":"m","tools":["run_command"]}}"#;
const TOOL_ACTIVE: &str = r#"{"event":"step_update","step_update":{"conversation_id":"c-1","step_index":1,"state":"ACTIVE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"touch ran.txt"}}}}"#;
const TOOL_DONE: &str = r#"{"event":"step_update","step_update":{"conversation_id":"c-1","step_index":1,"state":"DONE","step_type":"tool","tool_name":"run_command","tool_info":{"name":"run_command","parameters":{"CommandLine":"touch ran.txt"},"output":"ok"}}}"#;
const RESULT: &str = r#"{"event":"result","result":{"conversation_id":"c-1","status":"SUCCESS","response":"done","num_turns":1,"usage":{"input_tokens":1,"output_tokens":1}}}"#;

/// Collects events until `SessionEnded` (or 10 s).
async fn run_session(
    adapter: &AgyAdapter,
    cwd: &Path,
    program: &Path,
) -> (Vec<AgentEvent>, Duration) {
    let started = Instant::now();
    let mut session = adapter
        .start(config(cwd, program), Transport::Local)
        .await
        .expect("start");
    session
        .commands
        .send(SessionCommand::SendUserMessage {
            text: "go".to_owned(),
        })
        .await
        .expect("send");
    let mut events = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(env) = session.events.recv().await {
            let ended = matches!(env.event, AgentEvent::SessionEnded { .. });
            events.push(env.event);
            if ended {
                break;
            }
        }
    })
    .await;
    (events, started.elapsed())
}

fn fatal_error(events: &[AgentEvent]) -> Option<&str> {
    events.iter().find_map(|e| match e {
        AgentEvent::Error {
            message,
            fatal: true,
        } => Some(message.as_str()),
        _ => None,
    })
}

#[tokio::test]
async fn tripwire_aborts_when_a_tool_runs_without_a_hook_request() {
    let base = temp_dir("trip-ungated");
    let cwd = base.join("work");
    fs::create_dir_all(&cwd).expect("mkdir");
    let program = base.join("fake-agy.sh");
    // A fail-open agy: runs the tool without ever calling the hook, then carries on.
    write_script(
        &program,
        &format!(
            "read -r line\nprintf '%s\\n' '{INIT}'\nprintf '%s\\n' '{TOOL_ACTIVE}'\ntouch ran-ungated.txt\nprintf '%s\\n' '{TOOL_DONE}'\nsleep 30\n"
        ),
    );

    let (events, elapsed) = run_session(&adapter_in(&base), &cwd, &program).await;
    let msg = fatal_error(&events).expect("a fatal Error must be emitted");
    assert!(msg.contains("gate bypass suspected"), "{msg}");
    assert!(
        msg.contains("without a preceding allowed hook request"),
        "{msg}"
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::SessionEnded { .. })
    ));
    assert!(
        elapsed < Duration::from_secs(8),
        "the child must be killed at once, not left to run for 30 s (took {elapsed:?})"
    );
    // The tripwire cannot stop the first ungated call (agy already ran it); it stops the rest.
    assert!(cwd.join("ran-ungated.txt").exists());
    assert!(
        !cwd.join(agents_dir_name()).exists(),
        "cwd hook config removed after the abort"
    );
    cleanup(&base);
}

#[tokio::test]
async fn tripwire_aborts_when_the_hook_config_is_corrupted_mid_session() {
    for (name, corrupt) in [
        ("mode 0000", "chmod 000"),
        ("malformed", "printf '{ nope' >"),
        ("deleted", "rm -f"),
    ] {
        let base = temp_dir("trip-corrupt");
        let cwd = base.join("work");
        fs::create_dir_all(&cwd).expect("mkdir");
        let program = base.join("fake-agy.sh");
        let hooks = format!("{}/hooks.json", agents_dir_name());
        write_script(
            &program,
            &format!(
                "read -r line\nprintf '%s\\n' '{INIT}'\n{corrupt} '{hooks}'\nprintf '%s\\n' '{TOOL_ACTIVE}'\nsleep 30\n"
            ),
        );
        let (events, elapsed) = run_session(&adapter_in(&base), &cwd, &program).await;
        let msg = fatal_error(&events).unwrap_or_else(|| panic!("{name}: fatal Error expected"));
        assert!(msg.contains("hook config no longer valid"), "{name}: {msg}");
        assert!(elapsed < Duration::from_secs(8), "{name}: took {elapsed:?}");
        assert!(matches!(
            events.last(),
            Some(AgentEvent::SessionEnded { .. })
        ));
        cleanup(&base);
    }
}

#[tokio::test]
async fn gated_tool_calls_do_not_trip_the_wire() {
    let base = temp_dir("trip-control");
    let cwd = base.join("work");
    fs::create_dir_all(&cwd).expect("mkdir");
    let program = base.join("fake-agy.sh");
    // A well-behaved agy: asks the real helper (over the adapter's socket) before running.
    write_script(
        &program,
        &format!(
            "read -r line\nprintf '%s\\n' '{INIT}'\nprintf '%s\\n' '{TOOL_ACTIVE}'\nprintf '%s' '{{\"toolCall\":{{\"name\":\"run_command\",\"args\":{{\"CommandLine\":\"touch ran.txt\"}}}}}}' | '{}' --socket \"$ATRIUM_AGY_GATE_SOCKET\" > hook-reply.txt\ntouch ran.txt\nprintf '%s\\n' '{TOOL_DONE}'\nprintf '%s\\n' '{RESULT}'\n",
            hook_bin().display()
        ),
    );
    let (events, _) = run_session(&adapter_in(&base), &cwd, &program).await;
    assert!(
        fatal_error(&events).is_none(),
        "unexpected fatal error: {events:?}"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::ToolCallFinished {
            status: ToolStatus::Succeeded,
            ..
        }
    )));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnCompleted { .. }))
    );
    let reply = fs::read_to_string(cwd.join("hook-reply.txt")).expect("hook reply");
    assert!(reply.contains("\"allow\""), "{reply}");
    cleanup(&base);
}

#[tokio::test]
async fn coverage_check_can_be_disabled_explicitly() {
    let base = temp_dir("trip-disabled");
    let cwd = base.join("work");
    fs::create_dir_all(&cwd).expect("mkdir");
    let program = base.join("fake-agy.sh");
    write_script(
        &program,
        &format!(
            "read -r line\nprintf '%s\\n' '{INIT}'\nprintf '%s\\n' '{TOOL_ACTIVE}'\nprintf '%s\\n' '{TOOL_DONE}'\nprintf '%s\\n' '{RESULT}'\n"
        ),
    );
    let mut adapter = adapter_in(&base);
    adapter.enforce_gate_coverage = false;
    let (events, _) = run_session(&adapter, &cwd, &program).await;
    assert!(fatal_error(&events).is_none(), "{events:?}");
    cleanup(&base);
}
