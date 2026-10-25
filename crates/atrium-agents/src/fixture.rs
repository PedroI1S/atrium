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

    pub fn input(&self, input: &mut AdapterInput) {
        match input {
            AdapterInput::Stdin { line }
            | AdapterInput::Stdout { line }
            | AdapterInput::Stderr { line } => *line = self.line(line),
            AdapterInput::HookRequest { payload, .. }
            | AdapterInput::HookResponse { payload, .. } => self.value(payload),
            AdapterInput::Gate { check, decision } => {
                self.value(&mut check.input);
                if let Some(title) = &mut check.title {
                    *title = self.text(title);
                }
                if let crate::GateDecision::Deny { reason } = decision {
                    *reason = self.text(reason);
                }
            }
            AdapterInput::Command { .. } | AdapterInput::Exit { .. } => {}
        }
    }

    /// True if no secret, home path or user name is left in `text`.
    pub fn is_clean(&self, text: &str) -> bool {
        self.secrets.iter().all(|s| !text.contains(s.as_str()))
            && (self.home.is_empty() || !text.contains(&self.home))
            && replace_word(text, &self.user, "") == text
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
