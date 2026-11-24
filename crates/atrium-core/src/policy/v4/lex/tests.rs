//! Unit tests for P1 exact lexer, CR-19 folding, best-effort lexer, N11
//! bounded brace expansion, and §8 budgets (contract §2, §3, §4, §8, §10, §12).

use super::*;
use crate::policy::limits;
use crate::policy::model::{
    BestEffortRedirectKind, ByteProvenance, Cap, ComputeBudget, ExhaustReason, FoldRules,
    ModelRedirect, PartOutcome, PipelineId, Separator, StdFd, UnreadReason, Word,
};

fn unread_reasons(out: &LexExactOutput) -> Vec<&UnreadReason> {
    out.outcomes
        .iter()
        .filter_map(|pr| match &pr.outcome {
            PartOutcome::Unread(r) => Some(r),
            _ => None,
        })
        .collect()
}

fn has_exhausted(out: &LexExactOutput) -> bool {
    out.outcomes
        .iter()
        .any(|pr| matches!(pr.outcome, PartOutcome::Exhausted(_)))
}

/// Budgets large enough for every input in this file.
const BIG_UNITS: u64 = 100_000_000;
const BIG_ALLOC: usize = 128 * 1024 * 1024;

/// Compute and allocation a clean `lex_exact` run of `line` charges.
fn exact_cost(line: &str) -> (u64, usize) {
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let out = lex_exact(line, 0, None, &mut b).unwrap();
    assert!(!has_exhausted(&out), "`{line}` must lex cleanly");
    (
        BIG_UNITS - b.remaining_units(),
        BIG_ALLOC - b.remaining_alloc_bytes(),
    )
}

/// `true` iff `res` reports exhaustion, either on entry (`Err`) or among
/// its outcomes.
fn exact_exhausted(res: &Result<LexExactOutput, PartOutcome>) -> bool {
    match res {
        Err(PartOutcome::Exhausted(_)) => true,
        Ok(out) => has_exhausted(out),
        Err(_) => false,
    }
}

/// Asserts that `kept` (a run cut short by exhaustion) holds only what the
/// full run found, in order and whole: every unit but the last equals the
/// full run's unit, and the last one's argv and redirects are prefixes of the
/// full run's, made of complete words. Every unit outcome points at a kept
/// unit.
fn assert_exact_prefix(kept: &LexExactOutput, full: &LexExactOutput, ctx: &str) {
    assert!(kept.units.len() <= full.units.len(), "{ctx}: extra units");
    for (i, u) in kept.units.iter().enumerate() {
        let f = &full.units[i];
        assert_eq!(u.index, f.index, "{ctx}: unit {i} index");
        if i + 1 < kept.units.len() {
            assert_eq!(u.argv, f.argv, "{ctx}: unit {i} argv");
            assert_eq!(u.redirects, f.redirects, "{ctx}: unit {i} redirects");
        } else {
            assert!(
                u.argv.len() <= f.argv.len() && u.argv[..] == f.argv[..u.argv.len()],
                "{ctx}: last unit argv {:?} is not a word prefix of {:?}",
                u.argv,
                f.argv
            );
            assert!(
                u.redirects.len() <= f.redirects.len()
                    && u.redirects[..] == f.redirects[..u.redirects.len()],
                "{ctx}: last unit redirects are not a prefix"
            );
        }
    }
    for pr in &kept.outcomes {
        if let PartId::Unit(UnitIndex(idx)) = pr.part {
            assert!(
                idx < kept.units.len(),
                "{ctx}: outcome for missing unit {idx}"
            );
        }
    }
}

/// Smallest budget along one dimension (the other one ample) for which
/// `clean` reports a run without exhaustion.
fn min_clean_budget(compute: bool, mut clean: impl FnMut(&mut ComputeBudget) -> bool) -> u64 {
    let make = |n: u64| {
        if compute {
            ComputeBudget::new(n, BIG_ALLOC)
        } else {
            ComputeBudget::new(BIG_UNITS, usize::try_from(n).unwrap())
        }
    };
    let (mut lo, mut hi) = (0u64, if compute { BIG_UNITS } else { BIG_ALLOC as u64 });
    assert!(clean(&mut make(hi)), "must finish with ample budget");
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if clean(&mut make(mid)) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

// ---------------------------------------------------------------------------
// §3 Allowed unquoted bytes & boundaries
// ---------------------------------------------------------------------------

#[test]
fn lex_exact_accepts_every_allowed_unquoted_byte() {
    let all_allowed = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./,:@%+- a=b";
    let mut budget = ComputeBudget::with_default_limits();
    let out = lex_exact(all_allowed, 0, None, &mut budget).expect("valid line");
    assert!(unread_reasons(&out).is_empty(), "{:?}", out.outcomes);
    assert_eq!(out.units.len(), 1);
    assert_eq!(out.units[0].argv.len(), 2);
    assert!(out.units[0].argv[0].is_all_unquoted());
    assert_eq!(
        out.units[0].argv[1].literal_bytes(),
        b"a=b",
        "non-leading '=' allowed"
    );
}

#[test]
fn lex_exact_rejects_leading_equals_zsh_expansion() {
    let mut budget = ComputeBudget::with_default_limits();
    let out = lex_exact("echo =ls", 0, None, &mut budget).unwrap();
    assert!(
        unread_reasons(&out)
            .iter()
            .any(|r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte: b'=' })),
        "{:?}",
        out.outcomes
    );

    // Escaped `\=` at word start is literal and allowed (§3).
    let mut budget2 = ComputeBudget::with_default_limits();
    let out2 = lex_exact(r"echo \=ls", 0, None, &mut budget2).unwrap();
    assert!(unread_reasons(&out2).is_empty(), "{:?}", out2.outcomes);
    assert_eq!(out2.units[0].argv[1].literal_bytes(), b"=ls");
    assert_eq!(
        out2.units[0].argv[1].provenance()[0],
        ByteProvenance::Escaped
    );
}

#[test]
fn lex_exact_globs_and_negated_classes() {
    // Valid globs: `*`, `?`, `[a-z]`, `[0-9]`, `[*]`.
    for line in [
        "cat proj/key*",
        "cat proj/key?",
        "cat proj/key[a-z]",
        "cat proj/key[0-9_]",
        "cat proj/[*]",
    ] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            unread_reasons(&out).is_empty(),
            "line `{line}`: {:?}",
            out.outcomes
        );
        assert!(out.units[0].argv[1].has_unquoted_glob(), "{line}");
    }

    // Negated classes (`!` and `^` inside `[...]`) -> `NegatedGlobClass`.
    for line in ["cat [!a]", "cat [^a]", "cat [a!b]", "cat [a^b]"] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::NegatedGlobClass)),
            "line `{line}`: {:?}",
            out.outcomes
        );
    }

    // Empty `[]`, unclosed `[abc`, nested `[[:alpha:]]`, slash `[a/b]`, stray `]`.
    for line in [
        "cat []",
        "cat [abc",
        "cat [[:alpha:]]",
        "cat [a/b]",
        "cat a]b",
    ] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            !unread_reasons(&out).is_empty(),
            "expected Unread for `{line}`"
        );
    }
}

#[test]
fn lex_exact_tilde_rules() {
    // Allowed: `~`, `~/foo`, `~/'foo'`.
    for line in ["ls ~", "cat ~/README.md", "cat ~/'my file'"] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            unread_reasons(&out).is_empty(),
            "line `{line}`: {:?}",
            out.outcomes
        );
        assert!(out.units[0].argv[1].starts_with_unquoted_tilde(), "{line}");
    }

    // Disallowed leading tilde forms: `~user`, `~+`, `~-`, `~''`.
    for line in ["cat ~root/.ssh", "cd ~+", "cd ~-", "echo ~''"] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::UnsupportedTildeForm)),
            "line `{line}`: {:?}",
            out.outcomes
        );
    }

    // Disallowed non-leading unquoted `~`: `a~b`, `--dir=~/x`.
    for line in ["echo a~b", "ls --dir=~/x"] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte: b'~' })),
            "line `{line}`: {:?}",
            out.outcomes
        );
    }

    // Quoted `~` is literal and NOT `starts_with_unquoted_tilde`.
    for line in ["cat '~'", "cat \"~\""] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            unread_reasons(&out).is_empty(),
            "{line}: {:?}",
            out.outcomes
        );
        assert!(!out.units[0].argv[1].starts_with_unquoted_tilde());
        assert_eq!(out.units[0].argv[1].literal_bytes(), b"~");
    }
}

#[test]
fn lex_exact_quotes_escapes_and_provenance() {
    // Brief provenance check: `cat 'proj/key*'` vs `cat proj/key*`.
    let mut b1 = ComputeBudget::with_default_limits();
    let quoted = lex_exact("cat 'proj/key*'", 0, None, &mut b1).unwrap();
    assert!(unread_reasons(&quoted).is_empty());
    let qw = &quoted.units[0].argv[1];
    assert_eq!(qw.literal_bytes(), b"proj/key*");
    assert!(!qw.has_unquoted_glob());
    assert!(
        qw.provenance()
            .iter()
            .all(|&p| p == ByteProvenance::SingleQuoted)
    );

    let mut b2 = ComputeBudget::with_default_limits();
    let unquoted = lex_exact("cat proj/key*", 0, None, &mut b2).unwrap();
    assert!(unread_reasons(&unquoted).is_empty());
    let uw = &unquoted.units[0].argv[1];
    assert_eq!(uw.literal_bytes(), b"proj/key*");
    assert!(uw.has_unquoted_glob());

    // Single quotes accept any bytes (`$`, backtick, `!`, `\`, `\n`, non-ASCII)
    // and bytes coming out of quotes are never re-parsed as syntax.
    let mut b3 = ComputeBudget::with_default_limits();
    let sq = lex_exact("echo '$x `id` ! \\n café; sudo id'", 0, None, &mut b3).unwrap();
    assert!(unread_reasons(&sq).is_empty());
    assert_eq!(sq.units.len(), 1);
    assert_eq!(sq.units[0].argv.len(), 2);

    // Double quotes allow `\"`, `\\`, spaces, non-ASCII, globs (non-expanding).
    let mut b4 = ComputeBudget::with_default_limits();
    let dq = lex_exact(r#"echo "a \"b\" \\ c * café""#, 0, None, &mut b4).unwrap();
    assert!(unread_reasons(&dq).is_empty(), "{:?}", dq.outcomes);
    let dw = &dq.units[0].argv[1];
    assert_eq!(dw.literal_bytes(), "a \"b\" \\ c * café".as_bytes());
    assert!(!dw.has_unquoted_glob());
    assert_eq!(dw.provenance()[2], ByteProvenance::Escaped); // \"
    assert_eq!(dw.provenance()[6], ByteProvenance::Escaped); // \\

    // Disallowed inside double quotes: `$`, backtick, `!`, `\q`, `\$`, `\\\n`, unclosed.
    for line in [
        r#"echo "$HOME""#,
        r#"echo "`id`""#,
        r#"echo "hi!""#,
        r#"echo "\q""#,
        r#"echo "\$""#,
        "echo \"a\\\nb\"",
        r#"echo "unterminated"#,
        "echo 'unterminated",
    ] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::UnsupportedQuoteOrEscape)),
            "line `{line}`: {:?}",
            out.outcomes
        );
    }

    // Outside quotes: `\` allowed before EVERY allowed char and glob char.
    for ch in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_./,:@%+-=*?[]".chars()
    {
        let line = format!("echo \\{ch}");
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(&line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out).is_empty(),
            "escaped `\\{ch}` must be allowed: {:?}",
            out.outcomes
        );
        assert_eq!(out.units[0].argv[1].literal_bytes(), &[ch as u8]);
        assert_eq!(
            out.units[0].argv[1].provenance(),
            &[ByteProvenance::Escaped]
        );
        assert!(!out.units[0].argv[1].has_unquoted_glob());
    }
    let mut b5 = ComputeBudget::with_default_limits();
    let esc = lex_exact(r"cat proj/key\*\[0\]\?", 0, None, &mut b5).unwrap();
    assert!(unread_reasons(&esc).is_empty(), "{:?}", esc.outcomes);
    assert_eq!(esc.units[0].argv[1].literal_bytes(), b"proj/key*[0]?");
    assert!(!esc.units[0].argv[1].has_unquoted_glob());

    // Outside quotes: disallowed escapes (`\\\n`, `\ `, `\$`, `\;`, `\~`, `\\`, trailing `\`).
    for line in [
        "echo a\\\nb",
        r"echo a\ b",
        r"echo \$x",
        r"echo a\;b",
        r"echo \~",
        r"echo \\",
        "echo a\\",
    ] {
        let mut budget = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut budget).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::UnsupportedQuoteOrEscape)),
            "line `{line}`: {:?}",
            out.outcomes
        );
    }
}

