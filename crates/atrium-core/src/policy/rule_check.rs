//! Shared rule validation for both `atrium-daemon` (`CreateRule`) and the
//! policy engine (`engine.rs`). Matches `shell.rs` and `deny.rs` tables and
//! mirrors the app rules editor (`app/src/chat/rule-form.ts`).

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{RuleDecision, RuleDraft, RuleMatcher, RuleScope};
use globset::GlobBuilder;

/// `shell.rs` `SHELLS`.
const SHELLS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ksh", "fish", "ash", "csh", "tcsh", "mksh", "yash", "rbash",
    "posh", "elvish", "xonsh",
];

/// Run their text in this shell (`shell.rs` `runs_here`).
const RUNS_HERE: &[&str] = &["eval", "source", "."];

/// Wrappers `shell.rs` `unwrap_reading` strips, plus privilege tools in
/// `deny.rs` `PRIV` (`sudo`, `sudoedit`, `su`, `doas`, `pkexec`, `run0`, …).
const WRAPPERS: &[&str] = &[
    "time",
    "nohup",
    "builtin",
    "command",
    "setsid",
    "unbuffer",
    "caffeinate",
    "sandbox-exec",
    "exec",
    "nice",
    "ionice",
    "stdbuf",
    "xargs",
    "busybox",
    "toybox",
    "timeout",
    "chrt",
    "taskset",
    "arch",
    "chroot",
    "flock",
    "script",
    "env",
    "sudo",
    "sudoedit",
    "doas",
    "su",
    "runuser",
    "setpriv",
    "pkexec",
    "run0",
];

struct InterpreterSpec {
    short: &'static str,
    long: &'static [&'static str],
    sub: &'static [&'static str],
    verbs: &'static [&'static str],
}

const PYTHON_SPEC: InterpreterSpec = InterpreterSpec {
    short: "c",
    long: &[],
    sub: &[],
    verbs: &[],
};

const NODE_SPEC: InterpreterSpec = InterpreterSpec {
    short: "ep",
    long: &["--eval", "--print"],
    sub: &[],
    verbs: &[],
};

const DENO_SPEC: InterpreterSpec = InterpreterSpec {
    short: "",
    long: &["--eval", "--eval-file"],
    sub: &["eval", "repl"],
    verbs: &[
        "run", "test", "task", "bench", "serve", "x", "install", "compile", "jupyter",
    ],
};

const BUN_SPEC: InterpreterSpec = InterpreterSpec {
    short: "ep",
    long: &["--eval", "--print"],
    sub: &["eval", "repl", "exec"],
    verbs: &["run", "test", "x", "create", "build"],
};

const E_SPEC: InterpreterSpec = InterpreterSpec {
    short: "e",
    long: &[],
    sub: &[],
    verbs: &[],
};

const PHP_SPEC: InterpreterSpec = InterpreterSpec {
    short: "rbef",
    long: &[
        "--run",
        "--process-code",
        "--process-begin",
        "--process-end",
        "--process-file",
    ],
    sub: &[],
    verbs: &[],
};

const PWSH_CODE: &[&str] = &["command", "encodedcommand", "commandwithargs"];
const PWSH_SHORT: &[&str] = &["c", "e", "ec", "cwa"];

const PACKAGE_RUNNERS: &[&str] = &["npx", "pnpx", "bunx", "uvx", "pipx"];
const RUNNER_CODE: &[&str] = &["-c", "--call"];
const STDIN: &[&str] = &[
    "-",
    "/dev/stdin",
    "/dev/fd/0",
    "/proc/self/fd/0",
    "/proc/thread-self/fd/0",
];
const FIND_EXEC: &[&str] = &["-exec", "-execdir", "-ok", "-okdir"];
const RESERVED: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "function", "select", "{", "}", "!", "[[", "coproc",
];

/// Folds a command word the way macOS APFS and `deny.rs` fold paths: basename,
/// lower-cased (which maps K U+212A to k), with `ſ` (U+017F) mapped to `s`.
pub fn fold_name(word: &str) -> String {
    super::shell::base(word).to_lowercase().replace('ſ', "s")
}

