//! `DefaultPolicy`: hard deny, then rules (thread, Home, global), then the
//! preset. Pure: everything comes in `PolicyInput`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use atrium_protocol::agent::ToolKind;
use atrium_protocol::policy::{
    HOME_READONLY_COMMANDS, Preset, ReadOnlyCommand, Recursion, Rule, RuleDecision, RuleId,
    RuleMatcher, RuleScope, ToolSubject,
};
use globset::GlobBuilder;

use super::DefaultPolicy;
use super::deny::{self, Analysis, Sensitive};
use super::rule_check;
use super::shell::Unit;
use crate::{PolicyEngine, PolicyInput, PolicyVerdict};

fn effective_kind(input: &PolicyInput<'_>) -> ToolKind {
    match input.check.subject {
        ToolSubject::Command { .. } => ToolKind::Execute,
        _ => input.check.kind,
    }
}

fn glob_matches(glob: &str, p: &Path) -> bool {
    GlobBuilder::new(glob)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher().is_match(p))
        .unwrap_or(false)
}

fn looks_like_path_word(w: &str) -> bool {
    if w.is_empty() || w.contains('\0') || w.contains("://") {
        return false;
    }
    w.starts_with(['/', '~'])
        || w == "."
        || w == ".."
        || w.starts_with("./")
        || w.starts_with("../")
        || w.starts_with("$HOME")
        || w.starts_with("${HOME}")
        || w.starts_with("$PWD")
        || w.starts_with("${PWD}")
        || (w.contains('/') && !w.contains("://"))
        || (w.starts_with('.') && w.len() > 1 && !w.starts_with(".."))
}

fn push_expanded_path(word: &str, out: &mut HashSet<String>) {
    if word.is_empty() || word.contains('\0') || word.contains("://") {
        return;
    }
    if word.contains('{')
        && let Some(expanded) = super::words::brace_expand(word)
    {
        for w in expanded {
            out.insert(w);
        }
        return;
    }
    out.insert(word.to_string());
}

fn is_generic_path_long_flag(flag: &str) -> bool {
    matches!(
        flag,
        "--file"
            | "--output"
            | "--out-dir"
            | "--manifest-path"
            | "--target-dir"
            | "--git-dir"
            | "--work-tree"
            | "--exec-path"
            | "--directory"
            | "--target-directory"
            | "--chdir"
            | "--cwd"
            | "--prefix"
            | "--dir"
            | "--reference"
            | "--from-file"
            | "--exclude-from"
            | "--files-from"
            | "--arg-file"
            | "--cookie"
            | "--cookie-jar"
            | "--upload-file"
            | "--config"
            | "--output-document"
            | "--path"
            | "--root"
    )
}

fn is_cmd_path_short_flag(cmd: &str, ch: char) -> bool {
    match cmd {
        "cp" | "mv" | "ln" | "install" => ch == 't',
        "touch" | "truncate" => ch == 'r',
        "sort" => matches!(ch, 'o' | 'T'),
        "tree" => ch == 'o',
        "sed" | "awk" | "gawk" | "mawk" | "jq" => ch == 'f',
        "tar" => matches!(ch, 'f' | 'C' | 'T' | 'X'),
        "diff" => ch == 'X',
        "curl" | "wget" => matches!(ch, 'o' | 'O' | 'T' | 'c' | 'K'),
        "xargs" => ch == 'a',
        "git" => ch == 'C',
        _ => false,
    }
}

fn is_script_cmd(cmd: &str) -> bool {
    matches!(cmd, "sed" | "awk" | "gawk" | "mawk")
}

fn is_explicit_rel_or_var_path(w: &str) -> bool {
    w.starts_with("~/")
        || w == "~"
        || w.starts_with("./")
        || w.starts_with("../")
        || w.starts_with("$HOME")
        || w.starts_with("${HOME}")
        || w.starts_with("$PWD")
        || w.starts_with("${PWD}")
}

/// A path word whose target can't be read lexically (an unparsed `sed`
/// script, a variable, a glob): an allow scope never covers it, a deny scope
/// always matches it (fail closed). Real paths never hold a NUL.
const UNRESOLVED: &str = "\0unresolved";

