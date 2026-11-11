//! The synthetic agent behind `AgentLaunch::Fake`: a session with no
//! provider process, for transport, rendering and performance tests.
//!
//! It runs one turn when it starts and one for each `SendUserMessage`
//! after. A turn streams an assistant message of `chunk_chars`-character
//! deltas at `tokens_per_second` (1 token ≈ 4 characters) for `duration_ms`,
//! with a tool call every `tool_call_every_ms` (0: none). `Interrupt` ends
//! the turn early, `Shutdown` the session; both close what is open, in the
//! contract's order.

use std::collections::VecDeque;
use std::time::Duration;

use atrium_agents::{COMMAND_BUFFER, EVENT_BUFFER, MAX_QUEUED_MESSAGES, Session};
use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, MessageBody, MessageKind, Provider, SessionCommand, ToolKind,
    ToolStatus, TurnOutcome, Usage,
};
use atrium_protocol::limits::MAX_INLINE_MESSAGE_BYTES;
use atrium_protocol::policy::EnforcementDescriptor;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::now_ms;

/// Native session ids of synthetic sessions start with this; they can't be
/// resumed.
pub(crate) const NATIVE_ID_PREFIX: &str = "atrium-fake:";

/// The provider a synthetic thread is listed under (a thread needs one).
pub(crate) const PROVIDER: Provider = Provider::Claude;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Params {
    pub(crate) tokens_per_second: u32,
    pub(crate) chunk_chars: u32,
    pub(crate) duration_ms: u64,
    pub(crate) tool_call_every_ms: u64,
}

/// Nothing is intercepted because nothing runs: reported as it is, Partial.
fn descriptor() -> EnforcementDescriptor {
    EnforcementDescriptor::evaluate(PROVIDER, None, false, Vec::new(), Vec::new(), Vec::new())
}

pub(crate) fn start(params: Params, message: Option<String>) -> Session {
    let (events, receiver) = mpsc::channel(EVENT_BUFFER);
    let (commands, inbox) = mpsc::channel(COMMAND_BUFFER);
    let agent = Agent {
        params,
        events,
        commands: inbox,
        seq: 0,
        queue: VecDeque::from([message.unwrap_or_else(|| "Synthetic turn".into())]),
        interrupted: false,
        stopping: false,
    };
    tokio::spawn(agent.run());
    Session {
        descriptor: descriptor(),
        process: None,
        events: receiver,
        commands,
    }
}

/// The receiver is gone: nobody is left to tell.
struct Gone;

struct Agent {
    params: Params,
    events: mpsc::Sender<AgentEventEnvelope>,
    commands: mpsc::Receiver<SessionCommand>,
    seq: u64,
    /// User messages waiting for their turn.
    queue: VecDeque<String>,
    interrupted: bool,
    stopping: bool,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    out
}

