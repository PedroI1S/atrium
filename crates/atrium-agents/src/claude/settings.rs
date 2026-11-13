//! Configuration provenance for a Claude Code session (contract §3, D-044).
//!
//! Claude Code loads managed, user, project and local settings, `--settings`
//! files and enabled plugins, and runs whatever hooks and helper commands
//! they declare on the host, outside Atrium's gate. Atrium keeps that native
//! behaviour (D-017) and reports it instead: every source the session would
//! load is listed in the descriptor with a content hash, and
//! [`EnforcementDescriptor::evaluate`] turns what isn't verified into Partial.
//!
//! - A settings file that exists but can't be read or parsed is
//!   `inspected: false`.
//! - Plugin directories (`enabledPlugins`, `--plugin-dir`) are always
//!   `inspected: false`: what a plugin's hooks, agents and commands can do is
//!   not analysed. A plugin that ships a `hooks` file also adds an
//!   `ExternalHooks` reason.
//! - Hooks and the command-running keys (`apiKeyHelper`, `awsAuthRefresh`,
//!   `awsCredentialExport`, `fileSuggestion`, `otelHeadersHelper`,
//!   `statusLine`) add an `ExternalHooks` reason. A `hooks` value in a shape
//!   that is not parsed makes the file `inspected: false`.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use atrium_protocol::policy::{ConfigSource, ConfigSourceKind, EnforcementReason, ReasonCode};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Settings keys whose value is a command Claude Code runs.
///
/// `fileSuggestion` (`{"type": "command", "command": ...}`, run for `@` file
/// completion) was confirmed against the CLI itself: its settings schema
/// describes it as "Custom file suggestion configuration for @ mentions" with
/// a `command`, and the code that runs it is the "FileSuggestion command"
/// (found in the `claude` 2.1.312 binary, no session started).
const COMMAND_KEYS: [&str; 6] = [
    "apiKeyHelper",
    "awsAuthRefresh",
    "awsCredentialExport",
    "fileSuggestion",
    "otelHeadersHelper",
    "statusLine",
];

/// What [`inspect`] needs to know about the session.
pub struct InspectInput<'a> {
    pub cwd: &'a Path,
    pub env: &'a [(String, String)],
    /// `--setting-sources` (`None` = Claude's default: user, project, local).
    pub setting_sources: Option<&'a str>,
    /// Extra CLI arguments (`--settings`, `--plugin-dir` are read from here).
    pub extra_args: &'a [String],
    /// Managed settings files; `None` = this platform's default locations.
    pub managed: Option<&'a [PathBuf]>,
}

#[derive(Debug, Default)]
pub struct Inspection {
    pub sources: Vec<ConfigSource>,
    /// `ExternalHooks` reasons, one per finding.
    pub reasons: Vec<EnforcementReason>,
}

/// The home directory as the child sees it.
fn lookup(env: &[(String, String)], key: &str) -> Option<String> {
    env.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var(key).ok())
}

fn user_dir(env: &[(String, String)]) -> PathBuf {
    lookup(env, "CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| lookup(env, "HOME").map(|h| PathBuf::from(h).join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"))
}

fn default_managed() -> Vec<PathBuf> {
    let mut v = vec![
        PathBuf::from("/Library/Application Support/ClaudeCode/managed-settings.json"),
        PathBuf::from("/etc/claude-code/managed-settings.json"),
    ];
    for dir in [
        "/Library/Application Support/ClaudeCode/managed-settings.d",
        "/etc/claude-code/managed-settings.d",
    ] {
        if let Ok(rd) = fs::read_dir(dir) {
            let mut files: Vec<PathBuf> = rd
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect();
            files.sort();
            v.extend(files);
        }
    }
    v
}

