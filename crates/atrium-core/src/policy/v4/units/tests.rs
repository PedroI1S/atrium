//! Unit tests for P1 unit builder, §4 wrapper unwrapping, §4 nested-shell
//! recursion, §3 `HOME` tracking, §8 budgets, and §10 contract rows.

use super::*;
use crate::policy::limits;
use crate::policy::model::{
    Cap, ComputeBudget, ExhaustReason, FoldRules, PartId, PartOutcome, PipelineId, Separator,
    UnitIndex, UnreadReason, WrapperKind,
};
use crate::policy::v4::lex::lex_exact;

fn unreads_of(parsed: &ParsedCommandUnits) -> Vec<&UnreadReason> {
    parsed
        .outcomes
        .iter()
        .filter_map(|pr| match &pr.outcome {
            PartOutcome::Unread(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn has_exhausted(parsed: &ParsedCommandUnits) -> bool {
    parsed
        .outcomes
        .iter()
        .any(|pr| matches!(pr.outcome, PartOutcome::Exhausted(_)))
}

// ---------------------------------------------------------------------------
// §10 rows decided by P1 (`build_units`)
// ---------------------------------------------------------------------------

#[test]
fn contract_section_10_p1_rows() {
    // Row 3: `SUDO id`, `/usr/bin/sudo id`
    for line in ["SUDO id", "/usr/bin/sudo id"] {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            unreads_of(&parsed).is_empty(),
            "{line}: {:?}",
            parsed.outcomes
        );
        assert_eq!(parsed.units.len(), 1);
        let folded = fold_command_word(&parsed.units[0].argv[0], &FoldRules::REV4, &mut b).unwrap();
        assert_eq!(folded, "sudo");
    }

    // Row 4: `sh -c 'X="$1" sudo id' _ foo` -> Unread in exact model, derived
    // `sudo id` unit in best-effort pass.
    let mut b = ComputeBudget::with_default_limits();
    let r4 = build_units("sh -c 'X=\"$1\" sudo id' _ foo", &FoldRules::REV4, &mut b).unwrap();
    assert!(
        unreads_of(&r4).iter().any(
            |r| matches!(r, UnreadReason::UnsupportedNestedShellForm { shell } if shell == "sh")
        ),
        "{:?}",
        r4.outcomes
    );
    assert_eq!(r4.units.len(), 1, "must not model-expand K7 script");
    assert!(
        r4.best_effort
            .units
            .iter()
            .any(|u| u.derived && u.effective_argv == ["sudo", "id"]),
        "{:?}",
        r4.best_effort.units
    );

    // Row 5: `command` x13 `sudo id` -> `MaxWrapReached` in exact model,
    // `sudo id` in best-effort pass.
    let row5 = format!("{}sudo id", "command ".repeat(13));
    let mut b = ComputeBudget::with_default_limits();
    let r5 = build_units(&row5, &FoldRules::REV4, &mut b).unwrap();
    assert!(
        unreads_of(&r5)
            .iter()
            .any(|r| matches!(r, UnreadReason::MaxWrapReached)),
        "{:?}",
        r5.outcomes
    );
    assert_eq!(r5.units[0].removed_wrappers.len(), limits::MAX_WRAP);
    assert_eq!(r5.best_effort.units[0].effective_argv, ["sudo", "id"]);

    // Row 6: `cat $'\xff'` -> Unread (`$`).
    let mut b = ComputeBudget::with_default_limits();
    let r6 = build_units(r"cat $'\xff'", &FoldRules::REV4, &mut b).unwrap();
    assert!(
        unreads_of(&r6)
            .iter()
            .any(|r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte: b'$' }))
    );

    // Row 7: `for f in *.rs; do wc -l "$f"; done` -> Unread (reserved word).
    let mut b = ComputeBudget::with_default_limits();
    let r7 = build_units(
        "for f in *.rs; do wc -l \"$f\"; done",
        &FoldRules::REV4,
        &mut b,
    )
    .unwrap();
    assert!(unreads_of(&r7).iter().any(|r| matches!(
        r,
        UnreadReason::ReservedWordOrConstruct { construct } if construct == "for"
    )));

    // Row 10: `cat /h/u/[!é]trium/secret` -> non-ASCII unquoted byte.
    let mut b = ComputeBudget::with_default_limits();
    let r10 = build_units("cat /h/u/[!é]trium/secret", &FoldRules::REV4, &mut b).unwrap();
    assert!(
        unreads_of(&r10)
            .iter()
            .any(|r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte } if *byte >= 0x80))
    );

    // Row 20: > `MAX_UNITS` -> `TooLarge(Cap::Units)`.
    let row20 = "cat a; ".repeat(600);
    let mut b = ComputeBudget::with_default_limits();
    assert_eq!(
        build_units(&row20, &FoldRules::REV4, &mut b),
        Err(PartOutcome::TooLarge(Cap::Units))
    );

    // Row 25: `cat <&3` -> `DescriptorOutsideStd { fd: 3 }`.
    let mut b = ComputeBudget::with_default_limits();
    let r25 = build_units("cat <&3", &FoldRules::REV4, &mut b).unwrap();
    assert!(
        unreads_of(&r25)
            .iter()
            .any(|r| matches!(r, UnreadReason::DescriptorOutsideStd { fd: 3 }))
    );
}