/// The files a `sed` script reads or writes: the `r`/`R`/`w`/`W` commands
/// and the `s` command's `w` flag, whatever the delimiter, with or without a
/// space before the name (the name runs to the end of the line). `None`
/// when the script can't be read: the caller fails closed.
fn sed_script_files(script: &str) -> Option<Vec<String>> {
    let c: Vec<char> = script.chars().collect();
    let n = c.len();
    let mut out = vec![];
    let mut i = 0usize;
    let rest_of_line = |i: &mut usize| -> String {
        let start = *i;
        while *i < n && c[*i] != '\n' {
            *i += 1;
        }
        c[start..*i].iter().collect::<String>().trim_start().to_string()
    };
    // until an unescaped `d` (a newline inside is fine for `s`)
    let delimited = |i: &mut usize, d: char| -> Option<()> {
        while *i < n {
            if c[*i] == '\\' {
                *i += 2;
                continue;
            }
            if c[*i] == d {
                *i += 1;
                return Some(());
            }
            *i += 1;
        }
        None
    };
    let addr = |i: &mut usize| -> Option<()> {
        if *i < n && c[*i].is_ascii_digit() {
            while *i < n && (c[*i].is_ascii_digit() || c[*i] == '~') {
                *i += 1;
            }
        } else if *i < n && c[*i] == '$' {
            *i += 1;
        } else if *i < n && (c[*i] == '/' || c[*i] == '\\') {
            let d = if c[*i] == '\\' {
                *i += 1;
                *c.get(*i)?
            } else {
                '/'
            };
            *i += 1;
            delimited(i, d)?;
            while *i < n && matches!(c[*i], 'I' | 'M') {
                *i += 1;
            }
        }
        Some(())
    };
    let ws = |i: &mut usize| {
        while *i < n && matches!(c[*i], ' ' | '\t') {
            *i += 1;
        }
    };
    loop {
        while i < n && (c[i].is_whitespace() || c[i] == ';') {
            i += 1;
        }
        if i >= n {
            break;
        }
        if c[i] == '#' {
            rest_of_line(&mut i);
            continue;
        }
        addr(&mut i)?;
        ws(&mut i);
        if i < n && c[i] == ',' {
            i += 1;
            ws(&mut i);
            if i < n && matches!(c[i], '+' | '~') {
                i += 1;
            }
            addr(&mut i)?;
            ws(&mut i);
        }
        while i < n && c[i] == '!' {
            i += 1;
            ws(&mut i);
        }
        let Some(&cmd) = c.get(i) else {
            return None;
        };
        i += 1;
        match cmd {
            '{' | '}' | '=' | 'd' | 'D' | 'g' | 'G' | 'h' | 'H' | 'n' | 'N' | 'p' | 'P' | 'x'
            | 'z' | 'F' => {}
            'q' | 'Q' | 'l' | 'L' => {
                ws(&mut i);
                while i < n && c[i].is_ascii_digit() {
                    i += 1;
                }
            }
            's' | 'y' => {
                let d = *c.get(i)?;
                if d == '\\' || d == '\n' {
                    return None;
                }
                i += 1;
                delimited(&mut i, d)?;
                delimited(&mut i, d)?;
                if cmd == 's' {
                    while i < n && matches!(c[i], 'g' | 'p' | 'i' | 'I' | 'm' | 'M' | 'e' | '0'..='9')
                    {
                        i += 1;
                    }
                    if i < n && c[i] == 'w' {
                        i += 1;
                        out.push(rest_of_line(&mut i));
                    }
                }
            }
            'r' | 'R' | 'w' | 'W' => out.push(rest_of_line(&mut i)),
            // text, labels, a command to run, a version: not files
            'a' | 'i' | 'c' | 'e' => loop {
                let line = rest_of_line(&mut i);
                if !line.ends_with('\\') || i >= n {
                    break;
                }
                i += 1;
            },
            'b' | 't' | 'T' | ':' | 'v' => {
                while i < n && !matches!(c[i], ';' | '\n') {
                    i += 1;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

fn push_script_path_tokens(cmd: &str, script: &str, out: &mut HashSet<String>) {
    if !is_script_cmd(cmd) {
        return;
    }
    if cmd == "sed" {
        match sed_script_files(script) {
            Some(files) => {
                for f in files {
                    if f.is_empty() {
                        out.insert(UNRESOLVED.to_string());
                    } else {
                        push_expanded_path(&f, out);
                    }
                }
            }
            None => {
                out.insert(UNRESOLVED.to_string());
            }
        }
        return;
    }
    let mut prev: Option<&str> = None;
    for tok in script.split_whitespace().take(64) {
        let t = tok.trim_matches(|c| "\"'{}();".contains(c));
        if t.is_empty() {
            continue;
        }
        let after_cmd = prev.is_some_and(|p| {
            matches!(p, ">" | ">>" | "<") || p.ends_with('>') || p.ends_with('<')
        });
        let quoted_path = tok.contains('"') && looks_like_path_word(t);
        if after_cmd || is_explicit_rel_or_var_path(t) || quoted_path {
            push_expanded_path(t, out);
        }
        prev = Some(t);
    }
}

fn is_cmd_nonpath_value_flag(cmd: &str, w: &str) -> bool {
    let long = w.starts_with("--");
    if long {
        return match cmd {
            "head" | "tail" => matches!(w, "--lines" | "--bytes"),
            "ls" => matches!(
                w,
                "--ignore"
                    | "--hide"
                    | "--sort"
                    | "--time"
                    | "--format"
                    | "--quoting-style"
                    | "--time-style"
                    | "--block-size"
            ),
            "cp" | "mv" | "ln" | "install" => {
                matches!(w, "--suffix" | "--mode" | "--owner" | "--group")
            }
            "mkdir" => w == "--mode",
            "touch" => matches!(w, "--date" | "--time"),
            "truncate" | "shred" => matches!(w, "--size" | "--iterations"),
            "sort" => matches!(
                w,
                "--key" | "--field-separator" | "--buffer-size" | "--parallel"
            ),
            "cut" => matches!(
                w,
                "--bytes" | "--characters" | "--delimiter" | "--fields" | "--output-delimiter"
            ),
            "uniq" => matches!(w, "--skip-fields" | "--skip-chars" | "--check-chars"),
            "diff" => matches!(w, "--exclude" | "--starting-file" | "--label"),
            "sed" | "awk" | "gawk" | "mawk" => matches!(w, "--expression" | "--assign"),
            "jq" => matches!(w, "--arg" | "--argjson"),
            "tar" => matches!(w, "--exclude" | "--transform" | "--strip-components"),
            "curl" | "wget" => matches!(
                w,
                "--data"
                    | "--header"
                    | "--request"
                    | "--user-agent"
                    | "--user"
                    | "--max-time"
                    | "--connect-timeout"
            ),
            "xargs" => matches!(
                w,
                "--max-args" | "--max-procs" | "--max-lines" | "--replace"
            ),
            _ => false,
        };
    }
    if !w.starts_with('-') || w.len() < 2 {
        return false;
    }
    let last = w.chars().last().unwrap_or('\0');
    match cmd {
        "head" | "tail" => matches!(last, 'n' | 'c'),
        "ls" => matches!(last, 'w' | 'T' | 'I'),
        "cp" | "mv" | "ln" => last == 'S',
        "mkdir" => last == 'm',
        "install" => matches!(last, 'm' | 'o' | 'g' | 'S'),
        "touch" => matches!(last, 'd' | 't'),
        "truncate" => last == 's',
        "shred" => matches!(last, 'n' | 's'),
        "tree" => matches!(last, 'L' | 'P' | 'I'),
        "sort" => matches!(last, 'k' | 't' | 'S'),
        "cut" => matches!(last, 'b' | 'c' | 'd' | 'f'),
        "uniq" => matches!(last, 'f' | 's' | 'w'),
        "diff" => matches!(last, 'U' | 'C' | 'I' | 'x' | 'S'),
        "sed" => last == 'e',
        "awk" | "gawk" | "mawk" => matches!(last, 'e' | 'F' | 'v'),
        "curl" => matches!(last, 'X' | 'H' | 'A' | 'u' | 'd' | 'm'),
        "wget" => matches!(last, 'A' | 'R' | 'U' | 'T' | 'Q' | 't'),
        "xargs" => matches!(last, 'n' | 'P' | 'L' | 'I' | 'd' | 's' | 'E'),
        "ag" | "ack" => matches!(last, 'A' | 'B' | 'C' | 'G' | 'm'),
        _ => false,
    }
}

fn collect_explicit_flag_paths(cmd: &str, args: &[String], out: &mut HashSet<String>) {
    for w in args {
        if let Some((flag, val)) = w.split_once('=') {
            if flag.starts_with("--")
                && (is_generic_path_long_flag(flag) || looks_like_path_word(val))
            {
                push_expanded_path(val, out);
            }
        } else if w.starts_with('-') && !w.starts_with("--") && w.len() > 2 {
            let cs: Vec<char> = w[1..].chars().collect();
            for k in 0..cs.len().saturating_sub(1) {
                let rest: String = cs[k + 1..].iter().collect();
                let rest = rest.strip_prefix('=').unwrap_or(&rest);
                if is_cmd_path_short_flag(cmd, cs[k]) || looks_like_path_word(rest) {
                    push_expanded_path(rest, out);
                }
            }
        }
    }
}

fn collect_generic_operands(cmd: &str, args: &[String], out: &mut HashSet<String>) {
    let mut after_dd = false;
    let mut skip_first_positional = matches!(cmd, "chmod" | "chown" | "chgrp" | "ag" | "ack");
    let mut need_script = matches!(cmd, "sed" | "awk" | "gawk" | "mawk" | "jq");
    let mut has_reference = false;
    let mut i = 0;
    while i < args.len() {
        let w = args[i].as_str();
        if after_dd {
            if w != "-" {
                push_expanded_path(w, out);
            }
            i += 1;
            continue;
        }
        if w == "--" {
            after_dd = true;
            i += 1;
            continue;
        }
        if let Some((flag, val)) = w.split_once('=')
            && flag.starts_with("--")
        {
            if flag == "--reference" {
                has_reference = true;
            }
            if matches!(flag, "--file" | "--from-file" | "--expression") {
                need_script = false;
            }
            if flag == "--expression" {
                push_script_path_tokens(cmd, val, out);
            }
            if is_generic_path_long_flag(flag) || looks_like_path_word(val) {
                push_expanded_path(val, out);
            }
            i += 1;
            continue;
        }
        if w.starts_with("--") {
            if w == "--reference" {
                has_reference = true;
            }
            if matches!(w, "--file" | "--from-file" | "--expression") {
                need_script = false;
            }
            if w == "--expression" && is_script_cmd(cmd) {
                if let Some(next) = args.get(i + 1) {
                    push_script_path_tokens(cmd, next, out);
                }
                i += 2;
                continue;
            }
            if is_generic_path_long_flag(w) {
                if let Some(next) = args.get(i + 1) {
                    push_expanded_path(next, out);
                }
                i += 2;
                continue;
            }
            if is_cmd_nonpath_value_flag(cmd, w) {
                if let Some(next) = args.get(i + 1)
                    && looks_like_path_word(next)
                {
                    push_expanded_path(next, out);
                }
                i += if matches!(w, "--arg" | "--argjson") {
                    3
                } else {
                    2
                };
                continue;
            }
            i += 1;
            continue;
        }
        if w.starts_with('-') && w.len() > 1 {
            let cs: Vec<char> = w[1..].chars().collect();
            for k in 0..cs.len().saturating_sub(1) {
                let rest: String = cs[k + 1..].iter().collect();
                let rest = rest.strip_prefix('=').unwrap_or(&rest);
                if cs[k] == 'e' && is_script_cmd(cmd) {
                    push_script_path_tokens(cmd, rest, out);
                }
                if is_cmd_path_short_flag(cmd, cs[k]) || looks_like_path_word(rest) {
                    push_expanded_path(rest, out);
                }
            }
            if (is_script_cmd(cmd) || cmd == "jq") && cs.iter().any(|c| matches!(c, 'e' | 'f')) {
                need_script = false;
            }
            if matches!(cmd, "ag" | "ack") && cs.contains(&'g') {
                skip_first_positional = false;
            }
            let last = cs[cs.len() - 1];
            if last == 'e' && is_script_cmd(cmd) {
                if let Some(next) = args.get(i + 1) {
                    push_script_path_tokens(cmd, next, out);
                }
                i += 2;
                continue;
            }
            if is_cmd_path_short_flag(cmd, last) {
                if let Some(next) = args.get(i + 1) {
                    push_expanded_path(next, out);
                }
                i += 2;
                continue;
            }
            if is_cmd_nonpath_value_flag(cmd, w) {
                if let Some(next) = args.get(i + 1)
                    && looks_like_path_word(next)
                {
                    push_expanded_path(next, out);
                }
                i += 2;
                continue;
            }
            i += 1;
            continue;
        }
        if cmd == "test" || cmd == "[" {
            if matches!(w, "]" | "=" | "!=" | "-a" | "-o" | "!") {
                i += 1;
                continue;
            }
            if w != "-" {
                push_expanded_path(w, out);
            }
            i += 1;
            continue;
        }
        if cmd == "dd"
            && let Some((k, v)) = w.split_once('=')
            && !k.is_empty()
        {
            if matches!(k, "if" | "of") || looks_like_path_word(v) {
                push_expanded_path(v, out);
            }
            i += 1;
            continue;
        }
        if matches!(cmd, "curl" | "wget") && w.contains("://") {
            i += 1;
            continue;
        }
        if skip_first_positional && !has_reference {
            skip_first_positional = false;
            if !(matches!(cmd, "chmod" | "chown" | "chgrp") && looks_like_path_word(w)) {
                i += 1;
                continue;
            }
        }
        if need_script {
            need_script = false;
            push_script_path_tokens(cmd, w, out);
            i += 1;
            continue;
        }
        if w != "-" {
            push_expanded_path(w, out);
            // sed's operands after the script are input files only
            if is_script_cmd(cmd) && cmd != "sed" {
                push_script_path_tokens(cmd, w, out);
            }
            if let Some((k, v)) = w.split_once('=')
                && !k.is_empty()
                && looks_like_path_word(v)
            {
                push_expanded_path(v, out);
            }
        }
        i += 1;
    }
}

fn unit_path_words(unit: &Unit) -> HashSet<String> {
    let mut out = HashSet::new();
    for r in &unit.seg.redirects {
        if r.op != super::RedirectOp::Dup {
            push_expanded_path(&r.target, &mut out);
        }
    }
    if let Some(first) = unit.seg.argv.first()
        && first.contains('/')
    {
        push_expanded_path(first, &mut out);
    }
    for (_, v) in &unit.seg.assignments {
        if looks_like_path_word(v) {
            push_expanded_path(v, &mut out);
        }
    }
    if unit.eff != unit.seg.argv && unit.seg.argv.len() > unit.eff.len() {
        for w in &unit.seg.argv[1..unit.seg.argv.len() - unit.eff.len()] {
            if let Some((_, v)) = w.split_once('=') {
                if looks_like_path_word(v) {
                    push_expanded_path(v, &mut out);
                }
            } else if looks_like_path_word(w) {
                push_expanded_path(w, &mut out);
            }
        }
    }
    let Some(first) = unit.eff.first() else {
        return out;
    };
    let cmd = rule_check::fold_name(first);
    let args = &unit.eff[1..];
    match cmd.as_str() {
        "rg" | "grep" | "egrep" | "fgrep" | "zgrep" | "ggrep" => {
            if let Some(s) = search_roots(cmd == "rg", args) {
                for r in s.roots {
                    push_expanded_path(&r, &mut out);
                }
                collect_explicit_flag_paths(cmd.as_str(), args, &mut out);
            } else {
                collect_generic_operands(cmd.as_str(), args, &mut out);
            }
        }
        "find" => {
            let (roots, expr_idx, explicit) = super::shell::find_roots_explicit(&unit.eff);
            if explicit {
                for r in roots {
                    push_expanded_path(&r, &mut out);
                }
            }
            let mut i = expr_idx;
            while i < unit.eff.len() {
                let w = unit.eff[i].as_str();
                let lw = w.to_lowercase();
                if matches!(lw.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir") {
                    while i < unit.eff.len() && unit.eff[i] != ";" && unit.eff[i] != "+" {
                        i += 1;
                    }
                    i += 1;
                    continue;
                }
                if matches!(lw.as_str(), "-fprint" | "-fprint0" | "-fprintf" | "-fls") {
                    if let Some(next) = unit.eff.get(i + 1) {
                        push_expanded_path(next, &mut out);
                    }
                    i += 2;
                    continue;
                }
                if looks_like_path_word(w) {
                    push_expanded_path(w, &mut out);
                }
                i += 1;
            }
        }
        "echo" | "printf" | "true" | "false" | "pwd" | "id" | "whoami" | "uname" | "date"
        | "sleep" | "which" | "whereis" | "type" | "kill" | "pkill" | "pgrep" | "ps"
        | "hostname" | "printenv" | "export" | "unset" | "set" | "alias" | "unalias"
        | "history" | "jobs" | "fg" | "bg" | "wait" | "exit" | "return" | "break" | "continue"
        | "shift" | "read" | "env" => {
            for w in args {
                if let Some((_, v)) = w.split_once('=') {
                    if looks_like_path_word(v) {
                        push_expanded_path(v, &mut out);
                    }
                } else if looks_like_path_word(w) {
                    push_expanded_path(w, &mut out);
                }
            }
        }
        "git" | "cargo" | "npm" | "pnpm" | "yarn" | "bun" | "deno" | "uv" | "pip" | "pip3"
        | "poetry" | "rustup" | "go" | "docker" | "podman" | "kubectl" | "helm" | "terraform"
        | "tofu" | "gh" | "make" | "just" | "cmake" | "ctest" | "npx" | "pnpx" | "bunx" | "uvx"
        | "pipx" => {
            let mut subcommand: Option<&str> = None;
            let mut after_dd = false;
            let mut i = 0;
            while i < args.len() {
                let w = args[i].as_str();
                if after_dd {
                    if cmd == "git" || looks_like_path_word(w) {
                        push_expanded_path(w, &mut out);
                    }
                    i += 1;
                    continue;
                }
                if w == "--" {
                    after_dd = true;
                    i += 1;
                    continue;
                }
                if let Some((flag, val)) = w.split_once('=')
                    && flag.starts_with("--")
                {
                    if is_generic_path_long_flag(flag) || looks_like_path_word(val) {
                        push_expanded_path(val, &mut out);
                    }
                    i += 1;
                    continue;
                }
                if matches!(
                    w,
                    "-C" | "--chdir"
                        | "--git-dir"
                        | "--work-tree"
                        | "--exec-path"
                        | "--manifest-path"
                        | "--target-dir"
                        | "--cwd"
                        | "--prefix"
                        | "--dir"
                        | "--file"
                        | "-o"
                        | "--output"
                        | "--out-dir"
                ) || (w == "-f" && cmd != "git")
                    || (cmd == "git" && matches!(w, "-F" | "--template"))
                {
                    if let Some(next) = args.get(i + 1) {
                        push_expanded_path(next, &mut out);
                    }
                    i += 2;
                    continue;
                }
                if matches!(
                    w,
                    "-c" | "--config-env"
                        | "-p"
                        | "--package"
                        | "--bin"
                        | "--lib"
                        | "--test"
                        | "--bench"
                        | "--example"
                        | "--features"
                        | "--target"
                        | "--profile"
                        | "-j"
                        | "--jobs"
                        | "--color"
                        | "-m"
                        | "--message"
                        | "-b"
                        | "--branch"
                        | "--tag"
                        | "-n"
                        | "--namespace"
                        | "--context"
                        | "--filter"
                        | "--template"
                ) || (w == "-F" && cmd != "git")
                    || (w == "-t" && !matches!(cmd.as_str(), "git" | "docker" | "podman"))
                {
                    if let Some(next) = args.get(i + 1)
                        && looks_like_path_word(next)
                    {
                        push_expanded_path(next, &mut out);
                    }
                    i += 2;
                    continue;
                }
                if w.starts_with('-') {
                    collect_explicit_flag_paths(
                        cmd.as_str(),
                        std::slice::from_ref(&args[i]),
                        &mut out,
                    );
                    i += 1;
                    continue;
                }
                if subcommand.is_none() {
                    subcommand = Some(w);
                    i += 1;
                    continue;
                }
                let git_nonpath_sub = cmd == "git"
                    && subcommand.is_some_and(|s| {
                        matches!(
                            s,
                            "push"
                                | "fetch"
                                | "pull"
                                | "remote"
                                | "branch"
                                | "tag"
                                | "merge"
                                | "rebase"
                                | "switch"
                                | "cherry-pick"
                                | "revert"
                                | "config"
                                | "rev-parse"
                                | "symbolic-ref"
                                | "for-each-ref"
                                | "show-ref"
                                | "describe"
                                | "help"
                                | "version"
                        )
                    });
                if (cmd == "git" && !git_nonpath_sub) || looks_like_path_word(w) {
                    push_expanded_path(w, &mut out);
                }
                i += 1;
            }
        }
        _ if rule_check::is_shell_or_runs_here(&cmd)
            || cmd.starts_with("python")
            || cmd.starts_with("pwsh")
            || cmd.starts_with("powershell")
            || matches!(
                cmd.as_str(),
                "node" | "ruby" | "perl" | "php" | "lua" | "osascript"
            ) =>
        {
            let mut after_dd = false;
            let mut found_script = false;
            let mut i = 0;
            while i < args.len() {
                let w = args[i].as_str();
                if after_dd {
                    if w != "-" {
                        push_expanded_path(w, &mut out);
                    }
                    i += 1;
                    continue;
                }
                if w == "--" {
                    after_dd = true;
                    i += 1;
                    continue;
                }
                if !found_script && w.starts_with('-') {
                    let lw = w.to_lowercase();
                    if matches!(w, "-c" | "-e" | "-p" | "-r" | "-W" | "-X" | "-Q" | "-m")
                        || matches!(
                            lw.as_str(),
                            "--eval"
                                | "--print"
                                | "--run"
                                | "-command"
                                | "--command"
                                | "-encodedcommand"
                                | "-ec"
                                | "-cwa"
                                | "-commandwithargs"
                        )
                    {
                        i += 2;
                        continue;
                    }
                    if matches!(w, "-I" | "-File") || lw == "-file" {
                        if let Some(next) = args.get(i + 1) {
                            push_expanded_path(next, &mut out);
                        }
                        i += 2;
                        continue;
                    }
                    collect_explicit_flag_paths(
                        cmd.as_str(),
                        std::slice::from_ref(&args[i]),
                        &mut out,
                    );
                    i += 1;
                    continue;
                }
                if w != "-" {
                    found_script = true;
                    push_expanded_path(w, &mut out);
                }
                i += 1;
            }
        }
        _ => collect_generic_operands(cmd.as_str(), args, &mut out),
    }
    out
}

/// Every path the call names, ours and the gate's (without the working folder
/// or command words that aren't paths).
fn all_paths(input: &PolicyInput<'_>, a: &Analysis) -> Vec<PathBuf> {
    let unit_paths: Vec<HashSet<String>> = a.units.iter().map(unit_path_words).collect();
    let mut v: Vec<PathBuf> = vec![];
    for (idx, u) in a.uses.iter().enumerate() {
        let keep = u.unit == usize::MAX
            || unit_paths
                .get(u.unit)
                .is_some_and(|words| words.contains(&u.word));
        if keep {
            v.extend(a.all_paths(idx).into_iter().cloned());
        }
    }
    // A word the command reads as a path that the walk didn't resolve (a
    // file named inside a script, `w/out.txt` with no space): resolve it
    // lexically, or fail closed.
    for (unit, words) in unit_paths.iter().enumerate() {
        let cwds = a.unit_cwd.get(unit).map(|c| c.candidates()).unwrap_or_default();
        for w in words {
            if a.use_index.contains_key(&(unit, w.clone())) {
                continue;
            }
            match lexical_paths(w, input.home, &cwds) {
                Some(ps) => v.extend(ps),
                None => v.push(PathBuf::from(UNRESOLVED)),
            }
        }
    }
    if a.aligned.is_none() && a.uses.is_empty() {
        v.extend(input.resolved_paths.iter().cloned());
    }
    v
}

fn name_matches(rule: &Rule, input: &PolicyInput<'_>) -> bool {
    rule.tool_kind == effective_kind(input)
        && rule
            .tool_name
            .as_ref()
            .is_none_or(|n| *n == input.check.name)
}

fn scope_rank(r: &Rule, input: &PolicyInput<'_>) -> Option<u8> {
    match &r.scope {
        RuleScope::Thread { thread_id } => (thread_id == input.thread).then_some(0),
        RuleScope::Home => (input.preset == Preset::Home).then_some(1),
        RuleScope::Global => Some(2),
    }
}

/// `word` made absolute without the filesystem (`~`, `$HOME`, `$PWD`, the
/// command's cwd candidates; `.`/`..` folded). `None` when it holds anything
/// that only the shell or the command expands.
fn lexical_paths(word: &str, home: &Path, cwds: &[&PathBuf]) -> Option<Vec<PathBuf>> {
    if word == UNRESOLVED || word.is_empty() {
        return None;
    }
    let (bases, rest): (Vec<PathBuf>, &str) = if word == "~" {
        (vec![home.to_path_buf()], "")
    } else if let Some(r) = word.strip_prefix("~/") {
        (vec![home.to_path_buf()], r)
    } else if let Some(r) = word.strip_prefix("${HOME}").or_else(|| word.strip_prefix("$HOME")) {
        (vec![home.to_path_buf()], r)
    } else if let Some(r) = word.strip_prefix("${PWD}").or_else(|| word.strip_prefix("$PWD")) {
        (cwds.iter().map(|c| (*c).clone()).collect(), r)
    } else if word.starts_with('/') {
        (vec![PathBuf::from("/")], word)
    } else {
        (cwds.iter().map(|c| (*c).clone()).collect(), word)
    };
    if bases.is_empty()
        || rest.starts_with('~')
        || rest.contains(['*', '?', '[', '$', '`', '\\', '\0'])
    {
        return None;
    }
    Some(
        bases
            .into_iter()
            .map(|b| {
                let mut p = b;
                for seg in rest.split('/') {
                    match seg {
                        "" | "." => {}
                        ".." => {
                            p.pop();
                        }
                        s => p.push(s),
                    }
                }
                p
            })
            .collect(),
    )
}

fn path_scope_ok(rule: &Rule, paths: &[PathBuf]) -> bool {
    let unresolved = paths.iter().any(|p| p.as_os_str() == UNRESOLVED);
    match &rule.path_scope {
        None => true,
        Some(g) => match rule.decision {
            RuleDecision::Allow => {
                !unresolved && !paths.is_empty() && paths.iter().all(|p| glob_matches(g, p))
            }
            RuleDecision::Deny => unresolved || paths.iter().any(|p| glob_matches(g, p)),
        },
    }
}

fn prefix_of(argv: &[String], prefix: &[String]) -> bool {
    !prefix.is_empty() && argv.len() >= prefix.len() && argv.iter().zip(prefix).all(|(a, b)| a == b)
}

fn harmless(unit: &Unit) -> bool {
    unit.seg.redirects.iter().all(super::Redirect::harmless)
}

/// Top-level simple commands of a parseable line.
fn parts(a: &Analysis) -> Vec<usize> {
    a.units
        .iter()
        .enumerate()
        .filter(|(_, u)| !u.derived)
        .map(|(i, _)| i)
        .collect()
}

/// Whether a deny rule matches anywhere in the call (loosely: through
/// assignments, wrappers, substitutions).
fn deny_matches(rule: &Rule, input: &PolicyInput<'_>, a: &Analysis, paths: &[PathBuf]) -> bool {
    if !name_matches(rule, input) {
        return false;
    }
    match &rule.matcher {
        RuleMatcher::Any => path_scope_ok(rule, paths),
        RuleMatcher::CommandPrefix { argv } => {
            a.units
                .iter()
                .any(|u| prefix_of(&u.eff, argv) || prefix_of(&u.seg.argv, argv))
                && path_scope_ok(rule, paths)
        }
        RuleMatcher::CommandExact { argv } => {
            a.units
                .iter()
                .any(|u| u.eff == *argv || u.seg.argv == *argv)
                && path_scope_ok(rule, paths)
        }
        RuleMatcher::PathGlob { glob } => {
            paths.iter().any(|p| glob_matches(glob, p)) && path_scope_ok(rule, paths)
        }
    }
}

/// Verifies that every `-exec`/`-execdir`/`-ok`/`-okdir` clause of this `find`
/// unit had its derived units extracted by `shell.rs`.
fn find_exec_extracted_for_unit(a: &Analysis, unit_idx: usize, argv: &[String]) -> bool {
    if super::shell::base(&argv[0]) != "find" {
        return false;
    }
    let (_, expr_start) = super::shell::find_roots(argv);
    let is_exec_kw_ci = |w: &str| {
        matches!(
            w.to_lowercase().as_str(),
            "-exec" | "-execdir" | "-ok" | "-okdir"
        )
    };
    if argv[1..expr_start.min(argv.len())]
        .iter()
        .any(|w| is_exec_kw_ci(w))
    {
        return false;
    }
    let mut clause_count = 0usize;
    let mut k = expr_start;
    while k < argv.len() {
        let w = argv[k].as_str();
        if is_exec_kw_ci(w) {
            if !matches!(w, "-exec" | "-execdir" | "-ok" | "-okdir") {
                return false;
            }
            let mut m = k + 1;
            let mut body_len = 0usize;
            while m < argv.len() && argv[m] != ";" && argv[m] != "+" {
                body_len += 1;
                m += 1;
            }
            if body_len == 0 {
                return false;
            }
            clause_count += 1;
            k = m + 1;
            continue;
        }
        k += 1;
    }
    if clause_count == 0 {
        return false;
    }
    let owned_exec = a.units.get(unit_idx + 1..).map_or(0, |tail| {
        tail.iter()
            .take_while(|u| u.derived)
            .filter(|u| u.exec_roots.is_some())
            .count()
    });
    owned_exec >= clause_count
}

fn rule_covers_argv(
    a: &Analysis,
    unit_idx: usize,
    rule: &Rule,
    argv: &[String],
    for_wrapper_eff: bool,
) -> bool {
    if argv.is_empty() {
        return false;
    }
    if for_wrapper_eff
        && (rule_check::is_wrapper(&argv[0]) || rule_check::is_shell_or_runs_here(&argv[0]))
    {
        return false;
    }
    match &rule.matcher {
        RuleMatcher::CommandPrefix { argv: prefix } => {
            if !prefix_of(argv, prefix) || rule_check::command_prefix_blocked(argv, prefix) {
                return false;
            }
            if rule_check::is_find(&argv[0])
                && argv[1..].iter().any(|w| {
                    matches!(
                        w.to_lowercase().as_str(),
                        "-exec" | "-execdir" | "-ok" | "-okdir"
                    )
                })
                && !find_exec_extracted_for_unit(a, unit_idx, argv)
            {
                return false;
            }
            true
        }
        RuleMatcher::CommandExact { argv: exact } => {
            !exact.is_empty()
                && argv == exact.as_slice()
                && !rule_check::command_exact_blocked(argv)
        }
        RuleMatcher::Any | RuleMatcher::PathGlob { .. } => false,
    }
}

struct AllowCtx<'a> {
    input: &'a PolicyInput<'a>,
    a: &'a Analysis,
    paths: &'a [PathBuf],
    glob_leaves: bool,
}

/// Which slots in `unit_ids` (or the whole call when `unit_ids` is empty) an
/// allow rule covers. When `unit_ids` contains command units, `as_eff` selects
/// `u.eff` (for wrapper targets) instead of `u.seg.argv`.
fn allow_covers(ctx: &AllowCtx<'_>, rule: &Rule, unit_ids: &[usize], as_eff: bool) -> Vec<bool> {
    let n = unit_ids.len().max(1);
    let none = vec![false; n];
    if !name_matches(rule, ctx.input) || matches!(ctx.input.check.subject, ToolSubject::Unknown) {
        return none;
    }
    // D-100: an escaping search glob is covered only by an unscoped `Any` rule.
    if ctx.glob_leaves && !(matches!(rule.matcher, RuleMatcher::Any) && rule.path_scope.is_none()) {
        return none;
    }
    let command = matches!(ctx.input.check.subject, ToolSubject::Command { .. });
    // an uncertain cwd: only `Any` covers it (contract §9: the line asks)
    let simple =
        command && ctx.a.reasons.is_empty() && !parts(ctx.a).is_empty() && !ctx.a.cwd_uncertain;
    if rule.path_scope.is_some() && command && !simple {
        return none;
    }
    if !path_scope_ok(rule, ctx.paths) {
        return none;
    }
    match &rule.matcher {
        RuleMatcher::Any => vec![true; n],
        RuleMatcher::CommandPrefix { .. } | RuleMatcher::CommandExact { .. } => {
            if !simple || unit_ids.is_empty() {
                return none;
            }
            unit_ids
                .iter()
                .map(|&i| {
                    let u = &ctx.a.units[i];
                    let argv = if as_eff { &u.eff } else { &u.seg.argv };
                    u.seg.assignments.is_empty()
                        && harmless(u)
                        && rule_covers_argv(ctx.a, i, rule, argv, as_eff)
                })
                .collect()
        }
        RuleMatcher::PathGlob { glob } => {
            if command || !rule_check::is_path_kind(rule.tool_kind) {
                return none;
            }
            let ok = !ctx.paths.is_empty() && ctx.paths.iter().all(|p| glob_matches(glob, p));
            vec![ok; n]
        }
    }
}

struct RuleOutcome {
    deny: Option<(Option<String>, Option<RuleId>)>,
    covered: Vec<bool>,
    allow_rule: Option<RuleId>,
    any_allow: bool,
}

fn apply_rules(
    input: &PolicyInput<'_>,
    a: &Analysis,
    rules: &[Rule],
    glob_leaves: bool,
) -> RuleOutcome {
    let paths = all_paths(input, a);
    let command = matches!(input.check.subject, ToolSubject::Command { .. });
    let part_ids = if command { parts(a) } else { vec![] };
    let derived_ids: Vec<usize> = if command {
        a.units
            .iter()
            .enumerate()
            .filter(|(_, u)| u.derived)
            .map(|(i, _)| i)
            .collect()
    } else {
        vec![]
    };
    let wrapper_eff_ids: Vec<usize> = if command {
        a.units
            .iter()
            .enumerate()
            .filter(|(_, u)| {
                u.eff != u.seg.argv
                    || u.seg
                        .argv
                        .first()
                        .is_some_and(|f| rule_check::is_wrapper(f))
            })
            .map(|(i, _)| i)
            .collect()
    } else {
        vec![]
    };
    let allow_ctx = AllowCtx {
        input,
        a,
        paths: &paths,
        glob_leaves,
    };
    let n = part_ids.len().max(1);
    let mut covered = vec![false; n];
    let mut top_non_cmd = vec![false; n];
    let mut derived_covered = vec![false; derived_ids.len()];
    let mut eff_covered = vec![false; wrapper_eff_ids.len()];
    let mut allow_rule: Option<RuleId> = None;
    let mut any_allow = false;
    for rank in 0..3u8 {
        let ranked: Vec<&Rule> = rules
            .iter()
            .filter(|r| scope_rank(r, input) == Some(rank))
            .collect();
        let all_done = covered.iter().all(|c| *c)
            && derived_covered.iter().all(|c| *c)
            && eff_covered.iter().all(|c| *c);
        if !all_done
            && let Some(r) = ranked
                .iter()
                .find(|r| r.decision == RuleDecision::Deny && deny_matches(r, input, a, &paths))
        {
            return RuleOutcome {
                deny: Some((
                    Some(format!("denied by a rule ({:?})", r.matcher)),
                    Some(r.id.clone()),
                )),
                covered,
                allow_rule,
                any_allow,
            };
        }
        for r in ranked.iter().filter(|r| r.decision == RuleDecision::Allow) {
            let c = allow_covers(&allow_ctx, r, &part_ids, false);
            let is_any = matches!(r.matcher, RuleMatcher::Any)
                && r.path_scope.is_none()
                && !c.is_empty()
                && c.iter().all(|hit| *hit);
            let is_non_cmd = matches!(r.matcher, RuleMatcher::Any | RuleMatcher::PathGlob { .. });
            if is_any {
                any_allow = true;
            }
            for (k, (slot, hit)) in covered.iter_mut().zip(&c).enumerate() {
                if *hit && !*slot {
                    *slot = true;
                    if is_non_cmd {
                        top_non_cmd[k] = true;
                    }
                    allow_rule.get_or_insert_with(|| r.id.clone());
                }
            }
            if !derived_ids.is_empty() {
                let dc = allow_covers(&allow_ctx, r, &derived_ids, false);
                for (slot, hit) in derived_covered.iter_mut().zip(dc) {
                    if hit && !*slot {
                        *slot = true;
                    }
                }
            }
            if !wrapper_eff_ids.is_empty() {
                let ec = allow_covers(&allow_ctx, r, &wrapper_eff_ids, true);
                for (slot, hit) in eff_covered.iter_mut().zip(ec) {
                    if hit && !*slot {
                        *slot = true;
                    }
                }
            }
        }
    }
    if !derived_covered.iter().all(|c| *c) || !eff_covered.iter().all(|c| *c) {
        for (slot, &non_cmd) in covered.iter_mut().zip(&top_non_cmd) {
            if !non_cmd {
                *slot = false;
            }
        }
    }
    RuleOutcome {
        deny: None,
        covered,
        allow_rule,
        any_allow,
    }
}

// ---------------------------------------------------------------------------
// Home's read-only commands
// ---------------------------------------------------------------------------

fn forbidden_flag(entry: &ReadOnlyCommand, args: &[String]) -> bool {
    let shorts: Vec<char> = entry
        .forbidden
        .iter()
        .filter(|f| f.len() == 2 && !f.starts_with("--"))
        .filter_map(|f| f.chars().nth(1))
        .collect();
    let longs: Vec<&str> = entry
        .forbidden
        .iter()
        .copied()
        .filter(|f| f.starts_with("--"))
        .collect();
    for a in args {
        if a == "--" {
            break;
        }
        if let Some(rest) = a.strip_prefix("--") {
            let name = format!("--{}", rest.split('=').next().unwrap_or(rest));
            if longs
                .iter()
                .any(|l| *l == name || (name.len() >= 3 && l.starts_with(&name)))
            {
                return true;
            }
        } else if a.starts_with('-') && a.len() > 1 && a[1..].chars().any(|c| shorts.contains(&c)) {
            return true;
        }
    }
    false
}

pub(crate) const RG_BOOL_S: &str = "iSsnNHIlcovwxuaUFPq0pb";
pub(crate) const RG_VAL_S: &str = "egtTmABCdjMEr";
pub(crate) const RG_BOOL_L: &[&str] = &[
    "hidden",
    "no-ignore",
    "no-ignore-vcs",
    "no-ignore-dot",
    "no-ignore-parent",
    "no-ignore-global",
    "no-ignore-files",
    "no-messages",
    "no-heading",
    "heading",
    "line-number",
    "no-line-number",
    "with-filename",
    "no-filename",
    "ignore-case",
    "smart-case",
    "case-sensitive",
    "count",
    "count-matches",
    "files-with-matches",
    "files-without-match",
    "only-matching",
    "invert-match",
    "word-regexp",
    "line-regexp",
    "fixed-strings",
    "multiline",
    "multiline-dotall",
    "text",
    "binary",
    "unrestricted",
    "files",
    "type-list",
    "json",
    "column",
    "no-column",
    "vimgrep",
    "trim",
    "pretty",
    "quiet",
    "stats",
    "null",
    "no-config",
    "version",
    "help",
    "pcre2",
    "no-pcre2",
    "crlf",
    "include-zero",
    "byte-offset",
    "glob-case-insensitive",
    "no-glob-case-insensitive",
];
pub(crate) const RG_VAL_L: &[&str] = &[
    "regexp",
    "glob",
    "iglob",
    "type",
    "type-not",
    "max-count",
    "after-context",
    "before-context",
    "context",
    "max-depth",
    "maxdepth",
    "threads",
    "max-columns",
    "encoding",
    "replace",
    "colors",
    "color",
    "sort",
    "sortr",
    "max-filesize",
    "path-separator",
    "engine",
    "context-separator",
    "type-add",
    "type-clear",
    "field-context-separator",
    "field-match-separator",
    "dfa-size-limit",
    "regex-size-limit",
];
pub(crate) const GREP_BOOL_S: &str = "ivnclLoqswxhHEFGPaIrRzZbTUy0123456789";
pub(crate) const GREP_VAL_S: &str = "emABCdD";
pub(crate) const GREP_BOOL_L: &[&str] = &[
    "ignore-case",
    "no-ignore-case",
    "invert-match",
    "line-number",
    "count",
    "files-with-matches",
    "files-without-match",
    "only-matching",
    "quiet",
    "silent",
    "no-messages",
    "word-regexp",
    "line-regexp",
    "no-filename",
    "with-filename",
    "extended-regexp",
    "fixed-strings",
    "basic-regexp",
    "perl-regexp",
    "text",
    "null-data",
    "null",
    "byte-offset",
    "initial-tab",
    "recursive",
    "dereference-recursive",
    "no-group-separator",
];
pub(crate) const GREP_VAL_L: &[&str] = &[
    "regexp",
    "max-count",
    "after-context",
    "before-context",
    "context",
    "directories",
    "devices",
    "include",
    "exclude",
    "exclude-dir",
    "label",
    "color",
    "colour",
    "binary-files",
    "group-separator",
];

/// GNU `grep`'s long option names (both tables), for reading a prefix.
pub(crate) fn grep_long_names() -> impl Iterator<Item = &'static str> {
    GREP_BOOL_L.iter().chain(GREP_VAL_L).copied()
}

pub(crate) struct Searched {
    pub(crate) roots: Vec<String>,
    pub(crate) recursive: bool,
}

/// Splits an `rg`/`grep` command into search roots; `None` for a flag it
/// doesn't know (the command then asks).
pub(crate) fn search_roots(is_rg: bool, args: &[String]) -> Option<Searched> {
    roots_of(is_rg, false, args)
}

/// [`search_roots`] for `rg` that also reads `-L` / `--follow` as plain
/// flags (round-8 major 5, D-098): the gate checks the links they follow
/// itself, and the flag/value pairing stays the parser's, so a `-L` given
/// as the value of `-e`, `-g`, `-r`, `--file`, … stays a value.
pub(crate) fn rg_roots_following(args: &[String]) -> Option<Searched> {
    roots_of(true, true, args)
}

fn roots_of(is_rg: bool, follow: bool, args: &[String]) -> Option<Searched> {
    let (bs, vs, bl, vl) = if is_rg {
        (RG_BOOL_S, RG_VAL_S, RG_BOOL_L, RG_VAL_L)
    } else {
        (GREP_BOOL_S, GREP_VAL_S, GREP_BOOL_L, GREP_VAL_L)
    };
    let mut positional: Vec<String> = vec![];
    let mut have_pattern_flag = false;
    let mut files_mode = false;
    let mut recursive = is_rg;
    let mut i = 0;
    let mut dd = false;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if dd || !a.starts_with('-') || a == "-" {
            if a == "-" {
                return None;
            }
            positional.push(a.clone());
            continue;
        }
        if a == "--" {
            dd = true;
        } else if let Some(rest) = a.strip_prefix("--") {
            let (name, has_val) = match rest.split_once('=') {
                Some((n, _)) => (n, true),
                None => (rest, false),
            };
            if follow && name == "follow" && !has_val {
                continue;
            }
            if name == "files" || name == "type-list" {
                files_mode = true;
            }
            if !is_rg && name.len() >= 3 && "dereference-recursive".starts_with(name) {
                return None; // follows every symlink below the root
            }
            if name == "regexp" {
                have_pattern_flag = true;
            }
            if matches!(name, "recursive" | "dereference-recursive" | "directories")
                || (!is_rg
                    && name.len() >= 3
                    && ("recursive".starts_with(name)
                        || "dereference-recursive".starts_with(name)
                        || "directories".starts_with(name))
                    && !bl.contains(&name)
                    && !vl.contains(&name))
            {
                recursive = true;
            }
            if vl.contains(&name) {
                if !has_val {
                    i += 1;
                }
            } else if !bl.contains(&name) {
                return None;
            }
        } else {
            let cs: Vec<char> = a[1..].chars().collect();
            for (k, c) in cs.iter().enumerate() {
                if !is_rg && *c == 'R' {
                    return None; // follows every symlink below the root
                }
                if !is_rg && matches!(c, 'r' | 'd') {
                    recursive = true;
                }
                if follow && *c == 'L' {
                    continue;
                }
                if *c == 'e' {
                    have_pattern_flag = true;
                }
                if vs.contains(*c) {
                    if k + 1 == cs.len() {
                        i += 1;
                    }
                    break;
                } else if !bs.contains(*c) {
                    return None;
                }
            }
        }
    }
    let skip = usize::from(!have_pattern_flag && !files_mode);
    let roots = positional.into_iter().skip(skip).collect();
    Some(Searched { roots, recursive })
}

/// Credential names a committed file can have (CR-12: `git show
/// REV:PATH` of a path whose name matches a credential asks): the
/// hard-deny list's files and the stores that ask, by their path below home
/// or, for names that are distinctive, by file name alone; private keys
/// (`id_*` but `.pub`); anything below a credential folder.
const GIT_CRED_SUFFIXES: &[&str] = &[
    ".aws/credentials",
    ".docker/config.json",
    ".kube/config",
    ".config/gh/hosts.yml",
    ".claude/.credentials.json",
    ".codex/auth.json",
    ".cargo/credentials",
    ".cargo/credentials.toml",
    ".config/git/credentials",
    ".azure/accesstokens.json",
    ".gem/credentials",
    ".terraform.d/credentials.tfrc.json",
    ".config/op/config",
];
const GIT_CRED_NAMES: &[&str] = &[
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".claude.json",
    ".pypirc",
    ".pgpass",
    ".vault-token",
    ".env",
    ".yarnrc.yml",
    "credentials.tfrc.json",
    ".bash_history",
    ".zsh_history",
    ".zhistory",
    ".sh_history",
    ".ksh_history",
    ".history",
    "fish_history",
    "accesstokens.json",
];
const GIT_CRED_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".config/gcloud",
    ".gemini",
    ".aws/sso/cache",
    ".zsh_sessions",
    ".bash_sessions",
    "library/keychains",
];

fn is_git_key_name(name: &str) -> bool {
    const CODE_EXTS: &[&str] = &[
        "pub", "rs", "ts", "tsx", "js", "jsx", "py", "go", "c", "cc", "cpp", "h", "hpp", "java",
        "rb", "md", "html", "css", "svg", "png",
    ];
    ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"]
        .iter()
        .any(|k| {
            name == *k
                || name.strip_prefix(k).is_some_and(|r| {
                    r.starts_with(['.', '-', '_'])
                        && !r
                            .rsplit('.')
                            .next()
                            .is_some_and(|ext| CODE_EXTS.contains(&ext))
                })
        })
}

/// Whether a `git show`/`git log`/`git diff` argument names a committed file
/// whose name matches a credential: `REV:PATH`, `:PATH`, `:N:PATH` or a
/// pathspec (including `:(magic)PATH` and short magic `:/`, `:!`, `:^`).
/// Extracts `<file>` from `git log -L` / `--line-range` (`start,end:file` or `:funcname:file`).
fn parse_git_line_range(spec: &str) -> Option<&str> {
    if let Some(after) = spec.strip_prefix(':') {
        let (func, file) = after.rsplit_once(':')?;
        (!func.is_empty() && !file.is_empty()).then_some(file)
    } else {
        let (range, file) = spec.rsplit_once(':')?;
        (!range.is_empty() && !file.is_empty()).then_some(file)
    }
}

/// Whether `git show`/`git log`/`git diff` arguments name a committed file
/// whose name matches a credential.
fn git_args_name_credential(args: &[String]) -> bool {
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--" {
            return args[i + 1..].iter().any(|a| git_names_credential(a));
        }
        let line_spec = if arg == "-L" || arg == "--line-range" {
            i += 1;
            match args.get(i) {
                Some(s) => Some(s.as_str()),
                None => return true,
            }
        } else if let Some(s) = arg.strip_prefix("-L") {
            Some(s)
        } else {
            arg.strip_prefix("--line-range=")
        };
        if let Some(spec) = line_spec {
            let Some(file) = parse_git_line_range(spec) else {
                return true;
            };
            if git_path_is_credential(file) {
                return true;
            }
        } else if git_names_credential(arg) {
            return true;
        }
        i += 1;
    }
    false
}

