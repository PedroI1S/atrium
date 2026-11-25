use atrium_protocol::policy::HOME_READONLY_COMMANDS;

use crate::policy::model::{
    AccessMode, ByteProvenance, CodeExecution, CommandUnit, ComputeBudget, DerivedDestinations,
    ExhaustReason, FilterDialect, FoldRules, HitMatch, HitSource, LinkPolicy, ModelRedirect,
    OperandRole, PartOutcome, RemovedWrapper, Separator, StdFd, ToolEffects, UnitIndex,
    UnreadReason, ValueRole, Word, WrapperKind,
};
use crate::policy::tools::ALLOW_HOME_FOLLOWED_SEARCH;

use super::{check_git_selection, classify_unit};

fn w(s: &str) -> Word {
    Word::from_parts(
        s.as_bytes().to_vec(),
        vec![ByteProvenance::Unquoted; s.len()],
    )
    .expect("equal len")
}

fn wq(s: &str) -> Word {
    Word::from_parts(
        s.as_bytes().to_vec(),
        vec![ByteProvenance::SingleQuoted; s.len()],
    )
    .expect("equal len")
}

fn make_unit(words: Vec<Word>) -> CommandUnit {
    CommandUnit {
        index: UnitIndex(0),
        argv: words,
        removed_wrappers: Vec::new(),
        redirects: Vec::new(),
        separator_before: Separator::Start,
        pipeline_id: None,
        depth: 0,
        parent_unit: None,
    }
}

fn unit_strs(args: &[&str]) -> CommandUnit {
    make_unit(args.iter().map(|s| w(s)).collect())
}

fn classify(args: &[&str]) -> ToolEffects {
    let u = unit_strs(args);
    let mut budget = ComputeBudget::with_default_limits();
    classify_unit(&u, &FoldRules::REV4, &mut budget).expect("default budget")
}

fn classify_words(words: Vec<Word>) -> ToolEffects {
    let u = make_unit(words);
    let mut budget = ComputeBudget::with_default_limits();
    classify_unit(&u, &FoldRules::REV4, &mut budget).expect("default budget")
}

fn git_sel(words: Vec<Word>) -> (ToolEffects, Vec<crate::policy::model::PartResult>) {
    let u = make_unit(words);
    let mut budget = ComputeBudget::with_default_limits();
    let eff = classify_unit(&u, &FoldRules::REV4, &mut budget).expect("classify");
    let out = check_git_selection(&u, &eff, &FoldRules::REV4, &mut budget);
    assert!(
        out.exhausted.is_none(),
        "unexpected exhaustion: {:?}",
        out.exhausted
    );
    (eff, out.results)
}

// ---------------------------------------------------------------------------
// 1. D-141 Kind 1: HomeReadOnly (§6, §10 row 1, §11)
// ---------------------------------------------------------------------------

#[test]
fn home_readonly_accepted_as_written_and_case_folding() {
    let eff = classify(&["cat", "README.md"]);
    assert_eq!(eff.tool_name, Some("cat"));
    assert!(eff.home_readonly_eligible);
    assert!(!eff.writes);
    assert!(eff.unread.is_empty());
    assert_eq!(eff.roles, vec![OperandRole::Command, OperandRole::Path]);
    assert_eq!(
        eff.operand_access,
        vec![AccessMode::Read, AccessMode::ReadWrite]
    );

    // §10 row 1 & §11 "Unchanged on purpose": Home eligibility matches command word as written.
    for non_exact in [&["/bin/cat", "README.md"][..], &["CAT", "README.md"][..]] {
        let e = classify(non_exact);
        assert_eq!(e.tool_name, Some("cat"));
        assert!(!e.home_readonly_eligible, "{non_exact:?}");
        assert!(!e.writes, "{non_exact:?}");
        assert!(e.unread.is_empty(), "{non_exact:?}");
    }

    // Unlisted flag on HomeReadOnly is allowed (AnyExceptForbidden) without Unread.
    let eff_flag = classify(&["cat", "--number", "README.md"]);
    assert!(eff_flag.home_readonly_eligible);
    assert!(eff_flag.unread.is_empty());
}

#[test]
fn home_readonly_guards_redirects_globs_and_wrappers() {
    // Unquoted glob removes Home eligibility.
    let eff_glob = classify(&["cat", "*.rs"]);
    assert!(!eff_glob.home_readonly_eligible);
    assert!(eff_glob.unread.is_empty());

    // Quoted glob is literal, keeps Home eligibility.
    let eff_qglob = classify_words(vec![w("cat"), wq("*.rs")]);
    assert!(eff_qglob.home_readonly_eligible);

    // Harmless redirects (2>&1, 2>/dev/null) keep Home eligibility and !writes.
    let mut u_harmless = unit_strs(&["cat", "README.md"]);
    u_harmless.redirects = vec![
        ModelRedirect::Dup {
            from: StdFd::Stderr,
            to: StdFd::Stdout,
        },
        ModelRedirect::ErrWrite {
            target: w("/dev/null"),
        },
    ];
    let mut budget = ComputeBudget::with_default_limits();
    let e_harmless = classify_unit(&u_harmless, &FoldRules::REV4, &mut budget).unwrap();
    assert!(e_harmless.home_readonly_eligible);
    assert!(!e_harmless.writes);

    // Output redirect outside /dev/null removes Home eligibility and sets writes = true.
    let mut u_write = unit_strs(&["cat", "README.md"]);
    u_write.redirects = vec![ModelRedirect::Write {
        target: w("out.txt"),
    }];
    let e_write = classify_unit(&u_write, &FoldRules::REV4, &mut budget).unwrap();
    assert!(!e_write.home_readonly_eligible);
    assert!(e_write.writes);
    assert!(e_write.unread.is_empty());

    // Input redirect removes Home eligibility without setting writes.
    let mut u_read = unit_strs(&["cat"]);
    u_read.redirects = vec![ModelRedirect::Read {
        target: w("in.txt"),
    }];
    let e_read = classify_unit(&u_read, &FoldRules::REV4, &mut budget).unwrap();
    assert!(!e_read.home_readonly_eligible);
    assert!(!e_read.writes);

    // Stripped wrapper removes Home eligibility.
    let mut u_wrap = unit_strs(&["cat", "README.md"]);
    u_wrap.removed_wrappers = vec![RemovedWrapper {
        kind: WrapperKind::Command,
        words: vec![w("command")],
    }];
    let e_wrap = classify_unit(&u_wrap, &FoldRules::REV4, &mut budget).unwrap();
    assert!(!e_wrap.home_readonly_eligible);
    assert!(!e_wrap.writes);
    assert!(e_wrap.unread.is_empty());
}

#[test]
fn home_readonly_forbidden_flags_and_only_never_emit_unread() {
    // Forbidden flags remove Home eligibility ONLY (never Unread).
    for cmd in [
        &["tree", "-o", "out.txt"][..],
        &["tree", "--output=out.txt"][..],
        &["tree", "--out=out.txt"][..], // 3+ char long abbrev matching `forbidden_flag`
        &["tree", "-aR"][..],           // short cluster
        &["date", "-s", "now"][..],
        &["date", "--file=/tmp/x"][..],
        &["file", "-Cf", "list"][..],
        &["git", "log", "--output=out.txt"][..],
        &["git", "branch", "new-branch"][..], // violates `only`
    ] {
        let e = classify(cmd);
        assert!(!e.home_readonly_eligible, "{cmd:?}");
        assert!(e.writes, "{cmd:?}");
        assert!(e.unread.is_empty(), "must not emit Unread for {cmd:?}");
    }

    // Near miss: forbidden-looking token after `--` is a positional operand, not a flag.
    let after_dd = classify(&["tree", "--", "-o"]);
    assert!(after_dd.home_readonly_eligible);
    assert!(!after_dd.writes);

    // `git branch` with `only` listing flags stays Home-eligible.
    let gb = classify(&["git", "branch", "--all", "--show-current"]);
    assert!(gb.home_readonly_eligible);
    assert!(!gb.writes);
    assert!(gb.unread.is_empty());
}

#[test]
fn home_readonly_commands_protocol_parity() {
    assert_eq!(HOME_READONLY_COMMANDS.len(), 24);

    for entry in HOME_READONLY_COMMANDS {
        let mut argv: Vec<&str> = entry.prefix.to_vec();
        if matches!(entry.prefix, ["rg"] | ["grep"]) {
            argv.push("pattern");
            argv.push("README.md");
        }
        let eff = classify(&argv);
        assert!(
            eff.home_readonly_eligible,
            "HOME_READONLY_COMMANDS entry {:?} must be Home-eligible",
            entry.prefix
        );
        assert!(
            eff.unread.is_empty(),
            "HOME_READONLY_COMMANDS entry {:?} must not emit Unread",
            entry.prefix
        );

        // Every listed forbidden flag must disqualify Home eligibility.
        for &flag in entry.forbidden {
            let mut bad_argv: Vec<&str> = entry.prefix.to_vec();
            bad_argv.push(flag);
            if matches!(entry.prefix, ["rg"] | ["grep"]) {
                bad_argv.push("arg");
                bad_argv.push("README.md");
            }
            let bad_eff = classify(&bad_argv);
            assert!(
                !bad_eff.home_readonly_eligible,
                "forbidden flag {flag} on {:?} must disable Home eligibility",
                entry.prefix
            );
        }

        // `only` entries accept every listed arg and reject unlisted positionals.
        if let Some(only) = entry.only {
            for &ok_arg in only {
                let mut ok_argv: Vec<&str> = entry.prefix.to_vec();
                ok_argv.push(ok_arg);
                let ok_eff = classify(&ok_argv);
                assert!(
                    ok_eff.home_readonly_eligible,
                    "only arg {ok_arg} on {:?} must stay Home-eligible",
                    entry.prefix
                );
            }
            let mut non_only: Vec<&str> = entry.prefix.to_vec();
            non_only.push("feature-branch");
            let non_eff = classify(&non_only);
            assert!(!non_eff.home_readonly_eligible);
            assert!(non_eff.unread.is_empty());
        }
    }
}