// ---------------------------------------------------------------------------
// §4 Wrappers: every modeled form, near misses, MAX_WRAP, outside wrappers
// ---------------------------------------------------------------------------

#[test]
fn unwrap_unit_accepts_every_closed_wrapper_form() {
    let cases: &[(&str, &[WrapperKind], &[&[u8]])] = &[
        ("command cat a", &[WrapperKind::Command], &[b"cat", b"a"]),
        ("command -- cat a", &[WrapperKind::Command], &[b"cat", b"a"]),
        ("exec cat a", &[WrapperKind::Exec], &[b"cat", b"a"]),
        ("nice cat a", &[WrapperKind::Nice], &[b"cat", b"a"]),
        ("nice -n 10 cat a", &[WrapperKind::Nice], &[b"cat", b"a"]),
        ("nice -n -5 cat a", &[WrapperKind::Nice], &[b"cat", b"a"]),
        ("nice -n +5 cat a", &[WrapperKind::Nice], &[b"cat", b"a"]),
        ("timeout 5 cat a", &[WrapperKind::Timeout], &[b"cat", b"a"]),
        (
            "timeout 0.5s cat a",
            &[WrapperKind::Timeout],
            &[b"cat", b"a"],
        ),
        ("timeout 2m cat a", &[WrapperKind::Timeout], &[b"cat", b"a"]),
        (
            "timeout -s KILL 5 cat a",
            &[WrapperKind::Timeout],
            &[b"cat", b"a"],
        ),
        (
            "timeout -k 2 5 cat a",
            &[WrapperKind::Timeout],
            &[b"cat", b"a"],
        ),
        (
            "timeout -s TERM -k 2s 10m cat a",
            &[WrapperKind::Timeout],
            &[b"cat", b"a"],
        ),
        (
            "timeout -k 2 -s 9 5 cat a",
            &[WrapperKind::Timeout],
            &[b"cat", b"a"],
        ),
        ("time cat a", &[WrapperKind::Time], &[b"cat", b"a"]),
        ("env cat a", &[WrapperKind::Env], &[b"cat", b"a"]),
        (
            "COMMAND NICE -n 5 timeout -s TERM 10 time /usr/bin/ENV cat a",
            &[
                WrapperKind::Command,
                WrapperKind::Nice,
                WrapperKind::Timeout,
                WrapperKind::Time,
                WrapperKind::Env,
            ],
            &[b"cat", b"a"],
        ),
    ];

    for &(line, want_kinds, want_argv) in cases {
        let mut b = ComputeBudget::with_default_limits();
        let lexed = lex_exact(line, 0, None, &mut b).unwrap();
        let res = unwrap_unit(
            lexed.units.into_iter().next().unwrap(),
            &FoldRules::REV4,
            &mut b,
        )
        .unwrap();
        assert_eq!(res.unread, None, "line `{line}`");
        let got_kinds: Vec<WrapperKind> =
            res.unit.removed_wrappers.iter().map(|w| w.kind).collect();
        assert_eq!(got_kinds, want_kinds, "line `{line}`");
        let got_argv: Vec<&[u8]> = res.unit.argv.iter().map(Word::literal_bytes).collect();
        assert_eq!(got_argv, want_argv, "line `{line}`");
    }
}