fn git_names_credential(arg: &str) -> bool {
    if arg.starts_with('-') {
        return false;
    }
    let path = if let Some(after) = arg.strip_prefix(':') {
        if let Some((n, p)) = after.split_once(':')
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
        {
            p
        } else {
            let mut s = after;
            loop {
                if let Some(rest) = s.strip_prefix('(') {
                    let mut depth = 1usize;
                    let mut end = None;
                    for (idx, ch) in rest.char_indices() {
                        match ch {
                            '(' => depth += 1,
                            ')' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = Some(idx);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    let Some(end_idx) = end else {
                        // Unclosed `:(...` pathspec magic: fail closed.
                        return true;
                    };
                    s = &rest[end_idx + 1..];
                } else if let Some(rest) = s.strip_prefix(['/', '!', '^', ':']) {
                    s = rest;
                } else {
                    break;
                }
            }
            if s.starts_with(['~', '@', '#', '+', '=', '%', '&', '$', '\\']) || s.contains(':') {
                // Unrecognized `:` magic or unparsed prefix: fail closed.
                return true;
            }
            s
        }
    } else if let Some((_, rest)) = arg.split_once(':') {
        rest
    } else {
        arg
    };
    git_path_is_credential(path)
}

/// One token of a Git pathspec read as a glob: `*` any run (Git's default
/// pathspec `*` crosses `/`), `?` and a `[...]` class any one character
/// (a superset: fails closed), anything else literal.
#[derive(Clone, Copy, PartialEq)]
enum GlobTok {
    Star,
    One,
    Lit(char),
}

fn glob_toks(p: &str) -> Vec<GlobTok> {
    let mut out = vec![];
    let mut it = p.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '*' => {
                if out.last() != Some(&GlobTok::Star) {
                    out.push(GlobTok::Star);
                }
            }
            '?' => out.push(GlobTok::One),
            '[' => {
                // skip to the closing `]` (or the end); one character
                for d in it.by_ref() {
                    if d == ']' {
                        break;
                    }
                }
                out.push(GlobTok::One);
            }
            '\\' => out.push(GlobTok::Lit(it.next().unwrap_or('\\'))),
            c => out.push(GlobTok::Lit(c)),
        }
    }
    out
}