// ---------------------------------------------------------------------------
// 2. D-141 Kind 2: Search (`rg` and `grep` family; §5, §6, §10 row 15, §11)
// ---------------------------------------------------------------------------

#[test]
fn search_pattern_operand_not_checked_as_path() {
    // §11 item 9: `grep .npmrc README.md` treats `.npmrc` as Pattern, README.md as Path.
    let eff = classify(&["grep", ".npmrc", "README.md"]);
    assert_eq!(eff.tool_name, Some("grep"));
    assert_eq!(eff.filter_dialect, Some(FilterDialect::GrepInclude));
    assert!(!eff.reads_recursively);
    assert!(eff.home_readonly_eligible);
    assert!(!eff.writes);
    assert!(eff.unread.is_empty());
    assert_eq!(
        eff.roles,
        vec![
            OperandRole::Command,
            OperandRole::Pattern,
            OperandRole::Path
        ]
    );
    assert_eq!(
        eff.operand_access,
        vec![AccessMode::Read, AccessMode::Read, AccessMode::Read]
    );

    // `-e` supplies the pattern so the first positional is a Path.
    let eff_e = classify(&["rg", "-e", ".npmrc", "src"]);
    assert_eq!(
        eff_e.roles,
        vec![
            OperandRole::Command,
            OperandRole::Flag,
            OperandRole::FlagValue(ValueRole::Pattern),
            OperandRole::Path,
        ]
    );
    assert!(eff_e.reads_recursively);
    assert!(eff_e.home_readonly_eligible);

    // `-f` supplies pattern from a Path operand (Read) and disables Home eligibility (forbidden).
    let eff_f = classify(&["rg", "-f", "pats.txt", "src"]);
    assert_eq!(
        eff_f.roles,
        vec![
            OperandRole::Command,
            OperandRole::Flag,
            OperandRole::FlagValue(ValueRole::Path),
            OperandRole::Path,
        ]
    );
    assert!(!eff_f.home_readonly_eligible);
    assert!(eff_f.unread.is_empty());

    // Attached path-valued flag fails closed to Unread because P2 resolves whole words.
    let eff_att_long = classify(&["rg", "--file=pats.txt", "src"]);
    assert!(
        eff_att_long
            .unread
            .iter()
            .any(|r| matches!(r, UnreadReason::UnsupportedFlagEffect { tool, flag } if tool == "rg" && flag == "--file"))
    );
    let eff_att_short = classify(&["grep", "-fpats.txt", "src"]);
    assert!(
        eff_att_short
            .unread
            .iter()
            .any(|r| matches!(r, UnreadReason::UnsupportedFlagEffect { tool, flag } if tool == "grep" && flag == "-f"))
    );
}

#[test]
fn search_filters_case_toggles_and_unsupported_forms() {
    // Single `-g` follows global case toggle regardless of flag ordering.
    let eff_cs = classify_words(vec![w("rg"), w("-g"), wq("*.rs"), w("needle"), w("src")]);
    assert!(eff_cs.unread.is_empty());
    assert_eq!(eff_cs.filters.len(), 1);
    assert_eq!(eff_cs.filters[0].dialect, FilterDialect::RgGlob);
    assert!(!eff_cs.filters[0].case_insensitive);
    assert_eq!(eff_cs.filters[0].pattern.as_str(), Some("*.rs"));

    // `--glob-case-insensitive` AFTER `-g` still governs `-g`.
    let eff_ci = classify_words(vec![
        w("rg"),
        w("-g"),
        wq("*.rs"),
        w("--glob-case-insensitive"),
        w("needle"),
        w("src"),
    ]);
    assert!(eff_ci.unread.is_empty());
    assert_eq!(eff_ci.filters.len(), 1);
    assert!(eff_ci.filters[0].case_insensitive);

    // Last toggle wins: `--glob-case-insensitive` then `--no-glob-case-insensitive`.
    let eff_ci_off = classify_words(vec![
        w("rg"),
        w("--glob-case-insensitive"),
        w("--no-glob-case-insensitive"),
        w("--glob=*.rs"),
        w("needle"),
        w("src"),
    ]);
    assert!(eff_ci_off.unread.is_empty());
    assert_eq!(eff_ci_off.filters.len(), 1);
    assert!(!eff_ci_off.filters[0].case_insensitive);

    // `--iglob` is always case-insensitive even after `--no-glob-case-insensitive`.
    let eff_iglob = classify_words(vec![
        w("rg"),
        w("--no-glob-case-insensitive"),
        w("--iglob"),
        wq("*.rs"),
        w("needle"),
        w("src"),
    ]);
    assert!(eff_iglob.unread.is_empty());
    assert_eq!(eff_iglob.filters.len(), 1);
    assert!(eff_iglob.filters[0].case_insensitive);

    // `grep --include` is always case-sensitive.
    let eff_grep = classify_words(vec![
        w("grep"),
        w("-r"),
        w("--include"),
        wq("*.rs"),
        w("needle"),
        w("src"),
    ]);
    assert!(eff_grep.unread.is_empty());
    assert!(eff_grep.reads_recursively);
    assert_eq!(eff_grep.filters.len(), 1);
    assert_eq!(eff_grep.filters[0].dialect, FilterDialect::GrepInclude);
    assert!(!eff_grep.filters[0].case_insensitive);

    // Multiple filters → Unread(UnsupportedFilterForm).
    let eff_multi = classify_words(vec![
        w("rg"),
        w("-g"),
        wq("*.rs"),
        w("-g"),
        wq("*.toml"),
        w("needle"),
        w("src"),
    ]);
    assert!(
        eff_multi
            .unread
            .contains(&UnreadReason::UnsupportedFilterForm)
    );
    assert!(!eff_multi.home_readonly_eligible);

    // Negated filter (`!`) → Unread(UnsupportedFilterForm).
    let eff_neg = classify_words(vec![w("rg"), w("-g"), wq("!*.env"), w("needle"), w("src")]);
    assert!(
        eff_neg
            .unread
            .contains(&UnreadReason::UnsupportedFilterForm)
    );

    // Listed unsupported filter flags (`-t`, `--max-depth`, `grep --exclude`).
    for cmd in [
        &["rg", "-t", "rust", "needle", "src"][..],
        &["rg", "--max-depth", "2", "needle", "src"][..],
        &["grep", "--exclude", "a", "needle", "src"][..],
    ] {
        let e = classify(cmd);
        assert!(
            e.unread.contains(&UnreadReason::UnsupportedFilterForm),
            "{cmd:?}"
        );
        assert!(!e.home_readonly_eligible, "{cmd:?}");
    }
}

#[test]
fn search_followed_links_unknown_flags_and_near_misses() {
    // §10 row 15 / §11 item 15: `rg -L` and `grep -R` follow links below root.
    let rg_l = classify(&["rg", "-L", "needle", "src"]);
    assert!(rg_l.reads_recursively);
    assert_eq!(
        rg_l.links,
        LinkPolicy {
            at_root: true,
            below_root: true
        }
    );
    assert_eq!(rg_l.home_readonly_eligible, ALLOW_HOME_FOLLOWED_SEARCH);
    assert!(rg_l.unread.is_empty());

    // `--no-follow` is NOT in RG_FLAGS, so §6 requires UnknownFlag/Unread
    // and disables Home eligibility (finding 13).
    let rg_nf = classify(&["rg", "-L", "--no-follow", "needle", "src"]);
    assert!(rg_nf.unread.iter().any(|r| matches!(
        r,
        UnreadReason::UnknownFlag { tool, flag } if tool == "rg" && flag == "--no-follow"
    )));
    assert!(!rg_nf.home_readonly_eligible);

    // Table-supported ordered toggle: `grep -R -r` disables below-root link following.
    let grep_r_override = classify(&["grep", "-R", "-r", "needle", "src"]);
    assert!(grep_r_override.reads_recursively);
    assert!(!grep_r_override.links.below_root);
    assert!(grep_r_override.home_readonly_eligible);
    assert!(grep_r_override.unread.is_empty());

    let grep_cap_r = classify(&["grep", "-R", "needle", "src"]);
    assert!(grep_cap_r.reads_recursively);
    assert!(grep_cap_r.links.below_root);
    assert_eq!(
        grep_cap_r.home_readonly_eligible,
        ALLOW_HOME_FOLLOWED_SEARCH
    );

    // `grep -d recurse` is a listed FlagUnread::FlagEffect -> UnsupportedFlagEffect.
    let grep_d = classify(&["grep", "-d", "recurse", "needle", "src"]);
    assert!(
        grep_d
            .unread
            .iter()
            .any(|r| matches!(r, UnreadReason::UnsupportedFlagEffect { tool, flag } if tool == "grep" && flag == "-d"))
    );
    assert!(!grep_d.home_readonly_eligible);

    // Unlisted flag emits UnknownFlag AND keeps conservative Path/ReadWrite on operands.
    let rg_unk = classify(&["rg", "--bogus-option", ".npmrc", "src"]);
    assert!(
        rg_unk
            .unread
            .iter()
            .any(|r| matches!(r, UnreadReason::UnknownFlag { tool, flag } if tool == "rg" && flag == "--bogus-option"))
    );
    assert!(!rg_unk.home_readonly_eligible);
    assert_eq!(rg_unk.roles[2], OperandRole::Path);
    assert_eq!(rg_unk.operand_access[2], AccessMode::ReadWrite);

    // Stdin `-` disables Home eligibility on search without Unread.
    let rg_stdin = classify(&["rg", "needle", "-"]);
    assert!(!rg_stdin.home_readonly_eligible);
    assert!(rg_stdin.unread.is_empty());

    // `egrep` is a Search tool but not in HOME_READONLY_COMMANDS.
    let egrep = classify(&["egrep", "needle", "README.md"]);
    assert_eq!(egrep.tool_name, Some("egrep"));
    assert!(!egrep.home_readonly_eligible);
    assert!(egrep.unread.is_empty());
}