pub(crate) fn is_shell_or_runs_here(word: &str) -> bool {
    let name = fold_name(word);
    SHELLS.contains(&name.as_str()) || RUNS_HERE.contains(&name.as_str())
}

pub(crate) fn is_wrapper(word: &str) -> bool {
    let name = fold_name(word);
    WRAPPERS.contains(&name.as_str())
}

pub(crate) fn is_find(word: &str) -> bool {
    fold_name(word) == "find"
}

pub(crate) fn is_path_kind(kind: ToolKind) -> bool {
    matches!(
        kind,
        ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move | ToolKind::Search
    )
}

fn is_option(w: &str) -> bool {
    w.len() > 1 && w.starts_with('-')
}

fn is_pwsh(folded_name: &str) -> bool {
    folded_name.starts_with("pwsh") || folded_name.starts_with("powershell")
}

fn interpreter_spec(folded_name: &str) -> Option<&'static InterpreterSpec> {
    if folded_name.starts_with("python") {
        return Some(&PYTHON_SPEC);
    }
    match folded_name {
        "node" => Some(&NODE_SPEC),
        "deno" => Some(&DENO_SPEC),
        "bun" => Some(&BUN_SPEC),
        "ruby" | "perl" | "lua" | "osascript" => Some(&E_SPEC),
        "php" => Some(&PHP_SPEC),
        _ => None,
    }
}

fn normalize_posix_path(raw: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
        .to_lowercase()
        .replace('ſ', "s")
}

fn is_stdin_script_path(raw: &str) -> bool {
    if raw == "-" {
        return true;
    }
    if !raw.starts_with('/') {
        return false;
    }
    let norm = normalize_posix_path(raw);
    STDIN.contains(&norm.as_str())
}

fn is_ps1_script(w: &str) -> bool {
    !w.is_empty() && !is_stdin_script_path(w) && w.to_ascii_lowercase().ends_with(".ps1")
}

fn pwsh_option_body(w: &str) -> Option<&str> {
    if w == "-" || w == "--" {
        return None;
    }
    if let Some(rest) = w.strip_prefix("--").or_else(|| w.strip_prefix('-')) {
        return Some(rest);
    }
    // On Unix `/path/to/script.ps1` starts with `/`, so only single-segment
    // `/Option` (that isn't a `.ps1` file in `/`) counts as a PowerShell flag.
    if let Some(rest) = w.strip_prefix('/')
        && !rest.contains('/')
        && !rest.to_ascii_lowercase().ends_with(".ps1")
    {
        return Some(rest);
    }
    None
}

fn pwsh_is_file_opt(bare: &str) -> bool {
    bare == "f" || (bare.len() >= 2 && "file".starts_with(bare))
}

fn pwsh_is_value_opt(bare: &str) -> bool {
    matches!(bare, "wd" | "ep" | "w" | "if" | "of" | "ea")
        || (bare.len() >= 2
            && ("workingdirectory".starts_with(bare)
                || "executionpolicy".starts_with(bare)
                || "windowstyle".starts_with(bare)
                || "inputformat".starts_with(bare)
                || "outputformat".starts_with(bare)
                || "encodedarguments".starts_with(bare)
                || "settingsfile".starts_with(bare)
                || "custompipename".starts_with(bare)))
        || (bare.len() >= 3 && "configurationname".starts_with(bare))
        || (bare.len() >= 14 && "configurationfile".starts_with(bare))
}

fn pwsh_is_bool_opt(bare: &str) -> bool {
    matches!(bare, "?" | "h" | "v" | "l" | "i" | "sta" | "mta" | "wa")
        || (bare.len() >= 2
            && ("help".starts_with(bare)
                || "version".starts_with(bare)
                || "wait".starts_with(bare)
                || "login".starts_with(bare)
                || "interactive".starts_with(bare)))
        || (bare.len() >= 3
            && ("nologo".starts_with(bare)
                || "noexit".starts_with(bare)
                || "noprofile".starts_with(bare)
                || "noninteractive".starts_with(bare)))
        || (bare.len() >= 4 && "noprofileloadtime".starts_with(bare))
}

