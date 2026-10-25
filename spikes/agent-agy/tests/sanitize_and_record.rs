//! Fixture sanitizing of temp-dir paths, and the recorder's loud failures, tested offline
//! against a fake `agy`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use atrium_agents::{AdapterInput, GateDecision, ToolCheck};
use atrium_protocol::agent::{SessionCommand, ToolKind};
use serde_json::json;
use spike_agent_agy::bridge::create_private_session_dir;
use spike_agent_agy::{FixtureSanitizer, leaked_temp_path};

fn temp_dir(label: &str) -> PathBuf {
    create_private_session_dir(&std::env::temp_dir(), &format!("atrium-agy-san-{label}"))
        .expect("create private dir")
}

/// Every spelling of the scratch dir that appeared in the live fixtures, built from a
/// made-up per-user temp id so the test never depends on this machine's layout.
const FAKE_ROOT: &str = "/var/folders/zz/abcdefghijklmnopqrstuvwxyz01/T";

fn sanitizer(scratch: &Path) -> FixtureSanitizer {
    FixtureSanitizer::new("/Users/someone", "someone", &["s3cr3t-value"], &[scratch])
}

#[test]
fn maps_every_spelling_of_the_scratch_dir_to_scratch() {
    let scratch = PathBuf::from(format!("{FAKE_ROOT}/atrium-agy-record-scratch/simple-1f-1"));
    let s = sanitizer(&scratch);
    let plain = scratch.display().to_string();
    let private = format!("/private{plain}");
    let cases = [
        (plain.clone(), "/scratch"),
        (private.clone(), "/scratch"),
        (format!("{plain}/hello.txt"), "/scratch/hello.txt"),
        (format!("{private}/sub/dir/a.rs"), "/scratch/sub/dir/a.rs"),
        (
            format!("file://{plain}/hello.txt"),
            "file:///scratch/hello.txt",
        ),
        (format!("file://{private}"), "file:///scratch"),
        (
            format!("cwd={private}, again {plain}."),
            "cwd=/scratch, again /scratch.",
        ),
        (format!("cd {plain} && ls"), "cd /scratch && ls"),
        (format!("\"{private}\""), "\"/scratch\""),
    ];
    for (input, want) in cases {
        let got = s.text(&input);
        assert_eq!(got, want, "input: {input}");
        assert!(s.is_clean(&got), "{got}");
        assert!(
            !s.is_clean(&input),
            "the unsanitized form must not count as clean: {input}"
        );
    }
}

#[test]
fn maps_other_temp_roots_whoever_they_belong_to() {
    let s = sanitizer(Path::new("/nonexistent/scratch"));
    // The scratch dir of another machine (never seen by this sanitizer) and the temp root itself.
    for input in [
        format!("{FAKE_ROOT}/other/thing"),
        format!("/private{FAKE_ROOT}/other/thing"),
        format!("file://{FAKE_ROOT}/other/thing"),
    ] {
        let got = s.text(&input);
        assert!(!got.contains("/var/folders"), "{input} -> {got}");
        assert!(!got.contains("/private/var"), "{input} -> {got}");
        assert!(got.contains("/scratch-tmp/other/thing"), "{input} -> {got}");
        assert!(leaked_temp_path(&got).is_none(), "{got}");
    }
    // Different ids and root letters (C is the cache dir, 0 the legacy one).
    for root in [
        "/var/folders/ab/c_d0e1/T",
        "/var/folders/qq/zzzz9999/C",
        "/var/folders/k4/x/0",
    ] {
        let got = s.text(&format!("{root}/x/y"));
        assert_eq!(got, "/scratch-tmp/x/y", "{root}");
    }
}

#[test]
fn leaves_unrelated_paths_alone() {
    let scratch = PathBuf::from(format!("{FAKE_ROOT}/atrium-agy-record-scratch/simple-1f-1"));
    let s = sanitizer(&scratch);
    for input in [
        "/usr/local/bin/agy",
        "/Users/someone-else/project/src/main.rs", // not the sanitizer's user path boundary
        "/scratch/already",
        "~/scratch/hello.txt",
        "/opt/var/folders-not/here",
    ] {
        let got = s.map_paths(input);
        assert_eq!(got, input, "unchanged");
    }
    // A sibling directory that merely shares the scratch dir's name as a prefix is not it.
    let sibling = format!("{}-extra", scratch.display());
    assert!(!s.map_paths(&sibling).contains("/scratch-extra"));
}

