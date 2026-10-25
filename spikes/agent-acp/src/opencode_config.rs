//! Per-session OpenCode configuration, injected through the environment.
//!
//! Why this exists (M0-04 live finding): with its default permissions
//! (`* * allow`) OpenCode never sends `session/request_permission`, and it
//! ignores the client `fs`/`terminal` capabilities, so the gate saw nothing.
//! OpenCode's ACP `session/new` has no permission or model parameter (it only
//! uses `cwd`, `additionalDirectories` and `mcpServers`), so the one lever an
//! ACP client has is the config of the `opencode acp` process it spawns. Atrium
//! starts one process per session, so a config passed through that process's
//! environment is per-session. The user's own config files are never touched.
//!
//! The injected config:
//! - `permissions: [{"*", "*", "ask"}]`: every action (read, edit, shell,
//!   grep, glob, webfetch, websearch, subagent, skill, MCP tools,
//!   `external_directory`, ...) resolves to `ask`, which OpenCode turns into
//!   `session/request_permission`. The last matching rule wins and global
//!   rules come after the defaults, so this replaces the default `* allow`.
//!   `*` matches reads too.
//! - the same `ask` rule on the two built-in subagents (`explore`, `general`),
//!   because agent rules are appended after global rules and `explore` ships
//!   with `shell`/`read`/`grep` allows that would otherwise override it.
//! - `{"execute", "*", "deny"}` after the ask rule. OpenCode's `execute` tool
//!   (Code Mode) runs model-written JavaScript with a raw `fetch` and tool
//!   calls inside; in the second live run it completed a network fetch with no
//!   `session/request_permission`, `ask` rule or not. OpenCode's docs say a
//!   `deny` on `execute` removes Code Mode, so the tool is disabled for the
//!   session: it cannot be gated, only removed (see the report).
//! - MCP servers from `SessionConfig::mcp_servers` as `mcp.servers.<name>`
//!   with `codemode: false`. By default OpenCode reaches MCP tools only
//!   through Code Mode, which is now denied, and `codemode: false` keeps them
//!   as direct tools (`<server>_<tool>`, covered by the ask rule). They are
//!   therefore not passed in ACP `session/new` when this config is injected.
//! - `model`, when [`crate::AcpAdapter`] was given one (the adapter still
//!   verifies it over ACP afterwards).
//!
//! It is delivered twice, with the same content: as `OPENCODE_CONFIG_CONTENT`
//! (inline; outranks a project `opencode.json`, which a repository could use
//! to allow everything) and as an `OPENCODE_CONFIG` file (the path the
//! orchestrator already proved for the model in the first live run). A user's
//! own `OPENCODE_CONFIG` is kept (ours is only set when none exists); a user's
//! own `OPENCODE_CONFIG_CONTENT` is merged (ours is appended last, so it wins).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use atrium_agents::McpServer;
use serde_json::{Value, json};

/// Env var holding an inline config.
pub const ENV_CONFIG_CONTENT: &str = "OPENCODE_CONFIG_CONTENT";
/// Env var holding the path of a config file.
pub const ENV_CONFIG_FILE: &str = "OPENCODE_CONFIG";

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// The `ask`-everything rule.
fn ask_all() -> Value {
    json!({"action": "*", "resource": "*", "effect": "ask"})
}

/// Removes Code Mode (`execute`), which cannot be gated (see the module docs).
fn deny_execute() -> Value {
    json!({"action": "execute", "resource": "*", "effect": "deny"})
}

fn rules() -> Value {
    json!([ask_all(), deny_execute()])
}

/// The config document Atrium injects (see the module docs).
pub fn build_config(model: Option<&str>, mcp_servers: &[McpServer]) -> Value {
    let mut doc = json!({
        "$schema": "https://opencode.ai/config.json",
        "permissions": rules(),
        "agents": {
            "explore": {"permissions": rules()},
            "general": {"permissions": rules()}
        }
    });
    if let Some(model) = model {
        doc["model"] = Value::String(model.to_owned());
    }
    if !mcp_servers.is_empty() {
        let mut servers = serde_json::Map::new();
        for s in mcp_servers {
            let command: Vec<String> = std::iter::once(s.command.to_string_lossy().into_owned())
                .chain(s.args.iter().cloned())
                .collect();
            let environment: serde_json::Map<String, Value> = s
                .env
                .iter()
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            servers.insert(
                s.name.clone(),
                json!({
                    "type": "local",
                    "command": command,
                    "environment": environment,
                    "codemode": false
                }),
            );
        }
        doc["mcp"] = json!({ "servers": servers });
    }
    doc
}

