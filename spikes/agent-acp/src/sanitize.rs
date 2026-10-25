//! Fixture sanitizing on top of [`atrium_agents::fixture::Sanitizer`].
//!
//! The contract `Sanitizer` removes secrets, the home directory and the user
//! name. It does not know about temp directories, and the first live
//! recording leaked the macOS scratch path (`/private/var/folders/..`, also
//! as `/var/folders/..`, in prose and in `file://` URLs). [`FixtureSanitizer`]
//! maps those first, on decoded string values, and then hands over to the
//! contract sanitizer:
//!
//! - the scratch directory (its realpath, the path with `/private` removed,
//!   and the path as given) becomes `/scratch`;
//! - any other macOS per-user temp root (`[/private]/var/folders/xx/yyyy/T`)
//!   becomes `/tmp`.
//!
//! [`FixtureSanitizer::is_clean`] fails if a temp path is left.

use std::path::Path;

use atrium_agents::AdapterInput;
use atrium_agents::fixture::Sanitizer;
use serde_json::Value;

pub const SCRATCH: &str = "/scratch";

pub struct FixtureSanitizer {
    inner: Sanitizer,
    /// Scratch-directory spellings, longest first.
    scratch: Vec<String>,
}

impl FixtureSanitizer {
    pub fn new(home: &str, user: &str, secrets: &[&str], scratch_dir: Option<&Path>) -> Self {
        let mut forms: Vec<String> = Vec::new();
        if let Some(dir) = scratch_dir {
            let mut add = |p: String| {
                let p = p.trim_end_matches('/').to_owned();
                if p.len() > 1 && !forms.contains(&p) {
                    forms.push(p);
                }
            };
            let given = dir.to_string_lossy().into_owned();
            let real = std::fs::canonicalize(dir)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| given.clone());
            for p in [real, given] {
                if let Some(rest) = p.strip_prefix("/private") {
                    add(rest.to_owned());
                } else if p.starts_with("/var/") || p.starts_with("/tmp/") {
                    add(format!("/private{p}"));
                }
                add(p);
            }
        }
        forms.sort_by_key(|f| std::cmp::Reverse(f.len()));
        Self {
            inner: Sanitizer::new(home, user, secrets),
            scratch: forms,
        }
    }

    /// Path mapping only (no secret / home / user handling).
    pub fn map_paths(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for form in &self.scratch {
            out = replace_path_prefix(&out, form, SCRATCH);
        }
        map_temp_roots(&out)
    }

    pub fn text(&self, s: &str) -> String {
        self.inner.text(&self.map_paths(s))
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
            AdapterInput::Gate { check, decision } => {
                self.value(&mut check.input);
                if let Some(title) = &mut check.title {
                    *title = self.text(title);
                }
                if let atrium_agents::GateDecision::Deny { reason } = decision {
                    *reason = self.text(reason);
                }
            }
            other => {
                // Hook payloads, commands and exits: let the contract sanitizer
                // handle what it handles, after mapping hook payload paths.
                if let AdapterInput::HookRequest { payload, .. }
                | AdapterInput::HookResponse { payload, .. } = other
                {
                    self.value(payload);
                }
                self.inner.input(other);
            }
        }
    }

    /// True if no secret, home path, user name or temp path is left in `text`.
    pub fn is_clean(&self, text: &str) -> bool {
        self.inner.is_clean(text) && !contains_temp_path(text)
    }
}

/// True if `text` still holds a macOS temp path (`/var/folders/..`,
/// `/private/var/..`, `/private/tmp/..`) or one of the scratch spellings.
pub fn contains_temp_path(text: &str) -> bool {
    text.contains("/var/folders/")
        || text.contains("/private/var/")
        || text.contains("/private/tmp/")
}

/// Replaces `prefix` with `to` where it ends a path component, so that
/// `/a/tmp.X` is replaced inside `/a/tmp.X/file` and `file:///a/tmp.X/file`
/// but not inside `/a/tmp.XY`.
fn replace_path_prefix(text: &str, prefix: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(prefix) {
        let after = &rest[pos + prefix.len()..];
        let boundary = after
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '_' | '-' | '.')));
        out.push_str(&rest[..pos]);
        out.push_str(if boundary { to } else { prefix });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `[/private]/var/folders/<a>/<b>/T` -> `/tmp`, anywhere in the text.