#[test]
fn lex_exact_separators_pipelines_and_outside_constructs() {
    // Valid separators `;`, `&&`, `||`, `|`, `\n` and pipeline grouping.
    let mut budget = ComputeBudget::with_default_limits();
    let out = lex_exact(
        "cat a | wc -l; echo x && echo y || echo z\nls",
        0,
        None,
        &mut budget,
    )
    .unwrap();
    assert!(unread_reasons(&out).is_empty(), "{:?}", out.outcomes);
    assert_eq!(out.units.len(), 6);
    assert_eq!(out.units[0].separator_before, Separator::Start);
    assert_eq!(out.units[1].separator_before, Separator::Pipe);
    assert_eq!(out.units[0].pipeline_id, Some(PipelineId(1)));
    assert_eq!(out.units[1].pipeline_id, Some(PipelineId(1)));
    assert_eq!(out.units[2].separator_before, Separator::Semi);
    assert_eq!(out.units[2].pipeline_id, None);
    assert_eq!(out.units[3].separator_before, Separator::And);
    assert_eq!(out.units[4].separator_before, Separator::Or);
    assert_eq!(out.units[5].separator_before, Separator::Newline);

    // Every outside-model construct listed in §3.
    let outside_cases = [
        ("cat $HOME", "dollar"),
        ("cat `id`", "backtick"),
        ("cat a{b,c}", "brace expansion"),
        ("echo a#b", "mid-word hash"),
        ("echo a!b", "bang"),
        ("echo a^b", "caret"),
        ("cat café", "non-ASCII"),
        ("cat a\r", "carriage return"),
        ("# comment\ncat a", "comment"),
        ("cat a &", "background"),
        ("cat a |& wc", "pipe stderr"),
        ("cat a ;; cat b", "double semi"),
        ("(cat a)", "subshell"),
        ("((1+1))", "arithmetic"),
        ("f() { echo hi; }", "function"),
        ("{ echo hi; }", "brace group"),
        ("cat <<EOF\nhello\nEOF", "heredoc"),
        ("cat <<-EOF\n\thello\nEOF", "heredoc strip"),
        ("cat <<<hello", "herestring"),
        ("cat <(echo hi)", "proc sub in"),
        ("tee >(cat)", "proc sub out"),
        ("FOO=1 cat a", "leading assignment"),
        ("FOO+=1 cat a", "append assignment"),
        ("> out.txt", "redirect-only command"),
    ];
    for (line, label) in outside_cases {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            !unread_reasons(&out).is_empty(),
            "expected Unread for `{label}` (`{line}`)"
        );
    }

    // Every reserved word in P0's `OUTSIDE_MODEL` (§3, matching `shell::RESERVED`).
    for entry in crate::policy::tools::outside_model()
        .iter()
        .filter(|e| e.contract_section == "§3")
    {
        let kw = entry.name;
        let line = format!("{kw} foo");
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(&line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out).iter().any(|r| matches!(
                r,
                UnreadReason::ReservedWordOrConstruct { construct } if construct == kw
            )),
            "reserved word `{kw}`: {:?}",
            out.outcomes
        );
    }

    // Standalone `{` / `}` in argument position reports `UnquotedByteOutsideModel`.
    for (line, want_b) in [("echo {", b'{'), ("echo }", b'}')] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out).iter().any(
                |r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte } if *byte == want_b)
            ),
            "line `{line}`: {:?}",
            out.outcomes
        );
    }

    // Quoted reserved word is NOT a reserved word.
    let mut b = ComputeBudget::with_default_limits();
    let out = lex_exact("'for' a b", 0, None, &mut b).unwrap();
    assert!(unread_reasons(&out).is_empty(), "{:?}", out.outcomes);
}

#[test]
fn lex_exact_closed_redirect_list_and_outside_redirects() {
    // Every modeled redirect form (§3).
    let mut b = ComputeBudget::with_default_limits();
    let out = lex_exact(
        "cat < in.txt > out.txt >> app.txt 2> err.txt 2>> eapp.txt 2>&1 0>&2 1>&-",
        0,
        None,
        &mut b,
    )
    .unwrap();
    assert!(unread_reasons(&out).is_empty(), "{:?}", out.outcomes);
    let r = &out.units[0].redirects;
    assert_eq!(r.len(), 8);
    assert!(matches!(&r[0], ModelRedirect::Read { target } if target.literal_bytes() == b"in.txt"));
    assert!(
        matches!(&r[1], ModelRedirect::Write { target } if target.literal_bytes() == b"out.txt")
    );
    assert!(
        matches!(&r[2], ModelRedirect::Append { target } if target.literal_bytes() == b"app.txt")
    );
    assert!(
        matches!(&r[3], ModelRedirect::ErrWrite { target } if target.literal_bytes() == b"err.txt")
    );
    assert!(
        matches!(&r[4], ModelRedirect::ErrAppend { target } if target.literal_bytes() == b"eapp.txt")
    );
    assert_eq!(
        r[5],
        ModelRedirect::Dup {
            from: StdFd::Stderr,
            to: StdFd::Stdout
        }
    );
    assert_eq!(
        r[6],
        ModelRedirect::Dup {
            from: StdFd::Stdin,
            to: StdFd::Stderr
        }
    );
    assert_eq!(r[7], ModelRedirect::Close { fd: StdFd::Stdout });

    // All 9 `n>&m` pairs and all 3 `n>&-` forms in `0..=2`.
    for n in 0u8..=2 {
        let close_line = format!("cat {n}>&-");
        let mut bc = ComputeBudget::with_default_limits();
        let out_c = lex_exact(&close_line, 0, None, &mut bc).unwrap();
        assert!(unread_reasons(&out_c).is_empty(), "{close_line}");
        assert_eq!(
            out_c.units[0].redirects,
            vec![ModelRedirect::Close {
                fd: StdFd::from_u8(n).unwrap()
            }]
        );
        for m in 0u8..=2 {
            let dup_line = format!("cat {n}>&{m}");
            let mut bd = ComputeBudget::with_default_limits();
            let out_d = lex_exact(&dup_line, 0, None, &mut bd).unwrap();
            assert!(unread_reasons(&out_d).is_empty(), "{dup_line}");
            assert_eq!(
                out_d.units[0].redirects,
                vec![ModelRedirect::Dup {
                    from: StdFd::from_u8(n).unwrap(),
                    to: StdFd::from_u8(m).unwrap()
                }]
            );
        }
    }

    // Quoted `'2'>out` is NOT an fd number: `'2'` stays in argv, `>out` is stdout Write.
    let mut b2 = ComputeBudget::with_default_limits();
    let out2 = lex_exact("echo '2'>out.txt", 0, None, &mut b2).unwrap();
    assert!(unread_reasons(&out2).is_empty());
    assert_eq!(out2.units[0].argv.len(), 2);
    assert_eq!(out2.units[0].argv[1].literal_bytes(), b"2");
    assert!(matches!(
        out2.units[0].redirects[0],
        ModelRedirect::Write { .. }
    ));

    // Combined redirects `&>` / `&>>` -> `CombinedRedirect`.
    for line in ["cat &> out", "cat &>> out"] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::CombinedRedirect)),
            "{line}: {:?}",
            out.outcomes
        );
    }

    // Descriptor > 2 -> `DescriptorOutsideStd`.
    for (line, want_fd) in [
        ("cat <&3", 3u32),
        ("cat 3> out", 3),
        ("cat 3>> out", 3),
        ("cat 3< in", 3),
        ("cat 3>&1", 3),
        ("cat 2>&3", 3),
        ("cat 3>&-", 3),
        ("cat 10> out", 10),
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::DescriptorOutsideStd { fd } if *fd == want_fd)),
            "{line}: {:?}",
            out.outcomes
        );
    }

    // Other outside redirects: `<&0`, `<>`, `>|`, `>&2`, `>&foo`, `1>`, `0<`, glob target, missing target.
    for line in [
        "cat <&0",
        "cat <> file",
        "cat >| file",
        "cat >&2",
        "cat >&-",
        "cat >&foo",
        "cat 1> out",
        "cat 0< in",
        "cat 2< in",
        "cat 2>&'1'",
        "cat > out*",
        "cat >",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::UnsupportedRedirect { .. })),
            "{line}: {:?}",
            out.outcomes
        );
    }
}

// ---------------------------------------------------------------------------
// §10 rows decided by `lex_exact` + `fold_command_word`
// ---------------------------------------------------------------------------

#[test]
fn contract_section_10_lex_rows() {
    // Row 3: `SUDO id`, `/usr/bin/sudo id` -> folded to `"sudo"`.
    for line in ["SUDO id", "/usr/bin/sudo id"] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(unread_reasons(&out).is_empty());
        let folded = fold_command_word(&out.units[0].argv[0], &FoldRules::REV4, &mut b).unwrap();
        assert_eq!(folded, "sudo", "{line}");
    }

    // Row 6: `cat $'\xff'` -> Unread (`$`).
    let mut b = ComputeBudget::with_default_limits();
    let r6 = lex_exact(r"cat $'\xff'", 0, None, &mut b).unwrap();
    assert!(
        unread_reasons(&r6)
            .iter()
            .any(|r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte: b'$' }))
    );

    // Row 7: `for f in *.rs; do wc -l "$f"; done` -> Unread (reserved word).
    let mut b = ComputeBudget::with_default_limits();
    let r7 = lex_exact("for f in *.rs; do wc -l \"$f\"; done", 0, None, &mut b).unwrap();
    assert!(unread_reasons(&r7).iter().any(|r| matches!(
        r,
        UnreadReason::ReservedWordOrConstruct { construct } if construct == "for"
    )));

    // Row 10: `cat /h/u/[!é]trium/secret` -> non-ASCII unquoted byte (+ negated class).
    let mut b = ComputeBudget::with_default_limits();
    let r10 = lex_exact("cat /h/u/[!é]trium/secret", 0, None, &mut b).unwrap();
    assert!(
        unread_reasons(&r10)
            .iter()
            .any(|r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte } if *byte >= 0x80))
    );
    assert!(
        unread_reasons(&r10)
            .iter()
            .any(|r| matches!(r, UnreadReason::NegatedGlobClass))
    );

    // Row 20: 600 segments (> `MAX_UNITS = 512`) -> `TooLarge(Cap::Units)`.
    let row20 = "cat a; ".repeat(600);
    let mut b = ComputeBudget::with_default_limits();
    assert_eq!(
        lex_exact(&row20, 0, None, &mut b),
        Err(PartOutcome::TooLarge(Cap::Units))
    );

    // Row 25: `cat <&3` -> `DescriptorOutsideStd { fd: 3 }`.
    let mut b = ComputeBudget::with_default_limits();
    let r25 = lex_exact("cat <&3", 0, None, &mut b).unwrap();
    assert!(
        unread_reasons(&r25)
            .iter()
            .any(|r| matches!(r, UnreadReason::DescriptorOutsideStd { fd: 3 }))
    );
}

// ---------------------------------------------------------------------------
// CR-19 command-word folding (M1-27)
// ---------------------------------------------------------------------------

