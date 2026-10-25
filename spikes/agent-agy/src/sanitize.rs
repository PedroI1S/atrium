//! Fixture sanitizer for recorded `agy` sessions.
//!
//! `atrium_agents::fixture::Sanitizer` (a contract type, not editable here) removes secrets, the
//! home directory and the user name. It does not know about the scratch directory a live
//! recording runs in, and on macOS that directory lives under the per-user temp dir
//! (`/var/folders/<id>/T/...`, reported by `agy` as its realpath `/private/var/folders/...`,
//! sometimes as a `file://` URL). Committing those leaks a per-user id, so [`FixtureSanitizer`]
//! maps them first and then runs the contract sanitizer:
//!
//! - the scratch directory (as given, its realpath, and the `/private` <-> non-`/private` forms)
//!   becomes `/scratch`;
//! - any other temp-dir root (`std::env::temp_dir()`, `$TMPDIR`, and any
//!   `/var/folders/<id>/<id>/T`-shaped path, with or without `/private`) becomes
//!   `/scratch-tmp`;
//! - `file://` URLs follow for free: `file:///var/folders/...` becomes `file:///scratch/...`.
//!
//! [`FixtureSanitizer::is_clean`] fails if a temp-dir path survives, so a recorder or test that
//! asserts it can never commit one.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use atrium_agents::AdapterInput;
use atrium_agents::fixture::Sanitizer;
use serde_json::Value;

/// What the scratch working directory becomes in recorded fixtures.
pub const SCRATCH_PLACEHOLDER: &str = "/scratch";
/// What any other temp-dir root becomes in recorded fixtures.
pub const TEMP_ROOT_PLACEHOLDER: &str = "/scratch-tmp";

pub struct FixtureSanitizer {
    inner: Sanitizer,
    /// Longest first.
    scratch: Vec<String>,
    /// Longest first.
    temp_roots: Vec<String>,
}

impl FixtureSanitizer {
    pub fn new(home: &str, user: &str, secrets: &[&str], scratch_dirs: &[&Path]) -> Self {
        let mut scratch = BTreeSet::new();
        for dir in scratch_dirs {
            scratch.extend(path_forms(dir));
        }
        Self {
            inner: Sanitizer::new(home, user, secrets),
            scratch: longest_first(scratch),
            temp_roots: longest_first(current_temp_roots()),
        }
    }

    /// Replaces scratch and temp-dir paths in `s`. Does nothing else.
    pub fn map_paths(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for form in &self.scratch {
            out = replace_path(&out, form, SCRATCH_PLACEHOLDER);
        }
        for root in &self.temp_roots {
            out = replace_path(&out, root, TEMP_ROOT_PLACEHOLDER);
        }
        replace_var_folders(&out)
    }

    pub fn text(&self, s: &str) -> String {
        self.inner.text(&self.map_paths(s))
    }

    pub fn line(&self, line: &str) -> String {
        self.inner.line(&self.map_line(line))
    }

    pub fn value(&self, value: &mut Value) {
        self.map_value(value);
        self.inner.value(value);
    }

    pub fn input(&self, input: &mut AdapterInput) {
        match &mut *input {
            AdapterInput::Stdin { line }
            | AdapterInput::Stdout { line }
            | AdapterInput::Stderr { line } => *line = self.map_line(line),
            other => {
                if let Ok(mut v) = serde_json::to_value(&*other) {
                    self.map_value(&mut v);
                    if let Ok(back) = serde_json::from_value::<AdapterInput>(v) {
                        *other = back;
                    }
                }
            }
        }
        self.inner.input(input);
    }

    /// True if the contract sanitizer finds nothing **and** no scratch or temp-dir path is left.
    pub fn is_clean(&self, text: &str) -> bool {
        self.inner.is_clean(text)
            && leaked_temp_path(text).is_none()
            && self.scratch.iter().all(|f| find_path(text, f, 0).is_none())
    }

    fn map_line(&self, line: &str) -> String {
        match serde_json::from_str::<Value>(line) {
            Ok(mut v) => {
                self.map_value(&mut v);
                v.to_string()
            }
            Err(_) => self.map_paths(line),
        }
    }

    fn map_value(&self, value: &mut Value) {
        match value {
            Value::String(s) => *s = self.map_paths(s),
            Value::Array(items) => items.iter_mut().for_each(|v| self.map_value(v)),
            Value::Object(map) => map.values_mut().for_each(|v| self.map_value(v)),
            _ => {}
        }
    }
}