/// Merges `ours` over a user's inline config: `permissions` arrays are
/// concatenated (ours last, so ours wins), `agents.<id>.permissions` likewise,
/// and every other key of the user's config is kept, except `model`, which
/// ours replaces when set. A user config that is not a JSON object is replaced.
pub fn merge_inline(existing: Option<&str>, ours: &Value) -> Value {
    let Some(Value::Object(mut base)) = existing.and_then(|s| serde_json::from_str(s).ok()) else {
        return ours.clone();
    };
    let Some(ours_obj) = ours.as_object() else {
        return ours.clone();
    };
    for (key, value) in ours_obj {
        match key.as_str() {
            "permissions" => append_array(&mut base, "permissions", value),
            "mcp" => {
                let mcp = base
                    .entry("mcp")
                    .or_insert_with(|| Value::Object(Default::default()));
                if let (Some(mcp), Some(ours_servers)) = (
                    mcp.as_object_mut(),
                    value.get("servers").and_then(Value::as_object),
                ) {
                    let servers = mcp
                        .entry("servers")
                        .or_insert_with(|| Value::Object(Default::default()));
                    if let Some(servers) = servers.as_object_mut() {
                        for (name, cfg) in ours_servers {
                            servers.insert(name.clone(), cfg.clone());
                        }
                    }
                }
            }
            "agents" => {
                let agents = base
                    .entry("agents")
                    .or_insert_with(|| Value::Object(Default::default()));
                if let (Some(agents), Some(ours_agents)) =
                    (agents.as_object_mut(), value.as_object())
                {
                    for (id, ours_agent) in ours_agents {
                        let slot = agents
                            .entry(id.clone())
                            .or_insert_with(|| Value::Object(Default::default()));
                        if let (Some(slot), Some(perms)) =
                            (slot.as_object_mut(), ours_agent.get("permissions"))
                        {
                            append_array(slot, "permissions", perms);
                        }
                    }
                }
            }
            _ => {
                base.insert(key.clone(), value.clone());
            }
        }
    }
    Value::Object(base)
}

fn append_array(target: &mut serde_json::Map<String, Value>, key: &str, extra: &Value) {
    let Some(extra) = extra.as_array() else {
        return;
    };
    let slot = target
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    if let Some(arr) = slot.as_array_mut() {
        arr.extend(extra.iter().cloned());
    } else {
        *slot = Value::Array(extra.clone());
    }
}

/// A generated config, on disk for the lifetime of the session. The file and
/// its private directory are removed on drop.
#[derive(Debug)]
pub struct InjectedConfig {
    dir: PathBuf,
    file: PathBuf,
    doc: Value,
}

impl InjectedConfig {
    pub fn create(model: Option<&str>, mcp_servers: &[McpServer]) -> io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!(
            "atrium-acp-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        create_private_dir(&dir)?;
        let doc = build_config(model, mcp_servers);
        let file = dir.join("opencode.json");
        let text = serde_json::to_string_pretty(&doc).map_err(io::Error::other)?;
        if let Err(e) = fs::write(&file, text) {
            let _ = fs::remove_dir_all(&dir);
            return Err(e);
        }
        Ok(Self { dir, file, doc })
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    pub fn document(&self) -> &Value {
        &self.doc
    }

    /// The environment to add to the `opencode acp` child. `lookup` resolves
    /// an existing variable (the session's `env`, then the parent process).
    pub fn env(&self, lookup: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
        let mut env = Vec::new();
        if lookup(ENV_CONFIG_FILE).is_none() {
            env.push((
                ENV_CONFIG_FILE.to_owned(),
                self.file.to_string_lossy().into_owned(),
            ));
        }
        let merged = merge_inline(lookup(ENV_CONFIG_CONTENT).as_deref(), &self.doc);
        env.push((ENV_CONFIG_CONTENT.to_owned(), merged.to_string()));
        env
    }
}

impl Drop for InjectedConfig {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_asks_for_everything_and_pins_the_model() {
        let doc = build_config(Some("opencode/deepseek-v4.1-flash-free"), &[]);
        assert_eq!(
            doc["permissions"],
            json!([
                {"action": "*", "resource": "*", "effect": "ask"},
                {"action": "execute", "resource": "*", "effect": "deny"}
            ])
        );
        assert_eq!(doc["model"], "opencode/deepseek-v4.1-flash-free");
        for agent in ["explore", "general"] {
            assert_eq!(doc["agents"][agent]["permissions"][0]["effect"], "ask");
            assert_eq!(doc["agents"][agent]["permissions"][1]["action"], "execute");
        }
        assert!(build_config(None, &[]).get("model").is_none());
        assert!(build_config(None, &[]).get("mcp").is_none());
    }