#[test]
fn unwrap_unit_rejects_wrapper_near_misses_and_outside_wrappers() {
    let near_misses = [
        ("command", "command"),
        ("command --", "command"),
        ("command -v cat", "command"),
        ("command -V cat", "command"),
        ("command -p cat a", "command"),
        ("exec", "exec"),
        ("exec -a foo cat a", "exec"),
        ("exec -c cat a", "exec"),
        ("exec -l cat a", "exec"),
        ("exec -- cat a", "exec"),
        ("nice", "nice"),
        ("nice -n 10", "nice"),
        ("nice -10 cat a", "nice"),
        ("nice -n10 cat a", "nice"),
        ("nice --adjustment=10 cat a", "nice"),
        ("nice -n foo cat a", "nice"),
        ("nice -n 5 -n 10 cat a", "nice"),
        ("nice -- cat a", "nice"),
        ("timeout", "timeout"),
        ("timeout 5", "timeout"),
        ("timeout -v 5 cat a", "timeout"),
        ("timeout --foreground 5 cat a", "timeout"),
        ("timeout --signal=KILL 5 cat a", "timeout"),
        ("timeout -sKILL 5 cat a", "timeout"),
        ("timeout -k2 5 cat a", "timeout"),
        ("timeout -s TERM -s KILL 5 cat a", "timeout"),
        ("timeout -k 1 -k 2 5 cat a", "timeout"),
        ("timeout -- 5 cat a", "timeout"),
        ("timeout 5 -v cat a", "timeout"),
        ("timeout abc cat a", "timeout"),
        ("time", "time"),
        ("time -p cat a", "time"),
        ("time -v cat a", "time"),
        ("time -- cat a", "time"),
        ("env", "env"),
        ("env -i cat a", "env"),
        ("env -C /tmp cat a", "env"),
        ("env -S 'sh -c' cat a", "env"),
        ("env -u HOME cat a", "env"),
        ("env X=1 cat a", "env"),
        ("env 'X=1' cat a", "env"),
        ("env -- cat a", "env"),
        // Outside-model wrappers in P0's table.
        ("nohup cat a", "nohup"),
        ("builtin echo hi", "builtin"),
        ("busybox cat a", "busybox"),
        ("chroot /tmp cat a", "chroot"),
        ("arch -arm64 cat a", "arch"),
        ("script -c 'cat a' /dev/null", "script"),
        ("flock /tmp/l cat a", "flock"),
        ("stdbuf -o0 cat a", "stdbuf"),
    ];

    for (line, want_wrapper) in near_misses {
        let mut b = ComputeBudget::with_default_limits();
        let lexed = lex_exact(line, 0, None, &mut b).unwrap();
        let res = unwrap_unit(
            lexed.units.into_iter().next().unwrap(),
            &FoldRules::REV4,
            &mut b,
        )
        .unwrap();
        assert_eq!(
            res.unread,
            Some(UnreadReason::UnsupportedWrapperForm {
                wrapper: want_wrapper.to_string()
            }),
            "line `{line}`"
        );
    }
}

