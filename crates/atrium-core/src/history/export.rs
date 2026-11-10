//! Writing one thread as Markdown or JSONL (contract §13).
//!
//! Both formats are written from the same **folded** event stream, because a
//! secret can be split across the pieces a stream arrives in (must-fix A12):
//!
//! - A message's `MessageDelta`s are joined, checked against its
//!   `MessageCompleted` ([`super::reconstruct_message`]'s rules) and replaced
//!   by one `MessageCompleted` whose body is `Inline` with the whole text.
//! - A tool call's streamed output (`ToolCallUpdated::output_delta` and
//!   `output`) is collected and written once, in `ToolCallFinished::output`.
//!   A `ToolCallUpdated` is kept only for what else it changes.
//! - What is still open when its session ends, or when the events run out
//!   (the thread was exported while running), is written then: one
//!   `MessageDelta` with the text so far, one `ToolCallUpdated` with the
//!   output so far.
//!
//! Redaction then runs on whole values, never on a fragment.
//!
//! # JSONL
//!
//! One folded event per line, in the wire shape of
//! [`StoredEvent`](atrium_protocol::wire::StoredEvent), every string redacted.
//! A line keeps the id, sequence and time of its event (for an item that was
//! still open, those of its latest piece). Lines are in event order, except
//! that an item which was still open is written where it was closed off. An
//! `Inline` body here can be larger than the limit adapters keep to.
//!
//! # Markdown
//!
//! A transcript for people: a heading per session, the messages, each tool
//! call with its input and result, approvals, questions, plans, errors and
//! turn totals. Running usage, context, quota and unmapped provider records
//! are left out; JSONL has them.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{self, Write};

use atrium_protocol::agent::{
    AgentEvent, AgentEventEnvelope, MessageBody, MessageKind, PlanStatus, Provider, RequestOutcome,
    ToolKind, ToolStatus, TurnOutcome, Usage,
};
use atrium_protocol::policy::{EnforcementDescriptor, EnforcementLevel};
use atrium_protocol::thread::ThreadId;
use atrium_protocol::wire::{EventId, ExportFormat, StoredEvent};
use serde_json::Value;

use super::{HistoryError, Redactor, verify_message};

/// Writes one thread's events as Markdown or JSONL, redacted with `redactor`.
/// Returns bytes written. User messages (`MessageKind::User`) are included
/// and redacted like the rest.
///
/// `events` are the thread's stored events, oldest first, from its first
/// event. A message that doesn't match its completion fails the export with
/// [`HistoryError::Integrity`]; what was written before that stays in `out`,
/// so the caller removes the file.
pub fn export_thread(
    events: &mut dyn Iterator<Item = StoredEvent>,
    format: ExportFormat,
    redactor: &Redactor,
    out: &mut dyn Write,
) -> Result<u64, HistoryError> {
    let mut out = Counted { out, bytes: 0 };
    let mut markdown = Markdown::new(redactor);
    let mut write = |event: StoredEvent| -> Result<(), HistoryError> {
        match format {
            ExportFormat::Jsonl => write_jsonl(&event, redactor, &mut out),
            ExportFormat::Markdown => markdown.write(&event, &mut out),
        }
    };
    let mut fold = Fold::default();
    for event in events {
        fold.push(event, &mut write)?;
    }
    fold.finish(&mut write)?;
    out.out.flush()?;
    Ok(out.bytes)
}

struct Counted<'a> {
    out: &'a mut dyn Write,
    bytes: u64,
}

impl Counted<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.bytes += bytes.len() as u64;
        Ok(())
    }
}

fn write_jsonl(
    event: &StoredEvent,
    redactor: &Redactor,
    out: &mut Counted<'_>,
) -> Result<(), HistoryError> {
    let mut value = serde_json::to_value(event).map_err(io::Error::from)?;
    redactor.value(&mut value);
    let mut line = serde_json::to_vec(&value).map_err(io::Error::from)?;
    line.push(b'\n');
    Ok(out.write(&line)?)
}

// ---------------------------------------------------------------------------
// Folding
// ---------------------------------------------------------------------------

/// Everything of a stored event but the event itself.
#[derive(Clone)]
struct Head {
    id: EventId,
    thread_id: ThreadId,
    incarnation: u32,
    seq: u64,
    at_ms: u64,
}

