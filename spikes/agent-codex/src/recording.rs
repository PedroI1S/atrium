//! Recorder-only isolation and redaction. Ordinary sessions keep native config.
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::process::Command;

use atrium_agents::fixture::{self, FixtureLine, Sanitizer};
use serde_json::{Value, json};

use crate::CodexNormalizer;
use atrium_agents::{AdapterInput, GateDecision};
use atrium_protocol::agent::{AgentEvent, ToolStatus};

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
                if matches!(
                    key.as_str(),
                    "installationId" | "installation_id" | "sourcePath" | "hookName" | "pluginName"
                ) && let Some(id) = value.as_str().filter(|id| !id.is_empty())
                {
                    identifiers.push(id.into());
                }
                collect_identifiers(value, identifiers);
            }
            for key in ["mcp_servers", "plugins"] {
                if let Some(entries) = map.get(key).and_then(Value::as_object) {
                    identifiers.extend(entries.keys().cloned());
                }
            }
            if map
                .get("method")
                .and_then(Value::as_str)
                .is_some_and(|m| m.starts_with("mcpServer/") || m.starts_with("hook/"))
            {
                fn names(v: &Value, ids: &mut Vec<String>) {
                    if let Some(o) = v.as_object() {
                        for (k, v) in o {
                            if matches!(
                                k.as_str(),
                                "name" | "serverName" | "server" | "id" | "sourcePath"
                            ) && let Some(s) = v.as_str().filter(|s| !s.is_empty())
                            {
                                ids.push(s.into());
                            }
                            names(v, ids);
                        }
                    } else if let Some(a) = v.as_array() {
                        for v in a {
                            names(v, ids);
                        }
                    }
                }
                names(&value["params"], identifiers);
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
        if value["dir"] == "stderr" {
            value["line"] = json!("<recorder-stderr>");
        }
        // Normalize invocation scratch roots before generic home substitution.
        replace_scratch(value);

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
    write_with_secrets(path, lines, &[])
}

pub fn write_with_secrets(
    path: &Path,
    lines: &[FixtureLine],
    extra: &[String],
) -> io::Result<Vec<FixtureLine>> {
    let secrets = std::env::vars()
        .filter(|(k, _)| {
            k.contains("TOKEN") || k.contains("SECRET") || k.contains("API_KEY") || k == "PASSWORD"
        })
        .map(|(_, v)| v)
        .chain(extra.iter().cloned())
        .filter(|s| !s.is_empty())
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

fn replace_scratch(value: &mut Value) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("live/scratch")
        .to_string_lossy()
        .into_owned();
    match value {
        Value::String(s) => *s = s.replace(&root, "/scratch"),
        Value::Array(a) => a.iter_mut().for_each(replace_scratch),
        Value::Object(o) => o.values_mut().for_each(replace_scratch),
        _ => {}
    }
}

/// Raw enforcement evidence, checked BEFORE any sanitizer or fixture write.
/// The sentinel is deliberately never supplied as a redaction secret.
pub fn verify_enforcement(scenario: &str, lines: &[FixtureLine]) -> io::Result<Vec<String>> {
    if !scenario.starts_with("gate-deny") {
        return Ok(vec![]);
    }
    let read = scenario == "gate-deny";
    let label = if read { "read" } else { "execute" };
    let decoded = decoded(lines)?;
    let leaked = decoded
        .iter()
        .any(|(v, _)| v.to_string().contains("FIXTURE_SECRET_MUST_NOT_APPEAR"));
    let denied_calls = lines
        .iter()
        .filter_map(|l| match &l.input {
            AdapterInput::Gate {
                check,
                decision: GateDecision::Deny { .. },
            } if if read {
                check.input.to_string().contains("secret.txt")
            } else {
                check.kind == atrium_protocol::agent::ToolKind::Execute
            } =>
            {
                check.call_id.as_deref()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let denied = !denied_calls.is_empty();
    let events = fixture::normalize(lines, &mut CodexNormalizer::default());
    let finished=events.iter().any(|e|matches!(e,AgentEvent::ToolCallFinished{call_id,status:ToolStatus::Denied,..} if denied_calls.contains(&call_id.as_str())));
    // Check native success too: the mapper intentionally marks denied call ids
    // Denied, so normalized status alone must never mask provider execution.
    let native_success = decoded.iter().any(|(v, _)| {
        let rpc = &v["line"];
        let item = &rpc["params"]["item"];
        rpc["method"] == "item/completed"
            && item["status"] == "completed"
            && (item["id"]
                .as_str()
                .is_some_and(|id| denied_calls.contains(&id))
                || if read {
                    item.to_string().contains("secret.txt")
                } else {
                    item["type"] == "commandExecution"
                })
    });
    let mut checks = vec![];
    for (claim, pass) in [
        ("target gate denied", denied),
        ("denied call finished", finished),
        ("sentinel absent from raw stream", !leaked),
        ("no native target execution", !native_success),
    ] {
        checks.push(format!(
            "{} {label}: {claim}",
            if pass { "PASS" } else { "FAIL" }
        ));
    }
    if leaked || !denied || !finished || native_success {
        return Err(io::Error::other(format!(
            "{}\n{label} bypassed the gate: refusing fixture write",
            checks.join("\n")
        )));
    }
    Ok(checks)
}

/// Identifiers from the effective config also redact unstructured diagnostics.
pub fn config_secrets(config: &Value) -> Vec<String> {
    let mut result = vec![];
    collect_identifiers(config, &mut result);
    // CLI TOML string keys contain JSON-style escapes; redact that spelling too.
    let escaped = result
        .iter()
        .filter_map(|s| {
            let encoded = json!(s).to_string();
            let encoded = encoded[1..encoded.len() - 1].to_owned();
            (encoded != *s).then_some(encoded)
        })
        .collect::<Vec<_>>();
    result.extend(escaped);
    result
}

/// Sanitize invocation metadata using the same decoded-value privacy pass.
pub fn sanitize_arguments(args: &[String], secrets: &[String]) -> io::Result<Vec<String>> {
    let lines = args
        .iter()
        .map(|text| FixtureLine {
            ms: 0,
            input: AdapterInput::Command {
                command: atrium_protocol::agent::SessionCommand::SendUserMessage {
                    text: text.clone(),
                },
            },
        })
        .collect::<Vec<_>>();
    let (cleaned, _) = clean(
        &lines,
        &hostnames()?,
        &std::env::var("HOME").unwrap_or_default(),
        &std::env::var("USER").unwrap_or_default(),
        secrets.to_vec(),
    )?;
    Ok(cleaned
        .into_iter()
        .filter_map(|l| match l.input {
            AdapterInput::Command {
                command: atrium_protocol::agent::SessionCommand::SendUserMessage { text },
            } => Some(text),
            _ => None,
        })
        .collect())
}

/// An invalid model is not a startup fixture if a native thread was created.
pub fn verify_startup_failure(lines: &[FixtureLine]) -> io::Result<()> {
    let events = fixture::normalize(lines, &mut CodexNormalizer::default());
    if matches!(events.first(), Some(AgentEvent::Error { fatal: true, .. }))
        && !events
            .iter()
            .any(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        && matches!(events.last(), Some(AgentEvent::SessionEnded { .. }))
    {
        Ok(())
    } else {
        Err(io::Error::other(
            "FAIL startup-error: expected fatal pre-session error then SessionEnded, no SessionStarted",
        ))
    }
}
