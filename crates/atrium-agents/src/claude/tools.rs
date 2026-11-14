//! Claude Code's tools as Atrium sees them: kind, title and the subject a
//! policy decision is made on.

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::ToolSubject;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::ToolCheck;

/// Maps Claude Code's tool name to Atrium's coarse [`ToolKind`].
pub fn tool_kind(name: &str) -> ToolKind {
    if name.starts_with("mcp__") {
        return ToolKind::Mcp;
    }
    match name {
        "Read" => ToolKind::Read,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ToolKind::Edit,
        "Glob" | "Grep" | "LS" | "ToolSearch" => ToolKind::Search,
        "Bash" | "BashOutput" | "KillShell" | "KillBash" | "Monitor" | "PowerShell" => {
            ToolKind::Execute
        }
        "WebFetch" | "WebSearch" => ToolKind::Fetch,
        _ => ToolKind::Other,
    }
}

/// A short human-readable summary of a tool call: the command line, the file
/// path, the URL...
pub fn tool_title(name: &str, input: &Value) -> Option<String> {
    let s = |key: &str| input.get(key).and_then(Value::as_str).map(str::to_owned);
    match name {
        "Bash" | "PowerShell" => s("command"),
        "Read" | "Edit" | "MultiEdit" | "Write" => s("file_path"),
        "NotebookEdit" => s("notebook_path"),
        "Glob" | "Grep" => s("pattern"),
        "WebFetch" => s("url"),
        "WebSearch" => s("query"),
        "Task" | "Agent" => s("description"),
        "Skill" => s("skill"),
        "ToolSearch" => s("query"),
        _ if name.starts_with("mcp__") => Some(name.trim_start_matches("mcp__").replace("__", ".")),
        _ => None,
    }
}

/// What the call acts on, for the policy engine (contract §5, review finding
/// 3). Anything this can't extract is `Unknown`, which the policy asks about.
pub fn tool_subject(name: &str, input: &Value) -> ToolSubject {
    let s = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let paths = |key: &str| match s(key) {
        Some(p) => ToolSubject::Paths {
            paths: vec![p],
            glob: None,
            recursive: false,
        },
        None => ToolSubject::Unknown,
    };
    match name {
        "Bash" | "PowerShell" => match s("command") {
            Some(line) => ToolSubject::Command { line, cwd: None },
            None => ToolSubject::Unknown,
        },
        "Read" | "Edit" | "MultiEdit" | "Write" => paths("file_path"),
        "NotebookEdit" => paths("notebook_path"),
        // Searches give their root, the pattern that scopes them, and, when
        // the pattern leaves the root, the place it points at.
        "Glob" => search_subject(input, "pattern", true),
        "Grep" => search_subject(input, "glob", false),
        "WebFetch" => match s("url") {
            Some(url) => ToolSubject::Fetch { url },
            None => ToolSubject::Unknown,
        },
        _ => match mcp_parts(name) {
            Some((server, tool)) => ToolSubject::Mcp { server, tool },
            None => ToolSubject::Unknown,
        },
    }
}

/// `mcp__<server>__<tool>`: the server name can't contain `__`, the tool name
/// can.
fn mcp_parts(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then(|| (server.to_owned(), tool.to_owned()))
}

/// The [`ToolCheck`] for a `PreToolUse` hook callback or a `can_use_tool`
/// request. `turn` is the adapter's own turn counter.
pub fn tool_check(call_id: Option<&str>, name: &str, input: &Value, turn: u32) -> ToolCheck {
    let subject = tool_subject(name, input);
    let kind = if matches!(subject, ToolSubject::Command { .. }) {
        ToolKind::Execute
    } else {
        tool_kind(name)
    };
    ToolCheck {
        call_id: call_id.map(str::to_owned),
        name: name.to_owned(),
        kind,
        title: tool_title(name, input),
        input: input.clone(),
        subject,
        turn,
    }
}