// ---------------------------------------------------------------------------
// 3. D-141 Kind 3: Wrapper & NestedShell (§4, §6)
// ---------------------------------------------------------------------------

#[test]
fn wrapper_and_nested_shell_classification() {
    // A unit still starting with a §4 wrapper is Unread(UnsupportedWrapperForm).
    for wrap in ["command", "exec", "nice", "timeout", "time", "env"] {
        let e = classify(&[wrap, "ls"]);
        assert_eq!(e.tool_name, Some(wrap));
        assert!(
            e.unread.contains(&UnreadReason::UnsupportedWrapperForm {
                wrapper: wrap.to_owned()
            }),
            "{wrap}"
        );
    }

    // NestedShell with script file -> pinned Unread (program).
    let sh_script = classify(&["sh", "build.sh", "--fast"]);
    assert_eq!(sh_script.tool_name, Some("sh"));
    assert_eq!(sh_script.roles[1], OperandRole::ProgramFile);
    assert_eq!(sh_script.operand_access[1], AccessMode::Read);
    assert_eq!(
        sh_script.code,
        CodeExecution::Program {
            program_operand: Some(1),
            program_words: vec!["sh".into(), "build.sh".into()],
        }
    );
    assert_eq!(
        sh_script.unread,
        vec![UnreadReason::Program {
            program_words: vec!["sh".into(), "build.sh".into()]
        }]
    );

    // Near misses / triggers -> UnsupportedNestedShellForm.
    for cmd in [
        &["sh"][..],
        &["bash", "-i"][..],
        &["sh", "-x", "build.sh"][..],
        &["dash", "-c", "echo hi"][..],
    ] {
        let e = classify(cmd);
        assert!(
            e.unread
                .iter()
                .any(|r| matches!(r, UnreadReason::UnsupportedNestedShellForm { .. })),
            "{cmd:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. D-141 Kind 4: CodeRunner (§4, §6, §10 row 14, §11 item 8)
// ---------------------------------------------------------------------------

#[test]
fn code_runner_pinned_programs_inline_code_and_near_misses() {
    // §10 row 14: `python build.py` is pinned Unread (program).
    let py = classify(&["python", "build.py", "--release"]);
    assert_eq!(py.tool_name, Some("python"));
    assert_eq!(py.roles[1], OperandRole::ProgramFile);
    assert_eq!(
        py.code,
        CodeExecution::Program {
            program_operand: Some(1),
            program_words: vec!["python".into(), "build.py".into()],
        }
    );
    assert_eq!(
        py.unread,
        vec![UnreadReason::Program {
            program_words: vec!["python".into(), "build.py".into()]
        }]
    );

    // Inline code flag `-c` (separate and attached).
    let py_c = classify(&["python3", "-c", "import os"]);
    assert_eq!(
        py_c.code,
        CodeExecution::Inline {
            code_operands: vec![2]
        }
    );
    assert_eq!(
        py_c.unread,
        vec![UnreadReason::CodeRunnerInlineCode {
            tool: "python3".into()
        }]
    );

    let py_ic = classify(&["python3", "-Icprint(1)"]);
    assert_eq!(
        py_ic.code,
        CodeExecution::Inline {
            code_operands: vec![1]
        }
    );
    assert!(matches!(
        py_ic.unread.as_slice(),
        [UnreadReason::CodeRunnerInlineCode { .. }]
    ));

    // Near miss: option before script is NOT pinned.
    let py_opt = classify(&["python", "-u", "build.py"]);
    assert!(matches!(
        py_opt.unread.as_slice(),
        [UnreadReason::CodeRunnerInlineCode { .. }]
    ));

    // `awk -f prog.awk` is pinned; inline `awk` or competing `-e`/`-f` is not.
    let awk_pin = classify(&["awk", "-f", "prog.awk", "data.txt"]);
    assert_eq!(awk_pin.roles[1], OperandRole::Flag);
    assert_eq!(awk_pin.roles[2], OperandRole::ProgramFile);
    assert_eq!(
        awk_pin.unread,
        vec![UnreadReason::Program {
            program_words: vec!["awk".into(), "-f".into(), "prog.awk".into()]
        }]
    );
    let awk_inline = classify(&["awk", "{print $1}", "data.txt"]);
    assert_eq!(
        awk_inline.code,
        CodeExecution::Inline {
            code_operands: vec![1]
        }
    );
    assert_eq!(
        awk_inline.unread,
        vec![UnreadReason::CodeRunnerInlineCode { tool: "awk".into() }]
    );
    for unpinned_awk in [
        &["awk", "-f", "a.awk", "-e", "{print}"][..],
        &["awk", "-f", "a.awk", "-f", "b.awk"][..],
        &["awk", "-f", "-"][..],
    ] {
        let e = classify(unpinned_awk);
        assert!(
            matches!(
                e.unread.as_slice(),
                [UnreadReason::CodeRunnerInlineCode { .. }]
            ),
            "{unpinned_awk:?}"
        );
    }

    // Multi-subcommand code runners: `cargo test`, `pipx run ruff`.
    let cargo_test = classify(&["cargo", "test", "--lib"]);
    assert_eq!(cargo_test.subcommand, Some("test"));
    assert_eq!(
        cargo_test.code,
        CodeExecution::Program {
            program_operand: None,
            program_words: vec!["cargo".into(), "test".into()],
        }
    );
    assert_eq!(
        cargo_test.unread,
        vec![UnreadReason::Program {
            program_words: vec!["cargo".into(), "test".into()]
        }]
    );

    let cargo_bare = classify(&["cargo"]);
    assert!(matches!(
        cargo_bare.unread.as_slice(),
        [UnreadReason::CodeRunnerInlineCode { tool }] if tool == "cargo"
    ));

    let pipx_run = classify(&["pipx", "run", "ruff", "check"]);
    assert_eq!(pipx_run.subcommand, Some("run"));
    assert_eq!(pipx_run.roles[2], OperandRole::ProgramFile);
    assert_eq!(
        pipx_run.unread,
        vec![UnreadReason::Program {
            program_words: vec!["pipx".into(), "run".into(), "ruff".into()]
        }]
    );

    let pipx_bare = classify(&["pipx", "run"]);
    assert!(matches!(
        pipx_bare.unread.as_slice(),
        [UnreadReason::CodeRunnerInlineCode { tool }] if tool == "pipx run"
    ));

    // CR-19 folded command runner names.
    let folded_py = classify(&["/usr/bin/PYTHON3", "-c", "1"]);
    assert_eq!(folded_py.tool_name, Some("python3"));
    assert!(matches!(
        folded_py.unread.as_slice(),
        [UnreadReason::CodeRunnerInlineCode { .. }]
    ));

    let folded_sh = classify(&["ba\u{017F}h", "run.sh"]);
    assert_eq!(folded_sh.tool_name, Some("bash"));
    assert!(matches!(
        folded_sh.unread.as_slice(),
        [UnreadReason::Program { .. }]
    ));
}

// ---------------------------------------------------------------------------
// 5. D-141 Kind 5: ConditionalRunner (`find`, `git`, `sed`; §6)
// ---------------------------------------------------------------------------

#[test]
fn conditional_runner_find() {
    // Accepted form (no trigger): unknown-tool treatment.
    let clean = classify(&["find", ".", "-name", "foo"]);
    assert_eq!(clean.tool_name, Some("find"));
    assert!(clean.writes);
    assert!(!clean.home_readonly_eligible);
    assert!(clean.unread.is_empty());
    assert_eq!(clean.roles[1], OperandRole::Path);

    // Every trigger flag makes the unit Unread(CodeRunnerInlineCode).
    for trig in ["-exec", "-execdir", "-ok", "-okdir"] {
        let e = classify(&["find", ".", trig, "echo", "{}", ";"]);
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode {
                tool: "find".into()
            }],
            "{trig}"
        );
    }
}

#[test]
fn conditional_runner_git_triggers_and_subcommands() {
    // Accepted HomeReadOnly subcommand with CR-12 name selection.
    let show = classify(&["git", "show", "HEAD:README.md"]);
    assert_eq!(show.tool_name, Some("git"));
    assert_eq!(show.subcommand, Some("show"));
    assert!(show.home_readonly_eligible);
    assert!(!show.writes);
    assert!(show.git_selects_by_name);
    assert!(show.unread.is_empty());

    // Global `-C repo` disables Home eligibility but preserves subcommand detection.
    let git_c = classify(&["git", "-C", "repo", "status"]);
    assert_eq!(git_c.subcommand, Some("status"));
    assert!(!git_c.home_readonly_eligible);
    assert!(git_c.unread.is_empty());

    // `git cat-file`: unknown-tool treatment plus CR-12 name selection.
    let cat_file = classify(&["git", "cat-file", "-p", "HEAD:.env"]);
    assert_eq!(cat_file.subcommand, Some("cat-file"));
    assert!(cat_file.git_selects_by_name);
    assert!(cat_file.writes);
    assert!(!cat_file.home_readonly_eligible);
    assert!(cat_file.unread.is_empty());

    // Every code-runner trigger on `git`.
    for trig_cmd in [
        &["git", "-c", "core.fsmonitor=evil", "status"][..],
        &["git", "--config-env=k=v", "log"][..],
        &["git", "--exec-path=/tmp", "status"][..],
        &["git", "merge-index", "prog", "file"][..],
        &["git", "filter-branch", "--tree-filter", "id"][..],
        &["git", "bisect", "run", "./test.sh"][..],
        &["git", "submodule", "foreach", "id"][..],
        &["git", "rebase", "-x", "id", "main"][..],
        &["git", "rebase", "--exec=id", "main"][..],
        &["git", "difftool", "-x", "id"][..],
        &["git", "mergetool", "--tool-cmd=id"][..],
        &["git", "diff", "--ext-diff"][..],
        &["git", "push", "--receive-pack=id"][..],
        &["git", "push", "--exec=id"][..],
    ] {
        let e = classify(trig_cmd);
        assert!(
            e.unread
                .iter()
                .any(|r| matches!(r, UnreadReason::CodeRunnerInlineCode { .. })),
            "expected trigger on {trig_cmd:?}"
        );
        assert!(!e.home_readonly_eligible, "{trig_cmd:?}");
        assert!(e.writes, "{trig_cmd:?}");
    }

    // Near misses (non-trigger subargs or flags after `--`).
    for safe_cmd in [
        &["git", "bisect", "start"][..],
        &["git", "submodule", "status"][..],
        &["git", "rebase", "main"][..],
        &["git", "diff", "--", "--ext-diff"][..],
    ] {
        let e = classify(safe_cmd);
        assert!(e.unread.is_empty(), "unexpected Unread on {safe_cmd:?}");
    }

    // DerivedDestinationWriter subcommands (`git clone`, `git checkout`).
    for ddw in [
        &["git", "clone", "url", "dir"][..],
        &["git", "checkout", "main"][..],
    ] {
        let e = classify(ddw);
        assert_eq!(e.derived_destinations, DerivedDestinations::Always);
        assert!(
            e.unread
                .iter()
                .any(|r| matches!(r, UnreadReason::DerivedDestinations { .. })),
            "{ddw:?}"
        );
    }
}

#[test]
fn conditional_runner_sed_script_parser_and_program_pin() {
    // Safe scripts: `e`/`w`/`r` inside pattern/replacement or `y///` do NOT trigger.
    // Under D-156, only pure zero-arg boolean flags and inspected `-e`/`--expression`
    // forms are accepted; `-i` / `-i.bak` / `-l` / `--line-length` have unmodeled
    // arities and fail closed to Unread.
    for safe in [
        vec![w("sed"), wq("s/foo/bar/g"), w("a.txt")],
        vec![w("sed"), wq("s/error/warn/g"), w("a.txt")],
        vec![w("sed"), wq("/write/d"), w("a.txt")],
        vec![w("sed"), wq("y/ewr/xyz/"), w("a.txt")],
        vec![w("sed"), w("-n"), wq("1,5p"), w("a.txt")],
        vec![w("sed"), w("-E"), w("-e"), wq("s|error|ok|g"), w("a.txt")],
        vec![w("sed"), wq("z"), w("a.txt")],
        vec![w("sed"), wq("s/é/x/"), w("a.txt")],
        vec![w("sed"), wq("s/x/é/"), w("a.txt")],
        vec![w("sed"), w("-n"), wq("y/é/ü/"), w("a.txt")],
        vec![w("sed"), w("-n"), wq("\\+é+p"), w("a.txt")],
    ] {
        let e = classify_words(safe.clone());
        assert!(e.unread.is_empty(), "expected safe sed for {safe:?}");
        assert!(e.writes);
        assert!(!e.home_readonly_eligible);
    }

    // Finding 5 & D-156: ALL `-i` / `--in-place` and `-l` / `--line-length` forms
    // (including `sed -i ''` and `sed -i.bak`) are outside the frozen table and
    // must NOT consume words as option values -> fail closed to Unread.
    for unmodeled_sed in [
        vec![w("sed"), w("-i"), w("p"), wq("w out"), w("input")],
        vec![w("sed"), w("-ni"), w("p"), wq("w out"), w("input")],
        vec![w("sed"), w("-i"), wq("s/a/b/"), w("input")],
        vec![w("sed"), w("-i"), w("-e"), w("p"), wq("w out"), w("input")],
        vec![w("sed"), w("-i"), wq(""), wq("s/a/b/"), w("a.txt")],
        vec![w("sed"), w("-i"), wq(""), wq("w out"), w("input")],
        vec![w("sed"), w("-i.bak"), wq("s/a/b/"), w("a.txt")],
        vec![w("sed"), w("-i.bak"), wq("w out"), w("input")],
        vec![w("sed"), w("--in-place=.bak"), wq("s/a/b/"), w("a.txt")],
        vec![w("sed"), w("--line-length"), w("80"), w("p"), w("a.txt")],
    ] {
        let e = classify_words(unmodeled_sed.clone());
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode { tool: "sed".into() }],
            "expected Unread on sed form {unmodeled_sed:?}"
        );
    }

    // Trigger scripts (`e`, `w`, `r`, `W`, `R`, substitute flags `e`/`w`).
    for bad in [
        "1e id",
        "s/a/b/e",
        "1w out.txt",
        "s/a/b/w out.txt",
        "s|a/b|c/d|w out.txt",
        "1r /etc/passwd",
        "1W out.txt",
        "1R in.txt",
        "{p;e id}",
        "s/unclosed",
        "s€a€b€",
        "s€a€b€p",
        "y€ab€cd€",
        "\\€re€p",
    ] {
        let e = classify_words(vec![w("sed"), wq(bad), w("a.txt")]);
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode { tool: "sed".into() }],
            "expected trigger on sed script {bad:?}"
        );
    }

    // Program file pin (`sed -f edit.sed`, `sed --file edit.sed`).
    let pin = classify(&["sed", "-f", "edit.sed", "a.txt"]);
    assert_eq!(pin.roles[1], OperandRole::Flag);
    assert_eq!(pin.roles[2], OperandRole::ProgramFile);
    assert_eq!(
        pin.unread,
        vec![UnreadReason::Program {
            program_words: vec!["sed".into(), "-f".into(), "edit.sed".into()]
        }]
    );

    // Near misses fail closed to plain CodeRunnerInlineCode.
    for near in [
        &["sed", "-f", "edit.sed", "-e", "p", "a.txt"][..],
        &["sed", "-f", "a.sed", "-f", "b.sed", "a.txt"][..],
        &["sed", "-f", "-", "a.txt"][..],
        &["sed", "-fedit.sed", "a.txt"][..],
        &["sed", "--unknown-flag", "p", "a.txt"][..],
        &["sed"][..],
    ] {
        let e = classify(near);
        assert!(
            matches!(e.unread.as_slice(), [UnreadReason::CodeRunnerInlineCode { tool }] if tool == "sed"),
            "expected plain CodeRunnerInlineCode on {near:?}, got {:?}",
            e.unread
        );
    }
}

