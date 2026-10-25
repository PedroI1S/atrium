use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use atrium_agents::fixture::FixtureLine;
use atrium_agents::*;
use atrium_protocol::agent::*;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio::time::{MissedTickBehavior, interval, timeout};

use crate::CodexNormalizer;
use crate::normalize::id;

/// Opt-in raw transcript; sanitize before persisting. Never enabled by default.
pub type Recording = Arc<Mutex<Vec<FixtureLine>>>;
#[derive(Default)]
pub struct CodexAdapter {
    pub recording: Option<Recording>,
    /// Invocation-local configuration, used by the fixture recorder. Empty for
    /// normal sessions; does not modify the user's configuration or auth store.
    pub config_overrides: Vec<String>,
}

/// Process-local MCP overrides. Names and values are quoted as TOML strings.
pub fn process_args(config: &SessionConfig) -> Vec<String> {
    let mut args = vec!["app-server".into(), "--listen".into(), "stdio://".into()];
    for server in &config.mcp_servers {
        let root = format!("mcp_servers.{}", json!(server.name));
        for (key, value) in [
            ("command", json!(server.command)),
            ("args", json!(server.args)),
        ] {
            args.extend(["-c".into(), format!("{root}.{key}={value}")]);
        }
        for (key, value) in &server.env {
            args.extend([
                "-c".into(),
                format!("{root}.env.{}={}", json!(key), json!(value)),
            ]);
        }
    }
    args
}
pub fn thread_params(config: &SessionConfig) -> Value {
    let (sandbox, approval) = match config.permissions {
        PermissionPolicy::ReadOnly => ("read-only", "untrusted"),
        PermissionPolicy::Ask => ("workspace-write", "untrusted"),
        PermissionPolicy::FullAccess => ("danger-full-access", "never"),
    };
    let mut p = json!({"cwd":config.cwd,"model":config.model,"sandbox":sandbox,"approvalPolicy":approval,"approvalsReviewer":"user"});
    if let Some(native) = &config.resume {
        p["threadId"] = json!(native);
    }
    p
}

struct State {
    normalizer: CodexNormalizer,
    pending: VecDeque<AgentEventEnvelope>,
    recording: Option<Recording>,
    started: Instant,
    seq: u64,
    in_flight_writes: usize,
}
impl State {
    fn input(&mut self, input: AdapterInput) {
        if let Some(recording) = &self.recording {
            recording.lock().expect("recording lock").push(FixtureLine {
                ms: self.started.elapsed().as_millis() as u64,
                input: input.clone(),
            });
        }
        for event in self.normalizer.on_input(&input) {
            self.pending.push_back(AgentEventEnvelope {
                seq: self.seq,
                at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
                event,
            });
            self.seq += 1;
        }
    }
    fn failure(&mut self, message: String) {
        self.input(AdapterInput::Stderr {
            line: format!("adapter transport error: {message}"),
        });
    }
}
async fn write(stdin: &mut ChildStdin, state: &mut State, v: Value) -> Result<(), AdapterError> {
    let line = v.to_string();
    stdin.write_all(format!("{line}\n").as_bytes()).await?;
    state.input(AdapterInput::Stdin { line });
    Ok(())
}
async fn handshake(
    stdin: &mut ChildStdin,
    stdout: &mut Lines<BufReader<ChildStdout>>,
    stderr: &mut Lines<BufReader<ChildStderr>>,
    state: &mut State,
    config: &SessionConfig,
) -> Result<(), AdapterError> {
    write(stdin,state,json!({"id":"atrium-init","method":"initialize","params":{"clientInfo":{"name":"atrium_m0","title":"Atrium M0","version":"0.0.0"},"capabilities":{"experimentalApi":true}}})).await?;
    let mut err_open = true;
    loop {
        tokio::select! {
            line=stderr.next_line(),if err_open=> match line? {Some(line)=>state.input(AdapterInput::Stderr {line}),None=>err_open=false},
            line=stdout.next_line()=> {
                let line=line?.ok_or_else(||AdapterError::Handshake("stdout closed before thread creation".into()))?;
                let v:Value=serde_json::from_str(&line).map_err(|e|AdapterError::Protocol(e.to_string()))?;
                state.input(AdapterInput::Stdout {line});
                if v.get("error").is_some() {return Err(AdapterError::Handshake(v["error"].to_string()));}
                if v["id"]=="atrium-init" {
                    write(stdin,state,json!({"method":"initialized"})).await?;
                    write(stdin,state,json!({"id":"atrium-thread","method":if config.resume.is_some(){"thread/resume"}else{"thread/start"},"params":thread_params(config)})).await?;
                }
                if v["id"]=="atrium-thread" {
                    if state.normalizer.native_id.is_none() {return Err(AdapterError::Handshake("thread response has no native id".into()));}
                    return Ok(());
                }
                // No server request is authorized during initialization.
                if v.get("id").is_some() && v.get("method").is_some() {
                    write(stdin,state,json!({"id":v["id"],"error":{"code":-32601,"message":"unsupported startup request"}})).await?;
                }
            }
        }
    }
}