/// What ties a gate decision to a `can_use_tool` request: the `tool_use_id`
/// (or none) together with the tool and its exact input, so a ticket made for
/// one input is never reused for another.
pub(crate) fn gate_key(call_id: Option<&str>, name: &str, input: &Value) -> String {
    let mut body = String::new();
    canonical_json(input, &mut body);
    let digest = Sha256::digest(format!("{name}\0{body}").as_bytes());
    format!("{}:{digest:x}", call_id.unwrap_or("anon"))
}

/// JSON with object keys sorted (recursively) and no insignificant
/// whitespace: what `atrium_core::policy::canonical_input` writes for the
/// input, so a re-serialized input (the CLI may order keys differently
/// between a hook call and `can_use_tool`, and `serde_json` here keeps the
/// wire order) gives the same key. `atrium-core` depends on this crate, so it
/// can't be called from here; the unit test below and `tests/claude_review.rs` pin the same text.
fn canonical_json(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canonical_json(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical_json(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The subject of `Glob` and `Grep`.
///
/// - `path` (the search root) is `.` when absent; present but empty or not a
///   string, the subject is `Unknown`.
/// - `Glob` needs a `pattern`; `Grep` needs its regex `pattern` (a string)
///   and may carry a `glob` filter. Any present-but-bad field is `Unknown`.
/// - The glob follows a small grammar ([`in_grammar`]); anything outside it,
///   and a root that starts with `~` other than as `~` or `~/…`, is
///   `Unknown`.
/// - Every brace expansion with a `/` in it is joined to the root and put in
///   `paths`, so the policy's own checks see the whole pattern (a credential
///   glob of more than one component can stay inside its root). One that
///   leaves the root (absolute, `~`, or with `..`) also puts the directory it
///   points into there.
fn search_subject(input: &Value, glob_key: &str, glob_is_required: bool) -> ToolSubject {
    // Both tools need a `pattern` (Glob: the glob itself; Grep: the regex).
    if !matches!(input.get("pattern"), Some(Value::String(p)) if !p.is_empty()) {
        return ToolSubject::Unknown;
    }
    let base = match input.get("path") {
        None => ".".to_owned(),
        Some(Value::String(p)) if !p.is_empty() && tilde_ok(p) => p.clone(),
        Some(_) => return ToolSubject::Unknown,
    };
    let glob = match input.get(glob_key) {
        None if glob_is_required => return ToolSubject::Unknown,
        None => None,
        Some(Value::String(g)) if !g.is_empty() => Some(g.clone()),
        Some(_) => return ToolSubject::Unknown,
    };
    let mut paths = vec![base.clone()];
    if let Some(g) = &glob {
        let Some(extra) = glob_targets(&base, g) else {
            return ToolSubject::Unknown;
        };
        for p in extra {
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
    }
    ToolSubject::Paths {
        paths,
        glob,
        recursive: true,
    }
}

/// Most alternatives a brace group may expand to before the glob is given up
/// on.
const MAX_EXPANSIONS: usize = 32;

/// What a glob needs in `paths` besides the base (see [`glob_target`]), or
/// `None` if it is outside the grammar or can't be bounded.
fn glob_targets(base: &str, glob: &str) -> Option<Vec<String>> {
    if !in_grammar(glob) {
        return None;
    }
    let mut out = Vec::new();
    for e in expand_braces(glob)? {
        for p in glob_target(base, &e)? {
            if !out.contains(&p) {
                out.push(p);
            }
        }
        if out.len() > MAX_TARGETS {
            return None;
        }
    }
    Some(out)
}

/// Most entries a glob may add to `paths` before it is given up on.
const MAX_TARGETS: usize = 64;

/// A path may start with `~` only as `~` or `~/…`: `~user`, `~*` and the
/// like may be another user's home, which the policy can't place.
fn tilde_ok(s: &str) -> bool {
    !s.starts_with('~') || s == "~" || s.starts_with("~/")
}

/// The grammar of a `Glob` pattern or a `Grep` glob we understand (the
/// escalation, D-071), checked on the raw pattern before any expansion, so
/// braces and classes can't be interleaved to make something else once
/// expanded:
///
/// - no backslash (on Unix it escapes);
/// - `~` only as [`tilde_ok`] allows;
/// - a brace group `{…}` holds only ASCII letters, digits, `_`, `-`, `.`
///   and `,`: no nested group, no empty alternative, no alternative that is
///   `.` or `..` (so no expansion can make a `..` segment, a `/` or a `~`);
/// - a class `[…]` holds only ASCII letters, digits, `_`, and ranges within
///   one of `a-z`, `A-Z`, `0-9` (low to high): no negation, no `[:class:]`,
///   nothing that can be `.`, `/` or `~`;
/// - any other `{`, `}`, `[` or `]` is outside it.
fn in_grammar(glob: &str) -> bool {
    if glob.contains('\\') || !tilde_ok(glob) {
        return false;
    }
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let body = match chars[i] {
            '{' => brace_body(&chars[i + 1..]),
            '[' => class_body(&chars[i + 1..]),
            '}' | ']' => return false,
            _ => {
                i += 1;
                continue;
            }
        };
        let Some(len) = body else {
            return false;
        };
        // The opening character, the body and the closing one.
        i += len + 2;
    }
    true
}

/// The length of a brace group's body (what follows `{`, up to its `}`), if
/// the group is in the grammar.
fn brace_body(rest: &[char]) -> Option<usize> {
    let end = rest.iter().position(|&c| c == '}')?;
    let body = &rest[..end];
    if !body
        .iter()
        .all(|&c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ','))
    {
        return None;
    }
    let text: String = body.iter().collect();
    text.split(',')
        .all(|alt| !matches!(alt, "" | "." | ".."))
        .then_some(end)
}

/// The length of a class's body (what follows `[`, up to its `]`), if the
/// class is in the grammar.
fn class_body(rest: &[char]) -> Option<usize> {
    let end = rest.iter().position(|&c| c == ']')?;
    let body = &rest[..end];
    if body.is_empty() {
        return None;
    }
    let one_of = |lo: char, hi: char| {
        lo <= hi
            && [
                char::is_ascii_lowercase,
                char::is_ascii_uppercase,
                char::is_ascii_digit,
            ]
            .iter()
            .any(|f| f(&lo) && f(&hi))
    };
    let mut k = 0;
    while k < body.len() {
        let c = body[k];
        if !(c.is_ascii_alphanumeric() || c == '_') {
            return None;
        }
        if body.get(k + 1) == Some(&'-') {
            // A range: its end is there and in the same run as `c`.
            if !one_of(c, *body.get(k + 2)?) {
                return None;
            }
            k += 3;
        } else {
            k += 1;
        }
    }
    Some(end)
}

/// All the strings a glob's braces stand for (nested groups included), or
/// `None` for an unbalanced one or past [`MAX_EXPANSIONS`].
fn expand_braces(glob: &str) -> Option<Vec<String>> {
    let Some(open) = glob.find('{') else {
        return if glob.contains('}') {
            None
        } else {
            Some(vec![glob.to_owned()])
        };
    };
    // The matching close.
    let mut depth = 0usize;
    let mut close = None;
    for (i, c) in glob[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    // Alternatives at the group's top level.
    let inner = &glob[open + 1..close];
    let mut alts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    for (i, c) in inner.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                alts.push(&inner[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    alts.push(&inner[start..]);
    let (head, tail) = (&glob[..open], &glob[close + 1..]);
    let mut all = Vec::new();
    for alt in alts {
        all.extend(expand_braces(&format!("{head}{alt}{tail}"))?);
        if all.len() > MAX_EXPANSIONS {
            return None;
        }
    }
    Some(all)
}

/// What one brace-free glob needs besides the base:
///
/// - inside the base: nothing for a single component (the policy matches it
///   against file names), the pattern joined to the base for more than one
///   (round 3, major 3: `.ssh/id_ed25519` from the home stays inside it);
/// - leaving it: the directory it points into and the pattern joined to the
///   base.
///
/// `None` if it can't be bounded.
fn glob_target(base: &str, glob: &str) -> Option<Vec<String>> {
    const WILDCARDS: [char; 3] = ['*', '?', '['];
    let first = glob.find(WILDCARDS);
    let (prefix, rest) = glob.split_at(first.unwrap_or(glob.len()));
    let climbs = |text: &str| text.split('/').any(|seg| seg == "..");
    // After the first wildcard nothing may climb out.
    if climbs(rest) {
        return None;
    }
    let join = |p: &str| {
        if p.starts_with('/') || p.starts_with('~') {
            p.to_owned()
        } else {
            format!("{}/{}", base.trim_end_matches('/'), p)
        }
    };
    let leaves = prefix.starts_with('/') || prefix.starts_with('~') || climbs(prefix);
    if !leaves {
        return if glob.contains('/') {
            pattern_forms(&join(glob))
        } else {
            Some(vec![])
        };
    }
    if first.is_none() {
        // No wildcard: the whole thing is the target.
        return pattern_forms(&join(glob));
    }
    let dir = match prefix.rfind('/') {
        Some(i) => &prefix[..=i],
        None if prefix.starts_with('~') => "~",
        None => "",
    };
    let mut out = vec![join(dir)];
    out.extend(pattern_forms(&join(glob))?);
    Some(out)
}

/// Most components of one pattern that start with a wildcard (each doubles
/// the forms [`pattern_forms`] gives).
const MAX_WILD_COMPONENTS: usize = 4;

/// The forms of a pattern joined to the base that go into `paths`, so the
/// policy sees every file the tool's glob can match (the escalation's
/// exhaustive test). The policy matches a `paths` entry as the shell does:
/// `*`, `?` or a class never match a leading `.`, and an empty component
/// matches nothing; the tool's glob has neither rule (`~/*` lists
/// `~/.netrc`, `..//*` is `../*`). So:
///
/// - empty and `.` components are dropped (`a//./b` is `a/b`; a trailing
///   `/` too, which can only make the match wider);
/// - unless the pattern has a `**` (the policy then matches it as a whole,
///   and a dot is not special), every way of putting a `.` in front of the
///   components that start with a wildcard is added (`~/*` also gives
///   `~/.*`).
///
/// `None` (the subject is then `Unknown`) past [`MAX_WILD_COMPONENTS`].
fn pattern_forms(joined: &str) -> Option<Vec<String>> {
    let root = if joined.starts_with('/') { "/" } else { "" };
    let parts: Vec<&str> = joined
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .collect();
    let wild: Vec<usize> = if joined.contains("**") {
        vec![]
    } else {
        (0..parts.len())
            .filter(|&i| parts[i].starts_with(['*', '?', '[']))
            .collect()
    };
    if wild.len() > MAX_WILD_COMPONENTS {
        return None;
    }
    let forms = (0..1u32 << wild.len())
        .map(|mask| {
            let body: Vec<String> = parts
                .iter()
                .enumerate()
                .map(|(i, c)| match wild.iter().position(|&w| w == i) {
                    Some(bit) if mask & (1 << bit) != 0 => format!(".{c}"),
                    _ => (*c).to_owned(),
                })
                .collect();
            match format!("{root}{}", body.join("/")) {
                // `./` and the like: the base itself.
                empty if empty.is_empty() => ".".to_owned(),
                form => form,
            }
        })
        .collect();
    Some(forms)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The text `atrium_core::policy::canonical_input` writes for the same
    /// value (`tests/claude_review.rs` checks it against the real function).
    pub(crate) const CANONICAL: &str = r#"{"a":[{"x":1,"y":2}],"b":"é\n","c":null}"#;

    #[test]
    fn canonical_json_sorts_keys_and_keeps_array_order() {
        let v = json!({"c":null,"b":"é\n","a":[{"y":2,"x":1}]});
        let mut out = String::new();
        canonical_json(&v, &mut out);
        assert_eq!(out, CANONICAL);
    }
}