impl Head {
    fn with(self, event: AgentEvent) -> StoredEvent {
        StoredEvent {
            id: self.id,
            thread_id: self.thread_id,
            envelope: AgentEventEnvelope {
                incarnation: self.incarnation,
                seq: self.seq,
                at_ms: self.at_ms,
                event,
            },
        }
    }
}

struct OpenMessage {
    /// Of the latest delta.
    head: Head,
    message_id: String,
    kind: MessageKind,
    text: String,
    deltas: u64,
}

struct OpenCall {
    incarnation: u32,
    call_id: String,
    output: String,
    /// Of the last update that carried output.
    last_output: Option<Head>,
}

type Emit<'a> = &'a mut dyn FnMut(StoredEvent) -> Result<(), HistoryError>;

/// Joins streamed pieces (see the module docs). Message and call ids are
/// scoped to an incarnation, so everything is keyed by both. Few items are
/// open at once, so they live in vectors.
#[derive(Default)]
struct Fold {
    messages: Vec<OpenMessage>,
    calls: Vec<OpenCall>,
    /// The incarnation of the latest event.
    incarnation: u32,
}

impl Fold {
    fn push(&mut self, event: StoredEvent, emit: Emit<'_>) -> Result<(), HistoryError> {
        let StoredEvent {
            id,
            thread_id,
            envelope,
        } = event;
        let AgentEventEnvelope {
            incarnation,
            seq,
            at_ms,
            event,
        } = envelope;
        let head = Head {
            id,
            thread_id,
            incarnation,
            seq,
            at_ms,
        };
        // A session normally closes what it opened before it ends. If the
        // next one starts without that, close the old one's items here, so
        // they stay with their session.
        if incarnation > self.incarnation {
            if self.incarnation > 0 {
                self.close(Some(self.incarnation), emit)?;
            }
            self.incarnation = incarnation;
        }
        match event {
            AgentEvent::MessageDelta {
                message_id,
                kind,
                text,
            } => {
                match self.message(incarnation, &message_id) {
                    Some(at) => {
                        let open = &mut self.messages[at];
                        open.head = head;
                        open.text.push_str(&text);
                        open.deltas += 1;
                    }
                    None => self.messages.push(OpenMessage {
                        head,
                        message_id,
                        kind,
                        text,
                        deltas: 1,
                    }),
                }
                Ok(())
            }
            AgentEvent::MessageCompleted {
                message_id,
                kind,
                body,
            } => {
                let (text, deltas) = match self.message(incarnation, &message_id) {
                    Some(at) => {
                        let open = self.messages.remove(at);
                        (open.text, open.deltas)
                    }
                    None => (String::new(), 0),
                };
                let text = verify_message(&message_id, text, deltas, &body)?;
                emit(head.with(AgentEvent::MessageCompleted {
                    message_id,
                    kind,
                    body: MessageBody::Inline { text },
                }))
            }
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                kind,
                title,
                input,
            } => {
                self.call(incarnation, &call_id);
                emit(head.with(AgentEvent::ToolCallStarted {
                    call_id,
                    name,
                    kind,
                    title,
                    input,
                }))
            }
            AgentEvent::ToolCallUpdated {
                call_id,
                title,
                kind,
                input,
                output_delta,
                output,
            } => {
                if output.is_some() || output_delta.is_some() {
                    let at = self.call(incarnation, &call_id);
                    let call = &mut self.calls[at];
                    if let Some(whole) = output {
                        call.output = whole;
                    }
                    if let Some(more) = output_delta {
                        call.output.push_str(&more);
                    }
                    call.last_output = Some(head.clone());
                }
                if title.is_none() && kind.is_none() && input.is_none() {
                    return Ok(());
                }
                emit(head.with(AgentEvent::ToolCallUpdated {
                    call_id,
                    title,
                    kind,
                    input,
                    output_delta: None,
                    output: None,
                }))
            }
            AgentEvent::ToolCallFinished {
                call_id,
                status,
                output,
                error,
                truncated_bytes,
            } => {
                let at = self.call(incarnation, &call_id);
                let streamed = self.calls.remove(at).output;
                // A final output replaces what was streamed.
                let output = output.or((!streamed.is_empty()).then_some(streamed));
                emit(head.with(AgentEvent::ToolCallFinished {
                    call_id,
                    status,
                    output,
                    error,
                    truncated_bytes,
                }))
            }
            event @ AgentEvent::SessionEnded { .. } => {
                self.close(Some(incarnation), emit)?;
                emit(head.with(event))
            }
            event => emit(head.with(event)),
        }
    }

    /// Writes what is still open once the events have run out.
    fn finish(&mut self, emit: Emit<'_>) -> Result<(), HistoryError> {
        self.close(None, emit)
    }

    fn message(&self, incarnation: u32, message_id: &str) -> Option<usize> {
        self.messages
            .iter()
            .position(|m| m.head.incarnation == incarnation && m.message_id == message_id)
    }

    /// The open call's index, adding it if this is the first event seen for it.
    fn call(&mut self, incarnation: u32, call_id: &str) -> usize {
        let found = self
            .calls
            .iter()
            .position(|c| c.incarnation == incarnation && c.call_id == call_id);
        found.unwrap_or_else(|| {
            self.calls.push(OpenCall {
                incarnation,
                call_id: call_id.to_owned(),
                output: String::new(),
                last_output: None,
            });
            self.calls.len() - 1
        })
    }

    /// Writes the open items of `incarnation` and older ones (`None`: all),
    /// oldest first.
    fn close(&mut self, incarnation: Option<u32>, emit: Emit<'_>) -> Result<(), HistoryError> {
        let closing = |of: u32| incarnation.is_none_or(|n| of <= n);
        let mut tail = Vec::new();
        for open in std::mem::take(&mut self.messages) {
            if !closing(open.head.incarnation) {
                self.messages.push(open);
                continue;
            }
            tail.push(open.head.with(AgentEvent::MessageDelta {
                message_id: open.message_id,
                kind: open.kind,
                text: open.text,
            }));
        }
        for open in std::mem::take(&mut self.calls) {
            if !closing(open.incarnation) {
                self.calls.push(open);
                continue;
            }
            let Some(head) = open.last_output else {
                continue;
            };
            if open.output.is_empty() {
                continue;
            }
            tail.push(head.with(AgentEvent::ToolCallUpdated {
                call_id: open.call_id,
                title: None,
                kind: None,
                input: None,
                output_delta: None,
                output: Some(open.output),
            }));
        }
        tail.sort_by_key(|e| e.id);
        tail.into_iter().try_for_each(emit)
    }
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

