//! Detects hooks in Claude Code's own settings that run outside Atrium's gate.
//!
//! By default Claude Code loads user, project and local settings, and the
//! hooks in them run on the host. A gate `Deny` still wins over any other
//! hook, but an external `PreToolUse` hook can rewrite a call's input after
//! the gate approved the original, and any command hook runs code Atrium never
//! sees. Atrium keeps the native behaviour by default (PLAN
//! §3.2, D-011), so the adapter *reports* the situation instead: the
//! enforcement level is [`Interception::ProviderGated`] ("partial") while such
//! hooks are present.
//!
//! Not inspected: hooks shipped by enabled plugins, and `--settings` files
//! passed in `ClaudeOptions::extra_args`.

use std::fs;
use std::path::{Path, PathBuf};

use atrium_agents::Interception;
use serde_json::Value;

/// One hook handler found in a settings file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalHook {
    /// The settings file it was found in.
    pub file: PathBuf,
    /// `user`, `project`, `local` or `managed`.
    pub scope: &'static str,
    /// The hook event, e.g. `PreToolUse`.
    pub event: String,
    /// The handler type (`command`, `http`, `prompt`, `agent`...).
    pub kind: String,
    /// The command line or URL, truncated.
    pub detail: String,
}

/// How much of the session's activity Atrium's gate decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enforcement {
    /// `AllToolCalls` when nothing outside the gate can act on tool calls;
    /// `ProviderGated` ("partial") otherwise.
    pub interception: Interception,
    pub external_hooks: Vec<ExternalHook>,
    /// Why the level is not full, for the UI and the report.
    pub reason: Option<String>,
}

impl Enforcement {
    pub fn from_hooks(external_hooks: Vec<ExternalHook>) -> Self {
        if external_hooks.is_empty() {
            return Self {
                interception: Interception::AllToolCalls,
                external_hooks,
                reason: None,
            };
        }
        let pre = external_hooks.iter().any(|h| h.event == "PreToolUse");
        let list: Vec<String> = external_hooks
            .iter()
            .take(4)
            .map(|h| format!("{} {} ({}: {})", h.scope, h.event, h.kind, h.detail))
            .collect();
        let more = external_hooks.len().saturating_sub(4);
        let reason = format!(
            "partial: {} hook(s) in Claude Code's own settings run outside the gate{}: {}{}",
            external_hooks.len(),
            if pre {
                " (a PreToolUse hook can rewrite a call's input after the gate approved it)"
            } else {
                ""
            },
            list.join("; "),
            if more > 0 {
                format!("; and {more} more")
            } else {
                String::new()
            }
        );
        Self {
            interception: Interception::ProviderGated,
            external_hooks,
            reason: Some(reason),
        }
    }
}

/// The settings files Claude Code would load for `cwd`, as
/// `(scope, path)`, honouring a `--setting-sources` value (`None` = all three).
/// Managed settings always apply.
pub fn settings_files(
    cwd: &Path,
    sources: Option<&str>,
    user_dir: &Path,
    managed: &[PathBuf],
) -> Vec<(&'static str, PathBuf)> {
    let wanted = |s: &str| match sources {
        None => true,
        Some(list) => list.split(',').any(|x| x.trim() == s),
    };
    let mut files = Vec::new();
    for m in managed {
        files.push(("managed", m.clone()));
    }
    if wanted("user") {
        files.push(("user", user_dir.join("settings.json")));
    }
    if wanted("project") || wanted("local") {
        // The project directory is the cwd or one of its parents.
        for dir in cwd.ancestors() {
            let claude = dir.join(".claude");
            if claude == user_dir {
                continue;
            }
            if wanted("project") {
                files.push(("project", claude.join("settings.json")));
            }
            if wanted("local") {
                files.push(("local", claude.join("settings.local.json")));
            }
        }
    }
    files
}

