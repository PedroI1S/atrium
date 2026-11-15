#![doc = "Fixture recording helpers, privacy scrubbing, and offline enforcement checks."]

use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use atrium_protocol::agent::{AgentEvent, Provider, ToolKind, TurnOutcome};
use serde_json::Value;

use crate::AdapterInput;
use crate::fixture::{self, FixtureLine, Sanitizer};

use super::CodexNormalizer;

/// Appends one sanitized fixture line to a JSONL recording if a tap file is open.
pub fn write_with_secrets(
    path: &Path,
    lines: &[FixtureLine],
    secrets: &[String],
) -> Result<Vec<FixtureLine>, Box<dyn std::error::Error>> {
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let san1 = Sanitizer::new(&home, &user, &secret_refs);
    let user_fallback = format!("{}user", "/Users/");
    let san2 = Sanitizer::new(&user_fallback, "user", &secret_refs);

    let mut clean = Vec::with_capacity(lines.len());
    let mut out = String::new();
    for line in lines {
        let mut cloned = line.clone();
        san1.input(&mut cloned.input);
        san2.input(&mut cloned.input);
        let serialized = serde_json::to_string(&cloned)?;
        if !san1.is_clean(&serialized) {
            return Err("sanitizer: line still contains machine-specific secrets".into());
        }
        out.push_str(&serialized);
        out.push('\n');
        clean.push(cloned);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, out)?;
    Ok(clean)
}

/// Appends a single JSONL line to `path` for incremental live recording.
pub fn append_line(path: &Path, line: &FixtureLine) -> std::io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(line)?)
}

/// Sanitizes CLI arguments before writing fixture markdown notes.
pub fn sanitize_arguments(args: &[String], secrets: &[String]) -> Result<Vec<String>, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let user = std::env::var("USER").unwrap_or_default();
    let secret_refs: Vec<&str> = secrets.iter().map(String::as_str).collect();
    let san = Sanitizer::new(&home, &user, &secret_refs);
    let out: Vec<String> = args.iter().map(|a| san.text(a)).collect();
    for a in &out {
        if !san.is_clean(a) {
            return Err("sanitizer: command argument still contains private values".into());
        }
    }
    Ok(out)
}

/// Checks whether a normalized + raw fixture trace respects denied/cancelled enforcement invariants.
pub fn verify_enforcement(scenario: &str, lines: &[FixtureLine]) -> Result<(), String> {
    let mut norm = CodexNormalizer::new();
    let events = fixture::normalize(lines, &mut norm);

    if events.is_empty() {
        return Err(format!("{scenario}: produced zero normalized events"));
    }

    let first_is_started = matches!(
        events.first(),
        Some(AgentEvent::SessionStarted {
            provider: Provider::Codex,
            ..
        })
    );
    let first_is_fatal = matches!(events.first(), Some(AgentEvent::Error { fatal: true, .. }));
    if !first_is_started && !first_is_fatal {
        return Err(format!(
            "{scenario}: unexpected first event: {:?}",
            events.first()
        ));
    }

    if !matches!(events.last(), Some(AgentEvent::SessionEnded { .. })) {
        return Err(format!("{scenario}: last event must be SessionEnded"));
    }

    if scenario == "gate-deny" || scenario == "gate-deny-execute" {
        let mut saw_deny_stdin = false;
        let mut saw_declined_item = false;
        for line in lines {
            if let AdapterInput::Stdin { line } = &line.input
                && let Ok(v) = serde_json::from_str::<Value>(line)
                && v.pointer("/result/decision").and_then(Value::as_str) == Some("decline")
            {
                saw_deny_stdin = true;
            }
            if let AdapterInput::Stdout { line } = &line.input
                && let Ok(v) = serde_json::from_str::<Value>(line)
                && v.get("method").and_then(Value::as_str) == Some("item/completed")
                && v.pointer("/params/item/status").and_then(Value::as_str) == Some("declined")
            {
                saw_declined_item = true;
            }
        }
        if !saw_deny_stdin || !saw_declined_item {
            return Err(format!(
                "{scenario}: missing raw decline response or declined item completion"
            ));
        }
    }

    if scenario == "interrupt" {
        let interrupted = events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::TurnCompleted {
                    outcome: TurnOutcome::Interrupted,
                    ..
                }
            )
        });
        if !interrupted {
            return Err("interrupt: missing TurnCompleted(Interrupted)".into());
        }
    }

    if scenario == "approval-allow" {
        let has_execute = events.iter().any(|e| {
            matches!(
                e,
                AgentEvent::ToolCallStarted {
                    kind: ToolKind::Execute,
                    ..
                }
            )
        });
        if !has_execute {
            return Err("approval-allow: missing ToolCallStarted(Execute)".into());
        }
    }

    Ok(())
}