fn check_pwsh_risk(name: &str, args: &[String]) -> Result<(), String> {
    for w in args {
        if let Some(opt) = pwsh_option_body(w) {
            let lw = opt.to_lowercase();
            let bare = lw.split([':', '=']).next().unwrap_or("");
            if PWSH_SHORT.contains(&bare)
                || (!bare.is_empty() && PWSH_CODE.iter().any(|c| c.starts_with(bare)))
            {
                return Err(format!(
                    "an allow rule can't allow `{name} {w}`: it runs inline code"
                ));
            }
        }
    }

    let mut found_script = false;
    let mut i = 0;
    while i < args.len() {
        let w = &args[i];
        if w == "--" {
            if args.get(i + 1).is_some_and(|next| is_ps1_script(next)) {
                found_script = true;
            }
            break;
        }
        let Some(opt) = pwsh_option_body(w) else {
            if is_ps1_script(w) {
                found_script = true;
                break;
            }
            return Err(format!(
                "an allow rule for `{name}` must name a .ps1 script"
            ));
        };
        let lw = opt.to_lowercase();
        let (bare, attached) = match lw.find([':', '=']) {
            Some(pos) => (&lw[..pos], Some(&opt[pos + 1..])),
            None => (lw.as_str(), None),
        };
        if pwsh_is_file_opt(bare) {
            let script = match attached {
                Some(s) => Some(s),
                None => args.get(i + 1).map(String::as_str),
            };
            if script.is_some_and(is_ps1_script) {
                found_script = true;
                break;
            }
            return Err(format!(
                "an allow rule for `{name}` must name a .ps1 script"
            ));
        }
        if pwsh_is_value_opt(bare) {
            if attached.is_some() {
                i += 1;
            } else if i + 1 < args.len() {
                i += 2;
            } else {
                return Err(format!(
                    "an allow rule for `{name}` must name a .ps1 script"
                ));
            }
            continue;
        }
        if pwsh_is_bool_opt(bare) {
            i += 1;
            continue;
        }
        return Err(format!(
            "an allow rule for `{name}` must name a .ps1 script"
        ));
    }

    if !found_script {
        return Err(format!(
            "an allow rule for `{name}` must name a .ps1 script"
        ));
    }
    Ok(())
}

fn interpreter_value_flag(name: &str, raw: &str, lw: &str) -> bool {
    match name {
        "node" => {
            matches!(raw, "-r" | "-C")
                || matches!(
                    lw,
                    "--require"
                        | "--import"
                        | "--loader"
                        | "--experimental-loader"
                        | "--experimental-specifier-resolution"
                        | "--input-type"
                        | "--conditions"
                        | "--title"
                        | "--max-old-space-size"
                        | "--env-file"
                        | "--watch-path"
                        | "--redirect-warnings"
                        | "--diagnostic-dir"
                        | "--cpu-prof-dir"
                        | "--heap-prof-dir"
                        | "--test-name-pattern"
                )
        }
        "deno" => {
            matches!(raw, "-c" | "-L")
                || matches!(
                    lw,
                    "--config"
                        | "--import-map"
                        | "--lock"
                        | "--env"
                        | "--env-file"
                        | "--ext"
                        | "--log-level"
                        | "--location"
                        | "--cert"
                        | "--v8-flags"
                )
        }
        "bun" => {
            matches!(raw, "-c" | "-r")
                || matches!(
                    lw,
                    "--config"
                        | "--cwd"
                        | "--env-file"
                        | "--preload"
                        | "--port"
                        | "--define"
                        | "--loader"
                )
        }
        "ruby" => {
            matches!(raw, "-I" | "-r" | "-C" | "-E" | "-F" | "-K")
                || matches!(
                    lw,
                    "--encoding" | "--external-encoding" | "--internal-encoding"
                )
        }
        "perl" => matches!(raw, "-I" | "-M" | "-m" | "-F" | "-D" | "-x"),
        "php" => matches!(raw, "-c" | "-d" | "-z" | "-S" | "-t"),
        "lua" => raw == "-l",
        "osascript" => matches!(raw, "-l" | "-s"),
        _ => false,
    }
}