#[test]
fn unwrap_unit_max_wrap_boundary() {
    // Exactly `MAX_WRAP` (8) wrappers before `cat a`: allowed.
    let ok8 = format!("{}cat a", "command ".repeat(limits::MAX_WRAP));
    let mut b = ComputeBudget::with_default_limits();
    let lexed = lex_exact(&ok8, 0, None, &mut b).unwrap();
    let res = unwrap_unit(
        lexed.units.into_iter().next().unwrap(),
        &FoldRules::REV4,
        &mut b,
    )
    .unwrap();
    assert_eq!(res.unread, None);
    assert_eq!(res.unit.removed_wrappers.len(), limits::MAX_WRAP);
    assert_eq!(res.unit.argv[0].literal_bytes(), b"cat");

    // `MAX_WRAP + 1` (9) wrappers: `MaxWrapReached`.
    let fail9 = format!("{}cat a", "command ".repeat(limits::MAX_WRAP + 1));
    let mut b = ComputeBudget::with_default_limits();
    let lexed = lex_exact(&fail9, 0, None, &mut b).unwrap();
    let res = unwrap_unit(
        lexed.units.into_iter().next().unwrap(),
        &FoldRules::REV4,
        &mut b,
    )
    .unwrap();
    assert_eq!(res.unread, Some(UnreadReason::MaxWrapReached));
    assert_eq!(res.unit.removed_wrappers.len(), limits::MAX_WRAP);
}

#[test]
fn build_units_flags_command_word_globs() {
    for line in [
        "/usr/bin/sud[o] id",
        "c*t README.md",
        "command /usr/bin/sud[o] id",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            unreads_of(&parsed)
                .iter()
                .any(|r| matches!(r, UnreadReason::CommandWordGlob)),
            "line `{line}`: {:?}",
            parsed.outcomes
        );
    }

    // Quoted glob in command word is literal (not `CommandWordGlob`).
    let mut b = ComputeBudget::with_default_limits();
    let parsed = build_units("'/usr/bin/sud[o]' id", &FoldRules::REV4, &mut b).unwrap();
    assert!(
        !unreads_of(&parsed)
            .iter()
            .any(|r| matches!(r, UnreadReason::CommandWordGlob))
    );
}

// ---------------------------------------------------------------------------
// §4 Nested shells: modeled forms, MAX_DEPTH, script files, near misses
// ---------------------------------------------------------------------------

