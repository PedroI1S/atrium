#![doc = "Offline stand-in for `codex app-server --listen stdio://` used by atrium-agents tests."]

use std::env;
use std::fs::{self, File};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

fn emit(tx: &mpsc::Sender<Value>, val: Value) {
    let _ = tx.send(val);
}

fn spawn_grandchild(dir: Option<&Path>) -> Option<Child> {
    let child = Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(d) = dir {
        let _ = fs::write(d.join("grandchild.pid"), format!("{}\n", child.id()));
    }
    Some(child)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--version") {
        if env::var("FAKE_VERSION_FAIL").as_deref() == Ok("1") {
            std::process::exit(1);
        }
        let ver = env::var("FAKE_VERSION").unwrap_or_else(|_| "codex-cli 0.163.0".to_owned());
        println!("{ver}");
        return;
    }

    let mode = env::var("FAKE_MODE").unwrap_or_else(|_| "simple".to_owned());
    let dir_str = env::var("FAKE_DIR").ok();
    let dir_path = dir_str.as_deref().map(Path::new);

    if let Ok(audit_path) = env::var("FAKE_AUDIT_FILE") {
        let payload = json!({
            "args": &args[1..],
            "cwd": env::current_dir().ok().map(|p| p.display().to_string()),
        });
        let _ = fs::write(audit_path, payload.to_string());
    }
    if let Some(d) = dir_path {
        let _ = fs::create_dir_all(d);
        let _ = fs::write(d.join("argv.txt"), format!("{}\n", args[1..].join("\n")));
        let _ = fs::write(d.join("pids.txt"), format!("{}\n", std::process::id()));
    }
    if let Ok(pid_file) = env::var("FAKE_PID_FILE") {
        let _ = fs::write(pid_file, format!("{}\n", std::process::id()));
    }

    let _grandchild = if env::var("FAKE_GRANDCHILD").as_deref() == Ok("1") || mode == "leader-exits"
    {
        spawn_grandchild(dir_path)
    } else {
        None
    };

    if mode == "leader-exits" {
        thread::sleep(Duration::from_millis(50));
        // Forget child handle so the grandchild survives leader exit in the same process group.
        std::mem::forget(_grandchild);
        std::process::exit(0);
    }

    if mode == "startup-fail" {
        eprintln!("fatal startup failure in fake codex");
        std::process::exit(2);
    }

    let (out_tx, out_rx) = mpsc::channel::<Value>();
    let writer_handle = thread::spawn(move || {
        let stdout = io::stdout();
        let mut lock = io::BufWriter::new(stdout.lock());
        for val in out_rx {
            if writeln!(lock, "{val}").is_err() || lock.flush().is_err() {
                break;
            }
        }
    });

    let interrupted = Arc::new(AtomicBool::new(false));
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let mut turn_num: u64 = 0;
    let mut unverified_replies: usize = 0;

    while let Some(Ok(line)) = lines.next() {
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(d) = dir_path
            && let Ok(mut f) = File::options()
                .create(true)
                .append(true)
                .open(d.join("responses.jsonl"))
        {
            let _ = writeln!(f, "{req}");
        }

        let id = req.get("id").cloned();
        let method = req
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match method {
            "initialize" => match mode.as_str() {
                "startup-hang" => {
                    thread::sleep(Duration::from_secs(60));
                    return;
                }
                "startup-rpc-error" => {
                    emit(
                        &out_tx,
                        json!({
                            "id": id,
                            "error": { "code": -32000, "message": "init rejected by fake" }
                        }),
                    );
                    return;
                }
                "startup-oversize" => {
                    let big = "a".repeat(9 * 1024 * 1024);
                    emit(&out_tx, json!({ "id": id, "result": { "blob": big } }));
                    return;
                }
                _ => {
                    emit(
                        &out_tx,
                        json!({
                            "id": id,
                            "result": {
                                "userAgent": "codex/0.163.0",
                                "codexHome": "/scratch/.codex"
                            }
                        }),
                    );
                }
            },
            "initialized" => {}
            "thread/start" | "thread/resume" => {
                let tid = req
                    .pointer("/params/threadId")
                    .and_then(Value::as_str)
                    .unwrap_or("thread-fake");
                emit(
                    &out_tx,
                    json!({
                        "id": id,
                        "result": {
                            "thread": {
                                "id": tid,
                                "cliVersion": "0.163.0"
                            }
                        }
                    }),
                );
            }
            "turn/start" => {
                turn_num += 1;
                let turn_id = format!("turn-{turn_num}");
                emit(
                    &out_tx,
                    json!({
                        "id": id,
                        "result": { "turn": { "id": turn_id, "items": [], "status": "inProgress" } }
                    }),
                );
                emit(
                    &out_tx,
                    json!({
                        "method": "turn/started",
                        "params": { "threadId": "thread-fake", "turn": { "id": turn_id, "status": "inProgress" } }
                    }),
                );

                match mode.as_str() {
                    "flood" => {
                        let count: usize = env::var("FAKE_FLOOD_COUNT")
                            .ok()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(6000);
                        let tx_flood = out_tx.clone();
                        let flag = interrupted.clone();
                        let tid_flood = turn_id.clone();
                        thread::spawn(move || {
                            emit(
                                &tx_flood,
                                json!({
                                    "method": "item/started",
                                    "params": {
                                        "threadId": "thread-fake",
                                        "turnId": tid_flood,
                                        "item": { "type": "agentMessage", "id": "msg-flood", "text": "" }
                                    }
                                }),
                            );
                            let chunk = "x".repeat(256);
                            for i in 0..count {
                                if flag.load(Ordering::Acquire) {
                                    return;
                                }
                                emit(
                                    &tx_flood,
                                    json!({
                                        "method": "item/agentMessage/delta",
                                        "params": {
                                            "threadId": "thread-fake",
                                            "turnId": tid_flood,
                                            "itemId": "msg-flood",
                                            "delta": format!("{i}:{chunk}")
                                        }
                                    }),
                                );
                            }
                        });
                    }
                    "delayed-turn" => {
                        let delay_ms: u64 = env::var("FAKE_TURN_DELAY_MS")
                            .ok()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(120);
                        let tx_delay = out_tx.clone();
                        let flag = interrupted.clone();
                        let tid = turn_id.clone();
                        let msg_id = format!("msg-{turn_num}");
                        let text = req
                            .pointer("/params/input/0/text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned();
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(delay_ms));
                            if flag.load(Ordering::Acquire) {
                                return;
                            }
                            emit_text_turn(&tx_delay, &tid, &msg_id, &format!("echo:{text}"));
                        });
                    }
                    "stuck" => {}
                    "gate" => {
                        let cmd =
                            env::var("FAKE_CMD").unwrap_or_else(|_| "cat public.txt".to_owned());
                        let call_id = format!("call-{turn_num}");
                        emit(
                            &out_tx,
                            json!({
                                "method": "item/started",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "item": {
                                        "type": "commandExecution",
                                        "id": call_id,
                                        "command": cmd,
                                        "cwd": "/scratch",
                                        "status": "inProgress",
                                        "commandActions": [{ "type": "read", "command": cmd, "path": "/scratch/public.txt" }]
                                    }
                                }
                            }),
                        );
                        emit(
                            &out_tx,
                            json!({
                                "id": format!("srv-cmd-{turn_num}"),
                                "method": "item/commandExecution/requestApproval",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "itemId": call_id,
                                    "command": cmd,
                                    "cwd": "/scratch"
                                }
                            }),
                        );
                    }
                    "mcp-and-patch" => {
                        emit(
                            &out_tx,
                            json!({
                                "method": "item/started",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "item": {
                                        "type": "fileChange",
                                        "id": "patch-1",
                                        "status": "inProgress",
                                        "changes": [{ "path": "/scratch/secret.txt", "kind": { "type": "update", "move_path": null }, "diff": "+x" }]
                                    }
                                }
                            }),
                        );
                        emit(
                            &out_tx,
                            json!({
                                "id": "srv-patch-1",
                                "method": "item/fileChange/requestApproval",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "itemId": "patch-1"
                                }
                            }),
                        );
                    }
                    "question" => {
                        emit(
                            &out_tx,
                            json!({
                                "id": "srv-q-1",
                                "method": "item/tool/requestUserInput",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "itemId": "q-item-1",
                                    "questions": [{
                                        "id": "color",
                                        "question": "Pick one",
                                        "options": [
                                            { "label": "Red", "description": "Warm" },
                                            { "label": "Blue", "description": "Cool" }
                                        ]
                                    }]
                                }
                            }),
                        );
                    }
                    "flood-ask" => {
                        for i in 0..70 {
                            let call_id = format!("flood-call-{i}");
                            emit(
                                &out_tx,
                                json!({
                                    "method": "item/started",
                                    "params": {
                                        "threadId": "thread-fake",
                                        "turnId": turn_id,
                                        "item": {
                                            "type": "commandExecution",
                                            "id": call_id,
                                            "command": "echo hi",
                                            "cwd": "/scratch",
                                            "status": "inProgress"
                                        }
                                    }
                                }),
                            );
                            emit(
                                &out_tx,
                                json!({
                                    "id": format!("srv-flood-{i}"),
                                    "method": "item/commandExecution/requestApproval",
                                    "params": {
                                        "threadId": "thread-fake",
                                        "turnId": turn_id,
                                        "itemId": call_id,
                                        "command": "echo hi",
                                        "cwd": "/scratch"
                                    }
                                }),
                            );
                        }
                        emit_text_turn(&out_tx, &turn_id, "msg-flood-ask", "done flood-ask");
                    }
                    "oversize-args" => {
                        let huge = "a".repeat(5 * 1024 * 1024);
                        emit(
                            &out_tx,
                            json!({
                                "id": "srv-huge",
                                "method": "item/commandExecution/requestApproval",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "itemId": "huge-call",
                                    "command": huge,
                                    "cwd": "/scratch"
                                }
                            }),
                        );
                    }
                    "oversize-midturn" => {
                        let huge = "b".repeat(9 * 1024 * 1024);
                        emit(
                            &out_tx,
                            json!({
                                "method": "item/agentMessage/delta",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "itemId": "msg-oversize",
                                    "delta": huge
                                }
                            }),
                        );
                    }
                    "unverified-approvals" => {
                        emit(
                            &out_tx,
                            json!({
                                "id": "srv-unv-exec",
                                "method": "execCommandApproval",
                                "params": {
                                    "conversationId": "thread-fake",
                                    "callId": "unv-exec-1",
                                    "command": ["bash", "-lc", "echo hi"],
                                    "cwd": "/scratch"
                                }
                            }),
                        );
                        emit(
                            &out_tx,
                            json!({
                                "id": "srv-unv-patch",
                                "method": "applyPatchApproval",
                                "params": {
                                    "conversationId": "thread-fake",
                                    "callId": "unv-patch-1",
                                    "fileChanges": { "/scratch/a.txt": { "type": "update" } }
                                }
                            }),
                        );
                        emit(
                            &out_tx,
                            json!({
                                "id": "srv-unv-perm",
                                "method": "item/permissions/requestApproval",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "itemId": "unv-perm-1",
                                    "permissions": {
                                        "fileSystem": { "write": ["/scratch"] },
                                        "network": { "enabled": true }
                                    }
                                }
                            }),
                        );
                        emit(
                            &out_tx,
                            json!({
                                "method": "item/started",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "item": {
                                        "type": "fileChange",
                                        "id": "unv-fc-1",
                                        "status": "inProgress",
                                        "changes": [{ "path": "/scratch/out.txt", "kind": { "type": "update", "move_path": null }, "diff": "+x" }]
                                    }
                                }
                            }),
                        );
                        emit(
                            &out_tx,
                            json!({
                                "id": "srv-unv-grantroot",
                                "method": "item/fileChange/requestApproval",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": turn_id,
                                    "itemId": "unv-fc-1",
                                    "grantRoot": "/"
                                }
                            }),
                        );
                    }
                    _ => {
                        let msg_id = format!("msg-{turn_num}");
                        emit_text_turn(&out_tx, &turn_id, &msg_id, "hello from fake codex");
                    }
                }
            }
            "turn/interrupt" => {
                if mode == "stuck" {
                    continue;
                }
                interrupted.store(true, Ordering::Release);
                if let Some(d) = dir_path {
                    let _ = fs::write(d.join("interrupted"), "1\n");
                }
                emit(&out_tx, json!({ "id": id, "result": {} }));
                emit(
                    &out_tx,
                    json!({
                        "method": "turn/completed",
                        "params": {
                            "threadId": "thread-fake",
                            "turn": { "id": format!("turn-{turn_num}"), "status": "interrupted" }
                        }
                    }),
                );
            }
            "" => {
                // JSON-RPC response from Atrium answering a server request.
                if let Some(req_id) = id.as_ref().and_then(Value::as_str) {
                    if req_id.starts_with("srv-cmd-") {
                        let decision = req
                            .pointer("/result/decision")
                            .and_then(Value::as_str)
                            .unwrap_or("decline");
                        let status = if decision == "accept" {
                            "completed"
                        } else {
                            "declined"
                        };
                        let cmd =
                            env::var("FAKE_CMD").unwrap_or_else(|_| "cat public.txt".to_owned());
                        let call_id = req_id.replace("srv-cmd-", "call-");
                        emit(
                            &out_tx,
                            json!({
                                "method": "serverRequest/resolved",
                                "params": { "threadId": "thread-fake", "requestId": req_id }
                            }),
                        );
                        emit(
                            &out_tx,
                            json!({
                                "method": "item/completed",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": format!("turn-{turn_num}"),
                                    "item": {
                                        "type": "commandExecution",
                                        "id": call_id,
                                        "command": cmd,
                                        "cwd": "/scratch",
                                        "status": status,
                                        "aggregatedOutput": if status == "completed" { "ok" } else { "" },
                                        "exitCode": if status == "completed" { 0 } else { 1 }
                                    }
                                }
                            }),
                        );
                        emit_text_turn(&out_tx, &format!("turn-{turn_num}"), "msg-gate", status);
                    } else if req_id == "srv-patch-1" {
                        let decision = req
                            .pointer("/result/decision")
                            .and_then(Value::as_str)
                            .unwrap_or("decline");
                        let status = if decision == "accept" {
                            "completed"
                        } else {
                            "declined"
                        };
                        emit(
                            &out_tx,
                            json!({
                                "method": "serverRequest/resolved",
                                "params": { "threadId": "thread-fake", "requestId": req_id }
                            }),
                        );
                        emit(
                            &out_tx,
                            json!({
                                "method": "item/completed",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": format!("turn-{turn_num}"),
                                    "item": {
                                        "type": "fileChange",
                                        "id": "patch-1",
                                        "status": status,
                                        "changes": [{ "path": "/scratch/secret.txt", "kind": { "type": "update", "move_path": null }, "diff": "+x" }]
                                    }
                                }
                            }),
                        );
                        emit_text_turn(&out_tx, &format!("turn-{turn_num}"), "msg-patch", status);
                    } else if req_id == "srv-huge" {
                        emit(
                            &out_tx,
                            json!({
                                "method": "item/completed",
                                "params": {
                                    "threadId": "thread-fake",
                                    "turnId": format!("turn-{turn_num}"),
                                    "item": {
                                        "type": "commandExecution",
                                        "id": "huge-call",
                                        "command": "echo declined",
                                        "status": "declined"
                                    }
                                }
                            }),
                        );
                        emit_text_turn(
                            &out_tx,
                            &format!("turn-{turn_num}"),
                            "msg-huge",
                            "declined",
                        );
                    } else if req_id == "srv-q-1" {
                        let answers = req.pointer("/result/answers").cloned().unwrap_or(json!({}));
                        emit(
                            &out_tx,
                            json!({
                                "method": "serverRequest/resolved",
                                "params": { "threadId": "thread-fake", "requestId": req_id }
                            }),
                        );
                        emit_text_turn(
                            &out_tx,
                            &format!("turn-{turn_num}"),
                            "msg-q",
                            &format!("answers:{answers}"),
                        );
                    } else if req_id.starts_with("srv-unv-") {
                        emit(
                            &out_tx,
                            json!({
                                "method": "serverRequest/resolved",
                                "params": { "threadId": "thread-fake", "requestId": req_id }
                            }),
                        );
                        unverified_replies += 1;
                        if unverified_replies == 4 {
                            emit(
                                &out_tx,
                                json!({
                                    "method": "item/completed",
                                    "params": {
                                        "threadId": "thread-fake",
                                        "turnId": format!("turn-{turn_num}"),
                                        "item": {
                                            "type": "fileChange",
                                            "id": "unv-fc-1",
                                            "status": "declined",
                                            "changes": [{ "path": "/scratch/out.txt", "kind": { "type": "update", "move_path": null }, "diff": "+x" }]
                                        }
                                    }
                                }),
                            );
                            emit_text_turn(
                                &out_tx,
                                &format!("turn-{turn_num}"),
                                "msg-unv",
                                "all unverified declined",
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if mode == "ignore-eof" || mode == "stuck" {
        loop {
            thread::sleep(Duration::from_secs(60));
        }
    }

    drop(out_tx);
    let _ = writer_handle.join();
}

fn emit_text_turn(tx: &mpsc::Sender<Value>, turn_id: &str, msg_id: &str, text: &str) {
    emit(
        tx,
        json!({
            "method": "item/started",
            "params": {
                "threadId": "thread-fake",
                "turnId": turn_id,
                "item": { "type": "agentMessage", "id": msg_id, "text": "" }
            }
        }),
    );
    emit(
        tx,
        json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-fake",
                "turnId": turn_id,
                "itemId": msg_id,
                "delta": text
            }
        }),
    );
    emit(
        tx,
        json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-fake",
                "turnId": turn_id,
                "item": { "type": "agentMessage", "id": msg_id, "text": text }
            }
        }),
    );
    emit(
        tx,
        json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "thread-fake",
                "turnId": turn_id,
                "tokenUsage": {
                    "total": { "inputTokens": 10, "cachedInputTokens": 0, "outputTokens": 5, "totalTokens": 15 },
                    "last": { "totalTokens": 15 },
                    "modelContextWindow": 200000
                }
            }
        }),
    );
    emit(
        tx,
        json!({
            "method": "turn/completed",
            "params": {
                "threadId": "thread-fake",
                "turn": { "id": turn_id, "status": "completed" }
            }
        }),
    );
}
