//! Recorded protocol transcripts and golden tests.
//!
//! A fixture is `fixtures/agents/<provider>/<name>.jsonl`, one
//! [`FixtureLine`] per line: every [`AdapterInput`] in the order it crossed
//! the adapter boundary, ending with `exit`. Next to it,
//! `<name>.events.json` holds the expected normalized events (a JSON array of
//! [`AgentEvent`]) and `<name>.md` says how it was recorded (CLI version,
//! command line, prompt, what the scenario covers, `live` or `hand-made`).
//!
//! Fixtures are committed, so everything is passed through a [`Sanitizer`]
//! before it's written: no home-directory paths, user names, tokens or env
//! values.

use std::fs;
use std::io;
use std::path::Path;

use atrium_protocol::agent::AgentEvent;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{AdapterInput, Normalizer};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureLine {
    /// Milliseconds since the process started.
    pub ms: u64,
    #[serde(flatten)]
    pub input: AdapterInput,
}

pub fn read(path: &Path) -> io::Result<Vec<FixtureLine>> {
    fs::read_to_string(path)?
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(io::Error::other))
        .collect()
}

/// Runs every input through `normalizer`, in order.
pub fn normalize(lines: &[FixtureLine], normalizer: &mut impl Normalizer) -> Vec<AgentEvent> {
    lines
        .iter()
        .flat_map(|l| normalizer.on_input(&l.input))
        .collect()
}

/// Golden test: normalizes `<fixture>.jsonl` and compares with
/// `<fixture>.events.json`. With `UPDATE_GOLDEN=1` it rewrites the expected
/// file instead; review that diff like code.
pub fn assert_golden(fixture: &Path, normalizer: &mut impl Normalizer) {
    let lines = read(fixture).unwrap_or_else(|e| panic!("reading {}: {e}", fixture.display()));
    let actual = normalize(&lines, normalizer);
    let expected_path = fixture.with_extension("events.json");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        let json = serde_json::to_string_pretty(&actual).expect("events serialize");
        fs::write(&expected_path, json + "\n").expect("writing golden file");
        return;
    }
    let expected: Vec<AgentEvent> = serde_json::from_str(
        &fs::read_to_string(&expected_path)
            .unwrap_or_else(|e| panic!("reading {}: {e}", expected_path.display())),
    )
    .expect("golden file parses");
    assert_eq!(
        actual,
        expected,
        "normalized events differ from {}",
        expected_path.display()
    );
}

/// Removes machine-specific values from recorded inputs.
///
/// It works on decoded values, never on raw text: a JSON line is parsed,
/// only its *string values* are rewritten (object keys and other values are
/// left alone), and it's serialized again with the key order preserved.
/// Within each string, secrets are replaced first (`<redacted>`), then the
/// home directory (`~`), then the user name (`user`, whole words only).
pub struct Sanitizer {
    home: String,
    user: String,
    secrets: Vec<String>,
}

impl Sanitizer {
    pub fn new(home: &str, user: &str, secrets: &[&str]) -> Self {
        Self {
            home: home.to_owned(),
            user: user.to_owned(),
            secrets: secrets
                .iter()
                .filter(|s| !s.is_empty())
                .map(|s| (*s).to_owned())
                .collect(),
        }
    }

    pub fn text(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for secret in &self.secrets {
            out = out.replace(secret, "<redacted>");
        }
        if !self.home.is_empty() {
            out = out.replace(&self.home, "~");
        }
        replace_word(&out, &self.user, "user")
    }

    pub fn value(&self, value: &mut Value) {
        match value {
            Value::String(s) => *s = self.text(s),
            Value::Array(items) => items.iter_mut().for_each(|v| self.value(v)),
            Value::Object(map) => map.values_mut().for_each(|v| self.value(v)),
            _ => {}
        }
    }

    /// A raw line: sanitized as JSON when it parses, as plain text otherwise.
    pub fn line(&self, line: &str) -> String {
        match serde_json::from_str::<Value>(line) {
            Ok(mut value) => {
                self.value(&mut value);
                value.to_string()
            }
            Err(_) => self.text(line),
        }
    }

    /// A tool check's input, title and subject (command lines, cwd, paths and
    /// URLs carry home paths and secrets).
    fn check(&self, check: &mut crate::ToolCheck) {
        use atrium_protocol::policy::ToolSubject;
        self.value(&mut check.input);
        if let Some(title) = &mut check.title {
            *title = self.text(title);
        }
        match &mut check.subject {
            ToolSubject::Command { line, cwd } => {
                *line = self.text(line);
                if let Some(cwd) = cwd {
                    *cwd = self.text(cwd);
                }
            }
            ToolSubject::Paths { paths, glob, .. } => {
                for path in paths {
                    *path = self.text(path);
                }
                if let Some(glob) = glob {
                    *glob = self.text(glob);
                }
            }
            ToolSubject::Fetch { url } => *url = self.text(url),
            ToolSubject::Mcp { .. } | ToolSubject::None | ToolSubject::Unknown => {}
        }
    }