// ---------------------------------------------------------------------------
// 6. D-141 Kind 6: DerivedDestinationWriter (§5, §6, §11 item 10)
// ---------------------------------------------------------------------------

#[test]
fn derived_destination_writers_and_near_misses() {
    // `cp` with recursive/archive flags triggers; plain `cp` does not.
    for trig in [
        &["cp", "-R", "src", "dst"][..],
        &["cp", "-r", "src", "dst"][..],
        &["cp", "-a", "src", "dst"][..],
        &["cp", "-av", "src", "dst"][..],
        &["cp", "--recursive", "src", "dst"][..],
        &["cp", "--archive", "src", "dst"][..],
    ] {
        let e = classify(trig);
        assert_eq!(
            e.derived_destinations,
            DerivedDestinations::Always,
            "{trig:?}"
        );
        assert_eq!(
            e.unread,
            vec![UnreadReason::DerivedDestinations { tool: "cp".into() }],
            "{trig:?}"
        );
        assert_eq!(e.roles[2], OperandRole::Path);
        assert_eq!(e.operand_access[2], AccessMode::ReadWrite);
    }

    for plain_cp in [&["cp", "a", "b"][..], &["cp", "--", "-R", "b"][..]] {
        let e = classify(plain_cp);
        assert_eq!(e.derived_destinations, DerivedDestinations::None);
        assert!(e.unread.is_empty());
        assert!(e.writes);
    }

    // `tar` with extract flags (dashed or old-style bundled) triggers; create/list do not.
    for trig in [
        &["tar", "-xf", "a.tar"][..],
        &["tar", "-x", "-f", "a.tar"][..],
        &["tar", "xf", "a.tar"][..],
        &["tar", "xzf", "a.tar.gz"][..],
        &["tar", "--extract", "-f", "a.tar"][..],
        &["tar", "--get", "-f", "a.tar"][..],
    ] {
        let e = classify(trig);
        assert_eq!(
            e.derived_destinations,
            DerivedDestinations::Always,
            "{trig:?}"
        );
        assert_eq!(
            e.unread,
            vec![UnreadReason::DerivedDestinations { tool: "tar".into() }],
            "{trig:?}"
        );
    }

    for non_extract_tar in [
        &["tar", "-cf", "out.tar", "src"][..],
        &["tar", "-tf", "a.tar"][..],
        &["tar", "cf", "out.tar", "src"][..],
    ] {
        let e = classify(non_extract_tar);
        assert_eq!(e.derived_destinations, DerivedDestinations::None);
        assert!(e.unread.is_empty());
    }

    // `mv`: WhenSourceIsDir with no P3 unread (P2 checks whether source is a dir).
    let mv = classify(&["mv", "src", "dst"]);
    assert_eq!(
        mv.derived_destinations,
        DerivedDestinations::WhenSourceIsDir
    );
    assert!(mv.unread.is_empty());
    assert!(mv.writes);

    // `rsync` and `unzip`: Always.
    for always in [&["rsync", "-a", "src", "dst"][..], &["unzip", "a.zip"][..]] {
        let e = classify(always);
        assert_eq!(e.derived_destinations, DerivedDestinations::Always);
        assert!(matches!(
            e.unread.as_slice(),
            [UnreadReason::DerivedDestinations { .. }]
        ));
    }
}