#[test]
fn fold_command_word_cr19_rules_and_no_unicode_normalization() {
    let cases = [
        ("SUDO", "sudo"),
        ("/usr/bin/sudo", "sudo"),
        ("/BIN/BASH", "bash"),
        ("/bin/ba\u{017f}h", "bash"),
        ("/bin/\u{212a}sh", "ksh"),
        ("CAFÉ", "café"),
        // NFD `e` + U+0301 is NOT normalized to NFC `é`.
        ("Cafe\u{0301}", "cafe\u{0301}"),
    ];
    for (input, want) in cases {
        let mut b = ComputeBudget::with_default_limits();
        let w = Word::derived(input.as_bytes().to_vec());
        let got = fold_command_word(&w, &FoldRules::REV4, &mut b).unwrap();
        assert_eq!(got, want, "input `{input}`");
    }

    // Respects `FoldRules` toggles.
    let mut b = ComputeBudget::with_default_limits();
    let w = Word::derived("ba\u{017f}h".as_bytes().to_vec());
    let no_fold = FoldRules {
        fold_long_s: false,
        fold_kelvin: false,
    };
    assert_eq!(
        fold_command_word(&w, &no_fold, &mut b).unwrap(),
        "ba\u{017f}h"
    );
    let w2 = Word::derived("\u{212a}sh".as_bytes().to_vec());
    assert_eq!(
        fold_command_word(&w2, &no_fold, &mut b).unwrap(),
        "\u{212a}sh"
    );

    // Review round 1 finding 5: string-level `str::to_lowercase()` preserves
    // contextual Greek final-sigma folding (`'ΟΣ'` -> `"ος"`, not `"οσ"`).
    let mut b_lex = ComputeBudget::with_default_limits();
    let lexed_sigma = lex_exact("'ΟΣ' arg", 0, None, &mut b_lex).unwrap();
    assert!(unread_reasons(&lexed_sigma).is_empty());
    assert_eq!(
        fold_command_word(&lexed_sigma.units[0].argv[0], &FoldRules::REV4, &mut b_lex).unwrap(),
        "ος"
    );
    for (input, rules, want) in [
        ("/usr/bin/ΟΣ", FoldRules::REV4, "ος"),
        ("ΟΣΑ", FoldRules::REV4, "οσα"),
        ("Σ", FoldRules::REV4, "σ"),
        ("ΟΣ\u{017f}", FoldRules::REV4, "οσs"),
        ("\u{017f}ΟΣ", FoldRules::REV4, "sος"),
        ("ΟΣ\u{212a}", no_fold, "οσ\u{212a}"),
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let w = Word::derived(input.as_bytes().to_vec());
        assert_eq!(
            fold_command_word(&w, &rules, &mut b).unwrap(),
            want,
            "input `{input}`"
        );
    }

    // Unicode folding charges allocation before calling `to_lowercase()`.
    let mut b_tight = ComputeBudget::new(1_000, 2);
    let w_sigma = Word::derived("ΟΣ".as_bytes().to_vec());
    assert_eq!(
        fold_command_word(&w_sigma, &FoldRules::REV4, &mut b_tight),
        Err(Exhausted {
            reason: ExhaustReason::AllocationBudget
        })
    );
}

#[test]
fn lex_exact_allows_permitted_quotes_and_escapes_inside_glob_class() {
    // Review round 1 finding 6: `cat /usr/bi[n'n']` uses an allowed non-negated
    // class and valid single quoting; accepted by sh, bash, and zsh.
    let mut b = ComputeBudget::with_default_limits();
    let out = lex_exact("cat /usr/bi[n'n']", 0, None, &mut b).unwrap();
    assert!(unread_reasons(&out).is_empty(), "{:?}", out.outcomes);
    let w = &out.units[0].argv[1];
    assert_eq!(w.literal_bytes(), b"/usr/bi[nn]");
    assert!(w.has_unquoted_glob());
    assert_eq!(w.provenance()[7], ByteProvenance::Unquoted); // `[`
    assert_eq!(w.provenance()[8], ByteProvenance::Unquoted); // `n`
    assert_eq!(w.provenance()[9], ByteProvenance::SingleQuoted); // `'n'`
    assert_eq!(w.provenance()[10], ByteProvenance::Unquoted); // `]`

    // Double quotes and permitted escapes inside a class also retain provenance.
    let mut b2 = ComputeBudget::with_default_limits();
    let out_dq = lex_exact(r#"cat /usr/bi[n"n"]"#, 0, None, &mut b2).unwrap();
    assert!(unread_reasons(&out_dq).is_empty(), "{:?}", out_dq.outcomes);
    assert_eq!(out_dq.units[0].argv[1].literal_bytes(), b"/usr/bi[nn]");
    assert!(out_dq.units[0].argv[1].has_unquoted_glob());
    assert_eq!(
        out_dq.units[0].argv[1].provenance()[9],
        ByteProvenance::DoubleQuoted
    );

    let mut b3 = ComputeBudget::with_default_limits();
    let out_esc = lex_exact(r"cat /usr/bi[n\n]", 0, None, &mut b3).unwrap();
    assert!(
        unread_reasons(&out_esc).is_empty(),
        "{:?}",
        out_esc.outcomes
    );
    assert_eq!(out_esc.units[0].argv[1].literal_bytes(), b"/usr/bi[nn]");
    assert!(out_esc.units[0].argv[1].has_unquoted_glob());
    assert_eq!(
        out_esc.units[0].argv[1].provenance()[9],
        ByteProvenance::Escaped
    );

    // Escaped `]` inside class does not close the class.
    let mut b4 = ComputeBudget::with_default_limits();
    let out_esc_close = lex_exact(r"cat /usr/bi[n\]a]", 0, None, &mut b4).unwrap();
    assert!(
        unread_reasons(&out_esc_close).is_empty(),
        "{:?}",
        out_esc_close.outcomes
    );
    assert_eq!(
        out_esc_close.units[0].argv[1].literal_bytes(),
        b"/usr/bi[n]a]"
    );
    assert_eq!(
        out_esc_close.units[0].argv[1].provenance()[9],
        ByteProvenance::Escaped
    );

    // Escaped `]` supplies a literal class member; a following unquoted `]`
    // closes the class (accepted under §3 with Escaped provenance preserved).
    let mut b5 = ComputeBudget::with_default_limits();
    let out_esc_end = lex_exact(r"cat /usr/bi[n\]]", 0, None, &mut b5).unwrap();
    assert!(
        unread_reasons(&out_esc_end).is_empty(),
        "escaped `]` followed by unquoted `]` must be accepted: {:?}",
        out_esc_end.outcomes
    );
    assert_eq!(out_esc_end.units[0].argv[1].literal_bytes(), b"/usr/bi[n]]");
    assert!(out_esc_end.units[0].argv[1].has_unquoted_glob());
    assert_eq!(
        out_esc_end.units[0].argv[1].provenance()[9],
        ByteProvenance::Escaped
    );
    assert_eq!(
        out_esc_end.units[0].argv[1].provenance()[10],
        ByteProvenance::Unquoted
    );

    // Without a trailing unquoted `]`, `\]` leaves the class genuinely unclosed.
    let mut b6 = ComputeBudget::with_default_limits();
    let out_unclosed = lex_exact(r"cat /usr/bi[n\]", 0, None, &mut b6).unwrap();
    assert!(
        unread_reasons(&out_unclosed)
            .iter()
            .any(|r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte: b'[' })),
        "{:?}",
        out_unclosed.outcomes
    );
    assert_eq!(out_unclosed.units[0].argv[1].literal_bytes(), b"/usr/bi[n]");
    assert_eq!(
        out_unclosed.units[0].argv[1].provenance()[9],
        ByteProvenance::Escaped
    );

    // Negation, empty quoted class, unclosed class, and disallowed quotes/escapes stay Unread.
    for line in ["cat /usr/bi['!'a]", r#"cat /usr/bi["^"a]"#] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::NegatedGlobClass)),
            "`{line}`: {:?}",
            out.outcomes
        );
    }
    for line in [
        r#"cat /usr/bi["$X"]"#,
        r"cat /usr/bi[n\ ]",
        "cat /usr/bi['x",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::UnsupportedQuoteOrEscape)),
            "`{line}`: {:?}",
            out.outcomes
        );
    }
    for line in ["cat /usr/bi['']", "cat /usr/bi[n'n'"] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::UnquotedByteOutsideModel { byte: b'[' })),
            "`{line}`: {:?}",
            out.outcomes
        );
    }
}

#[test]
fn lex_exact_empty_quotes_suppress_keywords_and_assignments() {
    // Review round 1 finding 7: `''if foo` is parsed as command word `"if"`,
    // not reserved word `if`, by sh, bash, and zsh.
    for (line, want_cmd) in [
        ("''if foo", b"if".as_slice()),
        (r#"""if foo"#, b"if".as_slice()),
        ("if'' foo", b"if".as_slice()),
        (r#"for"" x"#, b"for".as_slice()),
        ("''while x", b"while".as_slice()),
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out).is_empty(),
            "empty-quoted keyword `{line}` must not be Unread: {:?}",
            out.outcomes
        );
        assert_eq!(out.units.len(), 1, "`{line}`");
        assert_eq!(out.units[0].argv[0].literal_bytes(), want_cmd, "`{line}`");
    }

    // Empty quotes before `=` suppress leading assignment recognition.
    for line in [
        "''FOO=1 cat a",
        "FOO''=1 cat a",
        r#"F""OO=1 cat a"#,
        "FOO+''=1 cat a",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out).is_empty(),
            "quoted-name `{line}` must not be Assignment: {:?}",
            out.outcomes
        );
    }

    // Quotes AFTER `=` (in the value) remain a leading assignment.
    for line in ["FOO='' cat a", r#"FOO="" cat a"#, r#"FOO="bar" cat a"#] {
        let mut b = ComputeBudget::with_default_limits();
        let out = lex_exact(line, 0, None, &mut b).unwrap();
        assert!(
            unread_reasons(&out)
                .iter()
                .any(|r| matches!(r, UnreadReason::Assignment)),
            "value-quoted `{line}` must still emit Assignment: {:?}",
            out.outcomes
        );
    }

    // Empty quote in `2>&''-` prevents plain-unquoted close recognition.
    let mut b = ComputeBudget::with_default_limits();
    let out = lex_exact("cat 2>&''-", 0, None, &mut b).unwrap();
    assert!(
        unread_reasons(&out)
            .iter()
            .any(|r| matches!(r, UnreadReason::UnsupportedRedirect { .. })),
        "{:?}",
        out.outcomes
    );
}

// ---------------------------------------------------------------------------
// §2 Best-effort lexer & M1-27 / M1-31 carried scans
// ---------------------------------------------------------------------------

#[test]
fn lex_best_effort_extracts_section_10_and_m1_27_derived_units() {
    // §10 row 4: `sh -c 'X="$1" sudo id' _ foo`
    let mut b = ComputeBudget::with_default_limits();
    let r4 = lex_best_effort(&["sh -c 'X=\"$1\" sudo id' _ foo"], &mut b).unwrap();
    assert!(
        r4.units.iter().any(|u| u.derived
            && u.depth == 1
            && u.effective_argv == ["sudo", "id"]
            && u.assignments == [("X".to_string(), "$1".to_string())]),
        "{:?}",
        r4.units
    );

    // §10 row 5: `command` x13 `sudo id`
    let row5 = format!("{}sudo id", "command ".repeat(13));
    let mut b = ComputeBudget::with_default_limits();
    let r5 = lex_best_effort(&[&row5], &mut b).unwrap();
    assert_eq!(r5.units[0].effective_argv, ["sudo", "id"]);

    // M1-27 CR-19 folded shells, wrappers, substitutions, find -exec, eval, crontab.
    let m1_27_cases: &[(&str, &[&str])] = &[
        (
            "/bin/BASH -c 'cat ~/.ssh/id_rsa'",
            &["cat", "~/.ssh/id_rsa"],
        ),
        ("/bin/ba\u{017f}h -c 'rm -rf ~'", &["rm", "-rf", "~"]),
        (
            "/bin/\u{212a}sh -c 'cat ~/.ssh/id_rsa'",
            &["cat", "~/.ssh/id_rsa"],
        ),
        ("/usr/bin/ENV rm -rf ~", &["rm", "-rf", "~"]),
        (
            "ENV -S 'sh -c' 'cat ~/.ssh/id_rsa'",
            &["cat", "~/.ssh/id_rsa"],
        ),
        ("NICE sudo id", &["sudo", "id"]),
        ("TIMEOUT 5 sudo id", &["sudo", "id"]),
        ("SCRIPT -c 'rm -rf ~' /dev/null", &["rm", "-rf", "~"]),
        ("FIND . -exec sudo id \\;", &["sudo", "id"]),
        ("ARCH -arm64 sudo id", &["sudo", "id"]),
        ("CHROOT /tmp sudo id", &["sudo", "id"]),
        ("EVAL 'cat ~/.ssh/id_rsa'", &["cat", "~/.ssh/id_rsa"]),
        (
            "CRONTAB <<EOF\n* * * * * rm -rf ~\nEOF",
            &["rm", "-rf", "~"],
        ),
        ("NICE TIMEOUT 5 ENV -i SUDO id", &["SUDO", "id"]),
        ("echo 'sudo id' | BASH", &["sudo", "id"]),
        ("command env -S 'sh -c' 'sudo id'", &["sudo", "id"]),
        ("nice script -c 'sudo id' /dev/null", &["sudo", "id"]),
        ("X+=foo sudo id", &["sudo", "id"]),
        (r"echo $'a\'b' && sudo id", &["sudo", "id"]),
    ];
    for &(line, want_eff) in m1_27_cases {
        let mut b = ComputeBudget::with_default_limits();
        let rec = lex_best_effort(&[line], &mut b).unwrap();
        let found = rec
            .units
            .iter()
            .any(|u| u.effective_argv == want_eff || u.alt_argv.iter().any(|a| a == want_eff));
        assert!(found, "line `{line}`: units={:?}", rec.units);
    }

    // `$(/usr/bin/WHICH sudo) id` produces alt reading `["sudo", "id"]`.
    let mut b = ComputeBudget::with_default_limits();
    let rec = lex_best_effort(&["$(/usr/bin/WHICH sudo) id"], &mut b).unwrap();
    assert!(
        rec.units
            .iter()
            .any(|u| u.alt_argv.iter().any(|a| a == &["sudo", "id"])),
        "{:?}",
        rec.units
    );

    // Redirects recorded cleanly in best-effort units.
    let mut b = ComputeBudget::with_default_limits();
    let rec = lex_best_effort(&["cat < a &> b 2>&1"], &mut b).unwrap();
    assert_eq!(
        rec.units[0].redirects,
        vec![
            (BestEffortRedirectKind::Read, "a".to_string()),
            (BestEffortRedirectKind::Write, "b".to_string()),
            (BestEffortRedirectKind::Dup, "1".to_string()),
        ]
    );
}