fn interpreter_bool_or_attached(name: &str, raw: &str, lw: &str) -> bool {
    if lw.starts_with("--") && lw.contains('=') {
        return true;
    }
    match name {
        "node" => {
            matches!(raw, "-v" | "-h" | "-c")
                || matches!(
                    lw,
                    "--version"
                        | "--help"
                        | "--check"
                        | "--no-deprecation"
                        | "--throw-deprecation"
                        | "--trace-deprecation"
                        | "--trace-warnings"
                        | "--no-warnings"
                        | "--zero-fill-buffers"
                        | "--preserve-symlinks"
                        | "--preserve-symlinks-main"
                        | "--watch"
                        | "--test"
                        | "--enable-source-maps"
                        | "--experimental-vm-modules"
                        | "--no-experimental-fetch"
                )
        }
        "deno" => {
            matches!(raw, "-A" | "-q" | "-r" | "-h" | "-V")
                || matches!(
                    lw,
                    "--allow-all"
                        | "--allow-read"
                        | "--allow-write"
                        | "--allow-net"
                        | "--allow-env"
                        | "--allow-run"
                        | "--allow-ffi"
                        | "--allow-sys"
                        | "--allow-hrtime"
                        | "--quiet"
                        | "--reload"
                        | "--no-check"
                        | "--watch"
                        | "--cached-only"
                        | "--no-lock"
                        | "--no-remote"
                        | "--unstable"
                        | "--help"
                        | "--version"
                )
                || lw.starts_with("--allow-")
                || lw.starts_with("--unstable-")
        }
        "bun" => {
            matches!(raw, "-b" | "-h" | "-v")
                || matches!(
                    lw,
                    "--hot"
                        | "--watch"
                        | "--bun"
                        | "--smol"
                        | "--silent"
                        | "--no-install"
                        | "--prefer-offline"
                        | "--production"
                        | "--help"
                        | "--version"
                )
        }
        "ruby" => {
            matches!(lw, "--verbose" | "--version")
                || ((raw.starts_with("-I")
                    || raw.starts_with("-r")
                    || raw.starts_with("-E")
                    || raw.starts_with("-W")
                    || raw.starts_with("-0")
                    || raw.starts_with("-i"))
                    && raw.len() > 2)
                || (!raw.starts_with("--") && raw[1..].chars().all(|c| "wvVTsnapld0y".contains(c)))
        }
        "perl" => {
            ((raw.starts_with("-I")
                || raw.starts_with("-M")
                || raw.starts_with("-m")
                || raw.starts_with("-0")
                || raw.starts_with("-i"))
                && raw.len() > 2)
                || (!raw.starts_with("--")
                    && raw[1..].chars().all(|c| "wWvVTtsnpald0cuU".contains(c)))
        }
        "php" => !raw.starts_with("--") && raw[1..].chars().all(|c| "nqwlsmvih".contains(c)),
        "lua" => matches!(raw, "-i" | "-v" | "-W"),
        "osascript" => matches!(raw, "-i" | "-o"),
        _ => false,
    }
}