/// Whether some string matches both glob token lists.
fn globs_intersect(a: &[GlobTok], b: &[GlobTok]) -> bool {
    let (n, m) = (a.len(), b.len());
    // memo[i][j]: 0 unknown, 1 false, 2 true
    let mut memo = vec![0u8; (n + 1) * (m + 1)];
    fn go(a: &[GlobTok], b: &[GlobTok], i: usize, j: usize, memo: &mut [u8]) -> bool {
        let m = b.len();
        let k = i * (m + 1) + j;
        if memo[k] != 0 {
            return memo[k] == 2;
        }
        let r = if i == a.len() && j == m {
            true
        } else if i < a.len() && a[i] == GlobTok::Star {
            go(a, b, i + 1, j, memo) || (j < m && go(a, b, i, j + 1, memo))
        } else if j < m && b[j] == GlobTok::Star {
            go(a, b, i, j + 1, memo) || (i < a.len() && go(a, b, i + 1, j, memo))
        } else if i < a.len() && j < m {
            let ok = match (a[i], b[j]) {
                (GlobTok::Lit(x), GlobTok::Lit(y)) => x == y,
                _ => true,
            };
            ok && go(a, b, i + 1, j + 1, memo)
        } else {
            false
        };
        memo[k] = if r { 2 } else { 1 };
        r
    }
    go(a, b, 0, 0, &mut memo)
}

