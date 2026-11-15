#![doc = "Inspects Codex configuration files and active profile into ConfigSource entries."]

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use atrium_protocol::policy::{ConfigSource, ConfigSourceKind, EnforcementReason, ReasonCode};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Result of inspecting the Codex configuration sources for a session.
#[derive(Debug, Clone, Default)]
pub struct Inspection {
    pub sources: Vec<ConfigSource>,
    pub reasons: Vec<EnforcementReason>,
    pub active_profile: Option<String>,
}

/// Inputs to configuration inspection.
#[derive(Debug, Clone, Copy)]
pub struct InspectInput<'a> {
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
    pub profile: Option<&'a str>,
    pub args: &'a [String],
    /// Overrides the user Codex home directory (`None` reads `CODEX_HOME` or
    /// the `.codex` directory under `HOME`; `Some("")` disables user lookup).
    pub user_dir: Option<&'a str>,
}

/// Inspects Codex configuration files and any active profile.
pub fn inspect(input: InspectInput<'_>) -> Inspection {
    let mut out = Inspection::default();
    let user_dir = resolve_codex_home(input.env, input.user_dir);
    let cli_profile = cli_profile_override(input.profile, input.args);

    let mut files: Vec<(ConfigSourceKind, PathBuf, &'static str)> = Vec::new();
    let managed = PathBuf::from(std::path::MAIN_SEPARATOR_STR)
        .join("etc")
        .join("codex")
        .join("config.toml");
    files.push((ConfigSourceKind::ManagedPolicy, managed, "managed"));

    if let Some(dir) = &user_dir {
        files.push((
            ConfigSourceKind::UserSettings,
            dir.join("config.toml"),
            "user",
        ));
        files.push((
            ConfigSourceKind::Hooks,
            dir.join("hooks.json"),
            "user hooks",
        ));
    }

    let project_dir = input.cwd.join(".codex");
    files.push((
        ConfigSourceKind::ProjectSettings,
        project_dir.join("config.toml"),
        "project",
    ));
    files.push((
        ConfigSourceKind::Hooks,
        project_dir.join("hooks.json"),
        "project hooks",
    ));

    let mut parsed_configs: Vec<(String, toml::Table)> = Vec::new();
    let mut config_profile: Option<String> = None;

    for (kind, path, label) in files {
        let shown = display_path(&path, input.env);
        match fs::read(&path) {
            Ok(bytes) => {
                let sha = Some(sha256_hex(&bytes));
                if kind == ConfigSourceKind::Hooks {
                    inspect_hooks_file(&mut out, shown, sha, &bytes, label);
                } else {
                    match parse_toml(&bytes) {
                        Some(table) => {
                            if let Some(p) = table.get("profile").and_then(toml::Value::as_str)
                                && !p.is_empty()
                            {
                                config_profile = Some(p.to_owned());
                            }
                            if has_hooks_feature(&table) {
                                out.reasons.push(EnforcementReason {
                                    code: ReasonCode::ExternalHooks,
                                    message: format!(
                                        "{label} config enables Codex hooks ({shown})"
                                    ),
                                });
                            }
                            out.sources.push(ConfigSource {
                                kind,
                                path: shown.clone(),
                                sha256: sha,
                                inspected: true,
                            });
                            parsed_configs.push((shown, table));
                        }
                        None => {
                            out.sources.push(ConfigSource {
                                kind,
                                path: shown,
                                sha256: sha,
                                inspected: false,
                            });
                        }
                    }
                }
            }
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(_) => {
                out.sources.push(ConfigSource {
                    kind,
                    path: shown,
                    sha256: None,
                    inspected: false,
                });
            }
        }
    }

    let active_profile = cli_profile.or(config_profile);
    if let Some(profile_name) = active_profile {
        out.active_profile = Some(profile_name.clone());
        record_profile(&mut out, &profile_name, &parsed_configs);
    }

    // Rules (.rules) and plugins folders are outside Atrium's parser and may
    // alter provider gating (`prefix_rule` allow-prefixes, plugin MCP/hooks).
    let mut rules_dirs: Vec<PathBuf> = vec![
        PathBuf::from(std::path::MAIN_SEPARATOR_STR)
            .join("etc")
            .join("codex")
            .join("rules"),
    ];
    let mut plugin_dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = &user_dir {
        rules_dirs.push(dir.join("rules"));
        plugin_dirs.push(dir.join("plugins"));
    }
    rules_dirs.push(project_dir.join("rules"));
    plugin_dirs.push(project_dir.join("plugins"));

    for rdir in rules_dirs {
        inspect_rules_dir(&mut out, &rdir, input.env);
    }
    for pdir in plugin_dirs {
        inspect_plugins_dir(&mut out, &pdir, input.env);
    }
    record_config_plugins(&mut out, &parsed_configs);

    out
}

fn inspect_hooks_file(
    out: &mut Inspection,
    shown: String,
    sha: Option<String>,
    bytes: &[u8],
    label: &str,
) {
    let Ok(v) = serde_json::from_slice::<Value>(bytes) else {
        out.sources.push(ConfigSource {
            kind: ConfigSourceKind::Hooks,
            path: shown,
            sha256: sha,
            inspected: false,
        });
        return;
    };
    let has_entries = v.get("hooks").and_then(Value::as_object).is_some_and(|m| {
        m.values()
            .any(|arr| arr.as_array().is_some_and(|a| !a.is_empty()))
    });
    out.sources.push(ConfigSource {
        kind: ConfigSourceKind::Hooks,
        path: shown.clone(),
        sha256: sha,
        inspected: true,
    });
    if has_entries {
        out.reasons.push(EnforcementReason {
            code: ReasonCode::ExternalHooks,
            message: format!("{label} configured outside Atrium ({shown})"),
        });
    }
}

fn parse_toml(bytes: &[u8]) -> Option<toml::Table> {
    let text = std::str::from_utf8(bytes).ok()?;
    text.parse::<toml::Table>().ok()
}

fn has_hooks_feature(table: &toml::Table) -> bool {
    if table
        .get("features")
        .and_then(toml::Value::as_table)
        .and_then(|f| f.get("hooks"))
        .and_then(toml::Value::as_bool)
        == Some(true)
    {
        return true;
    }
    table
        .get("hooks")
        .and_then(toml::Value::as_table)
        .is_some_and(|h| !h.is_empty())
}

fn inspect_rules_dir(out: &mut Inspection, dir: &Path, env: &[(String, String)]) {
    let entries = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(_) => {
            out.sources.push(ConfigSource {
                kind: ConfigSourceKind::Other,
                path: display_path(dir, env),
                sha256: None,
                inspected: false,
            });
            return;
        }
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("rules"))
        .collect();
    paths.sort();
    for path in paths {
        let sha = fs::read(&path).ok().map(|b| sha256_hex(&b));
        out.sources.push(ConfigSource {
            kind: ConfigSourceKind::Other,
            path: display_path(&path, env),
            sha256: sha,
            inspected: false,
        });
    }
}