    #[test]
    fn mcp_servers_become_direct_tools() {
        let server = McpServer {
            name: "files".into(),
            command: "/bin/mcp-files".into(),
            args: vec!["--root".into(), "/scratch".into()],
            env: vec![("K".into(), "v".into())],
        };
        let doc = build_config(None, &[server]);
        let cfg = &doc["mcp"]["servers"]["files"];
        assert_eq!(cfg["type"], "local");
        assert_eq!(
            cfg["command"],
            json!(["/bin/mcp-files", "--root", "/scratch"])
        );
        assert_eq!(cfg["environment"], json!({"K": "v"}));
        assert_eq!(cfg["codemode"], false);
        let merged = merge_inline(
            Some(r#"{"mcp":{"servers":{"mine":{"type":"local","command":["x"]}}}}"#),
            &doc,
        );
        assert!(merged["mcp"]["servers"]["mine"].is_object());
        assert!(merged["mcp"]["servers"]["files"].is_object());
    }

    #[test]
    fn merge_keeps_user_rules_and_appends_ours_last() {
        let user = r#"{"permissions":[{"action":"read","resource":"*","effect":"allow"}],
                       "model":"user/model","theme":"x",
                       "agents":{"explore":{"permissions":[{"action":"shell","resource":"*","effect":"allow"}]}}}"#;
        let merged = merge_inline(Some(user), &build_config(Some("a/b"), &[]));
        let perms = merged["permissions"].as_array().unwrap();
        assert_eq!(perms.len(), 3);
        assert_eq!(perms[0]["effect"], "allow");
        assert_eq!(perms[1]["effect"], "ask", "ours follows the user's rules");
        assert_eq!(
            perms[2]["action"], "execute",
            "and the execute deny is last"
        );
        assert_eq!(merged["model"], "a/b");
        assert_eq!(merged["theme"], "x");
        let explore = merged["agents"]["explore"]["permissions"]
            .as_array()
            .unwrap();
        assert_eq!(explore.last().unwrap()["action"], "execute");
        assert_eq!(explore.len(), 3);
    }

    #[test]
    fn merge_replaces_unparseable_user_config() {
        let ours = build_config(None, &[]);
        assert_eq!(merge_inline(Some("{not json"), &ours), ours);
        assert_eq!(merge_inline(None, &ours), ours);
    }

    #[test]
    fn file_exists_for_the_session_and_is_removed_after() {
        let cfg = InjectedConfig::create(Some("a/b"), &[]).expect("create");
        let path = cfg.path().to_path_buf();
        let text = fs::read_to_string(&path).expect("config file");
        assert_eq!(
            serde_json::from_str::<Value>(&text).unwrap(),
            *cfg.document()
        );
        let env = cfg.env(|_| None);
        assert!(
            env.iter()
                .any(|(k, v)| k == ENV_CONFIG_FILE && Path::new(v) == path)
        );
        assert!(env.iter().any(|(k, _)| k == ENV_CONFIG_CONTENT));
        drop(cfg);
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn users_own_config_file_is_not_replaced() {
        let cfg = InjectedConfig::create(None, &[]).expect("create");
        let env = cfg.env(|k| (k == ENV_CONFIG_FILE).then(|| "/home/me/oc.json".to_owned()));
        assert!(!env.iter().any(|(k, _)| k == ENV_CONFIG_FILE));
        assert!(env.iter().any(|(k, _)| k == ENV_CONFIG_CONTENT));
    }
}
