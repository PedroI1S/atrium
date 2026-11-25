//! Contract rev 4 §13 boundary property tests.
//!
//! Independent of the implementation's classifier: uses a separate small
//! deterministic generator for each construct outside §3 and §4, and checks
//! the contract's boundary properties end-to-end through `SessionGate`:
//!
//! 1. In Home and under prefix/exact rules, a line containing any construct
//!    outside §3 or §4 is NEVER `Allow` (and when `Ask`, carries an unread
//!    notice with `rememberable == false`).
//! 2. A fixture-planted hard-deny target touched by a **modeled** part of the
//!    call is `HardDeny` in every setting (Home, prefix rule, unscoped Any
//!    rule, FullAccess), even when combined with outside-model constructs.
//!
//! Properties 3 & 4 (compute/allocation exhaustion across budget cutoffs) are
//! verified in unit tests inside `policy::v4::evaluate::tests` via
//! `evaluate_call_with_budget`.

mod common_adversarial;

use atrium_agents::{GateDecision, ToolCheck, ToolGate};
use atrium_protocol::agent::{ToolKind, UnreadNotice};
use atrium_protocol::policy::{Preset, Rule, RuleDecision, RuleMatcher, RuleScope};
use common_adversarial::{Tmp, block_on, cmd_check, exact, prefix, rule};

/// Small deterministic PRNG (SplitMix64) so property runs are reproducible
/// with zero external crate dependencies.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[(self.next_u64() as usize) % items.len()]
    }
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + ((self.next_u64() as usize) % (hi - lo))
    }
}

/// One generated outside-model construct together with the leading command
/// token (for building a prefix Allow rule) and whether the construct safely
/// composes with `;` before a following modeled unit (false for `#` comments
/// and here-docs that swallow the rest of the line).
struct OutsideSample {
    category: &'static str,
    line: String,
    lead_cmd: &'static str,
    composes_with_semi: bool,
}