// ---------------------------------------------------------------------------
// 7. OUTSIDE_MODEL, UNKNOWN_BY_DESIGN, and `cd` (§4, §5, §6, D-141)
// ---------------------------------------------------------------------------

#[test]
fn outside_model_unknown_by_design_and_cd() {
    for out in [
        &["zsh", "-c", "ls"][..],
        &["fish", "-c", "ls"][..],
        &["nohup", "make"][..],
        &["if", "true"][..],
        &["pushd", "/tmp"][..],
        &["popd"][..],
    ] {
        let e = classify(out);
        assert_eq!(e.tool_name, None, "{out:?}");
        assert!(
            e.unread
                .iter()
                .any(|r| matches!(r, UnreadReason::OutsideToolModel { .. })),
            "{out:?}"
        );
    }

    // UNKNOWN_BY_DESIGN: unknown-tool treatment (every word Path/ReadWrite, writes = true,
    // not Home-eligible, no Unread).
    for unk in [
        &["rm", "-rf", "build"][..],
        &["patch", "-p1"][..],
        &["curl", "https://example.com"][..],
        &["diff", "a", "b"][..],
        &["ln", "-s", "~/.ssh/id_rsa", "link"][..],
    ] {
        let e = classify(unk);
        assert_eq!(e.tool_name, None, "{unk:?}");
        assert!(e.writes, "{unk:?}");
        assert!(!e.home_readonly_eligible, "{unk:?}");
        assert!(e.unread.is_empty(), "{unk:?}");
        for idx in 1..unk.len() {
            assert_eq!(e.roles[idx], OperandRole::Path, "{unk:?}[{idx}]");
            assert_eq!(
                e.operand_access[idx],
                AccessMode::ReadWrite,
                "{unk:?}[{idx}]"
            );
        }
    }

    // `cd`: cwd tracker handoff to P2 via `cd_target_operand`.
    let cd_ok = classify(&["cd", "proj"]);
    assert!(!cd_ok.writes);
    assert_eq!(cd_ok.cd_target_operand, Some(1));
    assert_eq!(cd_ok.roles[1], OperandRole::Path);
    assert_eq!(cd_ok.operand_access[1], AccessMode::Read);
    assert!(cd_ok.unread.is_empty());

    let cd_flags = classify(&["cd", "-P", "--", "proj"]);
    assert_eq!(cd_flags.cd_target_operand, Some(3));
    assert!(cd_flags.unread.is_empty());

    let cd_dash = classify(&["cd", "-"]);
    assert_eq!(cd_dash.cd_target_operand, None);

    let cd_bad = classify(&["cd", "-Z", "proj"]);
    assert_eq!(cd_bad.cd_target_operand, None);
    assert!(matches!(
        cd_bad.unread.as_slice(),
        [UnreadReason::UnknownFlag { tool, flag }] if tool == "cd" && flag == "-Z"
    ));
}

// ---------------------------------------------------------------------------
// 8. CR-12 & CR-17 Git name selection + wildcard pathspecs (§6, §11 item 10)
// ---------------------------------------------------------------------------

#[test]
fn git_selection_credentials_cr17_ancestors_magic_and_wildcards() {
    // Committed credentials and CR-17 stores / proper directory ancestors -> StoreHit.
    for hit_cmd in [
        vec![w("git"), w("show"), w("HEAD:.env")],
        vec![w("git"), w("show"), w("HEAD:.npmrc")],
        vec![w("git"), w("show"), w("HEAD:sub/id_rsa")],
        vec![w("git"), w("cat-file"), w("-p"), w("HEAD:.aws/credentials")],
        vec![w("git"), w("log"), w("-p"), w("--"), w(".netrc")],
        vec![w("git"), w("diff"), w("HEAD"), w("--"), w(".kube/config")],
        vec![w("git"), w("show"), w("HEAD:.gem/credentials")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(".gem")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w("sub/.gem")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(".terraform.d")],
        vec![
            w("git"),
            w("show"),
            w("HEAD"),
            w("--"),
            w(".terraform.d/credentials.tfrc.json"),
        ],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(".config/op")],
        vec![
            w("git"),
            w("show"),
            w("HEAD"),
            w("--"),
            w(".config/op/config"),
        ],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(".yarnrc.yml")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(".ssh")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(".aws")],
        vec![w("git"), w("show"), w(":0:.env")],
        vec![w("git"), w("log"), w("-L"), w("1,10:.env")],
        vec![w("git"), w("log"), w("-L"), w(":fn:sub/.npmrc")],
        vec![w("git"), w("log"), w("-L"), w("malformed")],
        vec![w("git"), w("show"), w("HEAD:.ENV")],
        vec![w("git"), w("show"), w("HEAD:.ſsh/id_rsa")],
        vec![
            w("git"),
            w("show"),
            w("HEAD"),
            w("--"),
            wq(":(top)README.md"),
        ],
        vec![w("git"), w("show"), w("HEAD"), w("--"), wq(":!README.md")],
        // Finding 10: root pathspec selections and lone `:` pathspec are ancestor selections.
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(".")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w("./")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w("src/..")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(":")],
        vec![w("git"), w("show"), w("HEAD"), w(":")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), w(":.")],
        vec![w("git"), w("diff"), w(".")],
        vec![w("git"), w("log"), w("-p"), w("--"), w(".")],
    ] {
        let (_, res) = git_sel(hit_cmd.clone());
        assert!(
            res.iter().any(|r| matches!(
                &r.outcome,
                PartOutcome::StoreHit(h)
                    if h.source == HitSource::Model
                        && matches!(h.matched, HitMatch::StoreFamily { family: "git-credential-selection" })
            )),
            "expected StoreHit for {hit_cmd:?}, got {res:?}"
        );
    }

    // Clear cases (non-credentials, .ssh public exceptions, ordinary files in .gem).
    for clear_cmd in [
        vec![w("git"), w("show"), w("HEAD:README.md")],
        vec![w("git"), w("show"), w("HEAD:.ssh/id_rsa.pub")],
        vec![w("git"), w("show"), w("HEAD:.ssh/config")],
        vec![w("git"), w("show"), w("HEAD:.ssh/known_hosts")],
        vec![
            w("git"),
            w("show"),
            w("HEAD"),
            w("--"),
            w(".gem/ordinary.txt"),
        ],
        vec![w("git"), w("log"), w("-L"), w("1,10:src/main.rs")],
        vec![
            w("git"),
            w("--literal-pathspecs"),
            w("status"),
            w("--"),
            wq("*.key"),
        ],
        // Finding 11: `--noglob-pathspecs` makes wildcards literal unless explicit glob magic enables them.
        vec![
            w("git"),
            w("--noglob-pathspecs"),
            w("status"),
            w("--"),
            wq("*.rs"),
        ],
        vec![
            w("git"),
            w("--noglob-pathspecs"),
            w("show"),
            w("HEAD"),
            w("--"),
            wq("*.rs"),
        ],
        // Revision-object context kept separate from pathspec root-ancestor matching.
        vec![w("git"), w("show"), w("HEAD:.")],
        vec![w("git"), w("diff"), w("HEAD..main")],
        vec![w("git"), w("show"), w("HEAD"), w("--"), wq(r"\*.txt")],
    ] {
        let (_, res) = git_sel(clear_cmd.clone());
        assert!(
            res.is_empty(),
            "expected clear for {clear_cmd:?}, got {res:?}"
        );
    }

    // Active wildcard pathspecs -> Unread(GitWildcardPathspec).
    for wild_cmd in [
        vec![w("git"), w("show"), w("HEAD"), w("--"), wq(".gem/*")],
        vec![w("git"), w("status"), w("--"), wq("*.key")],
        vec![w("git"), w("ls-files"), wq("*.env")],
        // Explicit `:(glob)` overrides `--noglob-pathspecs`.
        vec![
            w("git"),
            w("--noglob-pathspecs"),
            w("status"),
            w("--"),
            wq(":(glob)*.rs"),
        ],
    ] {
        let (_, res) = git_sel(wild_cmd.clone());
        assert!(
            res.iter().any(|r| matches!(
                r.outcome,
                PartOutcome::Unread(UnreadReason::GitWildcardPathspec { .. })
            )),
            "expected GitWildcardPathspec for {wild_cmd:?}, got {res:?}"
        );
    }

    // `--noglob-pathspecs` keeps pathspec exclusion magic (`:!`) as StoreHit
    // while treating `*` as literal (no GitWildcardPathspec).
    let (_, noglob_excl) = git_sel(vec![
        w("git"),
        w("--noglob-pathspecs"),
        w("show"),
        w("HEAD"),
        w("--"),
        wq(":!*.rs"),
    ]);
    assert!(
        noglob_excl
            .iter()
            .any(|r| matches!(r.outcome, PartOutcome::StoreHit(_)))
    );
    assert!(!noglob_excl.iter().any(|r| matches!(
        r.outcome,
        PartOutcome::Unread(UnreadReason::GitWildcardPathspec { .. })
    )));

    // `:(glob).gem/*` produces both StoreHit (pathspec magic) and GitWildcardPathspec.
    let (_, glob_magic) = git_sel(vec![
        w("git"),
        w("show"),
        w("HEAD"),
        w("--"),
        wq(":(glob).gem/*"),
    ]);
    assert!(
        glob_magic
            .iter()
            .any(|r| matches!(r.outcome, PartOutcome::StoreHit(_)))
    );
    assert!(glob_magic.iter().any(|r| matches!(
        r.outcome,
        PartOutcome::Unread(UnreadReason::GitWildcardPathspec { .. })
    )));

    // `:(literal).gem/*` disables wildcards (StoreHit for magic, no GitWildcardPathspec).
    let (_, lit_magic) = git_sel(vec![
        w("git"),
        w("show"),
        w("HEAD"),
        w("--"),
        wq(":(literal).gem/*"),
    ]);
    assert!(
        lit_magic
            .iter()
            .any(|r| matches!(r.outcome, PartOutcome::StoreHit(_)))
    );
    assert!(!lit_magic.iter().any(|r| matches!(
        r.outcome,
        PartOutcome::Unread(UnreadReason::GitWildcardPathspec { .. })
    )));
}