fn check_interpreter_risk(
    name: &str,
    spec: &InterpreterSpec,
    args: &[String],
    prefix: bool,
) -> Result<(), String> {
    // Scan all args for inline-code flags (fail closed, even after a script name).
    for w in args {
        let lw = w.to_lowercase();
        if lw.starts_with("--") {
            if spec
                .long
                .iter()
                .any(|l| lw == *l || lw.starts_with(&format!("{l}=")))
            {
                return Err(format!(
                    "an allow rule can't allow `{name} {w}`: it runs inline code"
                ));
            }
        } else if is_option(w)
            && !spec.short.is_empty()
            && lw[1..].chars().any(|c| spec.short.contains(c))
        {
            return Err(format!(
                "an allow rule can't allow `{name} {w}`: it holds an inline-code flag"
            ));
        }
    }

    // Identify the script / module operand while consuming known option values
    // and failing closed on unknown options before the script.
    let is_python = name.starts_with("python");
    let mut verb_seen = false;
    let mut found_target = false;
    let mut i = 0;
    while i < args.len() {
        let w = &args[i];
        let lw = w.to_lowercase();
        if w == "--" {
            if let Some(next) = args.get(i + 1) {
                let ln = next.to_lowercase();
                if is_stdin_script_path(next) {
                    return Err(format!(
                        "an allow rule can't allow `{name} {next}`: it reads code from standard input"
                    ));
                }
                if !verb_seen && spec.sub.contains(&ln.as_str()) {
                    return Err(format!(
                        "an allow rule can't allow `{name} {next}`: it runs inline code"
                    ));
                }
                found_target = true;
            }
            break;
        }
        if !is_option(w) {
            if is_stdin_script_path(w) {
                return Err(format!(
                    "an allow rule can't allow `{name} {w}`: it reads code from standard input"
                ));
            }
            if !verb_seen && spec.sub.contains(&lw.as_str()) {
                return Err(format!(
                    "an allow rule can't allow `{name} {w}`: it runs inline code"
                ));
            }
            if !verb_seen && !spec.verbs.is_empty() && spec.verbs.contains(&lw.as_str()) {
                verb_seen = true;
                i += 1;
                continue;
            }
            found_target = true;
            break;
        }

        if is_python {
            if w == "-m" {
                if args
                    .get(i + 1)
                    .is_some_and(|m| !m.is_empty() && !m.starts_with('-'))
                {
                    found_target = true;
                    break;
                }
                return Err(format!(
                    "an allow rule for `{name}` must name the script or module it runs"
                ));
            }
            if w.starts_with("-m") && w.len() > 2 {
                found_target = true;
                break;
            }
            if matches!(w.as_str(), "-W" | "-X" | "-Q" | "--check-hash-based-pycs") {
                if i + 1 >= args.len() {
                    return Err(format!(
                        "an allow rule for `{name}` must name the script it runs"
                    ));
                }
                i += 2;
                continue;
            }
            if w.starts_with("--check-hash-based-pycs=")
                || ((w.starts_with("-W") || w.starts_with("-X") || w.starts_with("-Q"))
                    && w.len() > 2)
            {
                i += 1;
                continue;
            }
            if matches!(
                lw.as_str(),
                "--version"
                    | "--help"
                    | "--help-all"
                    | "--help-env"
                    | "--help-xoptions"
                    | "--verbose"
                    | "--quiet"
                    | "--isolated"
                    | "--ignore-environment"
                    | "--no-site"
                    | "--no-user-site"
                    | "--skip-source-first-line"
                    | "--bytes-warning"
                    | "--dont-write-bytecode"
                    | "--optimize"
                    | "--inspect"
                    | "--unbuffered"
            ) || (!w.starts_with("--") && w[1..].chars().all(|c| "bBdEhiIOqsSuvVx".contains(c)))
            {
                i += 1;
                continue;
            }
            return Err(format!(
                "an allow rule for `{name}` must name the script it runs"
            ));
        }

        if interpreter_value_flag(name, w, &lw) {
            if i + 1 >= args.len() {
                return Err(format!(
                    "an allow rule for `{name}` must name the script it runs"
                ));
            }
            i += 2;
            continue;
        }
        if interpreter_bool_or_attached(name, w, &lw) {
            i += 1;
            continue;
        }
        return Err(format!(
            "an allow rule for `{name}` must name the script it runs"
        ));
    }

    if !found_target {
        return Err(format!(
            "an allow rule for `{name}` must name the script it runs"
        ));
    }
    if prefix {
        let at = usize::from(
            !spec.verbs.is_empty()
                && args
                    .first()
                    .is_some_and(|a| spec.verbs.contains(&a.to_lowercase().as_str())),
        );
        if args.get(at).is_none_or(|w| w.starts_with('-')) {
            return Err(format!(
                "a prefix allow rule for `{name}` must name the script right after it"
            ));
        }
    }
    Ok(())
}