fn inspect_plugins_dir(out: &mut Inspection, dir: &Path, env: &[(String, String)]) {
    let entries = match fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(_) => {
            out.sources.push(ConfigSource {
                kind: ConfigSourceKind::Plugin,
                path: display_path(dir, env),
                sha256: None,
                inspected: false,
            });
            return;
        }
    };
    let mut subdirs = Vec::new();
    let mut has_files = false;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            subdirs.push(path);
        } else if path.is_file() {
            has_files = true;
        }
    }
    subdirs.sort();
    for pdir in subdirs {
        let shown = display_path(&pdir, env);
        let mut sha = None;
        for rel in ["hooks/hooks.json", "hooks.json"] {
            if let Ok(bytes) = fs::read(pdir.join(rel)) {
                sha = Some(sha256_hex(&bytes));
                out.reasons.push(EnforcementReason {
                    code: ReasonCode::ExternalHooks,
                    message: format!("Codex plugin ships hooks outside Atrium ({shown})"),
                });
                break;
            }
        }
        if sha.is_none() {
            for rel in [
                ".codex-plugin/plugin.json",
                "plugin.json",
                "manifest.json",
                "config.toml",
            ] {
                if let Ok(bytes) = fs::read(pdir.join(rel)) {
                    sha = Some(sha256_hex(&bytes));
                    break;
                }
            }
        }
        out.sources.push(ConfigSource {
            kind: ConfigSourceKind::Plugin,
            path: shown,
            sha256: sha,
            inspected: false,
        });
    }
    if has_files
        && out
            .sources
            .iter()
            .all(|s| s.kind != ConfigSourceKind::Plugin)
    {
        out.sources.push(ConfigSource {
            kind: ConfigSourceKind::Plugin,
            path: display_path(dir, env),
            sha256: None,
            inspected: false,
        });
    }
}