impl AgentAdapter for CodexAdapter {
    fn provider(&self) -> Provider {
        Provider::Codex
    }
    fn interception(&self) -> Interception {
        Interception::ProviderGated
    }
    fn start(
        &self,
        config: SessionConfig,
        _transport: Transport,
    ) -> BoxFuture<'_, Result<Session, AdapterError>> {
        Box::pin(async move {
            let started = Instant::now();
            let mut child = Command::new(
                config
                    .program
                    .as_deref()
                    .unwrap_or_else(|| std::path::Path::new("codex")),
            )
            .args(process_args(&config))
            .args(
                self.config_overrides
                    .iter()
                    .flat_map(|value| ["-c", value.as_str()]),
            )
            .current_dir(&config.cwd)
            .envs(config.env.clone())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
            let mut stdin = child.stdin.take().expect("piped stdin");
            let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
            let mut stderr = BufReader::new(child.stderr.take().expect("piped stderr")).lines();
            let mut state = State {
                normalizer: CodexNormalizer::default(),
                pending: VecDeque::new(),
                recording: self.recording.clone(),
                started,
                seq: 0,
                in_flight_writes: 0,
            };
            let result = timeout(
                STARTUP_TIMEOUT,
                handshake(&mut stdin, &mut stdout, &mut stderr, &mut state, &config),
            )
            .await;
            if !matches!(result, Ok(Ok(()))) {
                let _ = child.kill().await;
                let _ = child.wait().await;
                state.input(AdapterInput::Exit {
                    code: child.try_wait().ok().flatten().and_then(|s| s.code()),
                });
                return Err(match result {
                    Ok(Err(e)) => e,
                    _ => AdapterError::Handshake("startup timeout".into()),
                });
            }
            let (events_tx, events) = mpsc::channel(EVENT_BUFFER);
            let (commands, commands_rx) = mpsc::channel(COMMAND_BUFFER);
            tokio::spawn(run(
                child,
                stdin,
                stdout,
                stderr,
                state,
                config,
                events_tx,
                commands_rx,
            ));
            Ok(Session { events, commands })
        })
    }
}

// Writes are isolated so a provider that stops reading stdin cannot block control commands.
async fn writer(
    mut stdin: ChildStdin,
    mut rx: mpsc::Receiver<Value>,
    ack: mpsc::Sender<Result<String, String>>,
) {
    while let Some(v) = rx.recv().await {
        let line = v.to_string();
        let result = stdin
            .write_all(format!("{line}\n").as_bytes())
            .await
            .map(|()| line)
            .map_err(|e| e.to_string());
        let failed = result.is_err();
        if ack.send(result).await.is_err() || failed {
            break;
        }
    }
}
fn send(tx: &mpsc::Sender<Value>, v: Value, state: &mut State) -> bool {
    if tx.try_send(v).is_err() {
        state.failure("provider input queue exhausted or closed".into());
        false
    } else {
        state.in_flight_writes += 1;
        true
    }
}
fn start_turn(
    tx: &mpsc::Sender<Value>,
    text: String,
    state: &mut State,
    effort: &Option<String>,
    counter: &mut u64,
) -> bool {
    *counter += 1;
    send(
        tx,
        json!({"id":format!("atrium-turn-{counter}"),"method":"turn/start","params":{"threadId":state.normalizer.native_id,"input":[{"type":"text","text":text}],"effort":effort}}),
        state,
    )
}

