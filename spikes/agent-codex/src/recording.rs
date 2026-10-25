//! Recorder-only isolation and redaction. Ordinary sessions keep native config.
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::process::Command;

use atrium_agents::fixture::{self, FixtureLine, Sanitizer};
use serde_json::{Value, json};

use crate::CodexNormalizer;

/// These invocation-local overrides do not change Codex home, auth, or config files.
/// `config/read` supplies effective user/project server names. Unknown or managed
/// sources can still emit records, so fixture redaction is always applied as well.
pub fn config_overrides(config: &Value) -> Vec<String> {
    let mut result = vec!["features.hooks=false".into()];
    if let Some(servers) = config["mcp_servers"].as_object() {
        for name in servers.keys() {
            result.push(format!("mcp_servers.{}.enabled=false", json!(name)));
        }
    }
    if let Some(plugins) = config["plugins"].as_object() {
        for (plugin, config) in plugins {
            if let Some(servers) = config["mcp_servers"].as_object() {
                for name in servers.keys() {
                    result.push(format!(
                        "plugins.{}.mcp_servers.{}.enabled=false",
                        json!(plugin),
                        json!(name)
                    ));
                }
            }
        }
    }
    result
}

/// Read the OS hostname rather than trusting a possibly absent/stale HOSTNAME env.
/// Failure stops fixture writing instead of silently omitting hostname redaction.
pub fn hostname() -> io::Result<String> {
    let output = Command::new("hostname").output()?;
    let name = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    if !output.status.success() || name.trim().is_empty() {
        return Err(io::Error::other(
            "cannot determine OS hostname for fixture redaction",
        ));
    }
    Ok(name.trim().to_owned())
}

fn hostnames() -> io::Result<Vec<String>> {
    let mut names = vec![hostname()?];
    // macOS may advertise LocalHostName.local even when gethostname is an FQDN.
    #[cfg(target_os = "macos")]
    if let Ok(output) = Command::new("/usr/sbin/scutil")
        .args(["--get", "LocalHostName"])
        .output()
        && output.status.success()
        && let Ok(name) = String::from_utf8(output.stdout)
        && !name.trim().is_empty()
    {
        names.push(name.trim().to_owned());
        names.push(format!("{}.local", name.trim()));
    }
    // Full names must be replaced before their short aliases.
    names.sort_by_key(|name| std::cmp::Reverse(name.len()));
    names.dedup();
    Ok(names)
}

fn private_field(key: &str) -> bool {
    matches!(
        key,
        "installationId" | "installation_id" | "authMode" | "auth_mode" | "planType" | "plan_type"
    )
}

fn collect_identifiers(value: &Value, identifiers: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "installationId" | "installation_id")
                    && let Some(id) = value.as_str().filter(|id| !id.is_empty())
                {
                    identifiers.push(id.into());
                }
                collect_identifiers(value, identifiers);
            }
            if map.get("method").and_then(Value::as_str) == Some("remoteControl/status/changed") {
                for key in ["serverName", "environmentId"] {
                    if let Some(id) = value["params"][key].as_str().filter(|id| !id.is_empty()) {
                        identifiers.push(id.into());
                    }
                }
            }
        }
        Value::Array(values) => values
            .iter()
            .for_each(|v| collect_identifiers(v, identifiers)),
        _ => {}
    }
}

#[derive(Default)]
struct MetadataRedactor {
    servers: BTreeMap<String, String>,
    hooks: BTreeMap<String, String>,
}

fn alias(map: &mut BTreeMap<String, String>, value: &str, prefix: &str) -> String {
    let next = format!("{prefix}-{}", map.len() + 1);
    map.entry(value.into()).or_insert(next).clone()
}

/// Preserve record shape, lifecycle enums and correlation fields, but not arbitrary
/// strings from user configuration, commands, errors or hook output. This whitelist
/// is intentionally narrow: a new diagnostic text field must not leak by default.
fn diagnostic(value: &mut Value, key: &str, label: &str, family: &str) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                diagnostic(value, key, label, family);
            }
        }
        Value::Array(values) => {
            for value in values {
                diagnostic(value, key, label, family);
            }
        }
        Value::String(text) => {
            if matches!(
                key,
                "threadId"
                    | "turnId"
                    | "status"
                    | "failureReason"
                    | "eventName"
                    | "executionMode"
                    | "handlerType"
                    | "scope"
                    | "source"
                    | "kind"
            ) {
                return;
            }
            *text = match key {
                "name" | "serverName" | "server" | "hookName" | "id" => label.into(),
                "sourcePath" => format!("/fixture/hooks/{label}"),
                "command" | "commandWindows" | "command_windows" => format!("<{family}-command>"),
                "error" | "message" | "errorText" => format!("<{family}-error>"),
                _ => format!("<{family}-detail>"),
            };
        }
        _ => {}
    }
}

