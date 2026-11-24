//! Unit tests for P1 unit builder, §4 wrapper unwrapping, §4 nested-shell
//! recursion, §3 `HOME` tracking, §8 budgets, and §10 contract rows.

use super::*;
use crate::policy::limits;
use crate::policy::model::{
    Cap, ComputeBudget, ExhaustReason, FoldRules, PartId, PartOutcome, PipelineId, UnitIndex,
    UnreadReason, WrapperKind,
};
use crate::policy::tools;
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
    type WrapperCase<'a> = (&'a str, &'a [WrapperKind], &'a [&'a [u8]]);
    let cases: &[WrapperCase<'_>] = &[
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
    // Review round 1 finding 3: `MAX_WRAP - 1` (7) wrappers before `cat a` stay
    // Clear, while reaching `MAX_WRAP` (8 removals) makes the unit Unread with
    // `MaxWrapReached` (§4).
    let ok7 = format!("{}cat a", "command ".repeat(limits::MAX_WRAP - 1));
    let mut b = ComputeBudget::with_default_limits();
    let lexed = lex_exact(&ok7, 0, None, &mut b).unwrap();
    let res = unwrap_unit(
        lexed.units.into_iter().next().unwrap(),
        &FoldRules::REV4,
        &mut b,
    )
    .unwrap();
    assert_eq!(res.unread, None);
    assert_eq!(res.unit.removed_wrappers.len(), limits::MAX_WRAP - 1);
    assert_eq!(res.unit.argv[0].literal_bytes(), b"cat");

    let mut b7 = ComputeBudget::with_default_limits();
    let parsed7 = build_units(&ok7, &FoldRules::REV4, &mut b7).unwrap();
    assert!(unreads_of(&parsed7).is_empty(), "{:?}", parsed7.outcomes);

    // Reaching `MAX_WRAP` (8) wrappers: `MaxWrapReached`.
    let fail8 = format!("{}cat a", "command ".repeat(limits::MAX_WRAP));
    let mut b = ComputeBudget::with_default_limits();
    let lexed = lex_exact(&fail8, 0, None, &mut b).unwrap();
    let res = unwrap_unit(
        lexed.units.into_iter().next().unwrap(),
        &FoldRules::REV4,
        &mut b,
    )
    .unwrap();
    assert_eq!(res.unread, Some(UnreadReason::MaxWrapReached));
    assert_eq!(res.unit.removed_wrappers.len(), limits::MAX_WRAP);

    let mut b8 = ComputeBudget::with_default_limits();
    let parsed8 = build_units(&fail8, &FoldRules::REV4, &mut b8).unwrap();
    assert!(
        unreads_of(&parsed8)
            .iter()
            .any(|r| matches!(r, UnreadReason::MaxWrapReached)),
        "{:?}",
        parsed8.outcomes
    );

    // `MAX_WRAP + 1` (9) wrappers also stops at 8 removals with `MaxWrapReached`.
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
        "FOO=1 export HOME=/tmp; cat ~/x",
        "FOO=1 unset HOME; cat ~/x",
        "FOO=1 env HOME=/tmp true; cat ~/x",
        "export HOME=/tmp; cat ~/x",
        "unset HOME; cat ~/x",
        "env -u HOME true; cat ~/x",
        "env -i true; cat ~/x",
        "env HOME=/tmp true; cat ~/x",
        "env -- HOME=/tmp true; cat ~/x",
        "env -vu HOME true; cat ~/x",
        "printf -v HOME /tmp; cat ~/x",
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

#[test]
fn build_units_empty_quoted_home_does_not_trigger_home_set_earlier() {
    // Review round 2 finding 4: empty quotes before `=` suppress shell
    // assignment recognition, so `''HOME=/tmp` is a command name rather than
    // a `HOME` assignment and must emit neither `Assignment` nor `HomeSetEarlier`.
    for line in [
        "''HOME=/tmp; cat ~/x",
        r#"""HOME=/tmp; cat ~/x"#,
        "HOME''=/tmp; cat ~/x",
        r#"H""OME=/tmp; cat ~/x"#,
        "HOME+''=/tmp; cat ~/x",
        "''FOO=1 export HOME=/tmp; cat ~/x",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            !unreads_of(&parsed)
                .iter()
                .any(|r| matches!(r, UnreadReason::Assignment | UnreadReason::HomeSetEarlier)),
            "empty-quoted command word `{line}` must not emit Assignment or HomeSetEarlier: {:?}",
            parsed.outcomes
        );
        assert_eq!(parsed.units.len(), 2, "`{line}`");
    }

    // `FOO=1` is a leading assignment, while `''HOME=/tmp` is the command word
    // following it: `Assignment` applies to `FOO=1`, but `HOME` is not set.
    let mut b_mixed = ComputeBudget::with_default_limits();
    let parsed_mixed =
        build_units("FOO=1 ''HOME=/tmp; cat ~/x", &FoldRules::REV4, &mut b_mixed).unwrap();
    assert!(
        unreads_of(&parsed_mixed)
            .iter()
            .any(|r| matches!(r, UnreadReason::Assignment)),
        "{:?}",
        parsed_mixed.outcomes
    );
    assert!(
        !unreads_of(&parsed_mixed)
            .iter()
            .any(|r| matches!(r, UnreadReason::HomeSetEarlier)),
        "empty-quoted `''HOME=/tmp` after `FOO=1` must not trigger HomeSetEarlier: {:?}",
        parsed_mixed.outcomes
    );

    // Contrast: quoting only the value side (after `=`) remains a real `HOME`
    // assignment and still triggers both `Assignment` and `HomeSetEarlier`.
    for line in [
        "HOME=''/tmp; cat ~/x",
        r#"HOME="/tmp"; cat ~/x"#,
        "HOME+=/tmp; cat ~/x",
        "FOO=1 HOME=/tmp; cat ~/x",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            unreads_of(&parsed)
                .iter()
                .any(|r| matches!(r, UnreadReason::Assignment)),
            "expected Assignment for `{line}`: {:?}",
            parsed.outcomes
        );
        assert!(
            unreads_of(&parsed)
                .iter()
                .any(|r| matches!(r, UnreadReason::HomeSetEarlier)),
            "expected HomeSetEarlier for `{line}`: {:?}",
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
    let mut hook = |_unit: &CommandUnit,
                    folded: &str,
                    sink: &mut UnitReasonSink<'_>,
                    budget: &mut ComputeBudget| {
        if folded == "awk" {
            // Reserve before building the reason, so it cannot be lost.
            sink.reserve(1, budget)?;
            sink.push(
                UnreadReason::CodeRunnerInlineCode {
                    tool: "awk".to_string(),
                },
                budget,
            )?;
        }
        Ok(())
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

#[test]
fn build_units_treats_d141_unknown_by_design_tools_as_unknown() {
    // Tools D-141 moved out of `TOOLS` into `UNKNOWN_BY_DESIGN` receive §4's
    // unknown-tool treatment (kept in the exact model for hard-deny / store
    // checks; P1 strips wrappers cleanly and emits no spurious Unread).
    for name in ["rm", "curl", "patch", "diff", "sudo"] {
        assert!(tools::lookup(name).is_none(), "{name} moved out of TOOLS");
        assert!(
            tools::lookup_outside_model(name).is_none(),
            "{name} is unknown, not OUTSIDE_MODEL"
        );
        assert!(
            tools::lookup_unknown_by_design(name).is_some(),
            "{name} must be recorded in UNKNOWN_BY_DESIGN"
        );
    }
    assert!(
        tools::lookup("cat").is_some(),
        "cat remains HomeReadOnly in TOOLS"
    );

    for (line, want_cmd, want_wrapped) in [
        ("rm -rf proj", b"rm".as_slice(), false),
        ("command rm a", b"rm".as_slice(), true),
        ("curl https://example.com", b"curl".as_slice(), false),
        ("nice patch -p1", b"patch".as_slice(), true),
        ("env diff a b", b"diff".as_slice(), true),
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let parsed = build_units(line, &FoldRules::REV4, &mut b).unwrap();
        assert!(
            unreads_of(&parsed).is_empty(),
            "unknown tool `{line}` must not emit P1 Unread: {:?}",
            parsed.outcomes
        );
        assert_eq!(parsed.units.len(), 1, "{line}");
        assert_eq!(parsed.units[0].argv[0].literal_bytes(), want_cmd, "{line}");
        assert_eq!(
            !parsed.units[0].removed_wrappers.is_empty(),
            want_wrapped,
            "{line}"
        );
    }
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
    // Budget: entry + the first unit's lexing (measured alone) + a little,
    // far short of the 200 padding units.
    let padded = format!("cat ~/.ssh/id_rsa; {}", "echo x; ".repeat(200));
    let mut b_first = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let _ = lex_exact("cat ~/.ssh/id_rsa;", 0, None, &mut b_first).unwrap();
    let first_unit = BIG_UNITS - b_first.remaining_units();
    let mut b = ComputeBudget::new(first_unit + 20, BIG_ALLOC);
    let parsed = build_units(&padded, &FoldRules::REV4, &mut b).unwrap();
    assert!(has_exhausted(&parsed), "padded line must be Exhausted");
    assert!(!parsed.units.is_empty(), "early unit must be preserved");
    assert_eq!(parsed.units[0].argv[0].literal_bytes(), b"cat");
    assert_eq!(parsed.units[0].argv[1].literal_bytes(), b"~/.ssh/id_rsa");

    // Exhaustion during unwrap / fold stage in `build_units`: every lexed unit
    // is still preserved and every `PartId::Unit(idx)` in `outcomes` is valid.
    let two_units = "command cat ~/.ssh/id_rsa; $BAD echo hi";
    let mut b_m2 = ComputeBudget::with_default_limits();
    let lexed_two = lex_exact(two_units, 0, None, &mut b_m2).unwrap();
    let lex_two_used = limits::COMPUTE_BUDGET_UNITS - b_m2.remaining_units();
    assert_eq!(lexed_two.units.len(), 2);
    // Allow `lex_exact` to finish, then exhaust immediately inside `unwrap_unit`.
    let mut b_unwrap_exhaust = ComputeBudget::new(lex_two_used + 2, 100_000);
    let parsed_ue = build_units(two_units, &FoldRules::REV4, &mut b_unwrap_exhaust).unwrap();
    assert!(has_exhausted(&parsed_ue));
    assert_eq!(
        parsed_ue.units.len(),
        2,
        "all lexed units must be preserved when unwrap_unit exhausts"
    );
    for pr in &parsed_ue.outcomes {
        if let PartId::Unit(UnitIndex(idx)) = pr.part {
            assert!(
                idx < parsed_ue.units.len(),
                "UnitIndex({idx}) out of bounds for {} units",
                parsed_ue.units.len()
            );
        }
    }

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

    // Linear compute and allocation scaling for full `build_units` from 10 KB
    // to `MAX_BYTES`.
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
    let as_ = limits::COMPUTE_BUDGET_ALLOC_BYTES - bs.remaining_alloc_bytes();

    let mut bl = ComputeBudget::with_default_limits();
    let pl = build_units(&large, &FoldRules::REV4, &mut bl).unwrap();
    assert!(unreads_of(&pl).is_empty() && !has_exhausted(&pl));
    let cl = limits::COMPUTE_BUDGET_UNITS - bl.remaining_units();
    let al = limits::COMPUTE_BUDGET_ALLOC_BYTES - bl.remaining_alloc_bytes();

    let ratio = (cl as f64) / (cs as f64);
    let alloc_ratio = (al as f64) / (as_ as f64);
    assert!(cs > 0 && cl > cs);
    assert!(as_ > 0 && al > as_);
    assert!(
        cl <= 64 * (large.len() as u64) + 16_384 && ratio <= len_ratio * 1.5,
        "build_units compute charge exceeds linear bound: small={cs}, large={cl}, ratio={ratio:.2}, len_ratio={len_ratio:.2}"
    );
    assert!(
        al <= 32 * large.len() + 65_536 && alloc_ratio <= len_ratio * 2.0,
        "build_units alloc charge exceeds affine bound: small={as_}, large={al}, ratio={alloc_ratio:.2}, len_ratio={len_ratio:.2}"
    );
}

// ---------------------------------------------------------------------------
// Round 5 (review round 3): every scan charged before it runs
// ---------------------------------------------------------------------------

const BIG_UNITS: u64 = 100_000_000;
const BIG_ALLOC: usize = 128 * 1024 * 1024;

fn raw_unit(line: &str) -> RawCommandUnit {
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    lex_exact(line, 0, None, &mut b)
        .unwrap()
        .units
        .into_iter()
        .next()
        .unwrap()
}

fn charged(b: &ComputeBudget) -> u64 {
    BIG_UNITS - b.remaining_units()
}

#[test]
fn r5_f2_wrapper_operand_scans_are_charged() {
    // Review round 3, finding 2, failing case: unwrapping charged only 19
    // units while scanning the 60,000-byte `-n` operand several times.
    let line = format!("nice -n {} cat", "0".repeat(60_000));
    let raw = raw_unit(&line);
    assert_eq!(
        unwrap_unit(
            raw.clone(),
            &FoldRules::REV4,
            &mut ComputeBudget::new(100, 10_000)
        ),
        Err(Exhausted {
            reason: ExhaustReason::ComputeBudget
        })
    );
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let res = unwrap_unit(raw, &FoldRules::REV4, &mut b).unwrap();
    assert_eq!(res.unit.argv[0].literal_bytes(), b"cat");
    assert!(charged(&b) >= 60_000, "charged {}", charged(&b));

    // The same for every variable-length scan of exact unwrapping: each
    // charges at least the operand's length, so half of it exhausts.
    let n = 20_000;
    for (line, cmd) in [
        (format!("nice -n {} cat", "7".repeat(n)), "cat"),
        (format!("timeout -s {} 5 cat", "S".repeat(n)), "cat"),
        (format!("timeout -k {} 5 cat", "1".repeat(n)), "cat"),
        (format!("timeout {}s cat", "1".repeat(n)), "cat"),
        (format!("nice -{} cat", "n".repeat(n)), "nice"),
        (format!("{}* x", "a".repeat(n)), ""),
        (format!("env {}=1 cat", "A".repeat(n)), "env"),
    ] {
        let raw = raw_unit(&line);
        let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
        let res = unwrap_unit(raw.clone(), &FoldRules::REV4, &mut b).unwrap();
        if !cmd.is_empty() {
            assert_eq!(
                res.unit.argv[0].literal_bytes(),
                cmd.as_bytes(),
                "{}",
                &line[..20]
            );
        }
        assert!(
            charged(&b) >= n as u64,
            "`{}…`: charged {} for a {n}-byte operand",
            &line[..20],
            charged(&b)
        );
        assert!(
            unwrap_unit(
                raw,
                &FoldRules::REV4,
                &mut ComputeBudget::new(n as u64 / 2, BIG_ALLOC)
            )
            .is_err(),
            "`{}…` must exhaust below its operand's length",
            &line[..20]
        );
    }

    // Wrapper words are moved out with one charged shift of the argv, not
    // one per wrapper: the charge covers the argv, and stays linear in it.
    let wrapped = |k: usize| format!("command command command cat {}", "a ".repeat(k));
    let mut b1 = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let r1 = unwrap_unit(raw_unit(&wrapped(10_000)), &FoldRules::REV4, &mut b1).unwrap();
    assert_eq!(r1.unit.removed_wrappers.len(), 3);
    assert_eq!(r1.unit.argv.len(), 10_001);
    assert!(charged(&b1) >= 10_004);
    let mut b2 = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let _ = unwrap_unit(raw_unit(&wrapped(20_000)), &FoldRules::REV4, &mut b2).unwrap();
    assert!((charged(&b2) as f64) / (charged(&b1) as f64) <= 2.5);
}

#[test]
fn r5_f2_nested_shell_and_home_scans_are_charged() {
    let n = 20_000;
    let sh = tools::lookup("sh").unwrap();
    let unit_of = |line: &str| {
        let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
        unwrap_unit(raw_unit(line), &FoldRules::REV4, &mut b)
            .unwrap()
            .unit
    };

    // Nested-shell classification scans the script word (glob check, UTF-8
    // check, copy) and a script file's name; both are charged.
    let script = unit_of(&format!("sh -c '{}'", "a".repeat(n)));
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    assert!(matches!(
        classify_nested_shell(&script, "sh", sh, &mut b).unwrap(),
        NestedShellOutcome::ModeledScript(s) if s.len() == n
    ));
    assert!(charged(&b) >= 3 * n as u64);
    let file = unit_of(&format!("sh {}.sh", "a".repeat(n)));
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    assert!(matches!(
        classify_nested_shell(&file, "sh", sh, &mut b).unwrap(),
        NestedShellOutcome::Unread(UnreadReason::Program { .. })
    ));
    assert!(charged(&b) >= n as u64);
    // Each flag letter's lookup is charged.
    let flags = unit_of(&format!("sh -{}c 'x'", "e".repeat(n)));
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let _ = classify_nested_shell(&flags, "sh", sh, &mut b).unwrap();
    assert!(charged(&b) >= n as u64);

    // `env`'s HOME check scans each argument once, charged.
    let env = unit_of(&format!("env -{} x", "a".repeat(n)));
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    assert!(!command_modifies_home("env", &env.argv, &mut b).unwrap());
    assert!(charged(&b) >= n as u64);

    // The `~` check after a HOME change visits every word, charged.
    let many = unit_of(&format!("cat {}", "a ".repeat(n)));
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    assert!(!unit_uses_unquoted_tilde(&many, &mut b).unwrap());
    assert!(charged(&b) >= n as u64);
}

#[test]
fn r5_class_build_units_below_cost_keeps_only_finished_values() {
    // Every compute budget below the smallest clean one gives `Exhausted`,
    // keeps only units (with their words as written) the full run found, in
    // order, and never points an outcome at a missing unit. Reservations are
    // debug-asserted by `push_reserved`.
    for line in [
        "sudo id",
        "command nice -n 5 timeout -s TERM 3 cat a; ls",
        "HOME=/x cat ~/y; cat ~/z",
        "sh -c 'sudo id; bash -c \"ls\"'",
        "command command command command command command command command cat a",
        "zsh -c x | sh run.sh",
    ] {
        let clean = |b: &mut ComputeBudget| {
            build_units(line, &FoldRules::REV4, b).is_ok_and(|p| !has_exhausted(&p))
        };
        let mut bf = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
        let full = build_units(line, &FoldRules::REV4, &mut bf).unwrap();
        let mut lo = 0u64;
        let mut hi = BIG_UNITS;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if clean(&mut ComputeBudget::new(mid, BIG_ALLOC)) {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        for units in 0..lo {
            let res = build_units(
                line,
                &FoldRules::REV4,
                &mut ComputeBudget::new(units, BIG_ALLOC),
            );
            let parsed = match res {
                Err(PartOutcome::Exhausted(_)) => continue,
                Ok(p) => p,
                Err(other) => panic!("`{line}` units={units}: {other:?}"),
            };
            assert!(has_exhausted(&parsed), "`{line}` units={units}");
            assert!(parsed.units.len() <= full.units.len());
            for (i, u) in parsed.units.iter().enumerate() {
                let kept: Vec<&Word> = u.original_argv().collect();
                let want: Vec<&Word> = full.units[i].original_argv().collect();
                assert_eq!(u.index, full.units[i].index);
                if i + 1 < parsed.units.len() {
                    assert_eq!(kept, want, "`{line}` units={units} unit {i}");
                } else {
                    assert!(
                        kept.len() <= want.len() && kept[..] == want[..kept.len()],
                        "`{line}` units={units}: last unit is not a word prefix"
                    );
                }
            }
            for pr in &parsed.outcomes {
                if let PartId::Unit(UnitIndex(idx)) = pr.part {
                    assert!(idx < parsed.units.len(), "`{line}` units={units}");
                }
            }
        }
    }
}
