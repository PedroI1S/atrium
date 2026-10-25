//! Synthetic child: startup barrier ensures identity is durable before emitting.
use spike_daemon_socket::{agent::*, now_ms, wire::AgentLaunch};
use std::{
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};

struct Output {
    seq: u64,
    out: io::BufWriter<io::Stdout>,
    text: String,
    turn: u32,
}
impl Output {
    fn emit(&mut self, event: AgentEvent) -> io::Result<()> {
        serde_json::to_writer(
            &mut self.out,
            &AgentEventEnvelope {
                seq: self.seq,
                at_ms: now_ms(),
                event,
            },
        )?;
        self.out.write_all(b"\n")?;
        self.seq += 1;
        self.out.flush()
    }
    fn finish(&mut self, outcome: TurnOutcome) -> io::Result<()> {
        let text = std::mem::take(&mut self.text);
        self.emit(AgentEvent::MessageCompleted {
            message_id: self.turn.to_string(),
            kind: MessageKind::Assistant,
            text,
        })?;
        self.emit(AgentEvent::TurnCompleted {
            turn: self.turn,
            outcome,
            usage: None,
        })
    }
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let launch: AgentLaunch =
        serde_json::from_str(&std::env::args().nth(1).ok_or("missing launch")?)?;
    let AgentLaunch::Fake {
        tokens_per_second,
        chunk_chars,
        duration_ms,
        tool_call_every_ms,
    } = launch
    else {
        return Err("Fake only".into());
    };
    if chunk_chars == 0 || chunk_chars > 1_000_000 || tokens_per_second == 0 {
        return Err("invalid fake rate/chunk size".into());
    }
    let (tx, mut input) = tokio::sync::mpsc::channel(16);
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            if tx.blocking_send(line).is_err() {
                break;
            }
        }
    });
    if input.recv().await.transpose()?.as_deref() != Some("go") {
        return Err("missing startup barrier".into());
    }
    let mut out = Output {
        seq: 0,
        out: io::BufWriter::new(io::stdout()),
        text: String::new(),
        turn: 1,
    };
    out.emit(AgentEvent::SessionStarted {
        provider: Provider::Codex,
        native_session_id: format!("fake-{}", std::process::id()),
        model: Some("synthetic".into()),
        tools: vec!["fake_read".into()],
    })?;
    out.emit(AgentEvent::TurnStarted { turn: 1 })?;
    let period = Duration::from_secs_f64(
        (chunk_chars as f64 / (tokens_per_second as f64 * 4.)).max(0.000_001),
    );
    let mut tick = tokio::time::interval(period);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut start = Instant::now();
    let mut tool_at = start;
    let mut active = true;
    let mut input_open = true;
    loop {
        tokio::select! {
            line = input.recv(), if input_open => {
                let Some(line) = line.transpose()? else { input_open = false; continue; };
                let command: SessionCommand = serde_json::from_str(&line)?;
                match command {
                    SessionCommand::Shutdown => { if active { out.finish(TurnOutcome::Interrupted)?; } break; }
                    SessionCommand::Interrupt => { if active { out.finish(TurnOutcome::Interrupted)?; active = false; } }
                    SessionCommand::SendUserMessage { .. } if !active => { out.turn += 1; out.text.clear(); out.emit(AgentEvent::TurnStarted { turn: out.turn })?; start = Instant::now(); tool_at = start; active = true; }
                    _ => out.emit(AgentEvent::Error { message: "unsupported command or turn busy".into(), fatal: false })?
                }
            }
            _ = tick.tick() => {
                if !active { continue; }
                if start.elapsed() >= Duration::from_millis(duration_ms) { out.finish(TurnOutcome::Completed)?; break; }
                let text = "x".repeat(chunk_chars as usize); out.text.push_str(&text);
                out.emit(AgentEvent::MessageDelta { message_id: out.turn.to_string(), kind: MessageKind::Assistant, text })?;
                if tool_call_every_ms != 0 && tool_at.elapsed() >= Duration::from_millis(tool_call_every_ms) {
                    let id = format!("tool-{}", out.seq); tool_at = Instant::now();
                    out.emit(AgentEvent::ToolCallStarted { call_id: id.clone(), name: "fake_read".into(), kind: ToolKind::Read, title: Some("synthetic read".into()), input: serde_json::json!({}) })?;
                    out.emit(AgentEvent::ToolCallFinished { call_id: id, status: ToolStatus::Succeeded, output: Some("synthetic".into()), error: None })?;
                }
            }
        }
    }
    out.emit(AgentEvent::SessionEnded {
        exit_code: Some(0),
        reason: Some("fake finished".into()),
    })?;
    Ok(())
}