/// A pathspec with glob characters: whether it can match a committed
/// credential (any name, suffix or file below a credential folder, at any
/// depth). Fails closed: classes match any character, `*` crosses `/`.
fn git_glob_reaches_credential(pattern: &str) -> bool {
    let l = pattern.to_lowercase().replace('ſ', "s");
    let mut parts: Vec<&str> = vec![];
    for c in l.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(c),
        }
    }
    let p = glob_toks(&parts.join("/"));
    let hits = |target: &str| {
        globs_intersect(&p, &glob_toks(target))
            || globs_intersect(&p, &glob_toks(&format!("*/{target}")))
    };
    // A pattern ending in a literal source-code extension (`*.rs`) reaches
    // only such files: like `is_git_key_name`, those aren't keys, and a file
    // below a credential folder counts only when the pattern names the folder.
    let joined = parts.join("/");
    let code_ext = joined
        .rsplit_once('.')
        .is_some_and(|(_, ext)| !ext.contains(['*', '?', '[', '/', '\\']) && GIT_CODE_EXTS.contains(&ext));
    GIT_CRED_NAMES.iter().any(|n| hits(n))
        || GIT_CRED_SUFFIXES.iter().any(|s| hits(s))
        || GIT_CRED_DIRS
            .iter()
            .any(|d| (!code_ext || joined.contains(d)) && hits(&format!("{d}/?*")))
        || (!code_ext
            && ["id_rsa", "id_dsa", "id_ecdsa", "id_ed25519", "msal_token_cache."]
                .iter()
                .any(|k| hits(&format!("{k}*"))))
}