#[test]
fn lex_best_effort_heredocs_follow_d141_code_runner_vs_conditional_runner() {
    // Unconditional `ToolRowKind::CodeRunner` tools extract heredoc bodies in Scan 7.
    for line in [
        "ssh host <<EOF\nsudo id\nEOF",
        "parallel <<EOF\nsudo id\nEOF",
        "awk -f /dev/stdin <<EOF\nsudo id\nEOF",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let rec = lex_best_effort(&[line], &mut b).unwrap();
        assert!(
            rec.units
                .iter()
                .any(|u| u.derived && u.effective_argv == ["sudo", "id"]),
            "expected derived unit from CodeRunner heredoc `{line}`: {:?}",
            rec.units
        );
    }

    // `ToolRowKind::ConditionalRunner` (`find`, `git`, `sed`) do not feed
    // arbitrary heredoc data as shell commands in Scan 7 (`find -exec` is
    // still handled by Scan 5).
    for line in [
        "git commit -F - <<EOF\nsudo id\nEOF",
        "sed 's/a/b/' <<EOF\nsudo id\nEOF",
        "find . <<EOF\nsudo id\nEOF",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let rec = lex_best_effort(&[line], &mut b).unwrap();
        assert!(
            !rec.units
                .iter()
                .any(|u| u.derived && u.effective_argv == ["sudo", "id"]),
            "ConditionalRunner heredoc must not derive fake command `{line}`: {:?}",
            rec.units
        );
    }
}

#[test]
fn lex_best_effort_sed_stdin_script_heredoc_limit_matches_dev() {
    // Documents the round-2 D-141 Scan 7 decision limit: `sed -f -` /
    // `sed --file=-` with a heredoc can supply an `e` command on stdin, but
    // excluding all `ToolRowKind::ConditionalRunner` rows from Scan 7 omits
    // that body — matching `dev`'s `reads_stdin_as_script` (and per §2, misses
    // on Unread parts are outside the promise). Exact lexing still marks `<<`
    // as Unread.
    for line in [
        "sed -f - <<EOF\n1e sudo id\nEOF",
        "sed --file=- <<EOF\n1e sudo id\nEOF",
    ] {
        let mut b_be = ComputeBudget::with_default_limits();
        let rec = lex_best_effort(&[line], &mut b_be).unwrap();
        assert!(
            !rec.units.iter().any(|u| u.derived),
            "sed ConditionalRunner heredoc is intentionally not scanned as code: `{line}` -> {:?}",
            rec.units
        );

        let mut b_ex = ComputeBudget::with_default_limits();
        let exact = lex_exact(line, 0, None, &mut b_ex).unwrap();
        assert!(
            unread_reasons(&exact).iter().any(|r| matches!(
                r,
                UnreadReason::ReservedWordOrConstruct { construct } if construct == "<<"
            )),
            "exact lexer still marks heredoc Unread for `{line}`: {:?}",
            exact.outcomes
        );
    }
}

#[test]
fn brace_expand_budgeted_carries_m1_31_n11_guarantees() {
    let mut b = ComputeBudget::with_default_limits();
    assert_eq!(
        brace_expand_budgeted("a{b,c}d", &mut b).unwrap(),
        Some(vec!["acd".into(), "abd".into()])
    );
    assert_eq!(
        brace_expand_budgeted("{01..05..2}", &mut b).unwrap(),
        Some(vec!["05".into(), "03".into(), "01".into()])
    );
    assert_eq!(
        brace_expand_budgeted("{a..c}", &mut b).unwrap(),
        Some(vec!["c".into(), "b".into(), "a".into()])
    );

    // M1-31 review 1 finding 6: full i64 range does not overflow/panic.
    let mut b = ComputeBudget::with_default_limits();
    let r = brace_expand_budgeted("{9223372036854775807..-9223372036854775808}", &mut b).unwrap();
    assert_eq!(
        r,
        Some(vec!["{9223372036854775807..-9223372036854775808}".into()])
    );

    // M1-31 review 1 finding 7: deeply nested braces are linear.
    let nested = format!("{}x{}", "{".repeat(10_000), "}".repeat(10_000));
    let mut b = ComputeBudget::with_default_limits();
    let start_units = b.remaining_units();
    let res = brace_expand_budgeted(&nested, &mut b).unwrap();
    assert!(res.is_some());
    let charged = start_units - b.remaining_units();
    // Round 5 charges every pass: the `{`/`$` scan (1), the lower bound's
    // char copy, pairing walk and pair/stack growth (about 4), the initial
    // copy (1), and the expansion's char copy, matching walk and stack growth
    // (about 3): about 9 units per byte, linear. The bound was 8 before those
    // second walks were charged.
    assert!(
        charged < (nested.len() as u64) * 12,
        "nested braces must be linear, charged {charged} for len {}",
        nested.len()
    );

    // M1-31 review 1 finding 10 (review round 1 finding 8): `$`-bearing words
    // skip the naive lower-bound shortcut because introducing `$` immediately
    // before the next `{` suppresses its expansion; `"{,$}".repeat(9)` expands
    // to 89 bounded results (`<= MAX_BRACE_RESULTS = 256`), not 512.
    let dollar_word = "{,$}".repeat(9);
    let mut b = ComputeBudget::with_default_limits();
    let expanded = brace_expand_budgeted(&dollar_word, &mut b)
        .unwrap()
        .expect("9x `{,$}` produces 89 bounded results (M1-31 review 1 finding 10)");
    assert_eq!(expanded.len(), 89);

    let small_dollar = "{,$}{a,b}";
    let mut b2 = ComputeBudget::with_default_limits();
    assert!(
        brace_expand_budgeted(small_dollar, &mut b2)
            .unwrap()
            .is_some()
    );

    // Separate genuinely overflowing cases (both non-`$` shortcut and `$`-bearing
    // expansion exceeding `MAX_BRACE_RESULTS`).
    let overflow_plain = "{a,b}".repeat(9);
    let mut b3 = ComputeBudget::with_default_limits();
    assert!(
        brace_expand_budgeted(&overflow_plain, &mut b3)
            .unwrap()
            .is_none()
    );
    let overflow_dollar = "{,$}".repeat(12);
    let mut b4 = ComputeBudget::with_default_limits();
    assert!(
        brace_expand_budgeted(&overflow_dollar, &mut b4)
            .unwrap()
            .is_none()
    );
}

#[test]
fn exhaustion_at_flush_preserves_pending_unit_words_and_reasons() {
    // Review round 1 finding 1: exhausting at `flush_unit` (the run's last
    // compute charge) keeps the pending unit with both words.
    let (c, _) = exact_cost("sudo id");
    let mut b = ComputeBudget::new(c - 1, BIG_ALLOC);
    let out = lex_exact("sudo id", 0, None, &mut b).unwrap();
    assert!(
        out.outcomes.iter().any(|pr| matches!(
            pr.outcome,
            PartOutcome::Exhausted(ExhaustReason::ComputeBudget)
        )),
        "{:?}",
        out.outcomes
    );
    assert_eq!(out.units.len(), 1, "pending unit must not be dropped");
    assert_eq!(out.units[0].argv[0].literal_bytes(), b"sudo");
    assert_eq!(out.units[0].argv[1].literal_bytes(), b"id");

    // The pending unit's Unread reasons are kept alongside its words.
    let (c_r, _) = exact_cost("sudo $id");
    let mut b_reasons = ComputeBudget::new(c_r - 1, BIG_ALLOC);
    let out_r = lex_exact("sudo $id", 0, None, &mut b_reasons).unwrap();
    assert!(has_exhausted(&out_r), "{:?}", out_r.outcomes);
    assert_eq!(out_r.units.len(), 1);
    assert_eq!(out_r.units[0].argv[0].literal_bytes(), b"sudo");
    assert_eq!(out_r.units[0].argv[1].literal_bytes(), b"$id");
    assert!(
        out_r.outcomes.iter().any(|pr| matches!(
            pr,
            PartResult {
                part: PartId::Unit(UnitIndex(0)),
                outcome: PartOutcome::Unread(UnreadReason::UnquotedByteOutsideModel { byte: b'$' })
            }
        )),
        "pending unit reasons must be preserved: {:?}",
        out_r.outcomes
    );

    // Allocation: the unit's slots are reserved before its first word, so
    // the last allocation of `sudo id` is the buffers of `id`. Running out
    // there keeps the finished `sudo` (and drops the unfinished `id` whole).
    let (_, a) = exact_cost("sudo id");
    let mut b_alloc = ComputeBudget::new(BIG_UNITS, a - 1);
    let out_a = lex_exact("sudo id", 0, None, &mut b_alloc).unwrap();
    assert!(out_a.outcomes.iter().any(|pr| matches!(
        pr.outcome,
        PartOutcome::Exhausted(ExhaustReason::AllocationBudget)
    )));
    assert_eq!(out_a.units.len(), 1);
    assert_eq!(out_a.units[0].argv.len(), 1);
    assert_eq!(out_a.units[0].argv[0].literal_bytes(), b"sudo");

    // Review round 1 finding 1: best-effort `flush_builder` / conversion keep
    // the pending unit when the budget runs out one unit short of a clean run.
    let clean = |b: &mut ComputeBudget| lex_best_effort(&["sudo id"], b).is_ok();
    let need = min_clean_budget(true, clean);
    let mut b_be = ComputeBudget::new(need - 1, BIG_ALLOC);
    let mut rec = BestEffortRecord::default();
    let err = lex_best_effort_into(&["sudo id"], &mut rec, &mut b_be);
    assert_eq!(
        err,
        Err(PartOutcome::Exhausted(ExhaustReason::ComputeBudget))
    );
    assert_eq!(
        rec.units.len(),
        1,
        "best-effort pending unit must not be dropped"
    );
    assert_eq!(rec.units[0].raw_argv, ["sudo", "id"]);
    assert_eq!(rec.units[0].effective_argv, ["sudo", "id"]);
}

#[test]
fn auxiliary_allocations_and_cold_memo_copies_are_budgeted() {
    // Review round 1 finding 2: exact failing case — deeply nested braces grow
    // auxiliary stack/pair vectors in both lower-bound and expansion passes, so
    // `ComputeBudget::new(1_000_000, 200_100)` must exhaust AllocationBudget.
    let nested = format!("{}x{}", "{".repeat(10_000), "}".repeat(10_000));
    let mut b_tight = ComputeBudget::new(1_000_000, 200_100);
    assert_eq!(
        brace_expand_budgeted(&nested, &mut b_tight),
        Err(Exhausted {
            reason: ExhaustReason::AllocationBudget
        })
    );

    // With default limits, the full auxiliary allocation is accounted for.
    let mut b_def = ComputeBudget::with_default_limits();
    assert!(
        brace_expand_budgeted(&nested, &mut b_def)
            .unwrap()
            .is_some()
    );
    let charged_alloc = limits::COMPUTE_BUDGET_ALLOC_BYTES - b_def.remaining_alloc_bytes();
    assert!(
        charged_alloc > 700_000,
        "expected auxiliary stack/pair allocations to be charged, got {charged_alloc}"
    );

    // Cold memo insertion charges its clone before copying, and is skipped
    // (without exhausting) when the remaining budget cannot afford the cache copy.
    let line_full = "cat memo_budget_case_alpha; echo memo_budget_case_beta";
    let mut b_full = ComputeBudget::with_default_limits();
    let rec_full = lex_best_effort(&[line_full], &mut b_full).unwrap();
    let alloc_with_memo = limits::COMPUTE_BUDGET_ALLOC_BYTES - b_full.remaining_alloc_bytes();
    let units_with_memo = limits::COMPUTE_BUDGET_UNITS - b_full.remaining_units();

    // Warm hit charges identically.
    let mut b_warm = ComputeBudget::with_default_limits();
    let rec_warm = lex_best_effort(&[line_full], &mut b_warm).unwrap();
    assert_eq!(rec_full, rec_warm);
    assert_eq!(
        limits::COMPUTE_BUDGET_UNITS - b_warm.remaining_units(),
        units_with_memo
    );
    assert_eq!(
        limits::COMPUTE_BUDGET_ALLOC_BYTES - b_warm.remaining_alloc_bytes(),
        alloc_with_memo
    );

    // A fresh identical-length line given `alloc_with_memo - 100` bytes can
    // finish lexing cleanly by skipping the optional memo insertion copy.
    let line_skip = "cat memo_budget_skip_alpha; echo memo_budget_skip_beta";
    assert_eq!(line_skip.len(), line_full.len());
    let mut b_skip = ComputeBudget::new(units_with_memo * 2, alloc_with_memo - 100);
    let rec_skip = lex_best_effort(&[line_skip], &mut b_skip).unwrap();
    assert_eq!(rec_skip.units.len(), 2);
    assert!(!b_skip.is_exhausted());
}