#[test]
fn build_units_nested_shells_modeled_and_depth_limit() {
    for line in [
        "sh -c 'cat README.md'",
        "bash -c 'cat README.md'",
        "dash -c 'cat README.md'",
        "sh -e -u -x -c 'cat README.md'",
        "bash -ec 'cat README.md'",
        "dash -euxc 'cat README.md'",
        "/bin/BASH -c 'cat README.md'",
        "command sh -e -c 'cat README.md'",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            unreads_of(&parsed).is_empty(),
            "{line}: {:?}",
            parsed.outcomes
        );
        assert_eq!(parsed.units.len(), 2, "{line}");
        assert_eq!(parsed.units[0].depth, 0);
        assert_eq!(parsed.units[0].parent_unit, None);
        assert_eq!(parsed.units[1].depth, 1);
        assert_eq!(parsed.units[1].parent_unit, Some(UnitIndex(0)));
        assert_eq!(parsed.units[1].argv[0].literal_bytes(), b"cat");
        assert_eq!(parsed.units[1].argv[1].literal_bytes(), b"README.md");
    }

    // Inner pipeline gets a distinct `PipelineId` from outer pipeline.
    let mut b = ComputeBudget::with_default_limits();
    let piped = build_units("echo x | sh -c 'cat a | wc -l'", &FoldRules::REV4, &mut b).unwrap();
    assert!(unreads_of(&piped).is_empty(), "{:?}", piped.outcomes);
    assert_eq!(piped.units.len(), 4);
    assert_eq!(piped.units[0].pipeline_id, Some(PipelineId(1)));
    assert_eq!(piped.units[1].pipeline_id, Some(PipelineId(1)));
    assert_eq!(piped.units[2].parent_unit, Some(UnitIndex(1)));
    assert_eq!(piped.units[3].parent_unit, Some(UnitIndex(1)));
    assert_eq!(piped.units[2].pipeline_id, piped.units[3].pipeline_id);
    assert_ne!(piped.units[2].pipeline_id, piped.units[0].pipeline_id);

    // `MAX_DEPTH` = 6 levels allowed; 7th level -> `MaxNestingDepthReached`.
    let make_nested = |levels: usize| -> String {
        let mut inner = "cat README.md".to_string();
        for _ in 0..levels {
            let escaped = inner.replace('\\', r"\\").replace('"', r#"\""#);
            inner = format!("sh -c \"{escaped}\"");
        }
        inner
    };

    let depth6 = make_nested(usize::from(limits::MAX_DEPTH));
    let mut b = ComputeBudget::with_default_limits();
    let p6 = build_units(&depth6, &FoldRules::REV4, &mut b).unwrap();
    assert!(unreads_of(&p6).is_empty(), "{:?}", p6.outcomes);
    assert_eq!(p6.units.len(), usize::from(limits::MAX_DEPTH) + 1);
    assert_eq!(p6.units.last().unwrap().depth, limits::MAX_DEPTH);

    let depth7 = make_nested(usize::from(limits::MAX_DEPTH) + 1);
    let mut b = ComputeBudget::with_default_limits();
    let p7 = build_units(&depth7, &FoldRules::REV4, &mut b).unwrap();
    assert!(
        unreads_of(&p7)
            .iter()
            .any(|r| matches!(r, UnreadReason::MaxNestingDepthReached)),
        "{:?}",
        p7.outcomes
    );
}

#[test]
fn build_units_nested_shell_script_files_and_near_misses() {
    // Script files -> `Unread (program)` (§4).
    for (line, want_words) in [
        ("sh x.sh", vec!["sh", "x.sh"]),
        ("bash ./build.sh arg1", vec!["bash", "./build.sh"]),
        ("dash run.sh", vec!["dash", "run.sh"]),
        ("/bin/sh x.sh", vec!["/bin/sh", "x.sh"]),
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        let want: Vec<String> = want_words.into_iter().map(str::to_string).collect();
        assert!(
            unreads_of(&parsed).iter().any(
                |r| matches!(r, UnreadReason::Program { program_words } if program_words == &want)
            ),
            "line `{line}`: {:?}",
            parsed.outcomes
        );
    }

    // Unsupported nested-shell forms & outside shells.
    let outside_shells = [
        ("sh", "sh"),
        ("bash", "bash"),
        ("dash", "dash"),
        ("sh -", "sh"),
        ("sh -s", "sh"),
        ("sh -c", "sh"),
        ("sh -e", "sh"),
        ("sh -ce 'echo hi'", "sh"),
        ("sh -c -e 'echo hi'", "sh"),
        ("sh -c 'echo hi' _ arg1", "sh"),
        ("sh -i -c 'echo hi'", "sh"),
        ("sh -l -c 'echo hi'", "sh"),
        ("bash --rcfile /tmp/rc -c 'echo hi'", "bash"),
        ("bash --login -c 'echo hi'", "bash"),
        ("sh -o pipefail -c 'echo hi'", "sh"),
        ("sh +e -c 'echo hi'", "sh"),
        ("sh -- -c 'echo hi'", "sh"),
        ("sh -e x.sh", "sh"),
        ("sh *.sh", "sh"),
        ("sh -c cat*", "sh"),
        ("zsh -c 'echo hi'", "zsh"),
        ("ksh -c 'echo hi'", "ksh"),
        ("fish -c 'echo hi'", "fish"),
        ("ash -c 'echo hi'", "ash"),
        ("csh -c 'echo hi'", "csh"),
        ("tcsh -c 'echo hi'", "tcsh"),
        ("mksh -c 'echo hi'", "mksh"),
        ("yash -c 'echo hi'", "yash"),
        ("rbash -c 'echo hi'", "rbash"),
        ("posh -c 'echo hi'", "posh"),
        ("elvish -c 'echo hi'", "elvish"),
        ("xonsh -c 'echo hi'", "xonsh"),
    ];
    for (line, want_shell) in outside_shells {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            unreads_of(&parsed).iter().any(|r| matches!(
                r,
                UnreadReason::UnsupportedNestedShellForm { shell } if shell == want_shell
            )),
            "line `{line}`: {:?}",
            parsed.outcomes
        );
    }
}