/// The text cut to at most `max` bytes, on a character boundary.
fn cut(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

impl Agent {
    fn on_command(&mut self, command: Option<SessionCommand>) {
        match command {
            Some(SessionCommand::SendUserMessage { text }) => {
                if self.queue.len() < MAX_QUEUED_MESSAGES {
                    self.queue.push_back(text);
                }
            }
            Some(SessionCommand::Interrupt) => self.interrupted = true,
            // Both halves of the session were dropped, or it was told to end.
            Some(SessionCommand::Shutdown) | None => self.stopping = true,
            // It asks for nothing, so there is nothing to resolve or answer.
            Some(
                SessionCommand::ResolveApproval { .. } | SessionCommand::AnswerQuestion { .. },
            ) => {}
        }
    }

    /// Sends one event. A full event channel never stops commands from
    /// being taken: an interrupt or a shutdown gets through while this waits.
    async fn emit(&mut self, event: AgentEvent) -> Result<(), Gone> {
        let envelope = AgentEventEnvelope {
            incarnation: 0,
            seq: self.seq,
            at_ms: now_ms(),
            event,
        };
        self.seq += 1;
        loop {
            let command = tokio::select! {
                permit = self.events.reserve() => {
                    permit.map_err(|_| Gone)?.send(envelope);
                    return Ok(());
                }
                command = self.commands.recv(), if !self.stopping => command,
            };
            self.on_command(command);
        }
    }

    async fn run(mut self) {
        let _ = self.session().await;
    }

    async fn session(&mut self) -> Result<(), Gone> {
        self.emit(AgentEvent::SessionStarted {
            provider: PROVIDER,
            native_session_id: format!("{NATIVE_ID_PREFIX}{:x}", now_ms()),
            model: None,
            tools: vec!["Bash".into()],
            enforcement: descriptor(),
        })
        .await?;
        let mut turn = 0;
        while !self.stopping {
            match self.queue.pop_front() {
                Some(text) => {
                    turn += 1;
                    self.turn(turn, &text).await?;
                }
                None => {
                    let command = self.commands.recv().await;
                    self.on_command(command);
                }
            }
        }
        self.emit(AgentEvent::SessionEnded {
            exit_code: Some(0),
            reason: None,
        })
        .await
    }

    async fn turn(&mut self, turn: u32, text: &str) -> Result<(), Gone> {
        self.interrupted = false;
        self.emit(AgentEvent::TurnStarted { turn }).await?;
        self.emit(AgentEvent::MessageCompleted {
            message_id: format!("u{turn}"),
            kind: MessageKind::User,
            body: MessageBody::Inline {
                text: cut(text, MAX_INLINE_MESSAGE_BYTES).to_owned(),
            },
        })
        .await?;

        let Params {
            tokens_per_second,
            chunk_chars,
            duration_ms,
            tool_call_every_ms,
        } = self.params;
        let chunk: String = "synthetic tokens "
            .chars()
            .cycle()
            .take(chunk_chars.clamp(1, 64 * 1024) as usize)
            .collect();
        let chunk_len = chunk.len() as u64;
        let message_id = format!("a{turn}");
        let (mut hash, mut chars, mut deltas, mut tools) = (Sha256::new(), 0u64, 0u64, 0u64);
        let started = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_millis(5));
        loop {
            let elapsed = (started.elapsed().as_millis() as u64).min(duration_ms);
            let due = elapsed.saturating_mul(u64::from(tokens_per_second) * 4) / 1000;
            while chars + chunk_len <= due && !self.interrupted && !self.stopping {
                self.emit(AgentEvent::MessageDelta {
                    message_id: message_id.clone(),
                    kind: MessageKind::Assistant,
                    text: chunk.clone(),
                })
                .await?;
                hash.update(chunk.as_bytes());
                chars += chunk_len;
                deltas += 1;
            }
            while tool_call_every_ms > 0
                && (tools + 1) * tool_call_every_ms <= elapsed
                && !self.interrupted
                && !self.stopping
            {
                tools += 1;
                let call_id = format!("c{turn}-{tools}");
                self.emit(AgentEvent::ToolCallStarted {
                    call_id: call_id.clone(),
                    name: "Bash".into(),
                    kind: ToolKind::Execute,
                    title: Some("echo synthetic".into()),
                    input: json!({ "command": "echo synthetic" }),
                })
                .await?;
                self.emit(AgentEvent::ToolCallFinished {
                    call_id,
                    status: ToolStatus::Succeeded,
                    output: Some("synthetic\n".into()),
                    error: None,
                    truncated_bytes: 0,
                })
                .await?;
            }
            if elapsed >= duration_ms || self.interrupted || self.stopping {
                break;
            }
            tokio::select! {
                _ = tick.tick() => {}
                command = self.commands.recv() => self.on_command(command),
            }
        }
        if deltas > 0 {
            self.emit(AgentEvent::MessageCompleted {
                message_id,
                kind: MessageKind::Assistant,
                body: MessageBody::Streamed {
                    chars,
                    deltas,
                    sha256: hex(&hash.finalize()),
                },
            })
            .await?;
        }
        let outcome = if self.interrupted || self.stopping {
            TurnOutcome::Interrupted
        } else {
            TurnOutcome::Completed
        };
        self.emit(AgentEvent::TurnCompleted {
            turn,
            outcome,
            usage: Some(Usage {
                output_tokens: Some(chars / 4),
                ..Usage::default()
            }),
        })
        .await
    }
}