const GIT_CODE_EXTS: &[&str] = &[
    "pub", "rs", "ts", "tsx", "js", "jsx", "py", "go", "c", "cc", "cpp", "h", "hpp", "java", "rb",
    "md", "html", "css", "svg", "png",
];

fn git_path_is_credential(path: &str) -> bool {
    if path.contains(['*', '?', '[']) && git_glob_reaches_credential(path) {
        return true;
    }
    let l = path.to_lowercase().replace('ſ', "s");
    let mut parts: Vec<&str> = vec![];
    for c in l.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(c),
        }
    }
    let Some(name) = parts.last() else {
        return false;
    };
    let joined = parts.join("/");
    if GIT_CRED_NAMES.contains(name)
        || name.starts_with("msal_token_cache.")
        || is_git_key_name(name)
    {
        return true;
    }
    if GIT_CRED_SUFFIXES
        .iter()
        .any(|s| joined == *s || joined.ends_with(&format!("/{s}")))
    {
        return true;
    }
    // below a credential folder (`.ssh/x`, `a/.gnupg/y`), but not public
    // ssh files
    GIT_CRED_DIRS.iter().any(|d| {
        let at = |i: usize| parts[i..].join("/");
        (0..parts.len()).any(|i| {
            let rest = at(i);
            rest.strip_prefix(d)
                .and_then(|r| r.strip_prefix('/'))
                .is_some_and(|r| {
                    !r.is_empty()
                        && !(*d == ".ssh"
                            && (name.ends_with(".pub")
                                || name.starts_with("known_hosts")
                                || matches!(*name, "config" | "authorized_keys")))
                })
        })
    })
}