// ---------------------------------------------------------------------------
// §3 `HOME` modification tracking
// ---------------------------------------------------------------------------

#[test]
fn build_units_tracks_home_modifications_for_later_tildes() {
    let triggers = [
        "HOME=/tmp; cat ~/x",
        "HOME=/tmp true; cat ~/x",
        "export HOME=/tmp; cat ~/x",
        "unset HOME; cat ~/x",
        "env -u HOME true; cat ~/x",
        "env -i true; cat ~/x",
        "env HOME=/tmp true; cat ~/x",
        "HOME=/tmp; cat < ~/x",
        "HOME=/tmp; sh -c 'cat ~/x'",
        "sh -c 'HOME=/tmp; cat ~/x'",
    ];
    for line in triggers {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            unreads_of(&parsed)
                .iter()
                .any(|r| matches!(r, UnreadReason::HomeSetEarlier)),
            "line `{line}`: {:?}",
            parsed.outcomes
        );
    }

    // Negative cases: `HOME` not modified, or `~` is quoted.
    let no_home_trigger = [
        "export FOO=1; cat ~/x",
        "unset FOO; cat ~/x",
        "env true; cat ~/x",
        "HOME=/tmp; cat '~/x'",
        "HOME=/tmp; cat \"~/x\"",
    ];
    for line in no_home_trigger {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            !unreads_of(&parsed)
                .iter()
                .any(|r| matches!(r, UnreadReason::HomeSetEarlier)),
            "unexpected HomeSetEarlier for `{line}`: {:?}",
            parsed.outcomes
        );
    }
}

// ---------------------------------------------------------------------------
// P3 code-runner hook
// ---------------------------------------------------------------------------

#[test]
fn build_units_code_runner_hook_attaches_p3_reasons() {
    let mut b = ComputeBudget::with_default_limits();
    let mut hook = |_unit: &CommandUnit, folded: &str, _budget: &mut ComputeBudget| {
        if folded == "awk" {
            Ok(vec![UnreadReason::CodeRunnerInlineCode {
                tool: "awk".to_string(),
            }])
        } else {
            Ok(vec![])
        }
    };
    let parsed = build_units_with_hook(
        "awk 'BEGIN{print 1}' README.md",
        &FoldRules::REV4,
        Some(&mut hook),
        &mut b,
    )
    .unwrap();
    assert!(parsed.outcomes.iter().any(|pr| matches!(
        pr,
        PartResult {
            part: PartId::Unit(UnitIndex(0)),
            outcome: PartOutcome::Unread(UnreadReason::CodeRunnerInlineCode { tool })
        } if tool == "awk"
    )));
}

// ---------------------------------------------------------------------------
// §8 Budgets in `unwrap_unit` and `build_units`
// ---------------------------------------------------------------------------