    pub fn input(&self, input: &mut AdapterInput) {
        match input {
            AdapterInput::Stdin { line }
            | AdapterInput::Stdout { line }
            | AdapterInput::Stderr { line } => *line = self.line(line),
            AdapterInput::HookRequest { payload, .. }
            | AdapterInput::HookResponse { payload, .. } => self.value(payload),
            AdapterInput::Gate { check, decision }
            | AdapterInput::Redeem {
                check, decision, ..
            } => {
                self.check(check);
                if let crate::GateDecision::Deny { reason } = decision {
                    *reason = self.text(reason);
                }
            }
            AdapterInput::Enforcement { descriptor } => {
                for source in &mut descriptor.config_sources {
                    source.path = self.text(&source.path);
                }
                for reason in &mut descriptor.reasons {
                    reason.message = self.text(&reason.message);
                }
            }
            // Prompts and answers are user content and can carry secrets
            // (gate review: the sanitizer ignored `Command`).
            AdapterInput::Command { command } => self.command(command),
            AdapterInput::WriteFailed { error } => *error = self.text(error),
            AdapterInput::QuestionTicket { .. }
            | AdapterInput::HookDisconnected { .. }
            | AdapterInput::Deadline { .. }
            | AdapterInput::Exit { .. } => {}
        }
    }

    fn command(&self, command: &mut atrium_protocol::agent::SessionCommand) {
        use atrium_protocol::agent::{ApprovalDecision, SessionCommand};
        match command {
            SessionCommand::SendUserMessage { text } => *text = self.text(text),
            SessionCommand::ResolveApproval {
                decision: ApprovalDecision::Deny { message: Some(m) },
                ..
            } => *m = self.text(m),
            SessionCommand::AnswerQuestion { answers, .. } => {
                for answer in answers {
                    if let Some(t) = &mut answer.text {
                        *t = self.text(t);
                    }
                }
            }
            SessionCommand::ResolveApproval { .. }
            | SessionCommand::Interrupt
            | SessionCommand::Shutdown => {}
        }
    }

    fn is_clean_raw(&self, text: &str) -> bool {
        self.secrets.iter().all(|s| !text.contains(s.as_str()))
            && (self.home.is_empty() || !text.contains(&self.home))
            && replace_word(text, &self.user, "") == text
    }

    /// True if no secret, home path or user name is left in `text`,
    /// including in decoded content with JSON-escaped or Unicode-escaped forms.
    pub fn is_clean(&self, text: &str) -> bool {
        if !self.is_clean_raw(text) {
            return false;
        }
        let decoded = decode_all_escapes(text);
        if decoded != text && !self.is_clean_raw(&decoded) {
            return false;
        }
        true
    }
}

