//! Runs the real `record` example against a scripted fake `agy` for every scenario, offline.
//!
//! This proves the recorder's control flow (resolving approvals, answering questions, the
//! second message of `multi-turn`, the seed session of `resume`, interrupting mid-tool-call,
//! the `startup-error` launch) and that what it writes passes the golden check. It does not
//! prove that the real `agy` behaves like the fake: that is what the live run is for.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use atrium_agents::fixture::assert_golden;
use spike_agent_agy::bridge::create_private_session_dir;
use spike_agent_agy::{AgyNormalizer, leaked_temp_path};

const FAKE_AGY: &str = r#"#!/bin/sh
model=""; conv=""
while [ $# -gt 0 ]; do
  case "$1" in
    --model) model="$2"; shift ;;
    --conversation) conv="$2"; shift ;;
  esac
  shift
done
case "$model" in
  invalid-model*) echo "Error: model '$model' not found" >&2; exit 2 ;;
esac
cid="${conv:-conv-$$}"
real=$(pwd -P)

emit_init() {
  printf '{"event":"init","conversation_id":"%s","init":{"model":"m","cwd":"%s","tools":["ask_question","read_file","run_command"]}}\n' "$cid" "$real"
}
emit_agent() {
  printf '{"event":"step_update","step_update":{"conversation_id":"%s","step_index":%s,"state":"DONE","step_type":"agent_response","text_delta":"%s"}}\n' "$cid" "$1" "$2"
}
emit_result() {
  printf '{"event":"result","result":{"conversation_id":"%s","status":"SUCCESS","response":"%s","num_turns":1,"usage":{"input_tokens":3,"output_tokens":1}}}\n' "$cid" "$1"
}
emit_tool_active() {
  printf '{"event":"step_update","step_update":{"conversation_id":"%s","step_index":2,"state":"ACTIVE","step_type":"tool","tool_name":"%s","tool_info":{"name":"%s","parameters":%s}}}\n' "$cid" "$1" "$1" "$2"
}
ask_hook() {
  printf '{"toolCall":{"name":"%s","args":%s},"workspacePaths":["%s"]}' "$1" "$2" "$real" | "$HOOK_BIN" --socket "$ATRIUM_AGY_GATE_SOCKET"
}
run_tool() {
  emit_tool_active "$1" "$2"
  reply=$(ask_hook "$1" "$2")
  case "$reply" in
    *'"decision":"allow"'*)
      printf '{"event":"step_update","step_update":{"conversation_id":"%s","step_index":2,"state":"DONE","step_type":"tool","tool_name":"%s","tool_info":{"name":"%s","parameters":%s,"output":"%s"}}}\n' "$cid" "$1" "$1" "$2" "$3" ;;
    *)
      printf '{"event":"step_update","step_update":{"conversation_id":"%s","step_index":2,"state":"ERROR","step_type":"tool","tool_name":"%s","tool_info":{"name":"%s","parameters":%s,"error":{"type":"TOOL_ERROR","message":"tool call denied by pre-tool hook"}}}}\n' "$cid" "$1" "$1" "$2" ;;
  esac
}

emit_init
read -r line
case "$FAKE_SCENARIO" in
  simple) emit_agent 1 PONG; emit_result PONG ;;
  tool-read)
    run_tool read_file '{"Path":"hello.txt"}' "1: Hello from Atrium tool-read scenario."
    emit_agent 3 "The file says: Hello from Atrium tool-read scenario."; emit_result done ;;
  gate-deny)
    run_tool read_file '{"Path":"secret.txt"}' "unused"
    emit_agent 3 "I was not allowed to read it."; emit_result done ;;
  approval-allow|approval-deny)
    run_tool run_command '{"CommandLine":"echo approved-ok"}' "approved-ok"
    emit_agent 3 "Finished."; emit_result done ;;
  question)
    run_tool ask_question '{"questions":[{"question":"Which build profile?","options":["Debug","Release"],"is_multi_select":false}]}' "answered"
    emit_agent 3 "Using the chosen profile."; emit_result done ;;
  interrupt)
    emit_tool_active run_command '{"CommandLine":"sleep 10"}'
    ask_hook run_command '{"CommandLine":"sleep 10"}' >/dev/null
    exec sleep 30 ;;
  multi-turn)
    n=0
    while :; do
      n=$((n + 1))
      case "$line" in *TURN_TWO_OK*) t=TURN_TWO_OK ;; *) t=TURN_ONE_OK ;; esac
      emit_agent $((n * 10)) "$t"; emit_result "$t"
      read -r line || break
    done ;;
  resume)
    if [ -z "$conv" ]; then
      printf '%s' "$cid" > .seed-conv
      emit_agent 1 OK; emit_result OK
    else
      [ "$conv" = "$(cat .seed-conv)" ] || { echo "unknown conversation $conv" >&2; exit 2; }
      emit_agent 1 "The codeword is COBALT-42."; emit_result done
    fi ;;
esac
"#;

const SCENARIOS: &[&str] = &[
    "simple",
    "tool-read",
    "gate-deny",
    "approval-allow",
    "approval-deny",
    "question",
    "interrupt",
    "multi-turn",
    "resume",
    "startup-error",
];

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
    exe
}

#[test]
fn recorder_records_every_scenario_against_a_scripted_agy() {
    let dir = create_private_session_dir(&std::env::temp_dir(), "atrium-agy-scen")
        .expect("create private dir");
    let fake = dir.join("fake-agy.sh");
    fs::write(&fake, FAKE_AGY).expect("write fake agy");
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("chmod");
    let fixtures = dir.join("fixtures");
    fs::create_dir_all(&fixtures).expect("mkdir fixtures");
    let record = record_exe();

    for scenario in SCENARIOS {
        let out = std::process::Command::new(&record)
            .env("FAKE_SCENARIO", scenario)
            .env("HOOK_BIN", env!("CARGO_BIN_EXE_spike-agy-hook"))
            .arg("--binary")
            .arg(&fake)
            .arg("--fixtures-dir")
            .arg(&fixtures)
            .arg(scenario)
            .output()
            .expect("run record");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{scenario}: record failed:\n{stderr}");

        let jsonl = fixtures.join(format!("{scenario}.jsonl"));
        assert!(jsonl.exists(), "{scenario}: no fixture written");
        let text = fs::read_to_string(&jsonl).expect("read");
        assert!(
            leaked_temp_path(&text).is_none(),
            "{scenario}: temp path in fixture"
        );
        assert!(
            !text.contains("TOP_SECRET_VALUE_994812_DO_NOT_LEAK"),
            "{scenario}: secret in fixture"
        );
        // The written events file must be exactly what the normalizer makes of the written lines.
        assert_golden(&jsonl, &mut AgyNormalizer::default());
        let md = fs::read_to_string(fixtures.join(format!("{scenario}.md"))).expect("md");
        assert!(md.contains("Status: `live`"), "{scenario}: {md}");
    }

    // Spot-check what distinguishes the scenarios.
    let read = |name: &str| fs::read_to_string(fixtures.join(name)).expect("read fixture");
    assert!(
        read("multi-turn.jsonl")
            .matches("\"dir\":\"stdin\"")
            .count()
            >= 2
    );
    assert!(read("resume.jsonl").contains("COBALT-42"));
    assert!(read("approval-deny.events.json").contains("\"denied\""));
    assert!(read("interrupt.events.json").contains("\"interrupted\""));
    assert!(read("startup-error.events.json").contains("\"fatal\": true"));
    assert!(read("gate-deny.jsonl").contains("\"/scratch"));

    let _ = fs::remove_dir_all(&dir);
}