#[test]
fn cached_best_effort_hits_assign_fresh_pipeline_ids() {
    // Review round 1 finding 4: warm thread-local memo with `curl x | cat`,
    // then lex `["curl x | cat", "echo ok | sh"]`. Both pipelines must receive
    // distinct `PipelineId`s.
    let mut b_warm = ComputeBudget::with_default_limits();
    let _ = lex_best_effort(&["curl x | cat"], &mut b_warm).unwrap();

    let mut b_two = ComputeBudget::with_default_limits();
    let rec = lex_best_effort(&["curl x | cat", "echo ok | sh"], &mut b_two).unwrap();
    let p_curl = rec
        .units
        .iter()
        .find(|u| u.raw_argv == ["curl", "x"])
        .and_then(|u| u.pipeline_id)
        .expect("curl pipeline_id");
    let p_cat = rec
        .units
        .iter()
        .find(|u| u.raw_argv == ["cat"])
        .and_then(|u| u.pipeline_id)
        .expect("cat pipeline_id");
    let p_echo = rec
        .units
        .iter()
        .find(|u| u.raw_argv == ["echo", "ok"])
        .and_then(|u| u.pipeline_id)
        .expect("echo pipeline_id");
    let p_sh = rec
        .units
        .iter()
        .find(|u| u.raw_argv == ["sh"])
        .and_then(|u| u.pipeline_id)
        .expect("sh pipeline_id");
    assert_eq!(p_curl, p_cat);
    assert_eq!(p_echo, p_sh);
    assert_ne!(
        p_curl, p_echo,
        "cached pipeline and newly parsed pipeline must not alias PipelineId"
    );

    // Two memo hits for the same text in one call must also get distinct PipelineIds.
    let mut b_dup = ComputeBudget::with_default_limits();
    let rec_dup = lex_best_effort(&["curl x | cat", "curl x | cat"], &mut b_dup).unwrap();
    assert_eq!(rec_dup.units.len(), 4);
    assert_eq!(rec_dup.units[0].pipeline_id, rec_dup.units[1].pipeline_id);
    assert_eq!(rec_dup.units[2].pipeline_id, rec_dup.units[3].pipeline_id);
    assert_ne!(
        rec_dup.units[0].pipeline_id, rec_dup.units[2].pipeline_id,
        "repeated cached texts must get distinct PipelineIds"
    );

    // Repeating a cached nested script containing a pipeline (per-call
    // recursion cache hit) must assign distinct PipelineIds.
    let mut b_rec = ComputeBudget::with_default_limits();
    let rec_nested =
        lex_best_effort(&["sh -c 'curl a | wc'; sh -c 'curl a | wc'"], &mut b_rec).unwrap();
    let derived_pipes: Vec<(Vec<String>, Option<PipelineId>)> = rec_nested
        .units
        .iter()
        .filter(|u| u.derived)
        .map(|u| (u.effective_argv.clone(), u.pipeline_id))
        .collect();
    assert_eq!(derived_pipes.len(), 4, "{derived_pipes:?}");
    assert_eq!(derived_pipes[0].1, derived_pipes[1].1);
    assert_eq!(derived_pipes[2].1, derived_pipes[3].1);
    assert!(derived_pipes[0].1.is_some());
    assert_ne!(
        derived_pipes[0].1, derived_pipes[2].1,
        "cached nested script executions must not alias PipelineId"
    );

    // Cold vs warm equivalence across multiple texts.
    let texts = ["curl cold_warm_p1 | cat", "echo cold_warm_p2 | sh"];
    let mut b_c = ComputeBudget::with_default_limits();
    let cold = lex_best_effort(&texts, &mut b_c).unwrap();
    let mut b_w = ComputeBudget::with_default_limits();
    let warm = lex_best_effort(&texts, &mut b_w).unwrap();
    assert_eq!(cold, warm, "cold and warm multi-text lexing must match");
}

// ---------------------------------------------------------------------------
// §8 Budgets: exhaustion, preservation, TooLarge, and linear charge scaling
// ---------------------------------------------------------------------------

#[test]
fn budgets_exhaustion_preserves_early_units_and_never_allows_padded_lines() {
    // Exhaustion in `lex_exact`: early unit preserved + Exhausted recorded.
    // The budget covers the first unit (measured alone) and a little more,
    // far less than the 200 padding units.
    let padded = format!("cat ~/.ssh/id_rsa; {}", "echo a; ".repeat(200));
    let (first_unit, _) = exact_cost("cat ~/.ssh/id_rsa;");
    let mut b = ComputeBudget::new(first_unit + 20, BIG_ALLOC);
    let out = lex_exact(&padded, 0, None, &mut b).unwrap();
    assert!(has_exhausted(&out), "must record Exhausted");
    assert!(
        !out.units.is_empty(),
        "unit 0 found before exhaustion must not be lost"
    );
    assert_eq!(out.units[0].argv[0].literal_bytes(), b"cat");
    assert_eq!(out.units[0].argv[1].literal_bytes(), b"~/.ssh/id_rsa");

    // Exhaustion on allocation budget in `lex_exact`.
    // 16 bytes cannot even reserve the slot that records exhaustion, so
    // `lex_exact` reports it on entry, before lexing anything.
    let mut b_alloc = ComputeBudget::new(100_000, 16);
    assert_eq!(
        lex_exact("cat README.md", 0, None, &mut b_alloc),
        Err(PartOutcome::Exhausted(ExhaustReason::AllocationBudget))
    );
    let (_, a_cat) = exact_cost("cat README.md");
    let mut b_alloc_late = ComputeBudget::new(100_000, a_cat - 1);
    let out_alloc = lex_exact("cat README.md", 0, None, &mut b_alloc_late).unwrap();
    assert!(out_alloc.outcomes.iter().any(|pr| matches!(
        pr.outcome,
        PartOutcome::Exhausted(ExhaustReason::AllocationBudget)
    )));

    // Exhaustion in `fold_command_word` (compute & alloc).
    let w = Word::derived(b"/usr/bin/SUDO".to_vec());
    let mut b_fc = ComputeBudget::new(2, 100);
    assert_eq!(
        fold_command_word(&w, &FoldRules::REV4, &mut b_fc),
        Err(Exhausted {
            reason: ExhaustReason::ComputeBudget
        })
    );
    let mut b_fa = ComputeBudget::new(100, 1);
    assert_eq!(
        fold_command_word(&w, &FoldRules::REV4, &mut b_fa),
        Err(Exhausted {
            reason: ExhaustReason::AllocationBudget
        })
    );

    // Exhaustion in `lex_best_effort` (compute & alloc).
    let mut b_bec = ComputeBudget::new(10, 10_000);
    assert_eq!(
        lex_best_effort(&["cat README.md; echo hi"], &mut b_bec),
        Err(PartOutcome::Exhausted(ExhaustReason::ComputeBudget))
    );
    let mut b_bea = ComputeBudget::new(10_000, 10);
    assert_eq!(
        lex_best_effort(&["cat README.md; echo hi"], &mut b_bea),
        Err(PartOutcome::Exhausted(ExhaustReason::AllocationBudget))
    );

    // Exact `MAX_UNITS` (512) succeeds; `MAX_UNITS + 1` (513) -> `TooLarge(Cap::Units)`.
    let exact_512 = "echo a; ".repeat(limits::MAX_UNITS);
    let mut b_ok = ComputeBudget::with_default_limits();
    let out_512 = lex_exact(&exact_512, 0, None, &mut b_ok).unwrap();
    assert_eq!(out_512.units.len(), limits::MAX_UNITS);
    let over_513 = "echo a; ".repeat(limits::MAX_UNITS + 1);
    let mut b_over = ComputeBudget::with_default_limits();
    assert_eq!(
        lex_exact(&over_513, 0, None, &mut b_over),
        Err(PartOutcome::TooLarge(Cap::Units))
    );

    // Exact `MAX_BYTES` succeeds; `MAX_BYTES + 1` -> `TooLarge(Cap::Bytes)`.
    let exact_bytes = format!("cat {}", "a".repeat(limits::MAX_BYTES - 4));
    assert_eq!(exact_bytes.len(), limits::MAX_BYTES);
    let mut b_max = ComputeBudget::with_default_limits();
    assert!(lex_exact(&exact_bytes, 0, None, &mut b_max).is_ok());

    // `MAX_BYTES` exceeded -> `TooLarge(Cap::Bytes)`.
    let over = "a".repeat(limits::MAX_BYTES + 1);
    let mut b = ComputeBudget::with_default_limits();
    assert_eq!(
        lex_exact(&over, 0, None, &mut b),
        Err(PartOutcome::TooLarge(Cap::Bytes))
    );
    let mut b2 = ComputeBudget::with_default_limits();
    assert_eq!(
        lex_best_effort(&[&over], &mut b2),
        Err(PartOutcome::TooLarge(Cap::Bytes))
    );

    // N11 best-effort memo determinism: repeated call charges identical
    // compute + alloc units, while a tight budget bypasses the memo and still
    // exhausts while preserving early units.
    let memo_line = "cat ~/.ssh/id_rsa; echo ok; echo done";
    let mut bm1 = ComputeBudget::with_default_limits();
    let rec1 = lex_best_effort(&[memo_line], &mut bm1).unwrap();
    let u1 = limits::COMPUTE_BUDGET_UNITS - bm1.remaining_units();
    let a1 = limits::COMPUTE_BUDGET_ALLOC_BYTES - bm1.remaining_alloc_bytes();

    let mut bm2 = ComputeBudget::with_default_limits();
    let rec2 = lex_best_effort(&[memo_line], &mut bm2).unwrap();
    let u2 = limits::COMPUTE_BUDGET_UNITS - bm2.remaining_units();
    let a2 = limits::COMPUTE_BUDGET_ALLOC_BYTES - bm2.remaining_alloc_bytes();
    assert_eq!(rec1, rec2);
    assert_eq!(u1, u2, "memo hit must charge identical compute units");
    assert_eq!(a1, a2, "memo hit must charge identical alloc bytes");

    // Enough to finish `cat` (and start the next word), far short of the
    // line: entry (2) + raw-text copy + worst-case memo lookup, plus 64.
    let pre_scan = 2 + memo_line.len() as u64 + 4 * (1 + memo_line.len() as u64);
    let mut bm_tight = ComputeBudget::new(pre_scan + 64, 100_000);
    let mut partial_rec = BestEffortRecord::default();
    let err = lex_best_effort_into(&[memo_line], &mut partial_rec, &mut bm_tight);
    assert_eq!(
        err,
        Err(PartOutcome::Exhausted(ExhaustReason::ComputeBudget))
    );
    assert!(
        !partial_rec.units.is_empty(),
        "partial best-effort units scanned before exhaustion must be preserved"
    );
    assert_eq!(partial_rec.units[0].raw_argv[0], "cat");
}