#[allow(clippy::too_many_arguments)]
async fn run(
    mut child: Child,
    stdin: ChildStdin,
    mut stdout: Lines<BufReader<ChildStdout>>,
    mut stderr: Lines<BufReader<ChildStderr>>,
    mut state: State,
    config: SessionConfig,
    events: mpsc::Sender<AgentEventEnvelope>,
    mut commands: mpsc::Receiver<SessionCommand>,
) {
    let (writes, write_rx) = mpsc::channel(COMMAND_BUFFER);
    let (acks, mut ack_rx) = mpsc::channel(COMMAND_BUFFER);
    let writer = tokio::spawn(writer(stdin, write_rx, acks));
    let mut gates: JoinSet<GateDecision> = JoinSet::new();
    let mut gate_checks: BTreeMap<tokio::task::Id, (String, ToolCheck)> = BTreeMap::new();
    let mut request_ids = BTreeMap::<String, Value>::new();
    let mut queued = VecDeque::new();
    let mut starting = false;
    let mut interrupt_pending = false;
    let mut counter = 0;
    let mut shutting: Option<Instant> = None;
    let mut out_open = true;
    let mut err_open = true;
    let mut tick = interval(std::time::Duration::from_millis(10));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let exit_code = loop {
        // Bound errors from a client that keeps submitting commands while its
        // event receiver is stalled. Preserve the lifecycle slots on failure.
        if shutting.is_none()
            && !state.pending.is_empty()
            && events.capacity() <= state.pending.len() + state.normalizer.closing_slots() + 2
        {
            state.failure("event backlog exhausted by client commands".into());
            shutting = Some(Instant::now());
        }
        tokio::select! {
            cmd=commands.recv(),if shutting.is_none()=> {
                let cmd=cmd.unwrap_or(SessionCommand::Shutdown);
                // Validate against state before recording (the normalizer also validates).
                let approval=if let SessionCommand::ResolveApproval {request_id,..}=&cmd {state.normalizer.is_pending(request_id,false)}else{false};
                let answer=if let SessionCommand::AnswerQuestion {request_id,answers}=&cmd {if state.normalizer.is_pending(request_id,true){state.normalizer.answer_labels(request_id,answers)}else{None}}else{None};
                state.input(AdapterInput::Command {command:cmd.clone()});
                match cmd {
                    SessionCommand::Shutdown=>{shutting=Some(Instant::now());queued.clear();gates.abort_all();gate_checks.clear();writer.abort();}
                    SessionCommand::Interrupt=> {
                        queued.clear();gates.abort_all();gate_checks.clear();
                        interrupt_pending=starting;
                        if let Some(turn)=state.normalizer.turn_id.clone() {
                            counter+=1;
                            if !send(&writes,json!({"id":format!("atrium-interrupt-{counter}"),"method":"turn/interrupt","params":{"threadId":state.normalizer.native_id,"turnId":turn}}),&mut state){shutting=Some(Instant::now());}
                        }
                    }
                    SessionCommand::SendUserMessage {text}=> {
                        if starting || state.normalizer.turn_id.is_some() {
                            if queued.len()<MAX_QUEUED_MESSAGES {queued.push_back(text);}
                        }else{starting=start_turn(&writes,text,&mut state,&config.effort,&mut counter);}
                    }
                    SessionCommand::ResolveApproval {request_id,decision} if approval=> {
                        let value=if matches!(decision,ApprovalDecision::Allow){"accept"}else{"decline"};
                        if let Some(native)=request_ids.get(&request_id) {
                            let v=json!({"id":native,"result":{"decision":value}});
                            send(&writes,v,&mut state);
                        }
                    }
                    SessionCommand::AnswerQuestion {request_id,..}=>if let Some(result)=answer && let Some(native)=request_ids.get(&request_id) {
                        let v=json!({"id":native,"result":result});
                        send(&writes,v,&mut state);
                    },
                    _=>{}
                }
            }
            _=events.closed(),if shutting.is_none() && commands.is_closed()=> {shutting=Some(Instant::now());gates.abort_all();gate_checks.clear();writer.abort();}
            result=ack_rx.recv(),if state.in_flight_writes>0=>match result {
                Some(Ok(line))=>{state.in_flight_writes-=1;state.input(AdapterInput::Stdin {line});},
                Some(Err(e))=>{state.in_flight_writes=0;state.failure(e);shutting=Some(Instant::now());},None=>{state.in_flight_writes=0;}
            },
            result=gates.join_next_with_id(),if !gates.is_empty()=>if let Some(result)=result {
                let (task_id,decision)=match result {
                    Ok((task_id,decision))=>(task_id,decision),
                    Err(e)=>(e.id(),GateDecision::Deny {reason:"gate panicked or was cancelled".into()}),
                };
                if let Some((request_id,check))=gate_checks.remove(&task_id) && state.normalizer.request_check(&request_id).is_some() {
                        state.input(AdapterInput::Gate {check,decision:decision.clone()});
                        if !matches!(decision,GateDecision::Ask) {
                            let result=json!({"decision":if matches!(decision,GateDecision::Allow){"accept"}else{"decline"}});
                            if let Some(native)=request_ids.get(&request_id) {send(&writes,json!({"id":native,"result":result}),&mut state);}
                        }
                }
            },
            permit=events.reserve(),if !state.pending.is_empty()=> {
                if let Ok(permit)=permit {permit.send(state.pending.pop_front().expect("pending event"));}
                else {state.pending.clear();}
            }
            line=stdout.next_line(),if out_open && state.in_flight_writes==0 && state.pending.is_empty() && events.capacity()>state.normalizer.closing_slots()+4=>match line {
                Ok(Some(line))=> {
                    let v=serde_json::from_str::<Value>(&line).unwrap_or(Value::Null);
                    let method=v["method"].as_str().unwrap_or_default().to_owned();
                    let rid=id(&v["id"]);
                    state.input(AdapterInput::Stdout {line});
                    if method=="turn/started" {
                        starting=false;
                        if interrupt_pending {
                            interrupt_pending=false;counter+=1;
                            send(&writes,json!({"id":format!("atrium-interrupt-{counter}"),"method":"turn/interrupt","params":{"threadId":state.normalizer.native_id,"turnId":state.normalizer.turn_id}}),&mut state);
                        }
                    }
                    if method=="turn/completed" || (v.get("error").is_some() && rid.starts_with("atrium-turn-")) {
                        starting=false;
                        if let Some(text)=queued.pop_front() {starting=start_turn(&writes,text,&mut state,&config.effort,&mut counter);}
                    }
                    if method=="serverRequest/resolved" {request_ids.remove(&id(&v["params"]["requestId"]));}
                    if v.get("id").is_some() && v.get("method").is_some() {
                        request_ids.insert(rid.clone(),v["id"].clone());
                        if let Some(check)=state.normalizer.request_check(&rid) {
                            let gate=config.gate.clone();
                            let gate_check=check.clone();
                            let task=gates.spawn(async move {
                                timeout(SHUTDOWN_GRACE,gate.check(&gate_check)).await.unwrap_or_else(|_|GateDecision::Deny {reason:"gate timeout".into()})
                            });
                            gate_checks.insert(task.id(),(rid,check));
                        }else if method!="item/tool/requestUserInput" {
                            send(&writes,json!({"id":v["id"],"error":{"code":-32601,"message":"unsupported server request; denied"}}),&mut state);
                            request_ids.remove(&rid);
                        }
                    }
                }
                Ok(None)=>{out_open=false;if shutting.is_none(){shutting=Some(Instant::now());writer.abort();}},
                Err(e)=>{state.failure(e.to_string());out_open=false;shutting=Some(Instant::now());writer.abort();}
            },
            line=stderr.next_line(),if err_open && state.pending.is_empty() && events.capacity()>state.normalizer.closing_slots()+4=>match line {Ok(Some(line))=>state.input(AdapterInput::Stderr {line}),_=>err_open=false},
            _=tick.tick()=> {
                if let Some(started)=shutting {
                    writer.abort();gates.abort_all();gate_checks.clear();
                    // Leave time to reap before the contract's shutdown deadline.
                    if started.elapsed()>=SHUTDOWN_GRACE.saturating_sub(std::time::Duration::from_millis(100)) {let _=child.start_kill();}
                    if let Ok(Some(status))=child.try_wait(){break status.code();}
                }
            }
        }
    };
    writer.abort();
    let _ = writer.await;
    while let Ok(Ok(line)) = ack_rx.try_recv() {
        state.input(AdapterInput::Stdin { line });
    }
    gates.abort_all();
    gate_checks.clear();
    while gates.join_next().await.is_some() {}
    let _ = child.wait().await;
    state.input(AdapterInput::Exit { code: exit_code });
    // Normal output leaves capacity for these closing events, even under
    // saturation. An abandoned receiver simply discards its remaining events.
    while let Some(event) = state.pending.pop_front() {
        if events.try_send(event).is_err() {
            break;
        }
    }
}