fn map_temp_roots(text: &str) -> String {
    const MARK: &str = "/var/folders/";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find(MARK) {
        let after = &rest[pos + MARK.len()..];
        // <a>/<b>/T, each segment without '/'
        let mut segs = after.splitn(3, '/');
        let (a, b, tail) = (segs.next(), segs.next(), segs.next());
        let matched = match (a, b, tail) {
            (Some(a), Some(b), Some(tail))
                if !a.is_empty()
                    && !b.is_empty()
                    && tail
                        .strip_prefix('T')
                        .is_some_and(|r| r.is_empty() || r.starts_with('/')) =>
            {
                Some(a.len() + 1 + b.len() + 1 + 1)
            }
            _ => None,
        };
        let start = if rest[..pos].ends_with("/private") {
            pos - "/private".len()
        } else {
            pos
        };
        match matched {
            Some(skip) => {
                out.push_str(&rest[..start]);
                out.push_str("/tmp");
                rest = &after[skip..];
            }
            None => {
                out.push_str(&rest[..pos + MARK.len()]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const REAL: &str = "/private/var/folders/y7/ch2jnhf54fb3wcbbk9xfdy6w0000gn/T/tmp.1z8JyCYZfd";

    fn sanitizer() -> FixtureSanitizer {
        // The directory doesn't exist, so canonicalize falls back to the given path.
        FixtureSanitizer::new("/Users/me", "me", &[], Some(&PathBuf::from(REAL)))
    }

    #[test]
    fn maps_both_forms_in_json_prose_and_file_urls() {
        let s = sanitizer();
        let non_private = REAL.strip_prefix("/private").unwrap();
        let lines = [
            format!(r#"{{"cwd":"{REAL}","mcpServers":[]}}"#),
            format!(r#"{{"path":"{non_private}/secret.txt"}}"#),
            format!(r#"{{"text":"Read file {REAL}/secret.txt, lines 1-1\n1: x"}}"#),
            format!(r#"{{"uri":"file://{REAL}/Cargo.toml"}}"#),
            format!(r#"{{"uri":"file://{non_private}/Cargo.toml"}}"#),
            format!("error: cannot open {non_private} for writing"),
        ];
        for line in lines {
            let out = s.line(&line);
            assert!(!contains_temp_path(&out), "temp path survived: {out}");
            assert!(s.is_clean(&out), "not clean: {out}");
            assert!(out.contains(SCRATCH), "scratch not mapped: {out}");
        }
        assert_eq!(
            s.line(&format!(r#"{{"uri":"file://{REAL}/Cargo.toml"}}"#)),
            r#"{"uri":"file:///scratch/Cargo.toml"}"#
        );
    }

    #[test]
    fn other_temp_roots_become_tmp_and_siblings_are_not_swallowed() {
        let s = sanitizer();
        let root = "/private/var/folders/y7/ch2jnhf54fb3wcbbk9xfdy6w0000gn/T";
        let out = s.text(&format!("{root}/opencode/tool-output/x and {REAL}Z/y"));
        assert_eq!(
            out,
            "/tmp/opencode/tool-output/x and /tmp/tmp.1z8JyCYZfdZ/y"
        );
        assert!(!contains_temp_path(&out));
    }

    #[test]
    fn is_clean_fails_when_a_temp_path_is_left() {
        let s = sanitizer();
        assert!(!s.is_clean(&format!("cwd {REAL}")));
        assert!(!s.is_clean("/var/folders/ab/cd/T/x"));
        assert!(s.is_clean("/scratch/secret.txt and /tmp/x"));
    }

    #[test]
    fn contract_sanitizing_still_applies() {
        let s = sanitizer();
        let out = s.text("/Users/me/src for me at /private/var/folders/aa/bb/T/z");
        assert_eq!(out, "~/src for user at /tmp/z");
    }

    #[test]
    fn sanitizes_gate_and_stdin_inputs() {
        let s = sanitizer();
        let mut input = AdapterInput::Stdin {
            line: format!(r#"{{"cwd":"{REAL}"}}"#),
        };
        s.input(&mut input);
        match input {
            AdapterInput::Stdin { line } => assert_eq!(line, r#"{"cwd":"/scratch"}"#),
            other => panic!("unexpected {other:?}"),
        }
    }
}