struct Markdown<'a> {
    redactor: &'a Redactor,
    titled: bool,
    /// Tool names by `(incarnation, call_id)`, for the result's line.
    tools: HashMap<(u32, String), String>,
    /// Open requests by `(incarnation, request_id)`: `true` for a question.
    questions: HashMap<(u32, String), bool>,
}

impl<'a> Markdown<'a> {
    fn new(redactor: &'a Redactor) -> Self {
        Self {
            redactor,
            titled: false,
            tools: HashMap::new(),
            questions: HashMap::new(),
        }
    }

    fn write(&mut self, stored: &StoredEvent, out: &mut Counted<'_>) -> Result<(), HistoryError> {
        let mut md = Block::new(self.redactor);
        if !self.titled {
            self.titled = true;
            md.raw("# Thread ").line(&stored.thread_id.0).raw("\n\n");
        }
        let incarnation = stored.envelope.incarnation;
        match &stored.envelope.event {
            AgentEvent::SessionStarted {
                provider,
                native_session_id,
                model,
                tools: _,
                enforcement,
            } => {
                md.raw("## Session ")
                    .uint(incarnation.into())
                    .raw(": ")
                    .raw(provider_name(*provider));
                if let Some(model) = model {
                    md.raw(", ").line(model);
                }
                md.raw("\n\n- Started: ").time(stored.envelope.at_ms);
                md.raw("\n- Native session: ").code(native_session_id);
                md.raw("\n- ").coverage(enforcement).raw("\n");
            }
            AgentEvent::EnforcementChanged { enforcement } => {
                md.raw("Coverage changed. ").coverage(enforcement).raw("\n");
            }
            AgentEvent::MessageCompleted {
                kind,
                body: MessageBody::Inline { text },
                ..
            } => {
                if !text.is_empty() {
                    md.raw(message_heading(*kind)).raw("\n\n").body(text);
                }
            }
            // The fold only passes a delta on for a message that never completed.
            AgentEvent::MessageDelta { kind, text, .. } => {
                md.raw(message_heading(*kind))
                    .raw(" (incomplete)\n\n")
                    .body(text);
            }
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                kind,
                title,
                input,
            } => {
                self.tools
                    .insert((incarnation, call_id.clone()), name.clone());
                md.raw("**Tool** ")
                    .code(name)
                    .raw(" (")
                    .raw(tool_kind(*kind))
                    .raw(")");
                if let Some(title) = title {
                    md.raw(": ").line(title);
                }
                md.raw("\n\n").json(input);
            }
            // The fold only passes output on an update for a call that never finished.
            AgentEvent::ToolCallUpdated {
                call_id,
                output: Some(output),
                ..
            } => {
                md.raw("**Output so far** ");
                self.tool(&mut md, incarnation, call_id, false);
                md.raw("\n\n").fenced("text", output);
            }
            AgentEvent::ToolCallFinished {
                call_id,
                status,
                output,
                error,
                truncated_bytes,
            } => {
                md.raw("**Result** ");
                self.tool(&mut md, incarnation, call_id, true);
                md.raw(": ").raw(tool_status(*status));
                if *truncated_bytes > 0 {
                    md.raw(", ")
                        .uint(*truncated_bytes)
                        .raw(" bytes of output dropped");
                }
                md.raw("\n\n");
                if let Some(output) = output.as_deref().filter(|o| !o.is_empty()) {
                    md.fenced("text", output);
                }
                if let Some(error) = error.as_deref().filter(|e| !e.is_empty()) {
                    md.raw("Error:\n\n").fenced("text", error);
                }
            }
            AgentEvent::ApprovalRequested {
                request_id,
                call_id,
                name,
                kind,
                title,
                input,
                reason,
                rememberable: _,
            } => {
                self.questions
                    .insert((incarnation, request_id.clone()), false);
                md.raw("**Approval requested** ")
                    .code(name)
                    .raw(" (")
                    .raw(tool_kind(*kind))
                    .raw(")");
                if let Some(title) = title {
                    md.raw(": ").line(title);
                }
                md.raw("\n\n");
                if let Some(reason) = reason {
                    md.raw("Reason: ").line(reason).raw("\n\n");
                }
                // With a tool call, the input is already under that call.
                if call_id.is_none() {
                    md.json(input);
                }
            }
            AgentEvent::QuestionAsked {
                request_id,
                questions,
            } => {
                self.questions
                    .insert((incarnation, request_id.clone()), true);
                md.raw("**Question**\n\n");
                for (number, question) in questions.iter().enumerate() {
                    md.uint(number as u64 + 1)
                        .raw(". ")
                        .line(&question.prompt)
                        .raw("\n");
                    for option in &question.options {
                        md.raw("   - ").line(&option.label);
                        if let Some(description) = &option.description {
                            md.raw(": ").line(description);
                        }
                        md.raw("\n");
                    }
                }
            }
            AgentEvent::RequestResolved {
                request_id,
                outcome,
            } => {
                md.raw(
                    match self.questions.remove(&(incarnation, request_id.clone())) {
                        Some(true) => "**Question** ",
                        Some(false) => "**Approval** ",
                        None => "**Request** ",
                    },
                )
                .raw(request_outcome(*outcome))
                .raw("\n");
            }
            AgentEvent::PlanUpdated { entries } => {
                md.raw("**Plan**\n\n");
                for entry in entries {
                    md.raw(match entry.status {
                        PlanStatus::Completed => "- [x] ",
                        PlanStatus::Pending | PlanStatus::InProgress => "- [ ] ",
                    })
                    .line(&entry.content);
                    if entry.status == PlanStatus::InProgress {
                        md.raw(" (in progress)");
                    }
                    md.raw("\n");
                }
            }
            AgentEvent::TurnCompleted {
                turn,
                outcome,
                usage,
            } => {
                md.raw("_Turn ")
                    .uint((*turn).into())
                    .raw(" ")
                    .raw(turn_outcome(*outcome));
                if let Some(usage) = usage {
                    md.usage(usage);
                }
                md.raw("_\n");
            }
            AgentEvent::RateLimited {
                resets_at_ms,
                message,
            } => {
                md.raw("**Rate limited**");
                if let Some(at) = resets_at_ms {
                    md.raw(" until ").time(*at);
                }
                if let Some(message) = message {
                    md.raw(": ").line(message);
                }
                md.raw("\n");
            }
            AgentEvent::Error {
                message,
                fatal,
                details,
            } => {
                md.raw(if *fatal {
                    "**Fatal error**: "
                } else {
                    "**Error**: "
                })
                .line(message)
                .raw("\n\n");
                if !details.is_empty() {
                    md.fenced("text", &details.join("\n"));
                }
            }
            AgentEvent::SessionEnded { exit_code, reason } => {
                md.raw("**Session ended**");
                if let Some(code) = exit_code {
                    md.raw(", exit code ").int((*code).into());
                }
                if let Some(reason) = reason {
                    md.raw(": ").line(reason);
                }
                md.raw("\n");
            }
            // In JSONL only. (`MessageCompleted` is always `Inline` after the
            // fold, and an update without output changes nothing shown here.)
            AgentEvent::MessageCompleted { .. }
            | AgentEvent::ToolCallUpdated { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::Usage { .. }
            | AgentEvent::ContextUpdated { .. }
            | AgentEvent::QuotaUpdated { .. }
            | AgentEvent::Unmapped { .. } => {}
        }
        Ok(out.write(md.finish().as_bytes())?)
    }