/// Replaces `word` where it isn't part of a longer identifier-like run.
fn replace_word(text: &str, word: &str, with: &str) -> String {
    if word.is_empty() {
        return text.to_owned();
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(word) {
        let before = rest[..pos].chars().next_back();
        let after = rest[pos + word.len()..].chars().next();
        out.push_str(&rest[..pos]);
        if before.is_some_and(is_ident) || after.is_some_and(is_ident) {
            out.push_str(word);
        } else {
            out.push_str(with);
        }
        rest = &rest[pos + word.len()..];
    }
    out.push_str(rest);
    out
}

/// Decodes JSON escape sequences (\uXXXX, \", \\, etc.) and Unicode escapes (\u{...}, \xXX).
fn decode_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();

    while let Some((_, c)) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }

        match chars.peek() {
            Some(&(_, '"')) => {
                chars.next();
                out.push('"');
            }
            Some(&(_, '\\')) => {
                chars.next();
                out.push('\\');
            }
            Some(&(_, '/')) => {
                chars.next();
                out.push('/');
            }
            Some(&(_, 'b')) => {
                chars.next();
                out.push('\x08');
            }
            Some(&(_, 'f')) => {
                chars.next();
                out.push('\x0C');
            }
            Some(&(_, 'n')) => {
                chars.next();
                out.push('\n');
            }
            Some(&(_, 'r')) => {
                chars.next();
                out.push('\r');
            }
            Some(&(_, 't')) => {
                chars.next();
                out.push('\t');
            }
            Some(&(_, 'u')) => {
                chars.next();
                if let Some(&(_, '{')) = chars.peek() {
                    chars.next();
                    let mut hex = String::new();
                    while let Some(&(_, hc)) = chars.peek() {
                        if hc == '}' {
                            chars.next();
                            break;
                        }
                        if hc.is_ascii_hexdigit() {
                            hex.push(hc);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    if let Ok(code) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(code) {
                            out.push(ch);
                        } else {
                            out.push_str("\\u{");
                            out.push_str(&hex);
                            out.push('}');
                        }
                    } else {
                        out.push_str("\\u{");
                        out.push_str(&hex);
                    }
                } else {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        if let Some(&(_, hc)) = chars.peek() {
                            if hc.is_ascii_hexdigit() {
                                hex.push(hc);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                    }
                    if hex.len() == 4 {
                        if let Ok(code) = u16::from_str_radix(&hex, 16) {
                            if (0xD800..=0xDBFF).contains(&code) {
                                let mut cloned = chars.clone();
                                if let (Some((_, '\\')), Some((_, 'u'))) =
                                    (cloned.next(), cloned.next())
                                {
                                    let mut hex2 = String::new();
                                    for _ in 0..4 {
                                        if let Some((_, hc)) = cloned.next() {
                                            if hc.is_ascii_hexdigit() {
                                                hex2.push(hc);
                                            } else {
                                                break;
                                            }
                                        }
                                    }
                                    if hex2.len() == 4
                                        && let Ok(code2) = u16::from_str_radix(&hex2, 16)
                                        && (0xDC00..=0xDFFF).contains(&code2)
                                        && let Some(Ok(ch)) =
                                            char::decode_utf16([code, code2]).next()
                                    {
                                        out.push(ch);
                                        chars = cloned;
                                        continue;
                                    }
                                }
                            }
                            if let Some(ch) = char::from_u32(code as u32) {
                                out.push(ch);
                            } else {
                                out.push_str("\\u");
                                out.push_str(&hex);
                            }
                        } else {
                            out.push_str("\\u");
                            out.push_str(&hex);
                        }
                    } else {
                        out.push_str("\\u");
                        out.push_str(&hex);
                    }
                }
            }
            Some(&(_, 'x')) => {
                chars.next();
                let mut hex = String::new();
                for _ in 0..2 {
                    if let Some(&(_, hc)) = chars.peek() {
                        if hc.is_ascii_hexdigit() {
                            hex.push(hc);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                }
                if hex.len() == 2 {
                    if let Ok(code) = u8::from_str_radix(&hex, 16) {
                        out.push(code as char);
                    } else {
                        out.push_str("\\x");
                        out.push_str(&hex);
                    }
                } else {
                    out.push_str("\\x");
                    out.push_str(&hex);
                }
            }
            _ => {
                out.push('\\');
            }
        }
    }

    out
}

fn decode_all_escapes(s: &str) -> String {
    let mut current = decode_escapes(s);
    for _ in 0..3 {
        if !current.contains('\\') {
            break;
        }
        let next = decode_escapes(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_values_not_keys() {
        let s = Sanitizer::new("/Users/type", "type", &[]);
        assert_eq!(
            s.line(r#"{"type":"x","cwd":"/Users/type/src","who":"type"}"#),
            r#"{"type":"x","cwd":"~/src","who":"user"}"#
        );
    }

    #[test]
    fn secrets_go_first_even_when_they_contain_the_user_name() {
        let s = Sanitizer::new("/Users/ana", "ana", &["sk-ana-123"]);
        assert_eq!(s.text("key sk-ana-123 for ana"), "key <redacted> for user");
    }

    #[test]
    fn json_escaped_secrets_are_redacted() {
        let s = Sanitizer::new("/h", "u", &[r#"p"w\d"#]);
        let line = serde_json::json!({ "v": r#"x p"w\d y"# }).to_string();
        assert_eq!(s.line(&line), r#"{"v":"x <redacted> y"}"#);
        assert!(s.is_clean(&s.line(&line)));
    }

    #[test]
    fn user_name_only_as_a_whole_word() {
        let s = Sanitizer::new("", "ana", &[]);
        assert_eq!(s.text("banana ana ana_x ana."), "banana user ana_x user.");
    }

    #[test]
    fn escaped_secret_detected_by_is_clean() {
        let s = Sanitizer::new("/home/user", "user", &["super_secret"]);
        // Raw matches
        assert!(!s.is_clean("super_secret"));
        // JSON-escaped \uXXXX form
        assert!(!s.is_clean(r"contains \u0073\u0075\u0070\u0065\u0072\u005f\u0073\u0065\u0063\u0072\u0065\u0074 within"));
        // Unicode-escaped \u{...} form
        assert!(!s.is_clean(r"contains \u{73}\u{75}\u{70}\u{65}\u{72}\u{5f}\u{73}\u{65}\u{63}\u{72}\u{65}\u{74} within"));
        // Mixed partial escape
        assert!(!s.is_clean(r"super_\u0073ecret"));
        // Clean text passes
        assert!(s.is_clean("completely benign text"));
    }

    #[test]
    fn fixture_lines_round_trip() {
        let line = FixtureLine {
            ms: 12,
            input: AdapterInput::Stdout {
                line: r#"{"type":"x"}"#.into(),
            },
        };
        let json = serde_json::to_string(&line).unwrap();
        assert_eq!(
            json,
            r#"{"ms":12,"dir":"stdout","line":"{\"type\":\"x\"}"}"#
        );
        assert_eq!(serde_json::from_str::<FixtureLine>(&json).unwrap(), line);
    }
}
