//! A stand-in for the `claude` CLI, for adapter tests that must run offline.
//! Behavior is chosen with `FAKE_MODE`; files go to `FAKE_DIR`.
//!
//! Modes: `echo` (answers and finishes the turn), `flood` (streams text
//! forever until interrupted), `hook` (asks the `PreToolUse` hook about a
//! Read and records the answer), `stuck` (starts a turn and never ends it,
//! ignoring interrupts), `exit` (exits at once), `hook-malformed` and
//! `hook-badcb` (bad `hook_callback` requests), `canuse`, `canuse-noid` and
//! `canuse-question` (a `can_use_tool` request no hook preceded), and
//! `grandchild` (starts a long-lived child and ignores stdin EOF).

use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

fn emit(v: &Value) {
    let mut out = std::io::stdout().lock();
    // A closed pipe means the adapter is gone.
    if writeln!(out, "{v}").is_err() || out.flush().is_err() {
        std::process::exit(0);
    }
}

fn append(dir: &std::path::Path, name: &str, text: &str) {
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))
    {
        let _ = writeln!(f, "{text}");
    }
}

fn main() {
    let mode = std::env::var("FAKE_MODE").unwrap_or_else(|_| "echo".into());
    let dir = PathBuf::from(std::env::var("FAKE_DIR").expect("FAKE_DIR"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    append(&dir, "pids.txt", &std::process::id().to_string());
    append(&dir, "argv.txt", &args.join(" "));
    if let Some(i) = args.iter().position(|a| a == "--mcp-config")
        && let Some(path) = args.get(i + 1)
        && let Ok(text) = fs::read_to_string(path)
    {
        let _ = fs::write(dir.join("mcp-copy.json"), text);
    }
    if std::env::var("FAKE_SLOW_RESUME").is_ok() && args.iter().any(|a| a == "--resume") {
        // A respawned process that takes its time to answer `initialize`.
        thread::sleep(Duration::from_secs(20));
    }
    if mode == "exit" {
        eprintln!("fake claude: refusing to start");
        std::process::exit(3);
    }
    let session_id = "fake-session-1";
    let flooding = Arc::new(AtomicBool::new(false));
    let stdin = std::io::stdin();
    for line in stdin.lock().lines().map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("control_request") => {
                let id = v["request_id"].clone();
                match v.pointer("/request/subtype").and_then(Value::as_str) {
                    Some("initialize") => {
                        append(&dir, "initialize.json", &line);
                        emit(
                            &json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{"commands":[]}}}),
                        );
                    }
                    Some("interrupt") => {
                        if mode == "stuck" {
                            continue;
                        }
                        fs::write(dir.join("interrupted"), "yes").ok();
                        flooding.store(false, Ordering::SeqCst);
                        emit(
                            &json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{"still_queued":[]}}}),
                        );
                        emit(&result("error_during_execution", true));
                    }
                    _ => {}
                }
            }
            Some("control_response") => {
                let id = v.pointer("/response/request_id").and_then(Value::as_str);
                if id == Some("c1") {
                    fs::write(dir.join("canuse_response.json"), line).ok();
                    emit(&assistant_text("done"));
                    emit(&result("success", false));
                } else if id == Some("h1") {
                    fs::write(dir.join("hook_response.json"), line).ok();
                    emit(
                        &json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"ok","is_error":false}]},"session_id":session_id}),
                    );
                    emit(&assistant_text("done"));
                    emit(&result("success", false));
                }
            }
            Some("user") => {
                emit(
                    &json!({"type":"system","subtype":"init","session_id":session_id,"model":"claude-fake","tools":["Read"],"cwd":"."}),
                );
                emit(
                    &json!({"type":"stream_event","event":{"type":"message_start","message":{"model":"claude-fake","id":"msg_fake"}},"session_id":session_id,"api_message_id":"msg_fake"}),
                );
                match mode.as_str() {
                    "echo" => {
                        emit(&assistant_text("hello"));
                        emit(&result("success", false));
                    }
                    "flood" => {
                        flooding.store(true, Ordering::SeqCst);
                        let flag = flooding.clone();
                        thread::spawn(move || {
                            while flag.load(Ordering::SeqCst) {
                                emit(
                                    &json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x".repeat(200)}},"session_id":session_id,"api_message_id":"msg_fake"}),
                                );
                            }
                        });
                    }
                    "hook" => {
                        emit(
                            &json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"x"}}]},"session_id":session_id}),
                        );
                        emit(
                            &json!({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"x"},"tool_use_id":"toolu_1"}}}),
                        );
                    }
                    "hook-malformed" => emit(
                        &json!({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{"hook_event_name":"PreToolUse"}}}),
                    ),
                    "hook-badcb" => emit(
                        &json!({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"someone-else","input":{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"x"},"tool_use_id":"toolu_1"}}}),
                    ),
                    "canuse" | "canuse-noid" => {
                        let mut req = json!({"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}});
                        if mode == "canuse" {
                            req["tool_use_id"] = json!("toolu_1");
                        }
                        emit(
                            &json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]},"session_id":session_id}),
                        );
                        emit(&json!({"type":"control_request","request_id":"c1","request":req}));
                    }
                    "canuse-question" => emit(
                        &json!({"type":"control_request","request_id":"c1","request":{"subtype":"can_use_tool","tool_name":"AskUserQuestion","tool_use_id":"toolu_q","input":{"questions":[{"question":"Q?","options":[{"label":"A"},{"label":"B"}]}]}}}),
                    ),
                    "grandchild" => {
                        if let Ok(child) = std::process::Command::new("sleep").arg("600").spawn() {
                            fs::write(dir.join("grandchild.pid"), child.id().to_string()).ok();
                        }
                    }
                    _ => {
                        // stuck: a turn that never ends.
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
            _ => {}
        }
    }
    if mode == "grandchild" {
        // Ignores EOF on stdin: only a kill ends this process.
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }
}

fn assistant_text(text: &str) -> Value {
    json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"text","text":text}]},"session_id":"fake-session-1"})
}

fn result(subtype: &str, is_error: bool) -> Value {
    json!({"type":"result","subtype":subtype,"is_error":is_error,"terminal_reason": if is_error {"aborted_streaming"} else {"completed"},"session_id":"fake-session-1","total_cost_usd":0.0,"usage":{"input_tokens":1,"output_tokens":1}})
}