fn record_config_plugins(out: &mut Inspection, parsed_configs: &[(String, toml::Table)]) {
    for (shown, table) in parsed_configs {
        let Some(plugins) = table.get("plugins").and_then(toml::Value::as_table) else {
            continue;
        };
        let mut names: Vec<&String> = plugins.keys().collect();
        names.sort();
        for name in names {
            let val = &plugins[name];
            if val
                .as_table()
                .and_then(|t| t.get("enabled"))
                .and_then(toml::Value::as_bool)
                == Some(false)
            {
                continue;
            }
            if val
                .as_table()
                .and_then(|t| t.get("hooks"))
                .and_then(toml::Value::as_table)
                .is_some_and(|h| !h.is_empty())
            {
                out.reasons.push(EnforcementReason {
                    code: ReasonCode::ExternalHooks,
                    message: format!("plugin '{name}' configures hooks outside Atrium ({shown})"),
                });
            }
            let bytes = toml::to_string(val)
                .map(String::into_bytes)
                .unwrap_or_default();
            out.sources.push(ConfigSource {
                kind: ConfigSourceKind::Plugin,
                path: format!("{shown}#plugins.{name}"),
                sha256: Some(sha256_hex(&bytes)),
                inspected: false,
            });
        }
    }
}

fn record_profile(out: &mut Inspection, name: &str, parsed_configs: &[(String, toml::Table)]) {
    let mut found = None;
    for (shown, table) in parsed_configs.iter().rev() {
        if let Some(profiles) = table.get("profiles").and_then(toml::Value::as_table)
            && let Some(profile_val) = profiles.get(name)
        {
            found = Some((shown.clone(), profile_val));
            break;
        }
    }
    match found {
        Some((shown, profile_val)) => {
            if let Some(pt) = profile_val.as_table()
                && has_hooks_feature(pt)
            {
                out.reasons.push(EnforcementReason {
                    code: ReasonCode::ExternalHooks,
                    message: format!("profile '{name}' enables Codex hooks ({shown})"),
                });
            }
            let bytes = toml::to_string(profile_val)
                .map(String::into_bytes)
                .unwrap_or_default();
            out.sources.push(ConfigSource {
                kind: ConfigSourceKind::ExplicitSettings,
                path: format!("{shown}#profiles.{name}"),
                sha256: Some(sha256_hex(&bytes)),
                inspected: true,
            });
        }
        None => {
            out.sources.push(ConfigSource {
                kind: ConfigSourceKind::ExplicitSettings,
                path: format!("profile:{name}"),
                sha256: None,
                inspected: false,
            });
        }
    }
}

