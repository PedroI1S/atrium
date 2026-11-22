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
    assert!(
        charged < (nested.len() as u64) * 8,
        "nested braces must be linear, charged {charged} for len {}",
        nested.len()
    );

    // M1-31 review 1 finding 10: `$`-bearing words skip lower-bound shortcut.
    let dollar_word = "{,$}".repeat(9);
    let mut b = ComputeBudget::with_default_limits();
    assert!(
        brace_expand_budgeted(&dollar_word, &mut b)
            .unwrap()
            .is_none()
    );
    let small_dollar = "{,$}{a,b}";
    let mut b2 = ComputeBudget::with_default_limits();
    assert!(
        brace_expand_budgeted(small_dollar, &mut b2)
            .unwrap()
            .is_some()
    );
}

// ---------------------------------------------------------------------------
// §8 Budgets: exhaustion, preservation, TooLarge, and linear charge scaling
// ---------------------------------------------------------------------------

#[test]
fn budgets_exhaustion_preserves_early_units_and_never_allows_padded_lines() {
    // Exhaustion in `lex_exact`: early unit preserved + Exhausted recorded.
    let padded = format!("cat ~/.ssh/id_rsa; {}", "echo a; ".repeat(200));
    let mut b = ComputeBudget::new(120, 10_000);
    let out = lex_exact(&padded, 0, None, &mut b).unwrap();
    assert!(has_exhausted(&out), "must record Exhausted");
    assert!(
        !out.units.is_empty(),
        "unit 0 found before exhaustion must not be lost"
    );
    assert_eq!(out.units[0].argv[0].literal_bytes(), b"cat");
    assert_eq!(out.units[0].argv[1].literal_bytes(), b"~/.ssh/id_rsa");

    // Exhaustion on allocation budget in `lex_exact`.
    let mut b_alloc = ComputeBudget::new(100_000, 16);
    let out_alloc = lex_exact("cat README.md", 0, None, &mut b_alloc).unwrap();
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

    let mut bm_tight = ComputeBudget::new(u1 / 2, 100_000);
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

    let small_len = 10 * 1024;
    let large_len = limits::MAX_BYTES;
    let small = make_single_unit(small_len);
    let large = make_single_unit(large_len);
    let len_ratio = (large.len() as f64) / (small.len() as f64);

    // `lex_exact` charge ratio must be linear in length (single-unit & multi-unit).
    let mut bs = ComputeBudget::with_default_limits();
    let _ = lex_exact(&small, 0, None, &mut bs).unwrap();
    let cs = limits::COMPUTE_BUDGET_UNITS - bs.remaining_units();

    let mut bl = ComputeBudget::with_default_limits();
    let _ = lex_exact(&large, 0, None, &mut bl).unwrap();
    let cl = limits::COMPUTE_BUDGET_UNITS - bl.remaining_units();

    let exact_ratio = (cl as f64) / (cs as f64);
    assert!(
        exact_ratio <= len_ratio * 1.15 && exact_ratio >= len_ratio * 0.85,
        "lex_exact charge not linear: small={cs}, large={cl}, ratio={exact_ratio:.2}, len_ratio={len_ratio:.2}"
    );

    let m_small = make_multi_unit(small_len);
    let m_large = make_multi_unit(large_len);
    let m_len_ratio = (m_large.len() as f64) / (m_small.len() as f64);
    let mut bms = ComputeBudget::with_default_limits();
    let _ = lex_exact(&m_small, 0, None, &mut bms).unwrap();
    let cms = limits::COMPUTE_BUDGET_UNITS - bms.remaining_units();
    let mut bml = ComputeBudget::with_default_limits();
    let _ = lex_exact(&m_large, 0, None, &mut bml).unwrap();
    let cml = limits::COMPUTE_BUDGET_UNITS - bml.remaining_units();
    let m_ratio = (cml as f64) / (cms as f64);
    assert!(
        m_ratio <= m_len_ratio * 1.15 && m_ratio >= m_len_ratio * 0.85,
        "lex_exact multi-unit charge not linear: small={cms}, large={cml}, ratio={m_ratio:.2}, len_ratio={m_len_ratio:.2}"
    );

    // `lex_best_effort` charge ratio must also be linear in length (including
    // adversarial unclosed `$(` padding that would go quadratic if rescanned).
    let adv_small = format!("echo {}", "$(".repeat(small_len / 2 - 4));
    let adv_large = format!("echo {}", "$(".repeat(large_len / 2 - 4));
    let adv_len_ratio = (adv_large.len() as f64) / (adv_small.len() as f64);

    let mut b_as = ComputeBudget::with_default_limits();
    let _ = lex_best_effort(&[&adv_small], &mut b_as).unwrap();
    let c_as = limits::COMPUTE_BUDGET_UNITS - b_as.remaining_units();

    let mut b_al = ComputeBudget::with_default_limits();
    let _ = lex_best_effort(&[&adv_large], &mut b_al).unwrap();
    let c_al = limits::COMPUTE_BUDGET_UNITS - b_al.remaining_units();

    let be_ratio = (c_al as f64) / (c_as as f64);
    assert!(
        be_ratio <= adv_len_ratio * 1.25,
        "lex_best_effort charge not linear on unclosed `$((`: small={c_as}, large={c_al}, ratio={be_ratio:.2}, len_ratio={adv_len_ratio:.2}"
    );
}
