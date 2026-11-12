//! Claude Code's tools as Atrium sees them: kind, title and the subject a
//! policy decision is made on.

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::ToolSubject;
use serde_json::Value;

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
        // Searches give their root and the pattern that scopes them.
        "Glob" => ToolSubject::Paths {
            paths: vec![s("path").unwrap_or_else(|| ".".to_owned())],
            glob: s("pattern"),
            recursive: true,
        },
        "Grep" => ToolSubject::Paths {
            paths: vec![s("path").unwrap_or_else(|| ".".to_owned())],
            glob: s("glob"),
            recursive: true,
        },
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

/// What ties a gate decision to a `can_use_tool` request: the `tool_use_id`,
/// or the tool and its exact input when the request has no id.
pub(crate) fn gate_key(call_id: Option<&str>, name: &str, input: &Value) -> String {
    match call_id {
        Some(id) => id.to_owned(),
        None => format!("anon:{name}:{input}"),
    }
}