fn check_package_runner_risk(name: &str, args: &[String], prefix: bool) -> Result<(), String> {
    if let Some(code) = args.iter().find(|w| {
        let lw = w.to_lowercase();
        RUNNER_CODE.contains(&lw.as_str())
            || lw.starts_with("--call=")
            || (is_option(w) && !lw.starts_with("--") && lw[1..].contains('c'))
    }) {
        return Err(format!(
            "an allow rule can't allow `{name} {code}`: it runs a shell command"
        ));
    }
    if prefix && args.first().is_none_or(|a| a.starts_with('-')) {
        return Err(format!(
            "a prefix allow rule for `{name}` must name the package right after it"
        ));
    }
    let mut i = 0;
    let mut found_pkg = false;
    while i < args.len() {
        let w = &args[i];
        let lw = w.to_lowercase();
        if w == "--" {
            found_pkg = args
                .get(i + 1)
                .is_some_and(|next| !next.is_empty() && next != "-");
            break;
        }
        if !is_option(w) {
            if w != "-" {
                found_pkg = true;
                break;
            }
            i += 1;
            continue;
        }
        if lw.starts_with("--") && lw.contains('=') {
            i += 1;
            continue;
        }
        if matches!(
            lw.as_str(),
            "-p" | "--package"
                | "--from"
                | "--with"
                | "--with-editable"
                | "--with-requirements"
                | "--python"
                | "--index-url"
                | "--extra-index-url"
                | "--node-options"
                | "-n"
                | "--cache"
                | "--userconfig"
        ) {
            if i + 1 >= args.len() {
                return Err(format!(
                    "an allow rule for `{name}` must name the package it runs"
                ));
            }
            i += 2;
            continue;
        }
        if matches!(
            lw.as_str(),
            "-y" | "--yes"
                | "-q"
                | "--quiet"
                | "--no"
                | "--no-install"
                | "--offline"
                | "--prefer-offline"
                | "--ignore-existing"
                | "--isolated"
                | "--no-cache"
                | "--no-build-isolation"
                | "--system"
                | "--verbose"
                | "-v"
                | "-h"
                | "--help"
                | "--version"
                | "--shell-auto-fallback"
        ) || (!lw.starts_with("--") && lw[1..].chars().all(|c| "yqnhv".contains(c)))
        {
            i += 1;
            continue;
        }
        return Err(format!(
            "an allow rule for `{name}` must name the package it runs"
        ));
    }
    if !found_pkg {
        return Err(format!(
            "an allow rule for `{name}` must name the package it runs"
        ));
    }
    Ok(())
}

fn git_config_is_exec(kv: &str) -> bool {
    let (key, val) = kv.split_once('=').unwrap_or((kv, ""));
    let lk = key.to_ascii_lowercase();
    val.trim_start().starts_with('!')
        || lk.starts_with("alias.")
        || (lk.starts_with("merge.") && lk.ends_with(".driver"))
        || (lk.starts_with("filter.")
            && (lk.ends_with(".clean") || lk.ends_with(".smudge") || lk.ends_with(".process")))
        || (lk.starts_with("diff.")
            && (lk.ends_with(".command") || lk.ends_with(".textconv") || lk == "diff.external"))
        || lk.starts_with("credential.")
        || lk.starts_with("uploadpack.")
        || lk.starts_with("receive.")
        || lk.starts_with("gpg.")
        || lk.starts_with("remote.")
        || lk.starts_with("protocol.")
        || matches!(
            lk.as_str(),
            "core.fsmonitor"
                | "core.sshcommand"
                | "core.askpass"
                | "core.editor"
                | "core.pager"
                | "core.gitproxy"
                | "sequence.editor"
                | "browser"
        )
}

/// Returns `(has_any_cli_config_or_exec, has_dangerous_exec_config)` for `git`.
fn git_cli_exec_config(args: &[String]) -> (bool, bool) {
    let mut has_any = false;
    let mut has_dangerous = false;
    let mut i = 0;
    while i < args.len() {
        let w = args[i].as_str();
        if w == "--" {
            break;
        }
        if w == "-c" {
            has_any = true;
            if args.get(i + 1).is_none_or(|kv| git_config_is_exec(kv)) {
                has_dangerous = true;
            }
            i += 2;
            continue;
        }
        if let Some(kv) = w.strip_prefix("-c")
            && !kv.is_empty()
        {
            has_any = true;
            if git_config_is_exec(kv) {
                has_dangerous = true;
            }
            i += 1;
            continue;
        }
        if w == "--config-env" || w == "--exec-path" {
            has_any = true;
            has_dangerous = true;
            i += 2;
            continue;
        }
        if w.starts_with("--config-env=") || w.starts_with("--exec-path=") {
            has_any = true;
            has_dangerous = true;
            i += 1;
            continue;
        }
        if matches!(
            w,
            "-C" | "--git-dir" | "--work-tree" | "--namespace" | "--super-prefix" | "--list-cmds"
        ) {
            i += 2;
            continue;
        }
        if w.starts_with('-') {
            i += 1;
            continue;
        }
        break;
    }
    for w in args {
        if w == "--" {
            break;
        }
        if matches!(
            w.as_str(),
            "--ext-diff" | "--config-env" | "--exec-path" | "--upload-pack" | "--receive-pack"
        ) || w.starts_with("--config-env=")
            || w.starts_with("--exec-path=")
            || w.starts_with("--upload-pack=")
            || w.starts_with("--receive-pack=")
            || w.starts_with("--exec=")
        {
            has_any = true;
            has_dangerous = true;
        }
    }
    (has_any, has_dangerous)
}