#[test]
fn real_scratch_dir_and_its_realpath_are_both_mapped() {
    // On macOS the temp dir is /var/folders/... and canonicalizes to /private/var/folders/...
    let base = temp_dir("real");
    let scratch = base.join("scratch");
    fs::create_dir_all(&scratch).expect("mkdir");
    let real = fs::canonicalize(&scratch).expect("canonicalize");
    let s = sanitizer(&scratch);
    for p in [scratch.clone(), real.clone()] {
        let got = s.text(&format!("{}/hello.txt", p.display()));
        assert_eq!(got, "/scratch/hello.txt", "{}", p.display());
    }
    // The temp root itself and the session dir under it.
    let tmp = std::env::temp_dir();
    let got = s.text(&format!(
        "{}/agy/s-1-1/g.sock",
        tmp.display().to_string().trim_end_matches('/')
    ));
    assert!(leaked_temp_path(&got).is_none(), "{got}");
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn sanitizes_every_adapter_input_kind() {
    let scratch = PathBuf::from(format!(
        "{FAKE_ROOT}/atrium-agy-record-scratch/gate-deny-1f-1"
    ));
    let s = sanitizer(&scratch);
    let p = format!("/private{}", scratch.display());
    let stdout = json!({
        "event": "init",
        "init": {"cwd": p, "workspaces": [format!("file://{p}")], "n": 3}
    })
    .to_string();
    let mut inputs = vec![
        AdapterInput::Stdout { line: stdout },
        AdapterInput::Stderr {
            line: format!("warning: cache at {p}/.cache and token s3cr3t-value"),
        },
        AdapterInput::Stdin {
            line: json!({"event":"user","message":{"content": format!("read {p}/secret.txt")}})
                .to_string(),
        },
        AdapterInput::HookRequest {
            id: "req-1".into(),
            payload: json!({"toolCall":{"name":"read_file","args":{"Path": format!("{p}/secret.txt")}},"workspacePaths":[p.clone()]}),
        },
        AdapterInput::HookResponse {
            id: "req-1".into(),
            payload: json!({"decision":"deny","reason": format!("{p}/secret.txt is off limits")}),
        },
        AdapterInput::Gate {
            check: ToolCheck {
                call_id: Some("step-2".into()),
                name: "read_file".into(),
                kind: ToolKind::Read,
                title: Some(format!("{p}/secret.txt")),
                input: json!({"Path": format!("{p}/secret.txt")}),
            },
            decision: GateDecision::Deny {
                reason: format!("denied: {p}/secret.txt"),
            },
        },
        AdapterInput::Command {
            command: SessionCommand::SendUserMessage {
                text: format!("look in {p}"),
            },
        },
        AdapterInput::Exit { code: Some(0) },
    ];
    for input in &mut inputs {
        s.input(input);
        let text = serde_json::to_string(input).expect("serialize");
        assert!(s.is_clean(&text), "not clean: {text}");
        assert!(!text.contains("/var/folders"), "{text}");
        assert!(!text.contains("/private"), "{text}");
        assert!(!text.contains("s3cr3t-value"), "{text}");
    }
    let hook = serde_json::to_string(&inputs[3]).expect("serialize");
    assert!(hook.contains("/scratch/secret.txt"), "{hook}");
    let out = serde_json::to_string(&inputs[0]).expect("serialize");
    assert!(out.contains("file:///scratch"), "{out}");
}

#[test]
fn is_clean_fails_when_a_temp_path_survives() {
    let s = sanitizer(Path::new("/nonexistent/scratch"));
    for dirty in [
        format!("cwd {FAKE_ROOT}/x"),
        format!("cwd /private{FAKE_ROOT}/x"),
        format!("file://{FAKE_ROOT}/x"),
        "/private/var/db/whatever".to_owned(),
        "/private/tmp/agy".to_owned(),
        format!(
            "{}/agy",
            std::env::temp_dir()
                .display()
                .to_string()
                .trim_end_matches('/')
        ),
    ] {
        assert!(!s.is_clean(&dirty), "must not be clean: {dirty}");
        assert!(
            leaked_temp_path(&dirty).is_some(),
            "must be detected: {dirty}"
        );
    }
    // The same text after mapping is clean.
    let fixed = s.text(&format!("cwd {FAKE_ROOT}/x"));
    assert!(s.is_clean(&fixed), "{fixed}");
}

// ---------------------------------------------------------------------------------------------
// Recorder against a fake agy
// ---------------------------------------------------------------------------------------------

fn record_exe() -> PathBuf {
    let helper = PathBuf::from(env!("CARGO_BIN_EXE_spike-agy-hook"));
    let exe = helper
        .parent()
        .expect("target dir")
        .join("examples")
        .join("agy-record");
    if !exe.exists() {
        let status = std::process::Command::new(env!("CARGO"))
            .args([
                "build",
                "-q",
                "-p",
                "spike-agent-agy",
                "--example",
                "agy-record",
            ])
            .status()
            .expect("cargo build --example agy-record");
        assert!(status.success(), "building the record example failed");
    }
    assert!(exe.exists(), "record example missing at {}", exe.display());
    exe
}

fn run_record(fake_agy_body: &str, scenario: &str, label: &str) -> (Output, PathBuf, PathBuf) {
    let dir = temp_dir(label);
    let fake = dir.join("fake-agy.sh");
    fs::write(&fake, format!("#!/bin/sh\n{fake_agy_body}")).expect("write fake agy");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
    let fixtures = dir.join("fixtures");
    fs::create_dir_all(&fixtures).expect("mkdir fixtures");
    let out = std::process::Command::new(record_exe())
        .arg("--binary")
        .arg(&fake)
        .arg("--fixtures-dir")
        .arg(&fixtures)
        .arg(scenario)
        .output()
        .expect("run record");
    (out, fixtures, dir)
}

const FAKE_SIMPLE: &str = r#"read -r line
real=$(pwd -P)
printf '{"event":"init","conversation_id":"c-1","init":{"model":"m","cwd":"%s","url":"file://%s","tools":["run_command"]}}\n' "$real" "$real"
printf '%s\n' "warning: scratch is $PWD and tmp is ${TMPDIR:-/tmp}" >&2
printf '{"event":"step_update","step_update":{"conversation_id":"c-1","step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"PONG"}}\n'
printf '{"event":"result","result":{"conversation_id":"c-1","status":"SUCCESS","response":"PONG","num_turns":1,"usage":{"input_tokens":3,"output_tokens":1}}}\n'
sleep 1
"#;

#[test]
fn recorder_writes_fixtures_without_any_temp_dir_path() {
    let (out, fixtures, dir) = run_record(FAKE_SIMPLE, "simple", "rec-ok");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "record failed: {stderr}");

    let mut checked = 0;
    for name in ["simple.jsonl", "simple.events.json", "simple.md"] {
        let text = fs::read_to_string(fixtures.join(name)).expect("fixture file written");
        assert!(
            leaked_temp_path(&text).is_none(),
            "{name} leaks a temp path: {text}"
        );
        assert!(!text.contains("/private"), "{name}");
        assert!(!text.contains("/var/folders"), "{name}");
        checked += 1;
    }
    assert_eq!(checked, 3);
    let jsonl = fs::read_to_string(fixtures.join("simple.jsonl")).expect("jsonl");
    assert!(
        jsonl.contains("\\\"cwd\\\":\\\"/scratch\\\""),
        "cwd must be /scratch: {jsonl}"
    );
    assert!(
        jsonl.contains("file:///scratch"),
        "file URL must be mapped: {jsonl}"
    );
    let md = fs::read_to_string(fixtures.join("simple.md")).expect("md");
    assert!(md.contains("Status: `live`"), "{md}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn recorder_fails_loudly_when_the_scenario_expectations_are_not_met() {
    // `question` needs a QuestionAsked answered by the harness; a fake that just says PONG
    // must make the recorder exit non-zero, name what is missing and write nothing.
    let (out, fixtures, dir) = run_record(FAKE_SIMPLE, "question", "rec-question");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must fail: {stderr}");
    assert!(stderr.contains("did not meet its expectations"), "{stderr}");
    assert!(stderr.contains("exactly one QuestionAsked"), "{stderr}");
    assert!(!fixtures.join("question.jsonl").exists());
    assert!(!fixtures.join("question.events.json").exists());
    assert!(!fixtures.join("question.md").exists());

    // Same for `multi-turn` (one turn only) and `interrupt` (no tool call to interrupt).
    for (scenario, needle) in [
        ("multi-turn", "two completed turns"),
        ("interrupt", "a tool call started before the interrupt"),
        ("approval-allow", "an ApprovalRequested"),
        ("resume", "COBALT-42"),
    ] {
        let (out, fixtures, dir2) = run_record(FAKE_SIMPLE, scenario, "rec-missing");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{scenario} must fail: {stderr}");
        assert!(stderr.contains(needle), "{scenario}: {stderr}");
        assert!(
            !fixtures.join(format!("{scenario}.jsonl")).exists(),
            "{scenario}"
        );
        let _ = fs::remove_dir_all(&dir2);
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn recorder_refuses_a_gate_deny_recording_that_contains_the_secret() {
    // A fake agy that reads secret.txt and echoes it: the gate did not hold. The secret is a
    // sanitizer secret, so a check done after sanitizing would never see it.
    let body = r#"read -r line
printf '{"event":"init","conversation_id":"c-1","init":{"model":"m","tools":["read_file"]}}\n'
printf '{"event":"step_update","step_update":{"conversation_id":"c-1","step_index":1,"state":"DONE","step_type":"agent_response","text_delta":"The file says: %s"}}\n' "$(cat secret.txt)"
printf '{"event":"result","result":{"conversation_id":"c-1","status":"SUCCESS","response":"x","num_turns":1}}\n'
sleep 1
"#;
    let (out, fixtures, dir) = run_record(body, "gate-deny", "rec-secret");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "must fail: {stderr}");
    assert!(
        stderr.contains("the secret value is in the raw recording"),
        "{stderr}"
    );
    assert!(!fixtures.join("gate-deny.jsonl").exists());
    let _ = fs::remove_dir_all(&dir);
}