// ---------------------------------------------------------------------------
// 9. §8 Budget exhaustion tests
// ---------------------------------------------------------------------------

#[test]
fn classify_unit_and_git_selection_respect_compute_and_alloc_budgets() {
    let cases = [
        unit_strs(&["cat", "README.md"]),
        unit_strs(&["rg", "-L", "-g", "a", "needle", "src"]),
        unit_strs(&["python", "build.py"]),
        unit_strs(&["sed", "s/error/warn/g", "file.txt"]),
        unit_strs(&["git", "show", "HEAD:.gem/credentials"]),
        unit_strs(&["cp", "-R", "src", "dst"]),
    ];

    for u in &cases {
        let mut zero_compute = ComputeBudget::new(0, 1024 * 1024);
        let err = classify_unit(u, &FoldRules::REV4, &mut zero_compute).unwrap_err();
        assert_eq!(err.reason, ExhaustReason::ComputeBudget);

        let mut zero_alloc = ComputeBudget::new(100_000, 0);
        let err = classify_unit(u, &FoldRules::REV4, &mut zero_alloc).unwrap_err();
        assert_eq!(err.reason, ExhaustReason::AllocationBudget);
    }

    // `check_git_selection` exhaustion on entry.
    let git_u = unit_strs(&["git", "show", "HEAD", "--", ".gem/credentials"]);
    let mut ok_budget = ComputeBudget::with_default_limits();
    let eff = classify_unit(&git_u, &FoldRules::REV4, &mut ok_budget).unwrap();

    let mut tight_compute = ComputeBudget::new(1, 1024 * 1024);
    let out_c = check_git_selection(&git_u, &eff, &FoldRules::REV4, &mut tight_compute);
    assert_eq!(
        out_c.exhausted.expect("compute exhausted").reason,
        ExhaustReason::ComputeBudget
    );

    let mut tight_alloc = ComputeBudget::new(100_000, 1);
    let out_a = check_git_selection(&git_u, &eff, &FoldRules::REV4, &mut tight_alloc);
    assert_eq!(
        out_a.exhausted.expect("alloc exhausted").reason,
        ExhaustReason::AllocationBudget
    );

    // Finding 4: `check_git_selection` retains earlier StoreHits when a later
    // operand exhausts the compute budget (`git show -- ':!x' y`).
    let git_two = make_unit(vec![w("git"), w("show"), w("--"), wq(":!x"), w("y")]);
    let mut ok_b2 = ComputeBudget::with_default_limits();
    let eff2 = classify_unit(&git_two, &FoldRules::REV4, &mut ok_b2).unwrap();
    let mut saw_partial_hit_with_exhausted = false;
    for units in 0..=40u64 {
        let mut b = ComputeBudget::new(units, 1024 * 1024);
        let out = check_git_selection(&git_two, &eff2, &FoldRules::REV4, &mut b);
        let has_hit = out
            .results
            .iter()
            .any(|r| matches!(r.outcome, PartOutcome::StoreHit(_)));
        if has_hit && let Some(ex) = out.exhausted {
            saw_partial_hit_with_exhausted = true;
            assert_eq!(ex.reason, ExhaustReason::ComputeBudget);
        }
    }
    assert!(
        saw_partial_hit_with_exhausted,
        "expected a budget boundary where StoreHit on ':!x' is retained alongside exhaustion on 'y'"
    );

    // Finding 12: program pinning charges before every scan and never rescans
    // a long named script without budget (`python <long_script>`).
    let long_script = "a".repeat(2048);
    let py_long = unit_strs(&["python", &long_script]);
    let mut full_b = ComputeBudget::with_default_limits();
    let full_eff = classify_unit(&py_long, &FoldRules::REV4, &mut full_b).unwrap();
    assert!(matches!(
        full_eff.unread.as_slice(),
        [UnreadReason::Program { .. }]
    ));
    let full_units = crate::policy::limits::COMPUTE_BUDGET_UNITS - full_b.remaining_units();
    // Every budget strictly below `full_units` must fail with ComputeBudget
    // (including right after initial validation, before cloning program_words).
    for probe in [
        full_units.saturating_sub(1),
        full_units.saturating_sub(long_script.len() as u64),
        full_units.saturating_sub(long_script.len() as u64 * 2),
    ] {
        let mut b = ComputeBudget::new(probe, 1024 * 1024);
        let err = classify_unit(&py_long, &FoldRules::REV4, &mut b).unwrap_err();
        assert_eq!(err.reason, ExhaustReason::ComputeBudget);
    }
}

// ---------------------------------------------------------------------------
// 10. Round 4 / D-156 regression tests (no option-value consumption & Review 3)
// ---------------------------------------------------------------------------