type Generator = (&'static str, fn(&mut Rng) -> OutsideSample);

const SAFE_FILES: &[&str] = &["README.md", "proj/a.txt", "proj/b.txt", "notes.txt"];
const VAR_NAMES: &[&str] = &["X", "FOO", "TARGET", "FILE_1", "ARG"];
const RESERVED_WORDS: &[&str] = &["if", "for", "while", "until", "case", "select"];

fn gen_unquoted_var(r: &mut Rng) -> OutsideSample {
    let var = *r.pick(VAR_NAMES);
    let line = match r.range(0, 3) {
        0 => format!("cat ${var}"),
        1 => format!("cat ${{{var}}}"),
        _ => format!("cat proj/${var}.txt"),
    };
    OutsideSample {
        category: "§3 unquoted $VAR",
        line,
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_cmd_subst(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let line = match r.range(0, 2) {
        0 => format!("cat $(echo {f})"),
        _ => format!("cat `echo {f}`"),
    };
    OutsideSample {
        category: "§3 command substitution",
        line,
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_ansi_c_quote(r: &mut Rng) -> OutsideSample {
    let payload = *r.pick(&[r"\xff", r"README\x2emd", r"a\nb", r"\141"]);
    OutsideSample {
        category: "§3 ANSI-C $'...'",
        line: format!("cat $'{payload}'"),
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_brace_expansion(r: &mut Rng) -> OutsideSample {
    let line = match r.range(0, 3) {
        0 => "cat proj/{a,b}.txt".into(),
        1 => "ls {README.md,notes.txt}".into(),
        _ => format!("head proj/{{a,b}}.txt"),
    };
    let lead = if line.starts_with("ls ") {
        "ls"
    } else if line.starts_with("head ") {
        "head"
    } else {
        "cat"
    };
    OutsideSample {
        category: "§3 brace expansion",
        line,
        lead_cmd: lead,
        composes_with_semi: true,
    }
}

fn gen_comment(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let tag = *r.pick(&["note", "todo", "x"]);
    OutsideSample {
        category: "§3 # comment",
        line: format!("cat {f} # {tag}"),
        lead_cmd: "cat",
        composes_with_semi: false,
    }
}

fn gen_negated_glob_class(r: &mut Rng) -> OutsideSample {
    let bang = *r.pick(&["!", "^"]);
    let ch = *r.pick(&["x", "z", "0"]);
    OutsideSample {
        category: "§3 negated glob class",
        line: format!("cat proj/[{bang}{ch}].txt"),
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_unquoted_non_ascii(r: &mut Rng) -> OutsideSample {
    let tok = *r.pick(&["café", "αβγ", "日本語", "naïve"]);
    OutsideSample {
        category: "§3 unquoted non-ASCII",
        line: format!("cat proj/{tok}.txt"),
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_leading_equals_and_tilde_user(r: &mut Rng) -> OutsideSample {
    let line = match r.range(0, 2) {
        0 => {
            let cmd = *r.pick(&["cat", "ls", "head"]);
            format!("={cmd} README.md")
        }
        _ => {
            let user = *r.pick(&["root", "nobody", "alice"]);
            format!("cat ~{user}/README.md")
        }
    };
    OutsideSample {
        category: "§3 leading = or ~user",
        line,
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_double_quote_disallowed(r: &mut Rng) -> OutsideSample {
    let var = *r.pick(VAR_NAMES);
    let line = match r.range(0, 4) {
        0 => format!("cat \"${var}\""),
        1 => "cat \"`echo a`\"".into(),
        2 => "cat \"hello!\"".into(),
        _ => r#"cat "proj/\q.txt""#.into(),
    };
    OutsideSample {
        category: "§3 double-quote disallowed",
        line,
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_unquoted_backslash_disallowed(r: &mut Rng) -> OutsideSample {
    let line = match r.range(0, 3) {
        0 => "cat README.md\\\n".into(),
        1 => r"cat a\ b".into(),
        _ => r"cat \$X".into(),
    };
    OutsideSample {
        category: "§3 unquoted backslash disallowed",
        line,
        lead_cmd: "cat",
        composes_with_semi: false,
    }
}

fn gen_compound_or_reserved(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let kw = *r.pick(RESERVED_WORDS);
    let line = match kw {
        "if" => format!("if true; then cat {f}; fi"),
        "for" => format!("for x in {f}; do cat \"$x\"; done"),
        "while" => format!("while false; do cat {f}; done"),
        "until" => format!("until true; do cat {f}; done"),
        "case" => format!("case a in a) cat {f};; esac"),
        _ => format!("select x in {f}; do cat {f}; done"),
    };
    OutsideSample {
        category: "§3 reserved word / compound",
        line,
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_subshell_or_group_or_bg(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let (line, semi) = match r.range(0, 3) {
        0 => (format!("(cat {f})"), true),
        1 => (format!("{{ cat {f}; }}"), true),
        _ => (format!("cat {f} &"), false),
    };
    OutsideSample {
        category: "§3 subshell / group / background",
        line,
        lead_cmd: "cat",
        composes_with_semi: semi,
    }
}

fn gen_disallowed_redirect(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let (line, semi) = match r.range(0, 7) {
        0 => (format!("cat {f} &> out.txt"), true),
        1 => (format!("cat {f} <&3"), true),
        2 => (format!("cat {f} 3>out.txt"), true),
        3 => (format!("cat {f} <>out.txt"), true),
        4 => (format!("cat <<< hello"), true),
        5 => (format!("cat <(cat {f})"), true),
        _ => ("cat <<EOF\nhello\nEOF".into(), false),
    };
    OutsideSample {
        category: "§3 disallowed redirect",
        line,
        lead_cmd: "cat",
        composes_with_semi: semi,
    }
}

fn gen_assignment(r: &mut Rng) -> OutsideSample {
    let var = *r.pick(VAR_NAMES);
    let f = *r.pick(SAFE_FILES);
    OutsideSample {
        category: "§3 leading assignment",
        line: format!("{var}=1 cat {f}"),
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_process_dependent_path(r: &mut Rng) -> OutsideSample {
    let p = *r.pick(&[
        "/proc/self/cmdline",
        "/dev/fd/3",
        "/dev/stdin",
        "/dev/stdout",
        "/dev/stderr",
    ]);
    OutsideSample {
        category: "§3 process-dependent path",
        line: format!("cat {p}"),
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_wrapper_outside_model(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let (line, lead) = match r.range(0, 6) {
        0 => (format!("env -i cat {f}"), "env"),
        1 => (format!("env FOO=1 cat {f}"), "env"),
        2 => (format!("env -C proj cat a.txt"), "env"),
        3 => (format!("command -v cat"), "command"),
        4 => (format!("time -p cat {f}"), "time"),
        _ => {
            let n = r.range(9, 15);
            (format!("{}cat {f}", "command ".repeat(n)), "command")
        }
    };
    OutsideSample {
        category: "§4 wrapper outside model / >MAX_WRAP",
        line,
        lead_cmd: lead,
        composes_with_semi: true,
    }
}

fn gen_nested_shell_outside_model(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let line = match r.range(0, 5) {
        0 => format!("zsh -c 'cat {f}'"),
        1 => format!("fish -c 'cat {f}'"),
        2 => format!("sh -c 'cat {f}' _ extra"),
        3 => format!("bash -i -c 'cat {f}'"),
        _ => "sh proj/run.sh".into(),
    };
    OutsideSample {
        category: "§4 nested shell outside model",
        line,
        lead_cmd: "sh",
        composes_with_semi: true,
    }
}

fn gen_command_word_glob(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let cmd = *r.pick(&["/bin/ca*", "c?t", "[c]at"]);
    OutsideSample {
        category: "§4 command-word glob",
        line: format!("{cmd} {f}"),
        lead_cmd: "cat",
        composes_with_semi: true,
    }
}

fn gen_inline_code_runner(r: &mut Rng) -> OutsideSample {
    let f = *r.pick(SAFE_FILES);
    let (line, lead) = match r.range(0, 6) {
        0 => ("python -c 'print(1)'".into(), "python"),
        1 => (format!("awk '{{print}}' {f}"), "awk"),
        2 => ("perl -e 'print 1'".into(), "perl"),
        3 => ("node -e 'console.log(1)'".into(), "node"),
        4 => ("find proj -exec cat {} +".into(), "find"),
        _ => (format!("sed 'e' {f}"), "sed"),
    };
    OutsideSample {
        category: "§4 inline code runner",
        line,
        lead_cmd: lead,
        composes_with_semi: true,
    }
}

const GENERATORS: &[Generator] = &[
    ("unquoted_var", gen_unquoted_var),
    ("cmd_subst", gen_cmd_subst),
    ("ansi_c_quote", gen_ansi_c_quote),
    ("brace_expansion", gen_brace_expansion),
    ("comment", gen_comment),
    ("negated_glob_class", gen_negated_glob_class),
    ("unquoted_non_ascii", gen_unquoted_non_ascii),
    (
        "leading_equals_and_tilde_user",
        gen_leading_equals_and_tilde_user,
    ),
    ("double_quote_disallowed", gen_double_quote_disallowed),
    (
        "unquoted_backslash_disallowed",
        gen_unquoted_backslash_disallowed,
    ),
    ("compound_or_reserved", gen_compound_or_reserved),
    ("subshell_or_group_or_bg", gen_subshell_or_group_or_bg),
    ("disallowed_redirect", gen_disallowed_redirect),
    ("assignment", gen_assignment),
    ("process_dependent_path", gen_process_dependent_path),
    ("wrapper_outside_model", gen_wrapper_outside_model),
    ("nested_shell_outside_model", gen_nested_shell_outside_model),
    ("command_word_glob", gen_command_word_glob),
    ("inline_code_runner", gen_inline_code_runner),
];

fn allow_prefix(argv: &[&str], id: &str) -> Rule {
    rule(
        RuleScope::Global,
        ToolKind::Execute,
        prefix(argv),
        RuleDecision::Allow,
        id,
    )
}

fn allow_any(id: &str) -> Rule {
    rule(
        RuleScope::Global,
        ToolKind::Execute,
        RuleMatcher::Any,
        RuleDecision::Allow,
        id,
    )
}

fn check(t: &Tmp, preset: Preset, rules: Vec<Rule>, c: &ToolCheck) -> GateDecision {
    let g = t.gate(preset, rules);
    block_on(g.gate.check(c))
}

/// Property 1 (§13): in Home and under prefix/exact rules, no line containing
/// a construct outside §3 or §4 is ever `Allow`.
#[test]
fn property_1_outside_model_never_allowed_in_home_or_prefix_rules() {
    let t = Tmp::new();
    for f in SAFE_FILES {
        t.write(f, "ordinary content\n");
    }
    t.write("proj/run.sh", "echo ok\n");

    let mut rng = Rng::new(0xA7B1_0001);
    let prefix_rules = vec![
        allow_prefix(&["cat"], "p-cat"),
        allow_prefix(&["ls"], "p-ls"),
        allow_prefix(&["head"], "p-head"),
        allow_prefix(&["env"], "p-env"),
        allow_prefix(&["command"], "p-cmd"),
        allow_prefix(&["time"], "p-time"),
        allow_prefix(&["sh"], "p-sh"),
        allow_prefix(&["python"], "p-py"),
        allow_prefix(&["awk"], "p-awk"),
        allow_prefix(&["perl"], "p-perl"),
        allow_prefix(&["node"], "p-node"),
        allow_prefix(&["find"], "p-find"),
        allow_prefix(&["sed"], "p-sed"),
    ];

    for &(name, make) in GENERATORS {
        for iter in 0..12 {
            let sample = make(&mut rng);
            let c = cmd_check(&sample.line);
            let exact_lead = rule(
                RuleScope::Global,
                ToolKind::Execute,
                exact(&[sample.lead_cmd]),
                RuleDecision::Allow,
                "e-lead",
            );

            for (label, rules) in [
                ("Home", vec![]),
                ("PrefixRules", prefix_rules.clone()),
                ("ExactLead", vec![exact_lead]),
            ] {
                let d = check(&t, Preset::Home, rules, &c);
                match d {
                    GateDecision::Allow => {
                        panic!(
                            "Property 1 violated ({name} / {}, iter {iter}, {label}): \
                             outside-model line {:?} was Allowed!",
                            sample.category, sample.line
                        );
                    }
                    GateDecision::Ask { ticket } => {
                        assert_eq!(
                            ticket.unread,
                            Some(UnreadNotice::Unread),
                            "Property 1 ({name} / {}, iter {iter}, {label}): \
                             expected UnreadNotice::Unread on {:?}",
                            sample.category,
                            sample.line
                        );
                        assert!(
                            !ticket.rememberable,
                            "Property 1 ({name} / {}, iter {iter}, {label}): \
                             expected rememberable == false on {:?}",
                            sample.category, sample.line
                        );
                    }
                    GateDecision::Deny { .. } => {
                        // A best-effort HardDeny (or Deny) is strictly stronger than Ask.
                    }
                }
            }
        }
    }
}

/// Property 2 (§13): a fixture-planted hard-deny target touched by a modeled
/// part of the command is HardDeny in every setting (Home, prefix rule,
/// unscoped Any rule, FullAccess), including when paired with constructs
/// outside §3 and §4.
#[test]
fn property_2_modeled_part_touching_planted_hard_deny_always_hard_denied() {
    let t = Tmp::new();
    for f in SAFE_FILES {
        t.write(f, "ordinary\n");
    }
    t.write("proj/run.sh", "echo ok\n");
    // Planted hard-deny targets inside fake temp home (never touching real $HOME).
    t.write(".ssh/id_rsa", "PRIVATE KEY");
    t.write(".aws/credentials", "[default]\naws_secret_access_key=x\n");
    t.write(".npmrc", "//registry.npmjs.org/:_authToken=secret\n");
    t.write(".local/share/atrium/db", "sqlite");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(t.home.join(".ssh/id_rsa"), t.home.join("proj/key_link"))
            .unwrap();
        std::fs::hard_link(t.home.join(".ssh/id_rsa"), t.home.join("proj/key_hl")).unwrap();
    }

    let mut modeled_hits: Vec<(&str, &str)> = vec![
        ("cat ~/.ssh/id_rsa", "credential-read"),
        ("cat ~/.aws/credentials", "credential-read"),
        ("cat ~/.npmrc", "credential-read"),
        ("cat ~/.local/share/atrium/db", "atrium-self"),
        ("rg secret ~/.aws", "credential-read"),
    ];
    #[cfg(unix)]
    {
        modeled_hits.push(("cat proj/key_link", "credential-read"));
        modeled_hits.push(("cat proj/key_hl", "credential-read"));
        modeled_hits.push(("cat proj/key_l*", "credential-read"));
    }

    let settings: Vec<(&str, Preset, Vec<Rule>)> = vec![
        ("Home", Preset::Home, vec![]),
        (
            "Prefix",
            Preset::Home,
            vec![
                allow_prefix(&["cat"], "p-cat"),
                allow_prefix(&["rg"], "p-rg"),
            ],
        ),
        ("Any", Preset::Home, vec![allow_any("any")]),
        ("FullAccess", Preset::FullAccess, vec![]),
    ];

    let mut rng = Rng::new(0xA7B1_0002);

    // 1. Standalone modeled hits across every setting.
    for &(hit_cmd, expected_id) in &modeled_hits {
        let c = cmd_check(hit_cmd);
        for (label, preset, rules) in &settings {
            let d = check(&t, *preset, rules.clone(), &c);
            match d {
                GateDecision::Deny { reason } => {
                    assert!(
                        reason.contains(expected_id),
                        "Property 2 standalone ({hit_cmd}, {label}): expected {expected_id}, got `{reason}`"
                    );
                }
                other => panic!(
                    "Property 2 standalone ({hit_cmd}, {label}): expected HardDeny({expected_id}), got {other:?}"
                ),
            }
        }
    }

    // 2. Paired with every outside-§3/§4 generator (before AND after where syntax allows).
    for &(name, make) in GENERATORS {
        for iter in 0..4 {
            let sample = make(&mut rng);
            let &(hit_cmd, expected_id) = rng.pick(&modeled_hits);
            let sep = *rng.pick(&["; ", " && ", " || "]);

            let mut combos = vec![format!("{hit_cmd}{sep}{}", sample.line)];
            if sample.composes_with_semi {
                combos.push(format!("{}{sep}{hit_cmd}", sample.line));
            }

            for combo in combos {
                let c = cmd_check(&combo);
                for (label, preset, rules) in &settings {
                    let d = check(&t, *preset, rules.clone(), &c);
                    match d {
                        GateDecision::Deny { reason } => {
                            assert!(
                                reason.contains(expected_id) || reason.contains("Always blocked"),
                                "Property 2 combo ({name}, iter {iter}, {label}) on {combo:?}: unexpected deny `{reason}`"
                            );
                        }
                        other => panic!(
                            "Property 2 combo ({name}, iter {iter}, {label}) on {combo:?}: \
                             expected HardDeny({expected_id}), got {other:?}"
                        ),
                    }
                }
            }
        }
    }
}