/// The first temp-dir path found in `text`, as a short snippet, if any. Looks for the macOS
/// per-user temp dir (`/var/folders/`, `/private/var/`, `/private/tmp`) and for this
/// machine's `std::env::temp_dir()` / `$TMPDIR` (with their `/private` forms).
pub fn leaked_temp_path(text: &str) -> Option<String> {
    let mut needles: Vec<String> = ["/var/folders/", "/private/var/", "/private/tmp"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    needles.extend(current_temp_roots());
    for needle in &needles {
        if let Some(pos) = find_path(text, needle, 0) {
            let end = (pos + needle.len() + 24).min(text.len());
            let end = (end..=text.len())
                .find(|i| text.is_char_boundary(*i))
                .unwrap_or(text.len());
            return Some(text[pos..end].to_owned());
        }
    }
    None
}

/// `path` as given, its realpath if it exists, and the `/private` and non-`/private` spellings
/// of each (macOS: `/var`, `/tmp` and `/etc` are symlinks into `/private`).
pub fn path_forms(path: &Path) -> BTreeSet<String> {
    let mut bases = vec![path.to_string_lossy().into_owned()];
    if let Ok(real) = fs::canonicalize(path) {
        bases.push(real.to_string_lossy().into_owned());
    }
    let mut out = BTreeSet::new();
    for base in bases {
        let base = base.trim_end_matches('/').to_owned();
        if base.is_empty() {
            continue;
        }
        if let Some(rest) = base.strip_prefix("/private")
            && rest.starts_with('/')
        {
            out.insert(rest.to_owned());
        } else if ["/var/", "/tmp/", "/etc/"]
            .iter()
            .any(|t| base.starts_with(t))
        {
            out.insert(format!("/private{base}"));
        }
        out.insert(base);
    }
    out
}

fn current_temp_roots() -> BTreeSet<String> {
    let mut roots = BTreeSet::new();
    roots.extend(path_forms(&std::env::temp_dir()));
    if let Some(t) = std::env::var_os("TMPDIR") {
        roots.extend(path_forms(Path::new(&t)));
    }
    roots.retain(|r| r.len() > 1);
    roots
}

fn longest_first(set: BTreeSet<String>) -> Vec<String> {
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort_by_key(|s| std::cmp::Reverse(s.len()));
    v
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Position of the first occurrence of `prefix` at or after `from` that starts a path (the
/// previous character is not part of a longer name or path segment) and ends a path segment
/// (the next character does not continue a name).
fn find_path(text: &str, prefix: &str, from: usize) -> Option<usize> {
    if prefix.is_empty() {
        return None;
    }
    let mut start = from;
    while let Some(rel) = text.get(start..)?.find(prefix) {
        let pos = start + rel;
        let before = text[..pos].chars().next_back();
        let after = text[pos + prefix.len()..].chars().next();
        let before_ok = !before.is_some_and(|c| is_name_char(c) || c == '.');
        let after_ok = prefix.ends_with('/') || !after.is_some_and(is_name_char);
        if before_ok && after_ok {
            return Some(pos);
        }
        start = pos + prefix.len();
    }
    None
}

fn replace_path(text: &str, prefix: &str, with: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(pos) = find_path(text, prefix, cursor) {
        out.push_str(&text[cursor..pos]);
        out.push_str(with);
        cursor = pos + prefix.len();
    }
    out.push_str(&text[cursor..]);
    out
}

/// Replaces any macOS per-user temp root, `[/private]/var/folders/<id>/<id>/<T|C|0>`, whoever's
/// it is (so a fixture recorded on another machine is cleaned too).
fn replace_var_folders(text: &str) -> String {
    const MARK: &str = "/var/folders/";
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(rel) = text[cursor..].find(MARK) {
        let mark_pos = cursor + rel;
        let mut start = mark_pos;
        if text[..mark_pos].ends_with("/private") {
            start -= "/private".len();
        }
        // Consume up to three more segments: two ids, then T / C / 0 / X.
        let mut end = mark_pos + MARK.len() - 1;
        for i in 0..3 {
            let Some(after) = text[end..].strip_prefix('/') else {
                break;
            };
            let seg_len = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            if seg_len == 0 || (i == 2 && !matches!(&after[..seg_len], "T" | "C" | "0" | "X")) {
                break;
            }
            end += 1 + seg_len;
        }
        out.push_str(&text[cursor..start.max(cursor)]);
        out.push_str(TEMP_ROOT_PLACEHOLDER);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}