#[test]
fn d156_git_trigger_detection_never_consumes_option_values() {
    // Non-triggering global forms: no word or short-option letter matches `-c`,
    // `--config-env`, or `--exec-path`.
    for no_trig in [
        &["git", "-C", "src", "status"][..],
        &["git", "--git-dir=repo-git", "status"][..],
        &["git", "--attr-source", "HEAD", "status"][..],
    ] {
        let e = classify(no_trig);
        assert_eq!(e.subcommand, Some("status"), "{no_trig:?}");
        assert!(
            e.unread.is_empty(),
            "unexpected trigger on {no_trig:?}: {:?}",
            e.unread
        );
        assert!(!e.home_readonly_eligible, "{no_trig:?}");
        assert!(e.writes, "{no_trig:?}");
    }

    // D-156: Round 2's `git -Csrc` finding is closed by never consuming option
    // values — any `c` inside a short-option cluster or any `-c` word fires.
    for trig in [
        &["git", "-Csrc", "status"][..],
        &["git", "-pCsrc", "status"][..],
        &["git", "-PCsrc", "status"][..],
        &["git", "-C", "-c", "status"][..],
        &["git", "--git-dir", "-c", "status"][..],
        &["git", "-C", "--", "-c", "x=y", "status"][..],
        &["git", "-c", "x=y", "status"][..],
        &["git", "-cx=y", "status"][..],
        &["git", "-pc", "x=y", "status"][..],
        &["git", "-pcx=y", "status"][..],
        &["git", "--config-env", "k=v", "status"][..],
        &["git", "--config-env=k=v", "status"][..],
        &["git", "--exec-path=alt-exec", "status"][..],
        &["git", "--exec-path", "status"][..],
    ] {
        let e = classify(trig);
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode { tool: "git".into() }],
            "expected trigger on {trig:?}"
        );
    }

    // Leading global option before subcommand triggers & derived writers still fires.
    let global_merge_index = classify(&["git", "-C", "repo", "merge-index", "prog", "file"]);
    assert_eq!(
        global_merge_index.unread,
        vec![UnreadReason::CodeRunnerInlineCode {
            tool: "git merge-index".into()
        }]
    );
    let global_diff_ext = classify(&["git", "-C", "repo", "diff", "--unified", "--ext-diff"]);
    assert_eq!(
        global_diff_ext.unread,
        vec![UnreadReason::CodeRunnerInlineCode {
            tool: "git diff".into()
        }]
    );
    let global_clone = classify(&["git", "-C", "repo", "clone", "url", "dir"]);
    assert_eq!(
        global_clone.unread,
        vec![UnreadReason::DerivedDestinations {
            tool: "git clone".into()
        }]
    );

    // Non-triggering subcommand forms (no trigger word or letter before unconsumed `--`).
    for sub_safe in [
        &["git", "diff", "--unified"][..],
        &["git", "diff", "--stat"][..],
        &["git", "diff", "--", "--ext-diff"][..],
        &["git", "diff", "--unified=3", "--", "--ext-diff"][..],
        &["git", "rebase", "-X", "ours", "main"][..],
        &["git", "rebase", "--onto", "main"][..],
        &["git", "difftool", "-t", "vimdiff"][..],
        &["git", "difftool", "--tool=vimdiff"][..],
        &["git", "mergetool", "-t", "vimdiff"][..],
        &["git", "push", "-u", "origin", "main"][..],
    ] {
        let e = classify(sub_safe);
        assert!(
            e.unread.is_empty(),
            "unexpected trigger on {sub_safe:?}: {:?}",
            e.unread
        );
    }

    // Review 3 Finding 3 (`git diff --unified --ext-diff`) and all D-156
    // subcommand triggers (no option value consumption; cluster letters fire).
    for (sub_trig, expected_tool) in [
        (&["git", "diff", "--unified", "--ext-diff"][..], "git diff"),
        (&["git", "diff", "-S", "--ext-diff"][..], "git diff"),
        (&["git", "diff", "-S", "--", "--ext-diff"][..], "git diff"),
        (&["git", "push", "-o", "--exec"][..], "git push"),
        (&["git", "rebase", "-Xcontext", "main"][..], "git rebase"),
        (&["git", "rebase", "-S0x1234", "main"][..], "git rebase"),
        (&["git", "rebase", "-sxyz", "main"][..], "git rebase"),
        (&["git", "rebase", "-s", "-x", "main"][..], "git rebase"),
        (&["git", "rebase", "--onto", "-x", "main"][..], "git rebase"),
        (&["git", "rebase", "-x", "id", "main"][..], "git rebase"),
        (&["git", "rebase", "-xid", "main"][..], "git rebase"),
        (&["git", "rebase", "-ix", "id", "main"][..], "git rebase"),
        (&["git", "rebase", "-ixid", "main"][..], "git rebase"),
        (
            &["git", "rebase", "-S", "-x", "id", "main"][..],
            "git rebase",
        ),
        (
            &["git", "rebase", "-r", "-x", "id", "main"][..],
            "git rebase",
        ),
        (&["git", "difftool", "-txxdiff"][..], "git difftool"),
        (&["git", "difftool", "-t", "-x"][..], "git difftool"),
        (&["git", "difftool", "-ytxxdiff"][..], "git difftool"),
        (&["git", "difftool", "-x", "id"][..], "git difftool"),
        (&["git", "difftool", "-xid"][..], "git difftool"),
        (&["git", "difftool", "-yx", "id"][..], "git difftool"),
        (&["git", "mergetool", "-txxdiff"][..], "git mergetool"),
        (&["git", "mergetool", "-x", "id"][..], "git mergetool"),
        (&["git", "mergetool", "-xid"][..], "git mergetool"),
        (&["git", "mergetool", "-yx", "id"][..], "git mergetool"),
    ] {
        let e = classify(sub_trig);
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode {
                tool: expected_tool.into()
            }],
            "expected trigger on {sub_trig:?}"
        );
    }

    // `git -C --literal-pathspecs show HEAD -- ':!.env'` must NOT honor
    // `--literal-pathspecs` after an unmodeled global option (`-C`).
    let (_, sel) = git_sel(vec![
        w("git"),
        w("-C"),
        w("--literal-pathspecs"),
        w("show"),
        w("HEAD"),
        w("--"),
        wq(":!.env"),
    ]);
    assert!(
        sel.iter()
            .any(|r| matches!(r.outcome, PartOutcome::StoreHit(_))),
        "expected StoreHit when `--literal-pathspecs` follows `-C`, got {sel:?}"
    );

    // Leading `-C status show HEAD:.env` and `-C branch status -- '*.rs'` must
    // still run CR-12 name selection and wildcard-pathspec checks.
    let (_, sel_show) = git_sel(vec![
        w("git"),
        w("-C"),
        w("status"),
        w("show"),
        w("HEAD:.env"),
    ]);
    assert!(
        sel_show
            .iter()
            .any(|r| matches!(r.outcome, PartOutcome::StoreHit(_))),
        "expected StoreHit on `git -C status show HEAD:.env`, got {sel_show:?}"
    );
    let (_, sel_wc) = git_sel(vec![
        w("git"),
        w("-C"),
        w("branch"),
        w("status"),
        w("--"),
        wq("*.rs"),
    ]);
    assert!(
        sel_wc.iter().any(|r| matches!(
            r.outcome,
            PartOutcome::Unread(UnreadReason::GitWildcardPathspec { .. })
        )),
        "expected GitWildcardPathspec on `git -C branch status -- '*.rs'`, got {sel_wc:?}"
    );
}

