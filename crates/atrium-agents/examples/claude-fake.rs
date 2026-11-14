//! A stand-in for the `claude` CLI, for adapter tests that must run offline.
//! Behavior is chosen with `FAKE_MODE`; files go to `FAKE_DIR`.
//!
//! `--version` prints a version (`FAKE_VERSION`, default 2.1.308) and exits.
//!
//! Modes: `echo` (answers and finishes the turn), `flood` (streams text
//! forever until interrupted), `hook` (asks the `PreToolUse` hook about a
//! Read and records the answer), `stuck` (starts a turn and never ends it,
//! ignoring interrupts), `exit` (exits at once), `hook-malformed` and
//! `hook-badcb` (bad `hook_callback` requests), `hook-bigargs` (a hook call
//! whose tool arguments exceed the cap), `canuse`, `canuse-noid` and
//! `canuse-question` (a `can_use_tool` request no hook preceded), and
//! `grandchild` (starts a long-lived child and ignores stdin EOF), and the
//! review round-1 modes: `no-init` (never answers `initialize`),
//! `canuse-cancel` / `canuse-result` (a `can_use_tool` request withdrawn, or
//! left open when the turn ends), `hook-result` (the same for a hook call),
//! `canuse-bigargs` / `canuse-bigargs-b` (a `can_use_tool` with oversize
//! arguments, before / after the `tool_use` record), `hook-ask-changed` (the
//! `can_use_tool` that follows an `ask` has another input), `hook-ask-anon2`
//! (two identical hook calls without ids, then two `can_use_tool`) and
//! `eof-flood` (an open turn, then thousands of records after stdin closes),
//! and the round-2 mode `id-clash` (a hook call that reuses the request id of
//! an open `can_use_tool`, after the file `go_on` appears; every answer is
//! logged to `responses.jsonl`).

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
    if std::env::args().nth(1).as_deref() == Some("--version") {
        let v = std::env::var("FAKE_VERSION").unwrap_or_else(|_| "2.1.308".into());
        println!("{v} (Claude Code)");
        return;
    }
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
    if std::env::var("FAKE_GRANDCHILD").is_ok()
        && let Ok(child) = std::process::Command::new("sleep").arg("600").spawn()
    {
        fs::write(dir.join("grandchild.pid"), child.id().to_string()).ok();
    }
    if mode == "exit" {
        eprintln!("fake claude: refusing to start");
        std::process::exit(3);
    }
    let session_id = "fake-session-1";
    let flooding = Arc::new(AtomicBool::new(false));
    let mut asked = 0usize;
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
                        if mode == "no-init" {
                            continue;
                        }
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
                if mode == "id-clash" {
                    // Round 2 mode: every answer, in order.
                    append(&dir, "responses.jsonl", &line);
                    continue;
                }
                if mode == "hook-ask-anon2" && matches!(id, Some("h1" | "h2")) {
                    let which = id.unwrap_or_default();
                    fs::write(dir.join(format!("hook_response_{which}.json")), &line).ok();
                    asked += 1;
                    if asked == 2 {
                        for rid in ["c1", "c2"] {
                            emit(
                                &json!({"type":"control_request","request_id":rid,"request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"ls"}}}),
                            );
                        }
                    }
                } else if id == Some("h1") && (mode == "hook-ask" || mode == "hook-ask-changed") {
                    fs::write(dir.join("hook_response.json"), &line).ok();
                    let decision = v
                        .pointer("/response/response/hookSpecificOutput/permissionDecision")
                        .and_then(Value::as_str);
                    if decision == Some("ask") {
                        emit(
                            &json!({"type":"control_request","request_id":"c1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"toolu_1","input":{"command": if mode == "hook-ask-changed" { "rm -rf x" } else { "echo hi" }},"decision_reason":"asked"}}),
                        );
                    } else {
                        emit(&assistant_text("denied"));
                        emit(&result("success", false));
                    }
                } else if id == Some("c1") {
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
                    "hook-bigargs" => {
                        // Arguments past MAX_PARTIAL_ARGS_BYTES, in a record
                        // under MAX_PROVIDER_RECORD_BYTES.
                        let input = json!({"command":"x".repeat(4 * 1024 * 1024 + 16)});
                        emit(
                            &json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":input}]},"session_id":session_id}),
                        );
                        emit(
                            &json!({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":input,"tool_use_id":"toolu_1"}}}),
                        );
                    }
                    "hook-result" => {
                        emit(
                            &json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"tool_use","id":"toolu_1","name":"Read","input":{"file_path":"x"}}]},"session_id":session_id}),
                        );
                        emit(
                            &json!({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"x"},"tool_use_id":"toolu_1"}}}),
                        );
                        // Let the adapter hand the call to the gate first.
                        thread::sleep(Duration::from_millis(300));
                        emit(&result("success", false));
                    }
                    "hook-ask-anon2" => {
                        for rid in ["h1", "h2"] {
                            emit(
                                &json!({"type":"control_request","request_id":rid,"request":{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"ls"}}}}),
                            );
                        }
                    }
                    "canuse-cancel" | "canuse-result" => {
                        emit(
                            &json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]},"session_id":session_id}),
                        );
                        emit(
                            &json!({"type":"control_request","request_id":"c1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"toolu_1","input":{"command":"ls"}}}),
                        );
                        if mode == "canuse-cancel" {
                            // Let the adapter hand the call to the gate first.
                            thread::sleep(Duration::from_millis(300));
                            emit(&json!({"type":"control_cancel_request","request_id":"c1"}));
                            emit(&assistant_text("after-cancel"));
                        } else {
                            thread::sleep(Duration::from_millis(300));
                            emit(&result("success", false));
                        }
                    }
                    "id-clash" => {
                        emit(
                            &json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]},"session_id":session_id}),
                        );
                        emit(
                            &json!({"type":"control_request","request_id":"r1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"toolu_1","input":{"command":"ls"}}}),
                        );
                        // The test says when to go on (the stdin loop waits).
                        let gate = dir.join("go_on");
                        for _ in 0..400 {
                            if gate.exists() {
                                break;
                            }
                            thread::sleep(Duration::from_millis(25));
                        }
                        // A hook call that uses the same request id.
                        emit(
                            &json!({"type":"control_request","request_id":"r1","request":{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{"hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"x"},"tool_use_id":"toolu_2"}}}),
                        );
                    }
                    "canuse-bigargs" | "canuse-bigargs-b" => {
                        let input = json!({"command":"x".repeat(4 * 1024 * 1024 + 16)});
                        let tool_use = json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":input}]},"session_id":session_id});
                        let request = json!({"type":"control_request","request_id":"c1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"toolu_1","input":input}});
                        if mode == "canuse-bigargs" {
                            emit(&request);
                            emit(&tool_use);
                        } else {
                            emit(&tool_use);
                            emit(&request);
                        }
                    }
                    "eof-flood" => {
                        emit(
                            &json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"par"}},"session_id":session_id,"api_message_id":"msg_fake"}),
                        );
                        emit(
                            &json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_t","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]},"session_id":session_id}),
                        );
                        emit(
                            &json!({"type":"control_request","request_id":"c1","request":{"subtype":"can_use_tool","tool_name":"Bash","tool_use_id":"toolu_1","input":{"command":"ls"}}}),
                        );
                    }
                    "hook-ask" | "hook-ask-changed" => {
                        let input = json!({"command":"echo hi"});
                        emit(
                            &json!({"type":"assistant","message":{"model":"claude-fake","id":"msg_fake","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":input}]},"session_id":session_id}),
                        );
                        emit(
                            &json!({"type":"control_request","request_id":"h1","request":{"subtype":"hook_callback","callback_id":"atrium-pre-tool-use","input":{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":input,"tool_use_id":"toolu_1"}}}),
                        );
                    }
                    "oversize" => {
                        let big = "x".repeat(9 * 1024 * 1024);
                        emit(&json!({"type":"system","subtype":"filler","pad":big}));
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
    if mode == "eof-flood" {
        // stdin closed: the soft stop. Keep talking, then leave.
        for _ in 0..6000 {
            emit(
                &json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}},"session_id":session_id,"api_message_id":"msg_fake"}),
            );
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