/// Reads the settings files and lists every hook handler in them.
pub fn detect_external_hooks(
    cwd: &Path,
    sources: Option<&str>,
    user_dir: &Path,
    managed: &[PathBuf],
) -> Vec<ExternalHook> {
    let mut found = Vec::new();
    for (scope, file) in settings_files(cwd, sources, user_dir, managed) {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Some(events) = v.get("hooks").and_then(Value::as_object) else {
            continue;
        };
        for (event, groups) in events {
            // `[{"matcher": "...", "hooks": [{"type": "command", ...}]}]`
            let handlers = groups.as_array().into_iter().flatten().flat_map(|g| {
                g.get("hooks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            });
            for h in handlers {
                let kind = h
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("command")
                    .to_owned();
                let detail = ["command", "url", "prompt"]
                    .iter()
                    .find_map(|k| h.get(*k).and_then(Value::as_str))
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect();
                found.push(ExternalHook {
                    file: file.clone(),
                    scope,
                    event: event.clone(),
                    kind,
                    detail,
                });
            }
        }
    }
    found
}

/// Where this machine keeps the user and managed settings (`env` can carry a
/// `CLAUDE_CONFIG_DIR` override).
pub fn default_locations(env: &[(String, String)]) -> (PathBuf, Vec<PathBuf>) {
    let get = |k: &str| {
        env.iter()
            .rev()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .or_else(|| std::env::var(k).ok())
    };
    let user_dir = get("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| get("HOME").map(|h| PathBuf::from(h).join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"));
    let managed = vec![
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"),
        PathBuf::from("/etc/claude-code/managed-settings.json"),
    ];
    (user_dir, managed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("atrium-settings-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    const HOOKS: &str = r#"{"hooks":{
        "PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"/usr/local/bin/audit.sh"}]}],
        "SessionStart":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#;

    #[test]
    fn finds_hooks_in_every_scope_the_sources_allow() {
        let root = tmp("scopes");
        let user = root.join("home/.claude");
        let cwd = root.join("proj/sub");
        fs::create_dir_all(&cwd).unwrap();
        write(&user.join("settings.json"), HOOKS);
        write(
            &root.join("proj/.claude/settings.local.json"),
            r#"{"hooks":{"PostToolUse":[{"hooks":[{"type":"http","url":"http://x"}]}]}}"#,
        );
        write(
            &root.join("proj/.claude/settings.json"),
            r#"{"permissions":{}}"#,
        );

        let all = detect_external_hooks(&cwd, None, &user, &[]);
        let names: Vec<_> = all.iter().map(|h| (h.scope, h.event.as_str())).collect();
        assert!(names.contains(&("user", "PreToolUse")));
        assert!(names.contains(&("user", "SessionStart")));
        assert!(names.contains(&("local", "PostToolUse")));
        assert_eq!(all.len(), 3);

        // External settings disabled: nothing is loaded, nothing to report.
        assert!(detect_external_hooks(&cwd, Some(""), &user, &[]).is_empty());
        // Only the project scopes.
        let p = detect_external_hooks(&cwd, Some("project,local"), &user, &[]);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].scope, "local");
    }

    #[test]
    fn managed_settings_apply_even_when_sources_are_empty() {
        let root = tmp("managed");
        let managed = root.join("managed-settings.json");
        write(&managed, HOOKS);
        let h = detect_external_hooks(&root, Some(""), &root.join("u"), &[managed]);
        assert_eq!(h.len(), 2);
        assert!(h.iter().all(|h| h.scope == "managed"));
    }

    #[test]
    fn a_pre_tool_use_hook_makes_enforcement_partial_with_a_reason() {
        let root = tmp("level");
        write(&root.join("u/settings.json"), HOOKS);
        let hooks = detect_external_hooks(&root, None, &root.join("u"), &[]);
        let e = Enforcement::from_hooks(hooks);
        assert_eq!(e.interception, Interception::ProviderGated);
        let reason = e.reason.unwrap();
        assert!(
            reason.contains("PreToolUse") && reason.contains("rewrite"),
            "{reason}"
        );
        assert!(reason.contains("audit.sh"));

        let none = Enforcement::from_hooks(vec![]);
        assert_eq!(none.interception, Interception::AllToolCalls);
        assert!(none.reason.is_none());
    }

    #[test]
    fn unreadable_or_invalid_settings_are_skipped() {
        let root = tmp("bad");
        write(&root.join("u/settings.json"), "{ not json");
        assert!(detect_external_hooks(&root, None, &root.join("u"), &[]).is_empty());
    }

    #[test]
    fn config_dir_override_moves_the_user_directory() {
        let (dir, _) = default_locations(&[("CLAUDE_CONFIG_DIR".into(), "/x/y".into())]);
        assert_eq!(dir, PathBuf::from("/x/y"));
    }
}