#[test]
fn d156_arities_stay_only_in_frozen_table_and_review3_findings_1_and_2() {
    // 1. Search tools (`rg` / `grep`): flag arities stay in the frozen table (D-156),
    //    so attached/separate values of listed value-taking flags still keep Home eligibility.
    for home_ok in [
        vec![w("rg"), w("-efoo"), w("src")],
        vec![w("rg"), w("-ezip"), w("src")],
        vec![w("rg"), wq("-g*.pdf"), w("needle"), w("src")],
        vec![w("rg"), w("-rfoo"), w("needle"), w("src")],
        vec![w("rg"), w("-Eutf-8"), w("needle"), w("src")],
        vec![w("rg"), w("-e"), w("-f"), w("src")],
        vec![w("grep"), w("-efoo"), w("README.md")],
        vec![w("grep"), w("-e"), w("-f"), w("README.md")],
    ] {
        let e = classify_words(home_ok.clone());
        assert!(
            e.unread.is_empty(),
            "unexpected unread on {home_ok:?}: {:?}",
            e.unread
        );
        assert!(
            e.home_readonly_eligible,
            "frozen Search flag table should keep Home eligibility on {home_ok:?}"
        );
    }
    for home_no in [
        &["rg", "-f", "pats.txt", "src"][..],
        &["rg", "-if", "pats.txt", "src"][..],
        &["rg", "-z", "needle", "src"][..],
        &["grep", "-f", "pats.txt", "README.md"][..],
        &["grep", "-if", "pats.txt", "README.md"][..],
    ] {
        let e = classify(home_no);
        assert!(
            !e.home_readonly_eligible,
            "expected !home_readonly_eligible on {home_no:?}"
        );
    }

    // 2. DerivedDestinationWriter `cp` (D-156): no arity table; clean forms stay
    //    clear, while any trigger word or short-option cluster containing `r`/`R`/`a` fires.
    for cp_no_trig in [
        &["cp", "-t", "archive", "a.txt"][..],
        &["cp", "-S", ".old", "a.txt", "b.txt"][..],
        &["cp", "--target-directory=dir", "a.txt"][..],
        &["cp", "--", "-R", "a.txt", "b.txt"][..],
    ] {
        let e = classify(cp_no_trig);
        assert_eq!(
            e.derived_destinations,
            DerivedDestinations::None,
            "{cp_no_trig:?}"
        );
        assert!(e.unread.is_empty(), "{cp_no_trig:?}");
    }
    for cp_trig in [
        &["cp", "-tarchive", "a.txt"][..],
        &["cp", "-t", "-r", "a.txt"][..],
        &["cp", "-vtdir", "a.txt"][..],
        &["cp", "-S.bak", "a.txt", "b.txt"][..],
        &["cp", "-S.orig", "a.txt", "b.txt"][..],
        &["cp", "-S", "-a", "a.txt", "b.txt"][..],
        &["cp", "--target-directory", "-R", "a.txt"][..],
        &["cp", "-t", "--", "-R", "a.txt"][..],
        &["cp", "-Rtdir", "src"][..],
        &["cp", "-rtdir", "src"][..],
        &["cp", "-atdir", "src"][..],
        &["cp", "-tdir", "-R", "src"][..],
    ] {
        let e = classify(cp_trig);
        assert_eq!(
            e.derived_destinations,
            DerivedDestinations::Always,
            "{cp_trig:?}"
        );
        assert_eq!(
            e.unread,
            vec![UnreadReason::DerivedDestinations { tool: "cp".into() }],
            "{cp_trig:?}"
        );
    }

    // 3. DerivedDestinationWriter `tar` (Review 3 Finding 1 & D-156):
    //    `tar -s -x -f archive.tar` and every form with `-x`/`--extract`/`--get`
    //    or an `x` letter in a cluster triggers DerivedDestinations.
    for tar_no_trig in [
        &["tar", "-cf", "out.tar", "src"][..],
        &["tar", "-C", "dir", "-cf", "out.tar", "src"][..],
        &["tar", "cf", "out.tar", "src"][..],
        &["tar", "--file=out.tar", "-c", "src"][..],
        &["tar", "-cf", "out.tar", "--", "-x"][..],
    ] {
        let e = classify(tar_no_trig);
        assert_eq!(
            e.derived_destinations,
            DerivedDestinations::None,
            "{tar_no_trig:?}"
        );
        assert!(e.unread.is_empty(), "{tar_no_trig:?}");
    }
    for tar_trig in [
        &["tar", "-s", "-x", "-f", "archive.tar"][..],
        &["tar", "-cfextra.tar", "src"][..],
        &["tar", "-cf", "-x", "src"][..],
        &["tar", "-Cextra", "-cf", "out.tar", "src"][..],
        &["tar", "-C", "-x", "-cf", "out.tar", "src"][..],
        &["tar", "-cf", "out.tar", "-Textra.list"][..],
        &["tar", "-cf", "out.tar", "-Xexclude.txt"][..],
        &["tar", "cf", "-x", "src"][..],
        &["tar", "cf", "--", "-x", "src"][..],
        &["tar", "-f", "--", "-x", "src"][..],
        &["tar", "--file", "--extract", "-c", "src"][..],
        &["tar", "-zxf", "a.tar.gz"][..],
        &["tar", "-zxfa.tar.gz"][..],
        &["tar", "-fa.tar", "-x"][..],
        &["tar", "-f", "a.tar", "-x"][..],
        &["tar", "--file=a.tar", "-x"][..],
        &["tar", "--file", "a.tar", "-x"][..],
        &["tar", "zxf", "a.tar.gz"][..],
    ] {
        let e = classify(tar_trig);
        assert_eq!(
            e.derived_destinations,
            DerivedDestinations::Always,
            "{tar_trig:?}"
        );
        assert_eq!(
            e.unread,
            vec![UnreadReason::DerivedDestinations { tool: "tar".into() }],
            "{tar_trig:?}"
        );
    }

    // 4. `awk` / `gawk` / `mawk` (Review 3 Finding 2 & D-156):
    //    Clean pinned forms retain `Unread(Program)`. Any competing code form
    //    (`-W…` attached or separate, `--source`, `-e`, `-E`, `-f`, `-i`, `-l`,
    //    or cluster containing any of those letters) yields plain `CodeRunnerInlineCode`.
    for (awk_pinned, expected_prog) in [
        (
            &["gawk", "-f", "prog.awk", "data.txt"][..],
            vec!["gawk", "-f", "prog.awk"],
        ),
        (
            &["awk", "-f", "prog.awk", "-v", "k=1", "data.txt"][..],
            vec!["awk", "-f", "prog.awk"],
        ),
        (
            &["awk", "-f", "prog.awk", "-F", ",", "data.txt"][..],
            vec!["awk", "-f", "prog.awk"],
        ),
        (
            &["awk", "-f", "prog.awk", "-F:", "data.txt"][..],
            vec!["awk", "-f", "prog.awk"],
        ),
        (
            &["gawk", "-f", "prog.awk", "--", "-Wsource=BEGIN{print 1}"][..],
            vec!["gawk", "-f", "prog.awk"],
        ),
    ] {
        let e = classify(awk_pinned);
        assert_eq!(
            e.unread,
            vec![UnreadReason::Program {
                program_words: expected_prog.into_iter().map(String::from).collect()
            }],
            "expected pinned program on {awk_pinned:?}"
        );
    }
    for (awk_unpinned, expected_tool) in [
        (
            &["gawk", "-f", "prog.awk", "-Wsource=BEGIN{print 1}"][..],
            "gawk",
        ),
        (
            &["gawk", "-f", "prog.awk", "-W", "source=BEGIN{print 1}"][..],
            "gawk",
        ),
        (
            &["gawk", "-f", "prog.awk", "-Wcompat", "data.txt"][..],
            "gawk",
        ),
        (
            &["gawk", "-f", "prog.awk", "--source=BEGIN{print 1}"][..],
            "gawk",
        ),
        (&["gawk", "-f", "prog.awk", "-i", "lib.awk"][..], "gawk"),
        (&["gawk", "-f", "prog.awk", "-l", "ext"][..], "gawk"),
        (&["gawk", "-f", "prog.awk", "-E", "other.awk"][..], "gawk"),
        (&["gawk", "-f", "-Wsource=1"][..], "gawk"),
        (&["awk", "-f", "prog.awk", "-Fsep", "data.txt"][..], "awk"),
        (
            &["awk", "-f", "prog.awk", "-F", "-e", "data.txt"][..],
            "awk",
        ),
        (
            &["awk", "-f", "prog.awk", "-F", "--", "-Wsource=1"][..],
            "awk",
        ),
        (&["awk", "-f", "prog.awk", "-vfoo=1", "data.txt"][..], "awk"),
        (
            &[
                "awk",
                "-f",
                "prog.awk",
                "--field-separator",
                "-e",
                "data.txt",
            ][..],
            "awk",
        ),
        (
            &["awk", "-f", "prog.awk", "-nf", "other.awk", "data.txt"][..],
            "awk",
        ),
        (
            &["awk", "-f", "prog.awk", "-ne", "{print}", "data.txt"][..],
            "awk",
        ),
        (
            &["awk", "-f", "prog.awk", "-e{print}", "data.txt"][..],
            "awk",
        ),
    ] {
        let e = classify(awk_unpinned);
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode {
                tool: expected_tool.into()
            }],
            "expected plain inline code on {awk_unpinned:?}"
        );
    }

    // 5. `sed` (D-156): no `-i` / `-l` arity modelling; `-i.ef`, `-i.ref`, `-l`,
    //    and `-l80` all fail closed to plain CodeRunnerInlineCode.
    for sed_unread in [
        vec![w("sed"), w("-i.ef"), wq("s/a/b/"), w("a.txt")],
        vec![w("sed"), w("-f"), w("edit.sed"), w("-i.ref"), w("a.txt")],
        vec![w("sed"), w("-l"), wq("w out"), w("p"), w("a.txt")],
        vec![w("sed"), w("-l80"), wq("s/a/b/"), w("a.txt")],
        vec![w("sed"), w("-f"), w("edit.sed"), w("-l80"), w("a.txt")],
        vec![w("sed"), w("-f"), w("-e"), w("a.txt")],
    ] {
        let e = classify_words(sed_unread.clone());
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode { tool: "sed".into() }],
            "expected Unread on {sed_unread:?}"
        );
    }
    let sed_f_n = classify(&["sed", "-f", "edit.sed", "-n", "a.txt"]);
    assert_eq!(
        sed_f_n.unread,
        vec![UnreadReason::Program {
            program_words: vec!["sed".into(), "-f".into(), "edit.sed".into()]
        }]
    );

    // 6. `find` (D-156): never consumes option values, so `-exec` anywhere fires.
    for find_trig in [
        &["find", ".", "-name", "-exec"][..],
        &["find", ".", "-name", "foo", "-exec", "id", ";"][..],
    ] {
        let e = classify(find_trig);
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode {
                tool: "find".into()
            }],
            "{find_trig:?}"
        );
    }

    // 7. Interpreters (`python`, `node`, `perl`, `ruby`; D-156): no short-option
    //    arity modelling; separate `-c` still locates code_operands, and any
    //    cluster containing an inline flag letter records that token as inline code.
    let py_sep = classify(&["python", "-W", "once", "-c", "print(1)"]);
    assert_eq!(
        py_sep.code,
        CodeExecution::Inline {
            code_operands: vec![4]
        }
    );
    let py_cluster = classify(&["python", "-ucprint(1)"]);
    assert_eq!(
        py_cluster.code,
        CodeExecution::Inline {
            code_operands: vec![1]
        }
    );
    for (interp_cmd, expected_tool, expected_idx) in [
        (&["python", "-Xtracemalloc", "script.py"][..], "python", 1),
        (&["python3", "-Wonce", "script.py"][..], "python3", 1),
        (&["node", "-rexpress", "app.js"][..], "node", 1),
        // `-Isrc/core` ends with `e`, so without `-I` arity it reads the
        // following word (`script.pl`, index 2) as `-e`'s operand.
        (&["perl", "-Isrc/core", "script.pl"][..], "perl", 2),
        (&["ruby", "-Ilib/ext", "app.rb"][..], "ruby", 1),
    ] {
        let e = classify(interp_cmd);
        assert_eq!(
            e.unread,
            vec![UnreadReason::CodeRunnerInlineCode {
                tool: expected_tool.into()
            }],
            "{interp_cmd:?}"
        );
        assert_eq!(
            e.code,
            CodeExecution::Inline {
                code_operands: vec![expected_idx]
            },
            "D-156 cluster letter should record operand {expected_idx} on {interp_cmd:?}"
        );
    }
}