#[test]
fn best_effort_repeated_equals_after_invalid_assignment_prefix_is_linear() {
    // Review round 2 finding 1: repeated `=` after a long invalid assignment
    // prefix must stay strictly linear (incremental validity + only the first
    // unquoted `=` considered, never rescanning the prefix per `=`).
    let small_line = format!("echo {}-{}", "a".repeat(6_000), "=".repeat(6_000));
    let large_line = format!("echo {}-{}", "a".repeat(60_000), "=".repeat(60_000));
    assert!(large_line.len() <= limits::MAX_BYTES);

    let mut b_small = ComputeBudget::with_default_limits();
    let rec_small = lex_best_effort(&[&small_line], &mut b_small).unwrap();
    let c_small = limits::COMPUTE_BUDGET_UNITS - b_small.remaining_units();
    assert_eq!(rec_small.units.len(), 1);
    assert!(rec_small.units[0].assignments.is_empty());

    let mut b_large = ComputeBudget::with_default_limits();
    let rec_large = lex_best_effort(&[&large_line], &mut b_large).unwrap();
    let c_large = limits::COMPUTE_BUDGET_UNITS - b_large.remaining_units();
    assert_eq!(rec_large.units.len(), 1);
    assert!(rec_large.units[0].assignments.is_empty());
    assert_eq!(rec_large.units[0].raw_argv.len(), 2);
    assert_eq!(rec_large.units[0].raw_argv[0], "echo");
    assert_eq!(rec_large.units[0].raw_argv[1].len(), 120_001);

    // Affine linear upper bound and growth-rate bound (not O(n^2) ~ 3.6e9).
    let len_ratio = (large_line.len() as f64) / (small_line.len() as f64);
    assert!(
        c_large <= (large_line.len() as u64) * 16 + 4_096,
        "best-effort compute charge not linear on repeated `=`: len={}, charged={c_large}",
        large_line.len()
    );
    assert!(
        (c_large as f64) / (c_small as f64) <= len_ratio * 1.5,
        "best-effort compute growth super-linear: small={c_small}, large={c_large}, len_ratio={len_ratio:.2}"
    );

    // Exact lexer also stays linear on the same input and emits no Assignment.
    let mut b_ex = ComputeBudget::with_default_limits();
    let ex = lex_exact(&large_line, 0, None, &mut b_ex).unwrap();
    let c_ex = limits::COMPUTE_BUDGET_UNITS - b_ex.remaining_units();
    assert!(unread_reasons(&ex).is_empty(), "{:?}", ex.outcomes);
    assert!(
        c_ex <= (large_line.len() as u64) * 8 + 4_096,
        "lex_exact compute charge not linear: charged={c_ex}"
    );

    // Verify incremental assignment edge cases in best-effort lexing.
    let mut b1 = ComputeBudget::with_default_limits();
    let r1 = lex_best_effort(&["FOO=a=b echo hi"], &mut b1).unwrap();
    assert_eq!(
        r1.units[0].assignments,
        vec![("FOO".to_string(), "a=b".to_string())]
    );
    assert_eq!(r1.units[0].raw_argv, ["echo", "hi"]);

    let mut b2 = ComputeBudget::with_default_limits();
    let r2 = lex_best_effort(&["FOO+=1=2 echo hi"], &mut b2).unwrap();
    assert_eq!(
        r2.units[0].assignments,
        vec![("FOO".to_string(), "1=2".to_string())]
    );
    assert_eq!(r2.units[0].raw_argv, ["echo", "hi"]);

    for non_assign in ["FOO++=1 echo hi", "''FOO=1 echo hi", "+=1 echo hi"] {
        let mut b = ComputeBudget::with_default_limits();
        let r = lex_best_effort(&[non_assign], &mut b).unwrap();
        assert!(
            r.units[0].assignments.is_empty(),
            "`{non_assign}` must not be treated as assignment: {:?}",
            r.units[0].assignments
        );
    }
}

#[test]
fn lex_exact_word_buffer_capacity_and_realloc_copy_are_budgeted() {
    // Review round 2 finding 2: a word's byte and provenance buffers are
    // charged by capacity before allocating. Measured as the difference a
    // second (non-command) word makes, so fixed per-line reservations cancel.
    let (c_base, a_base) = exact_cost("a");
    let (c_one, a_one) = exact_cost("a b");
    let (c_eight, a_eight) = exact_cost("a bcdefghi");
    let (c_nine, a_nine) = exact_cost("a bcdefghij");

    // A 1-byte word allocates 8 + 8 = 16 bytes; 8 bytes still fit.
    assert_eq!(a_one - a_base, 16);
    assert_eq!(a_eight - a_base, 16);
    // A 9-byte word grows each buffer from 8 to 16: 32 bytes in all.
    assert_eq!(a_nine - a_base, 32);
    // Each byte costs a scan and a push; the 9th also pays the 8 + 8 copy
    // of the old buffers, charged before the reallocation.
    assert_eq!(c_nine - c_eight, 2 + 16);
    assert_eq!(c_eight - c_one, 7 * 2);
    assert!(c_one > c_base);

    // One byte short of the measured allocation exhausts on the second
    // word's buffers and keeps the finished first word; the exact amount
    // succeeds with nothing left over.
    let mut b_short = ComputeBudget::new(BIG_UNITS, a_one - 1);
    let out_short = lex_exact("a b", 0, None, &mut b_short).unwrap();
    assert!(out_short.outcomes.iter().any(|pr| matches!(
        pr.outcome,
        PartOutcome::Exhausted(ExhaustReason::AllocationBudget)
    )));
    assert_eq!(out_short.units.len(), 1);
    assert_eq!(out_short.units[0].argv.len(), 1);
    assert_eq!(out_short.units[0].argv[0].literal_bytes(), b"a");

    let mut b_exact = ComputeBudget::new(BIG_UNITS, a_one);
    let out_exact = lex_exact("a b", 0, None, &mut b_exact).unwrap();
    assert!(!has_exhausted(&out_exact), "{:?}", out_exact.outcomes);
    assert!(unread_reasons(&out_exact).is_empty());
    assert_eq!(out_exact.units[0].argv[1].literal_bytes(), b"b");
    assert_eq!(b_exact.remaining_alloc_bytes(), 0);

    // Compute one unit short of the 9-byte word's run exhausts ComputeBudget.
    let mut b_copy_short = ComputeBudget::new(c_nine - 1, BIG_ALLOC);
    let out_cs = lex_exact("a bcdefghij", 0, None, &mut b_copy_short).unwrap();
    assert!(out_cs.outcomes.iter().any(|pr| matches!(
        pr.outcome,
        PartOutcome::Exhausted(ExhaustReason::ComputeBudget)
    )));
}

#[test]
fn lex_best_effort_find_exec_recurses_into_nested_scripts_within_caps() {
    // Review round 2 finding 3: `find . -exec sh -c 'sudo id' \;` must produce
    // both the derived `["sh", "-c", "sudo id"]` unit and the recursively
    // derived `["sudo", "id"]` unit.
    let mut b = ComputeBudget::with_default_limits();
    let rec = lex_best_effort(&[r"find . -exec sh -c 'sudo id' \;"], &mut b).unwrap();
    assert!(
        rec.units
            .iter()
            .any(|u| u.derived && u.effective_argv == ["sh", "-c", "sudo id"]),
        "missing derived sh -c unit: {:?}",
        rec.units
    );
    assert!(
        rec.units
            .iter()
            .any(|u| u.derived && u.effective_argv == ["sudo", "id"]),
        "missing derived nested payload unit: {:?}",
        rec.units
    );

    // Also applies across `-execdir` / `+`, `eval`, alternative wrapper
    // readings, command-word substitutions, and nested `find -exec`.
    for line in [
        r#"find . -execdir bash -c 'eval "sudo id"' +"#,
        r"find . -exec command -Z arg sh -c 'sudo id' \;",
        r"find . -exec $(which sh) -c 'sudo id' \;",
        r"find . -exec find / -exec sh -c 'sudo id' \; \;",
    ] {
        let mut b = ComputeBudget::with_default_limits();
        let rec = lex_best_effort(&[line], &mut b).unwrap();
        assert!(
            rec.units
                .iter()
                .any(|u| u.derived && u.effective_argv == ["sudo", "id"]),
            "line `{line}` missing derived `sudo id` unit: {:?}",
            rec.units
        );
    }

    // Respects `limits::MAX_DEPTH`: deeply nested `find -exec` chains never
    // exceed `MAX_DEPTH`.
    let deep_find = "find . -exec find . -exec find . -exec find . -exec find . -exec find . -exec find . -exec sh -c 'sudo id' +";
    let mut b_depth = ComputeBudget::with_default_limits();
    let rec_depth = lex_best_effort(&[deep_find], &mut b_depth).unwrap();
    assert!(
        rec_depth.units.iter().all(|u| u.depth <= limits::MAX_DEPTH),
        "derived unit exceeded MAX_DEPTH: {:?}",
        rec_depth.units
    );
    assert!(
        !rec_depth
            .units
            .iter()
            .any(|u| u.effective_argv == ["sudo", "id"]),
        "recursion past MAX_DEPTH must stop: {:?}",
        rec_depth.units
    );

    // Respects `limits::MAX_UNITS`: too many derived units return TooLarge(Units).
    let many_execs = format!("find . {}", r"-exec sh -c 'echo x' \; ".repeat(300));
    assert!(many_execs.len() <= limits::MAX_BYTES);
    let mut b_units = ComputeBudget::with_default_limits();
    assert_eq!(
        lex_best_effort(&[&many_execs], &mut b_units),
        Err(PartOutcome::TooLarge(Cap::Units))
    );
}