    /// The tool's name in code, or its call id when its start wasn't seen.
    fn tool(&mut self, md: &mut Block<'_>, incarnation: u32, call_id: &str, done: bool) {
        let key = (incarnation, call_id.to_owned());
        match self.tools.get(&key) {
            Some(name) => md.code(name),
            None => md.raw("call ").code(call_id),
        };
        if done {
            self.tools.remove(&key);
        }
    }
}

/// One event's Markdown. Text from the session can only get in through a
/// method that redacts it; [`Block::raw`] takes nothing but literals.
struct Block<'a> {
    redactor: &'a Redactor,
    text: String,
}

impl<'a> Block<'a> {
    fn new(redactor: &'a Redactor) -> Self {
        Self {
            redactor,
            text: String::new(),
        }
    }

    /// The block, ending in one blank line (empty stays empty).
    fn finish(mut self) -> String {
        if self.text.is_empty() {
            return self.text;
        }
        self.text.truncate(self.text.trim_end_matches('\n').len());
        self.text.push_str("\n\n");
        self.text
    }

    fn raw(&mut self, literal: &'static str) -> &mut Self {
        self.text.push_str(literal);
        self
    }

    fn uint(&mut self, n: u64) -> &mut Self {
        let _ = write!(self.text, "{n}");
        self
    }

