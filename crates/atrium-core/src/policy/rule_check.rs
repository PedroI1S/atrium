//! Shared rule validation for both `atrium-daemon` (`CreateRule`) and the
//! policy engine (`engine.rs`). Matches `shell.rs` and `deny.rs` tables and
//! mirrors the app rules editor (`app/src/chat/rule-form.ts`).

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{RuleDecision, RuleDraft, RuleMatcher};
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
const STDIN: &[&str] = &["-", "/dev/stdin", "/dev/fd/0", "/proc/self/fd/0"];
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

fn check_pwsh_risk(name: &str, args: &[String]) -> Result<(), String> {
    for w in args {
        let lw = w.to_lowercase();
        let opt = lw
            .strip_prefix("--")
            .or_else(|| lw.strip_prefix('-'))
            .or_else(|| lw.strip_prefix('/'));
        if let Some(opt) = opt {
            let bare = opt.split([':', '=']).next().unwrap_or("");
            if PWSH_SHORT.contains(&bare)
                || (!bare.is_empty() && PWSH_CODE.iter().any(|c| c.starts_with(bare)))
            {
                return Err(format!(
                    "an allow rule can't allow `{name} {w}`: it runs inline code"
                ));
            }
        }
    }
    if !args
        .iter()
        .any(|w| !w.starts_with('-') && w.to_ascii_lowercase().ends_with(".ps1"))
    {
        return Err(format!(
            "an allow rule for `{name}` must name a .ps1 script"
        ));
    }
    Ok(())
}

fn check_interpreter_risk(
    name: &str,
    spec: &InterpreterSpec,
    args: &[String],
    prefix: bool,
) -> Result<(), String> {
    let mut operands: Vec<String> = Vec::new();
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
        } else if is_option(w) {
            if !spec.short.is_empty() && lw[1..].chars().any(|c| spec.short.contains(c)) {
                return Err(format!(
                    "an allow rule can't allow `{name} {w}`: it holds an inline-code flag"
                ));
            }
        } else {
            operands.push(lw);
        }
    }
    if let Some(stdin) = operands.iter().find(|o| STDIN.contains(&o.as_str())) {
        return Err(format!(
            "an allow rule can't allow `{name} {stdin}`: it reads code from standard input"
        ));
    }
    let subcommand = operands.first().map(String::as_str);
    if let Some(sub) = subcommand
        && spec.sub.contains(&sub)
    {
        return Err(format!(
            "an allow rule can't allow `{name} {sub}`: it runs inline code"
        ));
    }
    let verb = usize::from(subcommand.is_some_and(|s| spec.verbs.contains(&s)));
    if operands.len() <= verb {
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
    if !args.iter().any(|w| !w.starts_with('-')) {
        return Err(format!(
            "an allow rule for `{name}` must name the package it runs"
        ));
    }
    Ok(())
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

/// Validates a `RuleDraft` from `CreateRule`, matching the app's rules editor.
pub fn validate_rule_draft(draft: &RuleDraft) -> Result<(), String> {
    match &draft.matcher {
        RuleMatcher::CommandPrefix { argv } | RuleMatcher::CommandExact { argv } => {
            if argv.is_empty() || argv.first().is_some_and(String::is_empty) {
                return Err("the rule's command is empty".into());
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
            if glob.is_empty()
                || GlobBuilder::new(glob)
                    .literal_separator(true)
                    .build()
                    .is_err()
            {
                return Err("invalid path glob".into());
            }
        }
        RuleMatcher::Any => {}
    }

    if let Some(scope) = &draft.path_scope {
        let is_path_kind = matches!(
            draft.tool_kind,
            ToolKind::Read | ToolKind::Edit | ToolKind::Delete | ToolKind::Move | ToolKind::Search
        );
        let offered = draft.decision == RuleDecision::Allow
            && (matches!(draft.matcher, RuleMatcher::PathGlob { .. })
                || (matches!(draft.matcher, RuleMatcher::Any) && is_path_kind));
        if !offered {
            return Err("path scope is only allowed on allow rules for file tools".into());
        }
        if scope.is_empty()
            || GlobBuilder::new(scope)
                .literal_separator(true)
                .build()
                .is_err()
        {
            return Err("invalid path scope glob".into());
        }
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
/// `find -exec`, or interpreters/runners with inline code or no pinned script).
pub(crate) fn command_prefix_blocked(argv: &[String], rule_prefix: &[String]) -> bool {
    let Some(first) = argv.first().filter(|s| !s.is_empty()) else {
        return true;
    };
    let name = fold_name(first);
    let args = &argv[1..];
    if SHELLS.contains(&name.as_str()) || RUNS_HERE.contains(&name.as_str()) {
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