/// The home directory shown as `~`.
fn display_path(path: &Path, env: &[(String, String)]) -> String {
    match lookup(env, "HOME").filter(|h| !h.is_empty()) {
        // By path components: `/home/anabel` is not inside `/home/ana`.
        Some(home) => match path.strip_prefix(&home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        None => path.display().to_string(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

/// Reads every configuration source the session would load.
pub fn inspect(input: &InspectInput<'_>) -> Inspection {
    let mut out = Inspection::default();
    let user_dir = user_dir(input.env);
    let wanted = |s: &str| match input.setting_sources {
        None => true,
        Some(list) => list.split(',').any(|x| x.trim() == s),
    };

    // Which files, in Claude's order of precedence (lowest first is not
    // needed: every one is read).
    let mut files: Vec<(ConfigSourceKind, &'static str, PathBuf)> = Vec::new();
    let managed: Vec<PathBuf> = match input.managed {
        Some(m) => m.to_vec(),
        None => default_managed(),
    };
    for m in managed {
        files.push((ConfigSourceKind::ManagedPolicy, "managed", m));
    }
    if wanted("user") {
        files.push((
            ConfigSourceKind::UserSettings,
            "user",
            user_dir.join("settings.json"),
        ));
    }
    if wanted("project") || wanted("local") {
        // The project directory is the cwd or one of its parents.
        for dir in input.cwd.ancestors() {
            let claude = dir.join(".claude");
            if claude == user_dir {
                continue;
            }
            if wanted("project") {
                files.push((
                    ConfigSourceKind::ProjectSettings,
                    "project",
                    claude.join("settings.json"),
                ));
            }
            if wanted("local") {
                files.push((
                    ConfigSourceKind::LocalSettings,
                    "local",
                    claude.join("settings.local.json"),
                ));
            }
        }
    }
    let mut plugin_dirs: Vec<PathBuf> = Vec::new();
    let mut args = input.extra_args.iter();
    while let Some(a) = args.next() {
        let value = |flag: &str, args: &mut std::slice::Iter<'_, String>| {
            if a == flag {
                args.next().cloned()
            } else {
                a.strip_prefix(&format!("{flag}=")).map(str::to_owned)
            }
        };
        if let Some(v) = value("--settings", &mut args) {
            if v.trim_start().starts_with('{') {
                inspect_text(
                    &mut out,
                    ConfigSourceKind::ExplicitSettings,
                    "--settings",
                    "(inline --settings)".to_owned(),
                    v.into_bytes(),
                );
            } else {
                files.push((
                    ConfigSourceKind::ExplicitSettings,
                    "--settings",
                    input.cwd.join(v),
                ));
            }
        } else if let Some(v) = value("--plugin-dir", &mut args) {
            plugin_dirs.push(input.cwd.join(v));
        }
    }

    let mut enabled: Vec<String> = Vec::new();
    for (kind, scope, path) in files {
        let shown = display_path(&path, input.env);
        match fs::read(&path) {
            Ok(bytes) => {
                let found = inspect_text(&mut out, kind, scope, shown, bytes);
                enabled.extend(found);
            }
            // Nothing there: nothing is loaded.
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            // It exists but can't be read: not inspected.
            Err(_) => out.sources.push(ConfigSource {
                kind,
                path: shown,
                sha256: None,
                inspected: false,
            }),
        }
    }

    // macOS managed preferences (MDM) are a plist this adapter doesn't parse.
    #[cfg(target_os = "macos")]
    if input.managed.is_none() {
        let plist = Path::new("/Library/Managed Preferences/com.anthropic.claudecode.plist");
        if let Ok(bytes) = fs::read(plist) {
            out.sources.push(ConfigSource {
                kind: ConfigSourceKind::ManagedPolicy,
                path: plist.display().to_string(),
                sha256: Some(sha256_hex(&bytes)),
                inspected: false,
            });
        }
    }

    // Plugins: enabled ones from settings (found in the installed list when
    // possible) and `--plugin-dir`. What they run is never analysed.
    enabled.sort();
    enabled.dedup();
    for id in enabled {
        let dir = installed_plugin_dir(&user_dir, &id);
        plugin_source(&mut out, input.env, &id, dir);
    }
    for dir in plugin_dirs {
        let id = display_path(&dir, input.env);
        plugin_source(&mut out, input.env, &id, Some(dir));
    }
    out
}

/// Parses one settings text, records it, and returns the enabled plugin ids.
fn inspect_text(
    out: &mut Inspection,
    kind: ConfigSourceKind,
    scope: &str,
    shown: String,
    bytes: Vec<u8>,
) -> Vec<String> {
    let sha = Some(sha256_hex(&bytes));
    let parsed = serde_json::from_slice::<Value>(&bytes)
        .ok()
        .filter(Value::is_object);
    let Some(v) = parsed else {
        out.sources.push(ConfigSource {
            kind,
            path: shown,
            sha256: sha,
            inspected: false,
        });
        return Vec::new();
    };
    let (findings, understood) = command_findings(&v, scope, &shown);
    // Hooks in a shape we don't parse could be running anything: not inspected.
    out.sources.push(ConfigSource {
        kind,
        path: shown.clone(),
        sha256: sha,
        inspected: understood,
    });
    for message in findings {
        out.reasons.push(EnforcementReason {
            code: ReasonCode::ExternalHooks,
            message,
        });
    }
    v.get("enabledPlugins")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, on)| on.as_bool() == Some(true))
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// One line for everything in a settings object that runs a command, and
/// whether every `hooks` entry had the shape we parse
/// (`{"Event": [{"matcher": ..., "hooks": [handler, ...]}]}`). Any other
/// shape is not understood: the caller must not call the file inspected.
fn command_findings(v: &Value, scope: &str, shown: &str) -> (Vec<String>, bool) {
    let mut found = Vec::new();
    let mut understood = true;
    if let Some(hooks) = v.get("hooks") {
        match hooks.as_object() {
            None => understood = false,
            Some(events) => {
                for (event, groups) in events {
                    let Some(groups) = groups.as_array() else {
                        understood = false;
                        continue;
                    };
                    for g in groups {
                        let Some(handlers) = g.get("hooks").and_then(Value::as_array) else {
                            understood = false;
                            continue;
                        };
                        for h in handlers {
                            if !h.is_object() {
                                understood = false;
                                continue;
                            }
                            let kind = h.get("type").and_then(Value::as_str).unwrap_or("command");
                            let detail = ["command", "url", "prompt"]
                                .iter()
                                .find_map(|k| h.get(*k).and_then(Value::as_str))
                                .unwrap_or("");
                            let risk = if event == "PreToolUse" {
                                " (a PreToolUse hook can rewrite a call's input after the gate approved it)"
                            } else {
                                ""
                            };
                            found.push(format!(
                                "{scope} settings ({shown}) run a {event} hook ({kind}: {}) outside the gate{risk}",
                                clip(detail, 80)
                            ));
                        }
                    }
                }
            }
        }
    }
    for key in COMMAND_KEYS {
        if v.get(key).is_some_and(|x| !x.is_null()) {
            found.push(format!(
                "{scope} settings ({shown}) set `{key}`, which runs a command outside the gate"
            ));
        }
    }
    (found, understood)
}

/// `installed_plugins.json` maps `name@marketplace` to its install path.
fn installed_plugin_dir(user_dir: &Path, id: &str) -> Option<PathBuf> {
    let text = fs::read_to_string(user_dir.join("plugins/installed_plugins.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let entry = v.get("plugins")?.get(id)?;
    let first = entry.as_array().and_then(|a| a.first()).unwrap_or(entry);
    first
        .get("installPath")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// A plugin source: never inspected. Hooks it ships add a reason.
fn plugin_source(out: &mut Inspection, env: &[(String, String)], id: &str, dir: Option<PathBuf>) {
    let mut sha = None;
    let mut shown = format!("plugin:{id}");
    if let Some(dir) = &dir {
        shown = display_path(dir, env);
        let candidates = [dir.join("hooks/hooks.json"), dir.join("hooks.json")];
        for hooks in candidates {
            if let Ok(bytes) = fs::read(&hooks) {
                sha = Some(sha256_hex(&bytes));
                out.reasons.push(EnforcementReason {
                    code: ReasonCode::ExternalHooks,
                    message: format!(
                        "plugin {} ships hooks that run outside the gate",
                        clip(id, 80)
                    ),
                });
                break;
            }
        }
        if sha.is_none()
            && let Ok(bytes) = fs::read(dir.join(".claude-plugin/plugin.json"))
        {
            sha = Some(sha256_hex(&bytes));
            let declares_hooks = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .is_some_and(|v| v.get("hooks").is_some());
            if declares_hooks {
                out.reasons.push(EnforcementReason {
                    code: ReasonCode::ExternalHooks,
                    message: format!(
                        "plugin {} declares hooks that run outside the gate",
                        clip(id, 80)
                    ),
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("atrium-cfg-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn run(
        cwd: &Path,
        user: &Path,
        sources: Option<&str>,
        extra: &[&str],
        managed: &[PathBuf],
    ) -> Inspection {
        let env = vec![
            ("CLAUDE_CONFIG_DIR".to_owned(), user.display().to_string()),
            ("HOME".to_owned(), "/nonexistent-home".to_owned()),
        ];
        let extra: Vec<String> = extra.iter().map(|s| (*s).to_owned()).collect();
        inspect(&InspectInput {
            cwd,
            env: &env,
            setting_sources: sources,
            extra_args: &extra,
            managed: Some(managed),
        })
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
        let all = run(&cwd, &user, None, &[], &[]);
        assert_eq!(all.sources.len(), 3);
        assert!(
            all.sources
                .iter()
                .all(|s| s.inspected && s.sha256.is_some())
        );
        assert_eq!(all.reasons.len(), 3, "{:?}", all.reasons);
        assert!(all.reasons.iter().any(|r| r.message.contains("audit.sh")));

        // External settings disabled: nothing is loaded, nothing to report.
        let none = run(&cwd, &user, Some(""), &[], &[]);
        assert!(none.sources.is_empty() && none.reasons.is_empty());
        let p = run(&cwd, &user, Some("project,local"), &[], &[]);
        assert_eq!(p.sources.len(), 2);
        assert_eq!(p.reasons.len(), 1);
    }

    #[test]
    fn managed_settings_apply_even_when_sources_are_empty() {
        let root = tmp("managed");
        let managed = root.join("managed-settings.json");
        write(&managed, HOOKS);
        let i = run(&root, &root.join("u"), Some(""), &[], &[managed]);
        assert_eq!(i.sources.len(), 1);
        assert_eq!(i.sources[0].kind, ConfigSourceKind::ManagedPolicy);
        assert_eq!(i.reasons.len(), 2);
    }

    #[test]
    fn command_keys_are_external_hooks() {
        let root = tmp("keys");
        let user = root.join("u");
        for key in COMMAND_KEYS {
            write(
                &user.join("settings.json"),
                &format!(r#"{{"{key}":"/bin/true"}}"#),
            );
            let i = run(&root, &user, None, &[], &[]);
            assert_eq!(i.reasons.len(), 1, "{key}");
            assert!(i.reasons[0].message.contains(key));
            assert_eq!(i.reasons[0].code, ReasonCode::ExternalHooks);
        }
    }

    #[test]
    fn a_file_that_cannot_be_parsed_or_read_is_not_inspected() {
        let root = tmp("bad");
        let user = root.join("u");
        write(&user.join("settings.json"), "{ not json");
        write(&root.join(".claude/settings.json"), "[1,2]");
        let i = run(&root, &user, None, &[], &[]);
        assert_eq!(i.sources.len(), 2);
        assert!(i.sources.iter().all(|s| !s.inspected));
        // Readable but unparseable files still have a hash.
        assert!(i.sources.iter().all(|s| s.sha256.is_some()));

        // A directory where the file should be: exists, can't be read.
        let d = tmp("bad-dir");
        fs::create_dir_all(d.join("u/settings.json")).unwrap();
        let i = run(&d, &d.join("u"), None, &[], &[]);
        assert_eq!(i.sources.len(), 1);
        assert!(!i.sources[0].inspected && i.sources[0].sha256.is_none());
    }

    #[test]
    fn explicit_settings_files_and_inline_json_are_inspected() {
        let root = tmp("explicit");
        write(&root.join("extra.json"), HOOKS);
        let i = run(
            &root,
            &root.join("u"),
            Some(""),
            &["--settings", "extra.json"],
            &[],
        );
        assert_eq!(i.sources.len(), 1);
        assert_eq!(i.sources[0].kind, ConfigSourceKind::ExplicitSettings);
        assert!(i.sources[0].inspected);
        assert_eq!(i.reasons.len(), 2);
        let i = run(
            &root,
            &root.join("u"),
            Some(""),
            &[r#"--settings={"statusLine":{"type":"command","command":"x"}}"#],
            &[],
        );
        assert_eq!(i.sources.len(), 1);
        assert!(i.sources[0].inspected);
        assert_eq!(i.reasons.len(), 1);
    }

    #[test]
    fn plugins_are_never_inspected_and_their_hooks_are_reported() {
        let root = tmp("plugins");
        let user = root.join("u");
        let plugin = root.join("p1");
        write(
            &plugin.join("hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"x"}]}]}}"#,
        );
        let i = run(
            &root,
            &user,
            Some(""),
            &["--plugin-dir", plugin.to_str().unwrap()],
            &[],
        );
        assert_eq!(i.sources.len(), 1);
        assert_eq!(i.sources[0].kind, ConfigSourceKind::Plugin);
        assert!(!i.sources[0].inspected);
        assert!(i.sources[0].sha256.is_some());
        assert_eq!(i.reasons.len(), 1);

        // enabledPlugins, resolved through the installed list.
        let installed = root.join("inst/fmt");
        write(
            &installed.join("hooks/hooks.json"),
            r#"{"hooks":{"PostToolUse":[]}}"#,
        );
        write(
            &user.join("settings.json"),
            r#"{"enabledPlugins":{"fmt@market":true,"off@market":false}}"#,
        );
        write(
            &user.join("plugins/installed_plugins.json"),
            &format!(
                r#"{{"version":2,"plugins":{{"fmt@market":[{{"installPath":"{}"}}]}}}}"#,
                installed.display()
            ),
        );
        let i = run(&root, &user, None, &[], &[]);
        let plugins: Vec<_> = i
            .sources
            .iter()
            .filter(|s| s.kind == ConfigSourceKind::Plugin)
            .collect();
        assert_eq!(plugins.len(), 1, "{:?}", i.sources);
        assert!(!plugins[0].inspected);
        // An enabled plugin that isn't installed is still listed.
        write(
            &user.join("settings.json"),
            r#"{"enabledPlugins":{"ghost@market":true}}"#,
        );
        let i = run(&root, &user, None, &[], &[]);
        assert!(
            i.sources
                .iter()
                .any(|s| s.path == "plugin:ghost@market" && !s.inspected)
        );
    }

    #[test]
    fn file_suggestion_runs_a_command_so_it_is_external_hooks() {
        let root = tmp("file-suggestion");
        let user = root.join("u");
        write(
            &user.join("settings.json"),
            r#"{"fileSuggestion":{"type":"command","command":"/tmp/exfil.sh"}}"#,
        );
        let i = run(&root, &user, None, &[], &[]);
        assert_eq!(i.reasons.len(), 1, "{:?}", i.reasons);
        assert!(i.reasons[0].message.contains("fileSuggestion"));
        assert_eq!(i.reasons[0].code, ReasonCode::ExternalHooks);
    }

    #[test]
    fn hooks_in_a_shape_we_do_not_parse_are_never_inspected_and_clean() {
        let root = tmp("odd-hooks");
        let user = root.join("u");
        for odd in [
            // Handlers directly in the event array, no matcher group.
            r#"{"hooks":{"PreToolUse":[{"type":"command","command":"/tmp/exfil.sh"}]}}"#,
            r#"{"hooks":"invalid"}"#,
            r#"{"hooks":[]}"#,
            r#"{"hooks":{"PreToolUse":"x"}}"#,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash"}]}}"#,
            r#"{"hooks":{"PreToolUse":[{"hooks":"x"}]}}"#,
            r#"{"hooks":{"PreToolUse":[{"hooks":["x"]}]}}"#,
            r#"{"hooks":{"PreToolUse":[1]}}"#,
            r#"{"hooks":null}"#,
        ] {
            write(&user.join("settings.json"), odd);
            let i = run(&root, &user, None, &[], &[]);
            assert_eq!(i.sources.len(), 1, "{odd}");
            assert!(!i.sources[0].inspected, "counted as inspected: {odd}");
        }
        // Empty is understood and clean.
        for ok in [
            r#"{"hooks":{}}"#,
            r#"{"hooks":{"PreToolUse":[]}}"#,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[]}]}}"#,
            r#"{"permissions":{}}"#,
        ] {
            write(&user.join("settings.json"), ok);
            let i = run(&root, &user, None, &[], &[]);
            assert!(i.sources[0].inspected, "{ok}");
            assert!(i.reasons.is_empty(), "{ok}: {:?}", i.reasons);
        }
        // The well-formed one still reports its handler.
        write(&user.join("settings.json"), HOOKS);
        let i = run(&root, &user, None, &[], &[]);
        assert!(i.sources[0].inspected);
        assert_eq!(i.reasons.len(), 2);
    }

    #[test]
    fn a_sibling_of_the_home_directory_is_not_shown_as_inside_it() {
        let env = vec![("HOME".to_owned(), "/home/ana".to_owned())];
        for (path, shown) in [
            (
                "/home/anabel/proj/.claude/settings.json",
                "/home/anabel/proj/.claude/settings.json",
            ),
            ("/home/ana/.claude/settings.json", "~/.claude/settings.json"),
            ("/home/ana", "~"),
            ("/home/ana2", "/home/ana2"),
        ] {
            assert_eq!(display_path(Path::new(path), &env), shown, "{path}");
        }
        let env = vec![("HOME".to_owned(), "/home/ana/".to_owned())];
        assert_eq!(display_path(Path::new("/home/ana/x"), &env), "~/x");
    }

    /// The `plugi` at the end of the `plugin-hook` fixture's reason is this cut,
    /// not a typo: the plugin's path in the reason is limited to 80 characters.
    #[test]
    fn a_long_plugin_path_is_cut_to_80_characters_in_the_reason() {
        let root =
            tmp("a-rather-long-name-for-the-folder-that-holds-the-plugin-under-test-xxxxxxxx");
        let plugin = root.join("plugin");
        write(
            &plugin.join("hooks/hooks.json"),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"x"}]}]}}"#,
        );
        let i = run(
            &root,
            &root.join("u"),
            Some(""),
            &["--plugin-dir", plugin.to_str().unwrap()],
            &[],
        );
        let reason = &i.reasons[0].message;
        let shown = reason
            .strip_prefix("plugin ")
            .and_then(|r| r.strip_suffix(" ships hooks that run outside the gate"))
            .unwrap();
        assert_eq!(shown.chars().count(), 80, "{reason}");
        assert!(plugin.to_str().unwrap().starts_with(shown));
        // The source itself keeps the whole path.
        assert_eq!(i.sources[0].path, plugin.display().to_string());
    }

    #[test]
    fn the_home_directory_is_shown_as_a_tilde() {
        let env = vec![("HOME".to_owned(), "/home/ana".to_owned())];
        assert_eq!(
            display_path(Path::new("/home/ana/.claude/settings.json"), &env),
            "~/.claude/settings.json"
        );
    }
}