    fn int(&mut self, n: i64) -> &mut Self {
        let _ = write!(self.text, "{n}");
        self
    }

    /// `YYYY-MM-DD HH:MM:SS UTC`.
    fn time(&mut self, unix_ms: u64) -> &mut Self {
        let secs = unix_ms / 1000;
        let (year, month, day) = civil_from_days(secs / 86_400);
        let _ = write!(
            self.text,
            "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
            secs % 86_400 / 3600,
            secs % 3600 / 60,
            secs % 60
        );
        self
    }

    /// Redacted, on one line.
    fn line(&mut self, text: &str) -> &mut Self {
        let text = one_line(&self.redactor.text(text));
        self.text.push_str(&text);
        self
    }

    /// Redacted, on one line, as inline code.
    fn code(&mut self, text: &str) -> &mut Self {
        let text = one_line(&self.redactor.text(text));
        if text.is_empty() {
            return self.raw("(unnamed)");
        }
        // A code span is closed by a run of backticks of the same length, so
        // use one longer than any inside; a space keeps an edge backtick apart.
        let ticks = "`".repeat(longest_run(&text, '`') + 1);
        let pad = if text.starts_with('`') || text.ends_with('`') {
            " "
        } else {
            ""
        };
        let _ = write!(self.text, "{ticks}{pad}{text}{pad}{ticks}");
        self
    }

    /// Redacted, as written, followed by a blank line.
    fn body(&mut self, text: &str) -> &mut Self {
        let text = self.redactor.text(text);
        self.text.push_str(text.trim_end_matches('\n'));
        self.text.push_str("\n\n");
        self
    }

    /// Redacted, in a fenced block, followed by a blank line.
    fn fenced(&mut self, info: &'static str, text: &str) -> &mut Self {
        let text = self.redactor.text(text);
        self.fence(info, &text)
    }

    /// The value redacted as JSON (so a key that names a secret hides its
    /// value), in a fenced block. `null` writes nothing.
    fn json(&mut self, value: &Value) -> &mut Self {
        if value.is_null() {
            return self;
        }
        let mut value = value.clone();
        self.redactor.value(&mut value);
        // A `Value` always serializes: its keys are strings.
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        self.fence("json", &text)
    }