fn cli_profile_override(profile: Option<&str>, args: &[String]) -> Option<String> {
    if let Some(p) = profile
        && !p.is_empty()
    {
        return Some(p.to_owned());
    }
    let mut iter = args.iter().peekable();
    let mut from_args = None;
    while let Some(arg) = iter.next() {
        if matches!(arg.as_str(), "-p" | "--profile") {
            if let Some(val) = iter.next()
                && !val.is_empty()
            {
                from_args = Some((*val).clone());
            }
        } else if let Some(val) = arg.strip_prefix("--profile=") {
            if !val.is_empty() {
                from_args = Some(val.to_owned());
            }
        } else if arg == "-c" {
            if let Some(kv) = iter.next()
                && let Some(val) = parse_profile_kv(kv)
            {
                from_args = Some(val);
            }
        } else if let Some(kv) = arg.strip_prefix("-c=")
            && let Some(val) = parse_profile_kv(kv)
        {
            from_args = Some(val);
        }
    }
    from_args
}

fn parse_profile_kv(kv: &str) -> Option<String> {
    let (k, v) = kv.split_once('=')?;
    if k.trim() != "profile" {
        return None;
    }
    let trimmed = v.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    (!unquoted.is_empty()).then(|| unquoted.to_owned())
}

fn resolve_codex_home(env: &[(String, String)], override_dir: Option<&str>) -> Option<PathBuf> {
    if let Some(dir) = override_dir {
        return (!dir.is_empty()).then(|| PathBuf::from(dir));
    }
    if let Some(dir) = env_value("CODEX_HOME", env)
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    env_value("HOME", env)
        .filter(|s| !s.is_empty())
        .map(|h| PathBuf::from(h).join(".codex"))
}

fn env_value(name: &str, env: &[(String, String)]) -> Option<String> {
    env.iter()
        .rev()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var(name).ok())
}