/// Checks whether an allow command rule (`CommandPrefix` or `CommandExact`)
/// would bypass Atrium's simple-command or inline-code checks.
pub fn command_allow_risk(argv: &[String], prefix: bool) -> Result<(), String> {
    let Some(first) = argv.first().filter(|s| !s.is_empty()) else {
        return Err("the rule's command is empty".into());
    };
    let name = fold_name(first);
    let args = &argv[1..];
    if SHELLS.contains(&name.as_str()) {
        return Err(format!("an allow rule can't allow shell `{name}`"));
    }
    if RUNS_HERE.contains(&name.as_str()) {
        return Err(format!("an allow rule can't allow `{name}`"));
    }
    if WRAPPERS.contains(&name.as_str()) {
        return Err(format!("an allow rule can't allow wrapper `{name}`"));
    }
    if is_pwsh(&name) {
        return check_pwsh_risk(&name, args);
    }
    if let Some(spec) = interpreter_spec(&name) {
        return check_interpreter_risk(&name, spec, args, prefix);
    }
    if PACKAGE_RUNNERS.contains(&name.as_str()) {
        return check_package_runner_risk(&name, args, prefix);
    }
    if name == "git" {
        let (has_any, has_dangerous) = git_cli_exec_config(args);
        if (prefix && has_any) || has_dangerous {
            return Err(
                "an allow rule can't allow `git` with command-line configuration or exec options"
                    .into(),
            );
        }
    }
    if name == "find" {
        if let Some(exec) = args
            .iter()
            .find(|w| FIND_EXEC.contains(&w.to_lowercase().as_str()))
        {
            return Err(format!("an allow rule can't allow `find {exec}`"));
        }
        if prefix {
            return Err("a prefix allow rule on `find` isn't allowed".into());
        }
    }
    Ok(())
}

fn validate_abs_glob(glob: &str, label: &str) -> Result<(), String> {
    if glob.is_empty() || glob.chars().count() > 1000 || glob.contains(['\n', '\r', '\0']) {
        return Err(format!("invalid {label}"));
    }
    if !glob.starts_with('/') {
        return Err(format!("{label} must be an absolute path starting with /"));
    }
    if glob.split('/').any(|seg| seg == "." || seg == "..") {
        return Err(format!("{label} cannot contain . or .. segments"));
    }
    if GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .is_err()
    {
        return Err(format!("invalid {label}"));
    }
    Ok(())
}