    /// `redacted` is already safe to write.
    fn fence(&mut self, info: &'static str, redacted: &str) -> &mut Self {
        // Longer than any run of backticks inside, so the content can't close it.
        let fence = "`".repeat((longest_run(redacted, '`') + 1).max(3));
        let _ = write!(
            self.text,
            "{fence}{info}\n{}\n{fence}\n\n",
            redacted.trim_end_matches('\n')
        );
        self
    }

    fn coverage(&mut self, enforcement: &EnforcementDescriptor) -> &mut Self {
        self.raw(match enforcement.level {
            EnforcementLevel::Full => "Coverage: full (Atrium saw every tool call before it ran)",
            EnforcementLevel::Partial => {
                "Coverage: partial (some tool calls could run without Atrium seeing them first)"
            }
        });
        for reason in &enforcement.reasons {
            self.raw("\n  - ").line(&reason.message);
        }
        self
    }

    fn usage(&mut self, usage: &Usage) -> &mut Self {
        if let Some(n) = usage.input_tokens {
            self.raw(", ").uint(n).raw(" input tokens");
        }
        if let Some(n) = usage.output_tokens {
            self.raw(", ").uint(n).raw(" output tokens");
        }
        if let Some(cost) = usage.cost_usd.filter(|c| c.is_finite()) {
            let _ = write!(self.text, ", ${cost:.4}");
        }
        self
    }
}

fn one_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn longest_run(text: &str, of: char) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for c in text.chars() {
        current = if c == of { current + 1 } else { 0 };
        longest = longest.max(current);
    }
    longest
}

/// Days since 1970-01-01 to `(year, month, day)`, proleptic Gregorian.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (year_of_era + era * 400 + u64::from(month <= 2), month, day)
}

fn provider_name(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "Claude Code",
        Provider::Codex => "Codex",
        Provider::Agy => "agy",
        Provider::Acp => "ACP agent",
    }
}

fn message_heading(kind: MessageKind) -> &'static str {
    match kind {
        MessageKind::User => "### User",
        MessageKind::Assistant => "### Assistant",
        MessageKind::Reasoning => "### Reasoning",
    }
}

fn tool_kind(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Read => "read",
        ToolKind::Edit => "edit",
        ToolKind::Delete => "delete",
        ToolKind::Move => "move",
        ToolKind::Search => "search",
        ToolKind::Execute => "execute",
        ToolKind::Fetch => "fetch",
        ToolKind::Think => "think",
        ToolKind::Mcp => "MCP",
        ToolKind::Other => "other",
    }
}

fn tool_status(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::Succeeded => "succeeded",
        ToolStatus::Failed => "failed",
        ToolStatus::Denied => "denied",
        ToolStatus::Cancelled => "cancelled",
    }
}

fn turn_outcome(outcome: TurnOutcome) -> &'static str {
    match outcome {
        TurnOutcome::Completed => "completed",
        TurnOutcome::Interrupted => "interrupted",
        TurnOutcome::Failed => "failed",
        TurnOutcome::LimitReached => "stopped at a limit",
    }
}

fn request_outcome(outcome: RequestOutcome) -> &'static str {
    match outcome {
        RequestOutcome::Approved => "approved",
        RequestOutcome::Denied => "denied",
        RequestOutcome::Answered => "answered",
        RequestOutcome::Cancelled => "cancelled",
        RequestOutcome::Expired => "expired",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_match_known_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(20_401), (2025, 11, 9));
        let redactor = Redactor::default();
        let mut md = Block::new(&redactor);
        md.time(1_762_696_202_000);
        assert_eq!(md.text, "2025-11-09 13:50:02 UTC");
    }

    #[test]
    fn code_and_fences_outlast_the_backticks_inside() {
        let redactor = Redactor::default();
        let mut md = Block::new(&redactor);
        md.code("a`b").raw("|").code("`x`").raw("|").code("");
        assert_eq!(md.text, "``a`b``|`` `x` ``|(unnamed)");
        let mut md = Block::new(&redactor);
        md.fenced("text", "```\ninner\n```\n");
        assert_eq!(md.text, "````text\n```\ninner\n```\n````\n\n");
    }

    #[test]
    fn one_line_joins_lines_with_single_spaces() {
        assert_eq!(one_line("a\r\nb\n\nc"), "a b c");
    }
}