/// Replaces `$HOME` prefix with a home-relative display path.
pub fn display_path(path: &Path, env: &[(String, String)]) -> String {
    if let Some(home) = env_value("HOME", env).filter(|s| !s.is_empty())
        && let Ok(rest) = path.strip_prefix(&home)
    {
        let sep = '/';
        if rest.as_os_str().is_empty() {
            return "~".to_owned();
        }
        return format!("~{sep}{}", rest.display());
    }
    path.display().to_string()
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "atrium-codex-settings-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn missing_configs_produce_empty_inspection() {
        let root = tmp_dir("empty");
        let i = inspect(InspectInput {
            cwd: &root,
            env: &[],
            profile: None,
            args: &[],
            user_dir: Some(""),
        });
        let non_managed: Vec<_> = i
            .sources
            .iter()
            .filter(|s| s.kind != ConfigSourceKind::ManagedPolicy)
            .collect();
        assert!(non_managed.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn user_config_and_active_profile_are_hashed_and_inspected() {
        let root = tmp_dir("profile");
        let home = root.join("home");
        let codex = home.join(".codex");
        let work = root.join("work");
        fs::create_dir_all(&codex).unwrap();
        fs::create_dir_all(&work).unwrap();
        fs::write(
            codex.join("config.toml"),
            "model = \"gpt-5\"\nprofile = \"strict\"\n\n[profiles.strict]\nsandbox_mode = \"read-only\"\n",
        )
        .unwrap();

        let env = vec![("HOME".to_owned(), home.display().to_string())];
        let i = inspect(InspectInput {
            cwd: &work,
            env: &env,
            profile: None,
            args: &[],
            user_dir: None,
        });
        assert_eq!(i.active_profile.as_deref(), Some("strict"));
        let user_src = i
            .sources
            .iter()
            .find(|s| s.kind == ConfigSourceKind::UserSettings)
            .expect("user config source");
        assert!(user_src.inspected);
        assert!(user_src.sha256.is_some());
        assert!(user_src.path.ends_with(".codex/config.toml"));
        assert!(user_src.path.starts_with('~'));

        let profile_src = i
            .sources
            .iter()
            .find(|s| s.kind == ConfigSourceKind::ExplicitSettings)
            .expect("profile source");
        assert!(profile_src.inspected);
        assert!(profile_src.sha256.is_some());
        assert!(profile_src.path.ends_with("#profiles.strict"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn invalid_toml_and_missing_profile_mark_uninspected() {
        let root = tmp_dir("bad");
        let user = root.join("u");
        fs::create_dir_all(&user).unwrap();
        fs::write(user.join("config.toml"), "not valid = [toml").unwrap();
        let i = inspect(InspectInput {
            cwd: &root,
            env: &[],
            profile: Some("ghost"),
            args: &[],
            user_dir: Some(user.to_str().unwrap()),
        });
        let user_src = i
            .sources
            .iter()
            .find(|s| s.kind == ConfigSourceKind::UserSettings)
            .unwrap();
        assert!(!user_src.inspected);
        assert!(user_src.sha256.is_some());

        let profile_src = i
            .sources
            .iter()
            .find(|s| s.kind == ConfigSourceKind::ExplicitSettings)
            .unwrap();
        assert!(!profile_src.inspected);
        assert_eq!(profile_src.path, "profile:ghost");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hooks_file_adds_external_hooks_reason() {
        let root = tmp_dir("hooks");
        let user = root.join("u");
        fs::create_dir_all(&user).unwrap();
        fs::write(
            user.join("hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"command":"echo hi"}]}}"#,
        )
        .unwrap();
        let i = inspect(InspectInput {
            cwd: &root,
            env: &[],
            profile: None,
            args: &[],
            user_dir: Some(user.to_str().unwrap()),
        });
        assert!(
            i.reasons
                .iter()
                .any(|r| r.code == ReasonCode::ExternalHooks)
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn features_hooks_key_adds_external_hooks_reason() {
        let root = tmp_dir("features-hooks");
        let user = root.join("u");
        fs::create_dir_all(&user).unwrap();
        fs::write(user.join("config.toml"), "[features]\nhooks = true\n").unwrap();
        let i = inspect(InspectInput {
            cwd: &root,
            env: &[],
            profile: None,
            args: &[],
            user_dir: Some(user.to_str().unwrap()),
        });
        assert!(
            i.reasons
                .iter()
                .any(|r| r.code == ReasonCode::ExternalHooks),
            "expected ExternalHooks for [features] hooks = true, got {:?}",
            i.reasons
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rules_and_plugins_are_recorded_from_home_as_uninspected() {
        let root = tmp_dir("rules-plugins");
        let home = root.join("home");
        let codex = home.join(".codex");
        let rules = codex.join("rules");
        let plugin = codex.join("plugins").join("sample-plugin");
        let work = root.join("work");
        fs::create_dir_all(&rules).unwrap();
        fs::create_dir_all(plugin.join("hooks")).unwrap();
        fs::create_dir_all(&work).unwrap();
        fs::write(
            rules.join("default.rules"),
            "prefix_rule(pattern=[\"git\"], decision=\"allow\")\n",
        )
        .unwrap();
        fs::write(plugin.join("hooks").join("hooks.json"), "{\"hooks\":{}}").unwrap();

        let env = vec![("HOME".to_owned(), home.display().to_string())];
        let i = inspect(InspectInput {
            cwd: &work,
            env: &env,
            profile: None,
            args: &[],
            user_dir: None,
        });
        let rule_src = i
            .sources
            .iter()
            .find(|s| s.path.ends_with(".codex/rules/default.rules"))
            .expect("rules file source recorded");
        assert_eq!(rule_src.kind, ConfigSourceKind::Other);
        assert!(!rule_src.inspected);
        assert!(rule_src.sha256.is_some());

        let plugin_src = i
            .sources
            .iter()
            .find(|s| s.kind == ConfigSourceKind::Plugin && s.path.ends_with("sample-plugin"))
            .expect("plugin dir source recorded");
        assert!(!plugin_src.inspected);
        assert!(plugin_src.sha256.is_some());
        assert!(
            i.reasons
                .iter()
                .any(|r| r.code == ReasonCode::ExternalHooks)
        );

        let _ = fs::remove_dir_all(&root);
    }
}