impl MetadataRedactor {
    fn value(&mut self, value: &mut Value) {
        // Field-based redaction applies to any decoded object, not just known
        // notifications, and retains nullable auth/plan schema types.
        match value {
            Value::Object(map) => {
                for (key, value) in map.iter_mut() {
                    if private_field(key) {
                        *value = if key.starts_with("installation") {
                            json!("<installation-id>")
                        } else {
                            Value::Null
                        };
                    } else {
                        self.value(value);
                    }
                }
            }
            Value::Array(values) => values.iter_mut().for_each(|v| self.value(v)),
            _ => {}
        }
        match value["method"].as_str() {
            Some("remoteControl/status/changed") => {
                value["params"]["serverName"] = json!("<hostname>");
                if !value["params"]["environmentId"].is_null() {
                    value["params"]["environmentId"] = json!("<environment-id>");
                }
            }
            Some("mcpServer/startupStatus/updated" | "mcpServer/status/updated") => {
                let params = &mut value["params"];
                let name = ["name", "serverName", "server"]
                    .iter()
                    .find_map(|key| params[key].as_str())
                    .unwrap_or("unknown");
                let label = alias(&mut self.servers, name, "mcp");
                diagnostic(params, "", &label, "mcp");
            }
            Some("hook/started" | "hook/completed") => {
                let params = &mut value["params"];
                let run = &params["run"];
                let name = run["id"]
                    .as_str()
                    .or_else(|| run["name"].as_str())
                    .unwrap_or("unknown");
                let label = alias(&mut self.hooks, name, "hook");
                diagnostic(params, "", &label, "hook");
            }
            _ => {}
        }
    }
}

// Decode the JSON-RPC strings nested inside FixtureLine before walking fields.
fn decoded(lines: &[FixtureLine]) -> io::Result<Vec<(Value, bool)>> {
    lines
        .iter()
        .map(|line| {
            let mut value = serde_json::to_value(line).map_err(io::Error::other)?;
            let mut parsed_line = false;
            if let Some(text) = value["line"].as_str()
                && let Ok(rpc) = serde_json::from_str::<Value>(text)
            {
                value["line"] = rpc;
                parsed_line = true;
            }
            Ok((value, parsed_line))
        })
        .collect()
}

fn clean(
    lines: &[FixtureLine],
    names: &[String],
    home: &str,
    user: &str,
    mut secrets: Vec<String>,
) -> io::Result<(Vec<FixtureLine>, Sanitizer)> {
    let mut values = decoded(lines)?;
    secrets.extend_from_slice(names);
    for (value, _) in &values {
        collect_identifiers(value, &mut secrets);
    }
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    secrets.dedup();
    let refs = secrets.iter().map(String::as_str).collect::<Vec<_>>();
    let sanitizer = Sanitizer::new(home, user, &refs);
    let mut metadata = MetadataRedactor::default();
    let mut cleaned = Vec::new();
    for (value, parsed_line) in &mut values {
        // Sensitive fields are removed structurally; generic secrets (including
        // copies of identifiers in diagnostic text) are removed before home/user.
        metadata.value(value);
        sanitizer.value(value);
        if *parsed_line {
            value["line"] = json!(value["line"].to_string());
        }
        let line: FixtureLine = serde_json::from_value(value.clone()).map_err(io::Error::other)?;
        cleaned.push(line);
    }
    Ok((cleaned, sanitizer))
}

/// Write both the sanitized transcript and goldens generated from that exact
/// transcript. Both outputs are checked before either is written, then read back.
pub fn write(path: &Path, lines: &[FixtureLine]) -> io::Result<Vec<FixtureLine>> {
    let secrets = std::env::vars()
        .filter(|(k, _)| {
            k.contains("TOKEN") || k.contains("SECRET") || k.contains("API_KEY") || k == "PASSWORD"
        })
        .map(|(_, v)| v)
        .collect();
    let (cleaned, sanitizer) = clean(
        lines,
        &hostnames()?,
        &std::env::var("HOME").unwrap_or_default(),
        &std::env::var("USER").unwrap_or_default(),
        secrets,
    )?;
    let jsonl = cleaned
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)?
        .join("\n")
        + "\n";
    let events = fixture::normalize(&cleaned, &mut CodexNormalizer::default());
    let golden = serde_json::to_string_pretty(&events).map_err(io::Error::other)? + "\n";
    if !sanitizer.is_clean(&jsonl) || !sanitizer.is_clean(&golden) {
        return Err(io::Error::other("fixture failed privacy validation"));
    }
    std::fs::write(path, jsonl)?;
    std::fs::write(path.with_extension("events.json"), golden)?;
    assert!(sanitizer.is_clean(&std::fs::read_to_string(path)?));
    assert!(sanitizer.is_clean(&std::fs::read_to_string(
        path.with_extension("events.json")
    )?));
    Ok(cleaned)
}