/// Validates a `RuleDraft` from `CreateRule`, matching the app's rules editor.
pub fn validate_rule_draft(draft: &RuleDraft) -> Result<(), String> {
    if let RuleScope::Thread { thread_id } = &draft.scope
        && (thread_id.0.trim().is_empty()
            || thread_id
                .0
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')))
    {
        return Err("thread scope requires a valid thread id".into());
    }

    if let Some(tn) = &draft.tool_name
        && (tn.is_empty()
            || tn.chars().count() > 100
            || tn.chars().any(|c| {
                c.is_whitespace() || c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')
            }))
    {
        return Err("invalid tool name".into());
    }

    match &draft.matcher {
        RuleMatcher::CommandPrefix { argv } | RuleMatcher::CommandExact { argv } => {
            if draft.tool_kind != ToolKind::Execute {
                return Err("command rules must have tool_kind Execute".into());
            }
            if argv.is_empty() || argv.first().is_some_and(String::is_empty) {
                return Err("the rule's command is empty".into());
            }
            let total_chars: usize = argv.iter().map(|s| s.chars().count()).sum::<usize>()
                + argv.len().saturating_sub(1);
            if total_chars > 4000 {
                return Err("the rule's command is too long".into());
            }
            if argv.iter().any(|w| {
                w.chars()
                    .any(|c| (c.is_control() && c != '\t') || matches!(c, '\u{2028}' | '\u{2029}'))
            }) {
                return Err(
                    "command rules cannot contain line breaks or control characters".into(),
                );
            }
            if RESERVED.contains(&fold_name(&argv[0]).as_str()) {
                return Err("a command rule must start with a simple command".into());
            }
            if draft.decision == RuleDecision::Allow {
                command_allow_risk(
                    argv,
                    matches!(draft.matcher, RuleMatcher::CommandPrefix { .. }),
                )?;
            }
        }
        RuleMatcher::PathGlob { glob } => {
            if !is_path_kind(draft.tool_kind) {
                return Err("path rules are only allowed for file tools".into());
            }
            validate_abs_glob(glob, "path glob")?;
        }
        RuleMatcher::Any => {
            if draft.tool_kind == ToolKind::Think {
                return Err("rules cannot target Think".into());
            }
        }
    }

    if let Some(scope) = &draft.path_scope {
        let offered = draft.decision == RuleDecision::Allow
            && is_path_kind(draft.tool_kind)
            && matches!(
                draft.matcher,
                RuleMatcher::PathGlob { .. } | RuleMatcher::Any
            );
        if !offered {
            return Err("path scope is only allowed on allow rules for file tools".into());
        }
        validate_abs_glob(scope, "path scope glob")?;
    }

    Ok(())
}

fn find_has_empty_exec(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let lw = args[i].to_lowercase();
        if FIND_EXEC.contains(&lw.as_str()) {
            let mut m = i + 1;
            let mut count = 0usize;
            while m < args.len() && args[m] != ";" && args[m] != "+" {
                count += 1;
                m += 1;
            }
            if count == 0 {
                return true;
            }
            i = m + 1;
            continue;
        }
        i += 1;
    }
    false
}

/// Whether `CommandPrefix` `rule_prefix` must be refused for `argv` at
/// evaluation time (shells, `eval`/`source`/`.`, `find -delete`, malformed
/// `find -exec`, `git -c`/exec config, or interpreters/runners with inline code or no pinned script).
pub(crate) fn command_prefix_blocked(argv: &[String], rule_prefix: &[String]) -> bool {
    let Some(first) = argv.first().filter(|s| !s.is_empty()) else {
        return true;
    };
    let name = fold_name(first);
    let args = &argv[1..];
    if SHELLS.contains(&name.as_str()) || RUNS_HERE.contains(&name.as_str()) {
        return true;
    }
    if name == "git" && git_cli_exec_config(args).0 {
        return true;
    }
    if name == "find" {
        if args.iter().any(|w| w.to_lowercase() == "-delete") || find_has_empty_exec(args) {
            return true;
        }
        return false;
    }
    if is_pwsh(&name)
        || interpreter_spec(&name).is_some()
        || PACKAGE_RUNNERS.contains(&name.as_str())
    {
        if command_allow_risk(rule_prefix, true).is_ok() {
            return false;
        }
        return command_allow_risk(argv, true).is_err();
    }
    false
}

/// Whether `CommandExact` must be refused for `argv` at evaluation time.
pub(crate) fn command_exact_blocked(argv: &[String]) -> bool {
    let Some(first) = argv.first().filter(|s| !s.is_empty()) else {
        return true;
    };
    let name = fold_name(first);
    if SHELLS.contains(&name.as_str()) || RUNS_HERE.contains(&name.as_str()) {
        return true;
    }
    if name == "git" && git_cli_exec_config(&argv[1..]).1 {
        return true;
    }
    if is_pwsh(&name)
        || interpreter_spec(&name).is_some()
        || PACKAGE_RUNNERS.contains(&name.as_str())
    {
        return command_allow_risk(argv, false).is_err();
    }
    if name == "find" {
        return argv[1..]
            .iter()
            .any(|w| FIND_EXEC.contains(&w.to_lowercase().as_str()));
    }
    false
}