#[test]
fn linear_charge_growth_from_10kb_to_max_bytes() {
    let make_single_unit = |target_len: usize| -> String {
        let mut s = String::with_capacity(target_len);
        s.push_str("cat ");
        while s.len() < target_len {
            s.push('a');
        }
        s
    };
    let make_multi_unit = |target_len: usize| -> String {
        let n_units = 64usize;
        let per = (target_len / n_units).saturating_sub(6);
        let mut s = String::with_capacity(target_len);
        for _ in 0..n_units {
            s.push_str("cat ");
            for _ in 0..per {
                s.push('x');
            }
            s.push_str("; ");
        }
        s
    };

    let big_units = 200_000_000u64;
    let big_alloc = 200 * 1024 * 1024usize;
    let small_len = 10 * 1024;
    let large_len = limits::MAX_BYTES;
    let small = make_single_unit(small_len);
    let large = make_single_unit(large_len);
    let len_ratio = (large.len() as f64) / (small.len() as f64);

    // `lex_exact` compute and allocation charges obey §8 affine linear upper
    // bounds `a * n + b` (single-unit & multi-unit, accounting for fixed unit
    // metadata overhead and power-of-two capacity growth without asserting a
    // spurious proportional lower bound).
    let mut bs = ComputeBudget::new(big_units, big_alloc);
    let _ = lex_exact(&small, 0, None, &mut bs).unwrap();
    let cs = big_units - bs.remaining_units();
    let as_ = big_alloc - bs.remaining_alloc_bytes();

    let mut bl = ComputeBudget::new(big_units, big_alloc);
    let _ = lex_exact(&large, 0, None, &mut bl).unwrap();
    let cl = big_units - bl.remaining_units();
    let al = big_alloc - bl.remaining_alloc_bytes();

    let single_fixed_alloc =
        4 * std::mem::size_of::<Word>() + 4 * std::mem::size_of::<RawCommandUnit>() + 1_024;
    let exact_ratio = (cl as f64) / (cs as f64);
    let exact_alloc_ratio = (al as f64) / (as_ as f64);
    assert!(cs > 0 && cl > cs);
    assert!(as_ > 0 && al > as_);
    assert!(
        cs <= 8 * (small.len() as u64) + 1_024
            && cl <= 8 * (large.len() as u64) + 1_024
            && exact_ratio <= len_ratio * 1.25,
        "lex_exact compute charge exceeds linear bound: small={cs}, large={cl}, ratio={exact_ratio:.2}, len_ratio={len_ratio:.2}"
    );
    assert!(
        as_ <= 4 * small.len() + single_fixed_alloc
            && al <= 4 * large.len() + single_fixed_alloc
            && exact_alloc_ratio <= len_ratio * 1.25,
        "lex_exact alloc charge exceeds affine bound: small={as_}, large={al}, ratio={exact_alloc_ratio:.2}, len_ratio={len_ratio:.2}"
    );

    let m_small = make_multi_unit(small_len);
    let m_large = make_multi_unit(large_len);
    let m_len_ratio = (m_large.len() as f64) / (m_small.len() as f64);
    let mut bms = ComputeBudget::new(big_units, big_alloc);
    let _ = lex_exact(&m_small, 0, None, &mut bms).unwrap();
    let cms = big_units - bms.remaining_units();
    let ams = big_alloc - bms.remaining_alloc_bytes();
    let mut bml = ComputeBudget::new(big_units, big_alloc);
    let _ = lex_exact(&m_large, 0, None, &mut bml).unwrap();
    let cml = big_units - bml.remaining_units();
    let aml = big_alloc - bml.remaining_alloc_bytes();
    let m_ratio = (cml as f64) / (cms as f64);
    let m_alloc_ratio = (aml as f64) / (ams as f64);
    // Fixed overhead for 64 units (`argv` vectors + `cat` buffers + `units` table).
    let multi_fixed_alloc =
        64 * (4 * std::mem::size_of::<Word>() + 16 + std::mem::size_of::<RawCommandUnit>()) + 4_096;
    assert!(cms > 0 && cml > cms);
    assert!(ams > 0 && aml > ams);
    assert!(
        cms <= 8 * (m_small.len() as u64) + 4_096
            && cml <= 8 * (m_large.len() as u64) + 4_096
            && m_ratio <= m_len_ratio * 1.25,
        "lex_exact multi-unit compute charge exceeds linear bound: small={cms}, large={cml}, ratio={m_ratio:.2}, len_ratio={m_len_ratio:.2}"
    );
    assert!(
        ams <= 4 * m_small.len() + multi_fixed_alloc
            && aml <= 4 * m_large.len() + multi_fixed_alloc
            && m_alloc_ratio <= m_len_ratio * 1.25,
        "lex_exact multi-unit alloc charge exceeds affine bound (b={multi_fixed_alloc}): small={ams}, large={aml}, ratio={m_alloc_ratio:.2}, len_ratio={m_len_ratio:.2}"
    );

    // Review round 1 finding 9: `lex_best_effort` compute AND allocation
    // scaling obey linear upper bounds on adversarial unclosed `$(` padding
    // (bounded by `MAX_DEPTH` with a fixed constant factor per level), measured
    // under explicit generous budgets.
    let adv_small = format!("echo {}", "$(".repeat(small_len / 2 - 4));
    let adv_large = format!("echo {}", "$(".repeat(large_len / 2 - 4));
    let adv_len_ratio = (adv_large.len() as f64) / (adv_small.len() as f64);

    let mut b_as = ComputeBudget::new(big_units, big_alloc);
    let _ = lex_best_effort(&[&adv_small], &mut b_as).unwrap();
    let c_as = big_units - b_as.remaining_units();
    let a_as = big_alloc - b_as.remaining_alloc_bytes();

    let mut b_al = ComputeBudget::new(big_units, big_alloc);
    let _ = lex_best_effort(&[&adv_large], &mut b_al).unwrap();
    let c_al = big_units - b_al.remaining_units();
    let a_al = big_alloc - b_al.remaining_alloc_bytes();

    let be_ratio = (c_al as f64) / (c_as as f64);
    let be_alloc_ratio = (a_al as f64) / (a_as as f64);
    assert!(c_as > 0 && c_al > c_as);
    assert!(a_as > 0 && a_al > a_as);
    assert!(
        be_ratio <= adv_len_ratio * 1.5,
        "lex_best_effort compute charge super-linear on unclosed `$(`: small={c_as}, large={c_al}, ratio={be_ratio:.2}, len_ratio={adv_len_ratio:.2}"
    );
    assert!(
        be_alloc_ratio <= adv_len_ratio * 2.0,
        "lex_best_effort alloc charge super-linear on unclosed `$(`: small={a_as}, large={a_al}, ratio={be_alloc_ratio:.2}, len_ratio={adv_len_ratio:.2}"
    );

    // Separately assert that under P0's frozen default limits this adversarial
    // input is Exhausted AND preserves units discovered before exhaustion
    // (§8). Which of the two budgets runs out first is not part of the
    // contract, so either reason is accepted.
    let mut b_def_plain = ComputeBudget::with_default_limits();
    assert!(matches!(
        lex_best_effort(&[&adv_large], &mut b_def_plain),
        Err(PartOutcome::Exhausted(_))
    ));

    let adv_with_early = format!(
        "cat ~/.ssh/id_rsa; echo {}",
        "$(".repeat(large_len / 2 - 20)
    );
    let mut b_def = ComputeBudget::with_default_limits();
    let mut partial = BestEffortRecord::default();
    assert!(matches!(
        lex_best_effort_into(&[&adv_with_early], &mut partial, &mut b_def),
        Err(PartOutcome::Exhausted(_))
    ));
    assert!(
        !partial.units.is_empty(),
        "units found before default-budget exhaustion must be kept"
    );
    assert_eq!(partial.units[0].raw_argv, ["cat", "~/.ssh/id_rsa"]);
}

// ---------------------------------------------------------------------------
// Round 5 (review round 3): nothing done or found without, or before, a charge
// ---------------------------------------------------------------------------

/// Words of the full run that each kept best-effort unit's `raw_argv` and
/// redirects are word prefixes of, in some full-run unit.
fn assert_best_effort_subset(kept: &BestEffortRecord, full: &BestEffortRecord, ctx: &str) {
    for (pos, u) in kept.units.iter().enumerate() {
        assert_eq!(u.index, pos, "{ctx}: unit index");
        // Some full-run unit has these finished words, and every alternative
        // reading kept is one that unit has (round 6: kept alternatives are
        // finished readings; when exhaustion stopped their duplicate check
        // they may repeat one, never invent one).
        assert!(
            full.units.iter().any(|f| {
                u.raw_argv.len() <= f.raw_argv.len()
                    && u.raw_argv[..] == f.raw_argv[..u.raw_argv.len()]
                    && u.redirects.len() <= f.redirects.len()
                    && u.redirects[..] == f.redirects[..u.redirects.len()]
                    && u.alt_argv
                        .iter()
                        .all(|a| f.alt_argv.contains(a) || *a == f.effective_argv)
            }),
            "{ctx}: kept unit {:?} / {:?} / alternatives {:?} is not made of finished results of the full run",
            u.raw_argv,
            u.redirects,
            u.alt_argv
        );
    }
}

#[test]
fn r5_f1_completed_word_or_redirect_is_never_dropped() {
    // Review round 3, finding 1, failing case: 16 allocation bytes. The run
    // must report exhaustion, and it may not have scanned a word it then
    // dropped: 16 bytes no longer reach the word's buffers, because the
    // unit's slots (and the slot recording `Exhausted`) are reserved first.
    let res = lex_exact("sudo", 0, None, &mut ComputeBudget::new(1000, 16));
    assert!(exact_exhausted(&res), "{res:?}");
    if let Ok(out) = &res {
        for u in &out.units {
            assert_eq!(u.argv.len(), 1);
            assert_eq!(u.argv[0].literal_bytes(), b"sudo");
        }
    }

    // The same shape one step later: `sudo` is scanned whole and a later
    // allocation (the reserved-word check's reason string, 4 bytes) fails.
    // The finished word is kept.
    let (_, a) = exact_cost("sudo");
    for short in 1..=4 {
        let mut b = ComputeBudget::new(BIG_UNITS, a - short);
        let out = lex_exact("sudo", 0, None, &mut b).unwrap();
        assert!(out.outcomes.iter().any(|pr| matches!(
            pr.outcome,
            PartOutcome::Exhausted(ExhaustReason::AllocationBudget)
        )));
        assert_eq!(out.units.len(), 1, "short by {short}");
        assert_eq!(out.units[0].argv[0].literal_bytes(), b"sudo");
    }

    // Neighbour: a finished redirect target. The last charge of
    // `cat > out.txt` comes after the target is read; the redirect is kept.
    let (c, _) = exact_cost("cat > out.txt");
    let mut b = ComputeBudget::new(c - 1, BIG_ALLOC);
    let out = lex_exact("cat > out.txt", 0, None, &mut b).unwrap();
    assert!(has_exhausted(&out));
    assert_eq!(out.units.len(), 1);
    assert!(matches!(
        &out.units[0].redirects[..],
        [ModelRedirect::Write { target }] if target.literal_bytes() == b"out.txt"
    ));

    // Neighbour: an assignment word keeps its `Assignment` reason, whose slot
    // and work are reserved before the word is scanned.
    let (c, _) = exact_cost("FOO=1");
    let mut b = ComputeBudget::new(c - 1, BIG_ALLOC);
    let out = lex_exact("FOO=1", 0, None, &mut b).unwrap();
    assert_eq!(out.units.len(), 1);
    assert!(unread_reasons(&out).contains(&&UnreadReason::Assignment));

    // Neighbour in the best-effort pass: a finished redirect target and a
    // here-string are kept when the budget runs out one unit early.
    for line in ["cat > out.txt", "FOO=1 sudo id"] {
        let need = min_clean_budget(true, |b| lex_best_effort(&[line], b).is_ok());
        let mut rec = BestEffortRecord::default();
        let r = lex_best_effort_into(
            &[line],
            &mut rec,
            &mut ComputeBudget::new(need - 1, BIG_ALLOC),
        );
        assert!(matches!(r, Err(PartOutcome::Exhausted(_))), "`{line}`");
        assert_eq!(rec.units.len(), 1, "`{line}`");
    }
    let mut rec = BestEffortRecord::default();
    let need = min_clean_budget(true, |b| lex_best_effort(&["cat > out.txt"], b).is_ok());
    let _ = lex_best_effort_into(
        &["cat > out.txt"],
        &mut rec,
        &mut ComputeBudget::new(need - 1, BIG_ALLOC),
    );
    assert_eq!(
        rec.units[0].redirects,
        [(BestEffortRedirectKind::Write, "out.txt".to_string())]
    );
    let mut rec = BestEffortRecord::default();
    let need = min_clean_budget(true, |b| lex_best_effort(&["FOO=1 sudo id"], b).is_ok());
    let _ = lex_best_effort_into(
        &["FOO=1 sudo id"],
        &mut rec,
        &mut ComputeBudget::new(need - 1, BIG_ALLOC),
    );
    assert_eq!(
        rec.units[0].assignments,
        [("FOO".to_string(), "1".to_string())]
    );
    assert_eq!(rec.units[0].raw_argv, ["sudo", "id"]);
}

#[test]
fn r5_class_exact_lexer_below_cost_keeps_only_finished_values() {
    // Every budget below the measured cost, in each dimension, must give
    // `Exhausted`, keep only whole words and redirects the full run found,
    // and never store into a slot that was not reserved (debug-asserted by
    // `push_reserved`). One unit below the compute cost (the final flush)
    // keeps everything.
    for line in [
        "sudo",
        "sudo id",
        "sudo $id",
        "FOO=1 HOME=/x cat ~/y",
        "FOO=$x BAR=é cat",
        "cat > $out 2>&$fd 3< x",
        "cat README.md > out.txt 2>> err.log",
        "cat 2>&1 <in.txt; ls",
        "cat <&3",
        "if true",
        "{ cat a",
        "a | b && c || d",
        "> out.txt",
        "cat <<EOF\nsudo id\nEOF\nls",
    ] {
        let mut bf = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
        let full = lex_exact(line, 0, None, &mut bf).unwrap();
        assert!(!has_exhausted(&full), "`{line}`");
        let c = BIG_UNITS - bf.remaining_units();
        let a = BIG_ALLOC - bf.remaining_alloc_bytes();

        for units in 0..c {
            let res = lex_exact(line, 0, None, &mut ComputeBudget::new(units, BIG_ALLOC));
            assert!(exact_exhausted(&res), "`{line}`: {units} of {c} units");
            if let Ok(out) = &res {
                assert_exact_prefix(out, &full, &format!("`{line}` units={units}"));
            }
        }
        let out = lex_exact(line, 0, None, &mut ComputeBudget::new(c - 1, BIG_ALLOC)).unwrap();
        assert_eq!(out.units.len(), full.units.len(), "`{line}`");
        for (k, f) in out.units.iter().zip(&full.units) {
            assert_eq!(k.argv, f.argv, "`{line}`");
            assert_eq!(k.redirects, f.redirects, "`{line}`");
        }

        for alloc in 0..a {
            let res = lex_exact(line, 0, None, &mut ComputeBudget::new(BIG_UNITS, alloc));
            assert!(exact_exhausted(&res), "`{line}`: {alloc} of {a} bytes");
            if let Ok(out) = &res {
                assert_exact_prefix(out, &full, &format!("`{line}` alloc={alloc}"));
            }
        }
    }
}