struct Home<'a> {
    input: &'a PolicyInput<'a>,
    a: &'a Analysis,
    s: Sensitive,
}

impl Home<'_> {
    /// Strictly inside home, and not a place a search could reach credentials
    /// from.
    fn root_ok(&self, p: &Path) -> bool {
        let h = self.input.home;
        p.starts_with(h) && p != h && !self.s.is_or_contains(p)
    }

    fn word_paths(&self, unit: usize, word: &str) -> Vec<PathBuf> {
        let mut out = vec![];
        if let Some(idxs) = self.a.use_index.get(&(unit, word.to_string())) {
            for &idx in idxs {
                out.extend(self.a.all_paths(idx).into_iter().cloned());
            }
        }
        out
    }

    /// Whether a path the call names is a credential store that asks
    /// (CR-12): ours, the gate's (symlinks resolved, hard links mapped onto
    /// the store), and, when the gate's list didn't line up, its flat list.
    fn names_ask_store(&self, unit: Option<usize>) -> bool {
        let hit = self.a.uses.iter().enumerate().any(|(idx, u)| {
            unit.is_none_or(|i| u.unit == i)
                && self.a.all_paths(idx).iter().any(|p| self.s.ask_hit(p))
        });
        hit || (self.a.aligned.is_none()
            && self.input.resolved_paths.iter().any(|p| self.s.ask_hit(p)))
    }

    fn readonly(&self, i: usize) -> bool {
        let u = &self.a.units[i];
        if !u.seg.assignments.is_empty() || !harmless(u) || u.seg.argv.is_empty() {
            return false;
        }
        if self.names_ask_store(Some(i)) {
            return false;
        }
        let argv = &u.seg.argv;
        let Some(entry) = HOME_READONLY_COMMANDS.iter().find(|e| {
            argv.len() >= e.prefix.len() && e.prefix.iter().zip(argv).all(|(p, a)| p == a)
        }) else {
            return false;
        };
        let args = &argv[entry.prefix.len()..];
        if forbidden_flag(entry, args) {
            return false;
        }
        if let Some(only) = entry.only
            && !args.iter().all(|a| only.contains(&a.as_str()))
        {
            return false;
        }
        if args
            .iter()
            .any(|a| a.starts_with("--exec-path") || a.starts_with("--config-env"))
        {
            return false;
        }
        // `git show HEAD:.npmrc`: a committed file named like a credential
        // (CR-12)
        if entry.prefix.first() == Some(&"git") && git_args_name_credential(args) {
            return false;
        }
        let is_rg = entry.prefix == ["rg"];
        match entry.recursion {
            Recursion::Never => true,
            Recursion::Always | Recursion::WithFlags(_) => {
                let Some(s) = search_roots(is_rg, args) else {
                    return false;
                };
                if !s.recursive {
                    return true;
                }
                if s.roots.is_empty() {
                    return false;
                }
                s.roots.iter().all(|r| {
                    let ps = self.word_paths(i, r);
                    !ps.is_empty() && ps.iter().all(|p| self.root_ok(p))
                })
            }
        }
    }

    fn evaluate(&self, covered: &[bool], allow_rule: Option<RuleId>) -> PolicyVerdict {
        let input = self.input;
        match &input.check.subject {
            ToolSubject::None => PolicyVerdict::Allow { rule: None },
            ToolSubject::Paths {
                paths,
                recursive,
                glob,
            } => match input.check.kind {
                // a credential store that asks (CR-12)
                ToolKind::Read | ToolKind::Search if self.names_ask_store(None) => {
                    PolicyVerdict::Ask
                }
                ToolKind::Search
                    if glob.as_ref().is_some_and(|g| {
                        let roots: Vec<PathBuf> = if paths.is_empty() {
                            vec![input.cwd.to_path_buf()]
                        } else {
                            (0..self.a.uses.len())
                                .flat_map(|i| self.a.all_paths(i).into_iter().cloned())
                                .collect()
                        };
                        self.s
                            .ask_search_glob_hit(&roots, g, self.a.glob_search.as_ref())
                    }) =>
                {
                    PolicyVerdict::Ask
                }
                ToolKind::Read => PolicyVerdict::Allow { rule: None },
                ToolKind::Search => {
                    if *recursive {
                        let roots: Vec<PathBuf> = if paths.is_empty() {
                            vec![input.cwd.to_path_buf()]
                        } else {
                            (0..self.a.uses.len())
                                .flat_map(|i| self.a.all_paths(i).into_iter().cloned())
                                .collect()
                        };
                        if !roots.iter().all(|r| self.root_ok(r)) {
                            return PolicyVerdict::Ask;
                        }
                    }
                    PolicyVerdict::Allow { rule: None }
                }
                _ => PolicyVerdict::Ask,
            },
            ToolSubject::Command { line, .. } => {
                let tops = parts(self.a);
                if tops.is_empty()
                    || !self.a.reasons.is_empty()
                    || self.a.cwd_uncertain
                    || line.contains('\n')
                {
                    return PolicyVerdict::Ask;
                }
                let mut used_rule = None;
                for (k, &i) in tops.iter().enumerate() {
                    if covered.get(k).copied().unwrap_or(false) {
                        used_rule = used_rule.or_else(|| allow_rule.clone());
                    } else if !self.readonly(i) {
                        return PolicyVerdict::Ask;
                    }
                }
                PolicyVerdict::Allow { rule: used_rule }
            }
            _ => PolicyVerdict::Ask,
        }
    }
}

