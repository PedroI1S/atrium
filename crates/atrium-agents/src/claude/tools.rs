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
    let digest = Sha256::digest(format!("{name}\0{input}").as_bytes());
    format!("{}:{digest:x}", call_id.unwrap_or("anon"))
}

/// The subject of `Glob` and `Grep`.
///
/// - `path` (the search root) is `.` when absent; present but empty or not a
///   string, the subject is `Unknown`.
/// - `Glob` needs a `pattern`; `Grep` needs its regex `pattern` (a string)
///   and may carry a `glob` filter. Any present-but-bad field is `Unknown`.
/// - A glob that leaves the root (absolute, `~`, or with `..`) puts the place
///   it points at (its fixed prefix up to the first wildcard, joined to the
///   root) in `paths` too, so the policy sees what the call really reads.
///   One that can't be bounded this way is `Unknown`.
fn search_subject(input: &Value, glob_key: &str, glob_is_required: bool) -> ToolSubject {
    // Both tools need a `pattern` (Glob: the glob itself; Grep: the regex).
    if !matches!(input.get("pattern"), Some(Value::String(p)) if !p.is_empty()) {
        return ToolSubject::Unknown;
    }
    let base = match input.get("path") {
        None => ".".to_owned(),
        Some(Value::String(p)) if !p.is_empty() => p.clone(),
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
        match glob_target(&base, g) {
            Ok(Some(target)) => paths.push(target),
            Ok(None) => {}
            Err(()) => return ToolSubject::Unknown,
        }
    }
    ToolSubject::Paths {
        paths,
        glob,
        recursive: true,
    }
}

/// Where a glob that leaves its base points: `Ok(None)` if it stays inside,
/// `Err` if that can't be told.
fn glob_target(base: &str, glob: &str) -> Result<Option<String>, ()> {
    const WILDCARDS: [char; 4] = ['*', '?', '[', '{'];
    // On Unix a backslash escapes: too subtle to bound.
    if glob.contains('\\') {
        return Err(());
    }
    let first = glob.find(WILDCARDS);
    let (prefix, rest) = glob.split_at(first.unwrap_or(glob.len()));
    // After the first wildcard, nothing may climb out or restart at a root
    // (`{/etc/passwd,x}`, `**/../..`).
    let climbs = |text: &str| text.split(['/', '{', '}', ',']).any(|seg| seg == "..");
    if climbs(rest)
        || rest.contains("{/")
        || rest.contains(",/")
        || rest.contains("{~")
        || rest.contains(",~")
    {
        return Err(());
    }
    let leaves = prefix.starts_with('/') || prefix.starts_with('~') || climbs(prefix);
    if !leaves {
        return Ok(None);
    }
    // No wildcard: the whole thing is the target. With one, its directory.
    let dir = if first.is_none() {
        prefix
    } else {
        match prefix.rfind('/') {
            Some(i) => &prefix[..=i],
            None if prefix.starts_with('~') => "~",
            None => "",
        }
    };
    if dir.starts_with('/') || dir.starts_with('~') {
        Ok(Some(dir.to_owned()))
    } else {
        Ok(Some(format!("{}/{}", base.trim_end_matches('/'), dir)))
    }
}