#[test]
fn r5_class_best_effort_below_cost_keeps_only_finished_values() {
    // Same class check for the best-effort pass, including recovery during
    // scanning, conversion, `find -exec` derivation and recursion. The
    // optional memo makes "measured cost" the smallest clean budget.
    for line in [
        "sudo id",
        "FOO=1 sudo id > out",
        "cat <<< 'sudo id'",
        "echo 'sudo id' | sh",
        "sh -c 'sudo id' && command -p env -i ls",
        r"find . -exec sh -c 'sudo id' \; ; ls",
        "cat <<EOF | sh\nsudo id\nEOF",
        "command -Z arg sudo id",
        "$(which sh) -c 'id'",
        r"find . -exec command -Z a sudo id \; -exec find b -ok ls \; \;",
        "cat x | crontab <<EOF\n* * * * * sudo id\nEOF",
    ] {
        let mut bf = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
        let full = lex_best_effort(&[line], &mut bf).unwrap();
        let need_c = min_clean_budget(true, |b| lex_best_effort(&[line], b).is_ok());
        let need_a = min_clean_budget(false, |b| lex_best_effort(&[line], b).is_ok());
        for units in 0..need_c {
            let mut rec = BestEffortRecord::default();
            let r =
                lex_best_effort_into(&[line], &mut rec, &mut ComputeBudget::new(units, BIG_ALLOC));
            assert!(
                matches!(r, Err(PartOutcome::Exhausted(_))),
                "`{line}` units={units}"
            );
            assert_best_effort_subset(&rec, &full, &format!("`{line}` units={units}"));
        }
        for alloc in 0..need_a {
            let mut rec = BestEffortRecord::default();
            let r = lex_best_effort_into(
                &[line],
                &mut rec,
                &mut ComputeBudget::new(BIG_UNITS, usize::try_from(alloc).unwrap()),
            );
            assert!(
                matches!(r, Err(PartOutcome::Exhausted(_))),
                "`{line}` alloc={alloc}"
            );
            assert_best_effort_subset(&rec, &full, &format!("`{line}` alloc={alloc}"));
        }
    }
}

#[test]
fn r5_f3_payload_dedup_is_linear_and_charged() {
    use super::best_effort::PayloadSet;

    let prefix = "a".repeat(500);
    let distinct =
        |n: usize| -> Vec<String> { (0..n).map(|i| format!("echo {prefix}{i:04}")).collect() };
    let total = |v: &[String]| v.iter().map(String::len).sum::<usize>() as u64;
    let insert_all = |items: Vec<String>| -> (u64, usize) {
        let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
        let mut set = PayloadSet::default();
        for s in items {
            set.insert(s, &mut b).unwrap();
        }
        (BIG_UNITS - b.remaining_units(), set.into_vec().len())
    };

    // Review round 3, finding 3: 255 distinct payloads sharing a 500-byte
    // prefix. Each payload is hashed once and probed O(1) times, never
    // compared against every earlier payload (32,385 comparisons of the
    // common prefix before), so the charge is linear in the bytes inserted.
    let d255 = distinct(255);
    let t255 = total(&d255);
    let (c255, n255) = insert_all(d255);
    assert_eq!(n255, 255);
    let d510 = distinct(510);
    let t510 = total(&d510);
    let (c510, n510) = insert_all(d510);
    assert_eq!(n510, 510);
    assert!(
        c255 <= 2 * t255 + 64 * 255,
        "charge {c255} for {t255} bytes"
    );
    assert!(
        c510 <= 2 * t510 + 64 * 510,
        "charge {c510} for {t510} bytes"
    );
    assert!(
        (c510 as f64) / (c255 as f64) <= 2.5,
        "doubling the payloads must about double the charge: {c255} -> {c510}"
    );

    // A comparison that does run is charged by the bytes it compares: 255
    // copies of one payload cost a hash each and a full comparison for each
    // duplicate, and only one copy is kept.
    let one = format!("echo {prefix}");
    let (c_dup, n_dup) = insert_all(vec![one.clone(); 255]);
    assert_eq!(n_dup, 1);
    assert!(
        c_dup >= (255 + 254) * one.len() as u64,
        "duplicate compares uncharged: {c_dup}"
    );

    // A budget below the hashing cost exhausts instead of finishing.
    let mut b = ComputeBudget::new(t255 / 2, BIG_ALLOC);
    let mut set = PayloadSet::default();
    let res: Result<(), Exhausted> = distinct(255)
        .into_iter()
        .try_for_each(|s| set.insert(s, &mut b));
    assert!(res.is_err());

    // The review's whole failing line through the best-effort pass: one
    // `sh` unit and the derived `echo` units of all 255 payloads.
    let line = format!(
        "sh {}",
        (0..255)
            .map(|i| format!("<(echo {}{i:04})", "a".repeat(500)))
            .collect::<Vec<_>>()
            .join(" ")
    );
    assert_eq!(line.len(), 130_817);
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let rec = lex_best_effort(&[&line], &mut b).unwrap();
    let mut echoes: Vec<&Vec<String>> = rec
        .units
        .iter()
        .filter(|u| u.derived && u.raw_argv.first().is_some_and(|w| w == "echo"))
        .map(|u| &u.raw_argv)
        .collect();
    echoes.sort();
    echoes.dedup();
    assert_eq!(echoes.len(), 255);
}

#[test]
fn r5_neighbours_lookups_comparisons_folds_and_memo_are_charged() {
    let charged = |b: &ComputeBudget| BIG_UNITS - b.remaining_units();

    // P0 table lookups charge their row scan before it runs.
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    assert!(lookup_tool("cat", &mut b).unwrap().is_some());
    assert!(charged(&b) >= tools::all_tools().len() as u64);
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    assert!(lookup_outside("zsh", &mut b).unwrap().is_some());
    assert!(charged(&b) >= tools::outside_model().len() as u64);
    let mut b = ComputeBudget::new(1, BIG_ALLOC);
    assert!(
        lookup_tool("cat", &mut b).is_err(),
        "a lookup must not run uncharged"
    );

    // CR-19 folding charges each of its passes over an ASCII word: the
    // basename search, the ASCII check and the lowercase copy.
    let long = Word::derived("A".repeat(10_000).into_bytes());
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let _ = fold_command_word(&long, &FoldRules::REV4, &mut b).unwrap();
    assert!(charged(&b) >= 3 * 10_000);

    // Here-document delimiter comparisons are charged by the bytes compared:
    // body lines as long as the delimiter each cost a full comparison.
    let delim = "D".repeat(1_000);
    let here = |body_line: &str| {
        format!(
            "cat <<{delim}\n{}{delim}\nls",
            format!("{body_line}\n").repeat(50)
        )
    };
    let (c_same, _) = exact_cost(&here(&"x".repeat(1_000)));
    let (c_diff, _) = exact_cost(&here(&"x".repeat(999)));
    assert!(
        c_same >= c_diff + 50 * 1_000,
        "same-length delimiter comparisons uncharged: {c_same} vs {c_diff}"
    );

    // The memo lookup compares the line with up to `LEX_MEMO` (4) entries;
    // that is charged before it runs, on a cold and a warm memo alike.
    let line = format!("echo {}", "m".repeat(20_000));
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let _ = lex_best_effort(&[&line], &mut b).unwrap();
    assert!(charged(&b) >= 5 * line.len() as u64);

    // Scan 8 walks each pipeline once: doubling the pipelines about doubles
    // the charge.
    let pipes = |k: usize| "echo a | sh; ".repeat(k);
    let mut b60 = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let _ = lex_best_effort(&[&pipes(60)], &mut b60).unwrap();
    let mut b120 = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let _ = lex_best_effort(&[&pipes(120)], &mut b120).unwrap();
    assert!((charged(&b120) as f64) / (charged(&b60) as f64) <= 2.5);
}

// ---------------------------------------------------------------------------
// Round 6 (review round 4): finished readings survive, scans charge first
// ---------------------------------------------------------------------------

#[test]
fn r6_f1_best_effort_unwrap_keeps_alternative_readings_on_exhaustion() {
    use super::best_effort::unwrap_best_effort;

    // Review round 4, finding 1, failing case: unwrapping `command -Z arg`
    // stores the alternative reading `sudo id`, then ran out on its final
    // argv shift and dropped it. Whatever the budget, an alternative kept is
    // the full run's, and once the first wrapper step is done (the
    // effective reading is stripped) the alternative is there.
    let line = "command -Z arg sudo id";
    let mut rec = BestEffortRecord::default();
    let res = lex_best_effort_into(&[line], &mut rec, &mut ComputeBudget::new(402, 100_000));
    assert!(matches!(res, Err(PartOutcome::Exhausted(_))), "{res:?}");
    let want_alt = vec!["sudo".to_string(), "id".to_string()];
    for u in &rec.units {
        assert!(
            u.alt_argv.iter().all(|a| *a == want_alt),
            "{:?}",
            u.alt_argv
        );
        if u.effective_argv != u.raw_argv {
            assert_eq!(
                u.alt_argv,
                std::slice::from_ref(&want_alt),
                "stripped reading without its alternative"
            );
        }
    }

    // The helper itself, at every budget below its cost: on exhaustion it
    // hands back whole words and every alternative found so far.
    let argv: Vec<String> = ["command", "-Z", "arg", "sudo", "id"]
        .iter()
        .map(ToString::to_string)
        .collect();
    let mut bf = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    let full = unwrap_best_effort(argv.clone(), &mut bf).unwrap_or_else(|_| unreachable!());
    assert_eq!(full.effective, ["arg", "sudo", "id"]);
    assert_eq!(full.alt, std::slice::from_ref(&want_alt));
    let cost = BIG_UNITS - bf.remaining_units();
    let mut kept_alt_on_exhaustion = false;
    for units in 0..cost {
        let Err((partial, _)) =
            unwrap_best_effort(argv.clone(), &mut ComputeBudget::new(units, BIG_ALLOC))
        else {
            panic!("{units} of {cost} units must exhaust");
        };
        let n = partial.effective.len();
        assert!(n <= argv.len() && partial.effective[..] == argv[argv.len() - n..]);
        assert!(partial.alt.iter().all(|a| *a == want_alt));
        if partial.effective != argv {
            assert_eq!(partial.alt, full.alt, "units={units}");
        }
        kept_alt_on_exhaustion |= !partial.alt.is_empty();
    }
    assert!(
        kept_alt_on_exhaustion,
        "some budget must run out after the alternative is found and keep it"
    );

    // Callers: a `find -exec` command and a `$(which …)` reading keep their
    // alternatives the same way (checked over every budget by the class
    // sweep); here, one unit short of a clean run keeps them all.
    for line in [
        r"find . -exec command -Z a sudo id \;",
        "$(which command) -Z a sudo id",
    ] {
        let mut bf = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
        let full = lex_best_effort(&[line], &mut bf).unwrap();
        let need = min_clean_budget(true, |b| lex_best_effort(&[line], b).is_ok());
        let mut rec = BestEffortRecord::default();
        let r = lex_best_effort_into(
            &[line],
            &mut rec,
            &mut ComputeBudget::new(need - 1, BIG_ALLOC),
        );
        assert!(matches!(r, Err(PartOutcome::Exhausted(_))), "`{line}`");
        assert_best_effort_subset(&rec, &full, line);
        let alts = |r: &BestEffortRecord| r.units.iter().map(|u| u.alt_argv.len()).sum::<usize>();
        assert_eq!(alts(&rec), alts(&full), "`{line}`: alternatives lost");
    }
}

#[test]
fn r6_f2_crontab_lines_are_charged_before_they_are_scanned() {
    use super::best_effort::crontab_commands_budgeted;

    // Review round 4, finding 2, failing case: `text.lines()` scanned all
    // 60,000 bytes for a line end before the first charge. The scanner now
    // charges each byte before examining it.
    let long = "x".repeat(60_000);
    assert!(crontab_commands_budgeted(&long, &mut ComputeBudget::new(0, 0)).is_err());
    assert!(crontab_commands_budgeted(&long, &mut ComputeBudget::new(1_000, BIG_ALLOC)).is_err());
    let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
    assert_eq!(crontab_commands_budgeted(&long, &mut b).unwrap(), "");
    assert!(BIG_UNITS - b.remaining_units() >= 60_000);

    // Same lines as `str::lines` (CRLF, no trailing empty line), same fields.
    for (text, want) in [
        ("* * * * * sudo id\n", "sudo id\n"),
        (
            "@reboot  run x\r\n# c\n\n  1 2 3 4 5   cmd a\nshort line",
            "run x\ncmd a\n",
        ),
        ("1 2 3 4 5 last", "last\n"),
        ("@daily", ""),
        ("", ""),
    ] {
        let mut b = ComputeBudget::new(BIG_UNITS, BIG_ALLOC);
        assert_eq!(
            crontab_commands_budgeted(text, &mut b).unwrap(),
            want,
            "{text:?}"
        );
    }
}