#[test]
fn build_units_and_unwrap_unit_budget_exhaustion_and_linearity() {
    // Exhaustion in `unwrap_unit` (compute & alloc).
    let mut b0 = ComputeBudget::with_default_limits();
    let raw = lex_exact("command env cat a", 0, None, &mut b0)
        .unwrap()
        .units
        .into_iter()
        .next()
        .unwrap();
    let mut b_uc = ComputeBudget::new(2, 10_000);
    assert_eq!(
        unwrap_unit(raw.clone(), &FoldRules::REV4, &mut b_uc),
        Err(Exhausted {
            reason: ExhaustReason::ComputeBudget
        })
    );
    let mut b_ua = ComputeBudget::new(10_000, 1);
    assert_eq!(
        unwrap_unit(raw, &FoldRules::REV4, &mut b_ua),
        Err(Exhausted {
            reason: ExhaustReason::AllocationBudget
        })
    );

    // Exhaustion mid-`build_units`: preserves earlier unit and records Exhausted.
    let padded = format!("cat ~/.ssh/id_rsa; {}", "echo x; ".repeat(200));
    let mut b = ComputeBudget::new(150, 20_000);
    let parsed = build_units(&padded, &FoldRules::REV4, &mut b).unwrap();
    assert!(has_exhausted(&parsed), "padded line must be Exhausted");
    assert!(!parsed.units.is_empty(), "early unit must be preserved");
    assert_eq!(parsed.units[0].argv[0].literal_bytes(), b"cat");
    assert_eq!(parsed.units[0].argv[1].literal_bytes(), b"~/.ssh/id_rsa");

    // Exhaustion during best-effort pass after exact lexing succeeded.
    let mut b_measure = ComputeBudget::with_default_limits();
    let exact_only = lex_exact("cat README.md", 0, None, &mut b_measure).unwrap();
    let exact_used = limits::COMPUTE_BUDGET_UNITS - b_measure.remaining_units();
    assert_eq!(exact_only.units.len(), 1);
    // Give enough for exact + unwrap + fold, not enough for best-effort.
    let mut b_tight = ComputeBudget::new(exact_used + 20, 100_000);
    let parsed_tight = build_units("cat README.md", &FoldRules::REV4, &mut b_tight).unwrap();
    assert!(
        has_exhausted(&parsed_tight),
        "tight budget must exhaust during best-effort pass, never Clear"
    );
    assert_eq!(parsed_tight.units.len(), 1);

    // Nested `-c` exceeding `MAX_UNITS` -> `TooLarge(Cap::Units)`.
    let many_inner = format!("sh -c '{}'", "echo a; ".repeat(520));
    let mut b = ComputeBudget::with_default_limits();
    assert_eq!(
        build_units(&many_inner, &FoldRules::REV4, &mut b),
        Err(PartOutcome::TooLarge(Cap::Units))
    );

    // Linear charge scaling for full `build_units` from 10 KB to `MAX_BYTES`.
    let make_padded = |len: usize| -> String {
        let mut s = String::from("cat ");
        while s.len() < len {
            s.push('a');
        }
        s
    };
    let small = make_padded(10 * 1024);
    let large = make_padded(limits::MAX_BYTES);
    let len_ratio = (large.len() as f64) / (small.len() as f64);

    let mut bs = ComputeBudget::with_default_limits();
    let ps = build_units(&small, &FoldRules::REV4, &mut bs).unwrap();
    assert!(unreads_of(&ps).is_empty() && !has_exhausted(&ps));
    let cs = limits::COMPUTE_BUDGET_UNITS - bs.remaining_units();

    let mut bl = ComputeBudget::with_default_limits();
    let pl = build_units(&large, &FoldRules::REV4, &mut bl).unwrap();
    assert!(unreads_of(&pl).is_empty() && !has_exhausted(&pl));
    let cl = limits::COMPUTE_BUDGET_UNITS - bl.remaining_units();

    let ratio = (cl as f64) / (cs as f64);
    assert!(
        ratio <= len_ratio * 1.15 && ratio >= len_ratio * 0.85,
        "build_units charge not linear: small={cs}, large={cl}, ratio={ratio:.2}, len_ratio={len_ratio:.2}"
    );
}