impl PolicyEngine for DefaultPolicy {
    fn evaluate(&self, input: &PolicyInput<'_>, rules: &[Rule]) -> PolicyVerdict {
        let a = deny::analyze(input);
        if a.overflow {
            return PolicyVerdict::Deny {
                reason: "too large to check".into(),
                rule: None,
            };
        }
        if let Some(id) = deny::hard_deny(input, &a) {
            return PolicyVerdict::HardDeny { id };
        }
        let esc = a.glob_escape.as_ref();
        let glob_leaves = esc.is_some_and(deny::GlobEscape::leaves);
        let glob_unbounded =
            esc.is_some_and(|e| e.unbounded) || a.glob_search.as_ref().is_some_and(|(_, u)| *u);
        let out = apply_rules(input, &a, rules, glob_leaves);
        if let Some((reason, rule)) = out.deny {
            return PolicyVerdict::Deny {
                reason: reason.unwrap_or_default(),
                rule,
            };
        }
        // A search glob that can name files outside every path the subject
        // gives (M1-18, P1; D-100): hard-denied (above) if an expansion hits
        // the hard-deny list, otherwise allowed by FullAccess or an Any rule
        // and asked in Home/Normal/Trusted/Careful and under prefix/scoped rules.
        // When glob expansion overflows (brace or variant cap), Ask everywhere.
        let unknown = matches!(input.check.subject, ToolSubject::Unknown)
            || a.unbounded_unknown_dangerous
            || glob_unbounded;
        let command = matches!(input.check.subject, ToolSubject::Command { .. });
        if !unknown
            && (!glob_leaves || out.any_allow)
            && out.covered.iter().all(|c| *c)
            && (command && !parts(&a).is_empty() || !command)
        {
            return PolicyVerdict::Allow {
                rule: out.allow_rule,
            };
        }
        if unknown {
            return PolicyVerdict::Ask;
        }
        if glob_leaves {
            return if input.preset == Preset::FullAccess {
                PolicyVerdict::Allow { rule: None }
            } else {
                PolicyVerdict::Ask
            };
        }
        match input.preset {
            Preset::Home | Preset::Normal | Preset::Trusted => {
                let h = Home {
                    input,
                    a: &a,
                    s: deny::sensitive(input),
                };
                h.evaluate(&out.covered, out.allow_rule)
            }
            Preset::Careful => {
                if matches!(input.check.subject, ToolSubject::None) {
                    PolicyVerdict::Allow { rule: None }
                } else {
                    PolicyVerdict::Ask
                }
            }
            Preset::FullAccess => PolicyVerdict::Allow { rule: None },
        }
    }
}
