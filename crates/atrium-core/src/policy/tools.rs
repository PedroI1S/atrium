//! Normative tool table for policy engine rev 4 (contract §6, annex).
//!
//! Static data (`ToolSpec` rows) accounting for every tool `dev` knows plus
//! §4's closed wrapper/nested-shell forms and §6's code-running/git forms.
//!
//! **Unknown tools vs. excluded forms (§4, §6):**
//! - An **unknown tool** (absent from both [`TOOLS`] and [`OUTSIDE_MODEL`])
//!   stays in the model for hard-deny and store checks (§4): every word counts
//!   as a candidate `Path`, the unit counts as a possible writer (§5), and it
//!   is never eligible for Home's read-only list. Under a covering Allow rule
//!   meeting §7's simple-command conditions, Clear targets remain Clear.
//! - An **explicitly excluded form** makes its unit `Unread`: any entry in
//!   [`OUTSIDE_MODEL`] (§3–§5), any flag absent from a [`FlagMode::ListedOnly`]
//!   row, any forbidden flag on a [`FlagMode::AnyExceptForbidden`] row, or any
//!   modeled form whose flags/operands trigger an `Unread` effect.
//!
//! Not wired into `SessionGate` until P5; parsing argv against this table is
//! P3's job (`v4/tools.rs`).

#![allow(dead_code)]

use std::fmt::Write as _;

use atrium_protocol::policy::Recursion;

use super::model::{FilterDialect, LinkPolicy, ValueRole, WrapperKind};

// ---------------------------------------------------------------------------
// Schema types for the normative tool table (§6)
// ---------------------------------------------------------------------------

/// Platforms on which a tool entry applies (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Platforms {
    Linux,
    MacOs,
    Both,
}

impl Platforms {
    const fn label(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::MacOs => "macOS",
            Self::Both => "Linux, macOS",
        }
    }
}

/// How flags absent from a row's `flags` slice are handled (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FlagMode {
    /// Only flags listed in `flags` are within the model; any other flag makes
    /// the unit `Unread(UnknownFlag)` (§6). Used for §4's closed wrappers and
    /// nested shells, `cd`, and search tools with complete `dev` flag tables
    /// (`rg`, `grep`, `ag`, `ack`).
    ListedOnly,
    /// Any flag not forbidden by the row (`home_readonly.forbidden` or a
    /// `runs_code` flag in `flags`) is accepted rather than `Unread`. Listed
    /// flags still apply their exact `FlagKind` and `FlagEffects`. For unlisted
    /// flags, every non-pattern/non-ref/non-text/non-member word (flag values
    /// included) is a candidate `Path`, preserving `dev`'s all-arguments
    /// extraction; operands the table marks as `Pattern`/`Text`/`Ref`/
    /// `ArchiveMember` keep that role (§11 item 9).
    AnyExceptForbidden,
}

impl FlagMode {
    const fn label(self) -> &'static str {
        match self {
            Self::ListedOnly => "ListedOnly",
            Self::AnyExceptForbidden => "AnyExceptForbidden",
        }
    }
}

/// Ordered effect toggle applied left-to-right across a unit's flags, where the
/// last matching flag wins (e.g. `ack --follow --nofollow`, `find -L -P`,
/// `cp -LP`, `ag -r -n`; contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum FlagToggle {
    /// Flag leaves this effect unchanged.
    #[default]
    Unchanged,
    /// Flag enables this effect (overriding any earlier `Disable`).
    Enable,
    /// Flag disables this effect (overriding any earlier `Enable`).
    Disable,
}

impl FlagToggle {
    /// Applies this toggle to a running boolean state (last non-`Unchanged`
    /// flag wins).
    pub(crate) const fn apply(self, current: bool) -> bool {
        match self {
            Self::Unchanged => current,
            Self::Enable => true,
            Self::Disable => false,
        }
    }
}

/// Whether a flag is boolean, requires a value, or accepts an optional value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FlagKind {
    Bool,
    Value(ValueRole),
    OptionalValue(ValueRole),
}

/// Semantic effects triggered when a flag is present on a unit (contract §5, §6).
///
/// Toggle fields are applied left-to-right as flags appear in argv so a later
/// disabling flag overrides an earlier enabling flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FlagEffects {
    /// Ordered toggle for write behaviour (`Enable` / `Disable`; last wins).
    pub(crate) writes: FlagToggle,
    /// Ordered toggle for derived destinations (`cp -r`/`-R`/`-a`, `tar -x`).
    pub(crate) derived_destinations: FlagToggle,
    /// Ordered toggle for recursive traversal (`Enable` / `Disable`; last wins).
    pub(crate) recursive: FlagToggle,
    /// Ordered toggle for following symlinks at root operands (last wins).
    pub(crate) follows_links_at_root: FlagToggle,
    /// Ordered toggle for following symlinks below traversed roots (last wins).
    pub(crate) follows_links_below_root: FlagToggle,
    /// Flag supplies the search pattern (or disables a positional pattern,
    /// e.g. `rg -e`/`--files`, `grep -e`/`-f`, `ag -g`, `ack -g`/`-f`/`--match`).
    pub(crate) provides_pattern: bool,
    /// Flag supplies an inline/file script so positional `[0]` is no longer a
    /// script operand (e.g. `sed -e`/`--expression`, `sed -f`/`--file`).
    pub(crate) provides_script: bool,
    /// Overrides the role of positional operands when present (last wins; e.g.
    /// `tar -c`/`-r`/`-u` → `Path`, `tar -x`/`-t` → `ArchiveMember`).
    pub(crate) positional_role: Option<ValueRole>,
    /// Search filter dialect and case-insensitivity (`rg -g`/`--glob`/`--iglob`,
    /// `grep --include`).
    pub(crate) filter: Option<(FilterDialect, bool)>,
    /// Flag causes the unit to execute inline code (e.g. `sh -c`, `python -c`,
    /// `find -exec`, `git -c`, `git rebase -x`, `sed -f`).
    pub(crate) runs_code: bool,
}

impl FlagEffects {
    const NONE: Self = Self {
        writes: FlagToggle::Unchanged,
        derived_destinations: FlagToggle::Unchanged,
        recursive: FlagToggle::Unchanged,
        follows_links_at_root: FlagToggle::Unchanged,
        follows_links_below_root: FlagToggle::Unchanged,
        provides_pattern: false,
        provides_script: false,
        positional_role: None,
        filter: None,
        runs_code: false,
    };
}

/// One flag accepted by a tool or subcommand (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FlagSpec {
    pub(crate) short: Option<char>,
    /// Long option name without leading `--` (or without leading `-` when
    /// `ToolSpec::single_dash_long` is `true`, as for `find` primaries).
    pub(crate) long: Option<&'static str>,
    pub(crate) kind: FlagKind,
    pub(crate) effects: FlagEffects,
}

/// Operand role layout for a tool or subcommand (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OperandLayout {
    /// Fixed positional operand roles starting at positional index 0.
    pub(crate) positional: &'static [ValueRole],
    /// Role for every remaining positional operand after `positional`.
    pub(crate) rest: Option<ValueRole>,
    /// Optional override for the final positional operand (e.g. destination
    /// root of `cp`/`mv`/`rsync`).
    pub(crate) last: Option<ValueRole>,
    /// Optional role override for operands appearing after `--` (e.g.
    /// `git checkout -- path/to/file` destination pathspecs).
    pub(crate) after_double_dash: Option<ValueRole>,
}

impl OperandLayout {
    const NONE: Self = Self {
        positional: &[],
        rest: None,
        last: None,
        after_double_dash: None,
    };

    const fn all(role: ValueRole) -> Self {
        Self {
            positional: &[],
            rest: Some(role),
            last: None,
            after_double_dash: None,
        }
    }

    const fn fixed(positional: &'static [ValueRole]) -> Self {
        Self {
            positional,
            rest: None,
            last: None,
            after_double_dash: None,
        }
    }

    const fn prefix_then(positional: &'static [ValueRole], rest: ValueRole) -> Self {
        Self {
            positional,
            rest: Some(rest),
            last: None,
            after_double_dash: None,
        }
    }
}

/// How a tool's path operands split between reads and writes (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperandAccessMode {
    /// All path operands are read.
    AllRead,
    /// All path/destination operands are written.
    AllWrite,
    /// Path operands are read when the unit does not write, and written when a
    /// writing flag or write condition is active (e.g. `codesign -s`/`-f`/
    /// `--remove-signature`, `sed -i`, `perl -i`).
    AllWriteWhenWriting,
    /// Source operands are read and the final path/destination operand (and any
    /// `DestinationRoot` operand) is written (e.g. `ln`, `cp`, `rsync`,
    /// `git clone`, `uniq` >= 2 operands, `xxd` >= 2 operands; §6).
    SourcesReadLastWrite,
    /// Positional path operands are read (unless a boolean `writes: Enable`
    /// flag such as `find -delete` is present); value-taking path flags with
    /// `writes: Enable` (e.g. `sort -o`, `tree -o`, `find -fprint`, `git diff
    /// --output`, `tar -f` in create/append/update, `tar -C` in extract) write.
    FlagOnlyWrite,
}

impl OperandAccessMode {
    const fn label(self) -> &'static str {
        match self {
            Self::AllRead => "AllRead",
            Self::AllWrite => "AllWrite",
            Self::AllWriteWhenWriting => "AllWriteWhenWriting",
            Self::SourcesReadLastWrite => "SourcesReadLastWrite",
            Self::FlagOnlyWrite => "FlagOnlyWrite",
        }
    }
}

/// Whether and when a tool/subcommand writes to the filesystem (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteBehavior {
    Never,
    Always,
    /// Writes only when a flag with `FlagEffects::writes == Enable` is active
    /// (e.g. `sort -o`, `tree -o`, `find -delete`, `sed -i`, `codesign -s`).
    WithFlag,
    /// Writes when at least `n` positional operands are given (e.g. `uniq` and
    /// `xxd` with `>= 2` operands; `words.rs`).
    WithOperandCount(usize),
    /// Writes when a `writes: Enable` flag is active, OR when at least `n`
    /// positional operands are given and no `writes: Disable` flag (such as
    /// `git branch -l`/`--list`) forces read-only listing (contract §5, §6).
    WithFlagOrOperandCount(usize),
}

/// Whether and when a writer writes to derived destinations (contract §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DerivedDestinationsBehavior {
    Never,
    Always,
    /// Only when a flag with `FlagEffects::derived_destinations == Enable` is
    /// active (e.g. `cp -R`/`-a`, `tar -x`).
    WithFlag,
    /// Only when a source operand resolves to a directory (`mv` of a
    /// directory; §5).
    WhenSourceIsDir,
}

/// Whether and when a tool/subcommand reads recursively (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecursiveBehavior {
    Never,
    Always,
    /// Only when a flag with `FlagEffects::recursive == Enable` is active
    /// (`grep -r`).
    WithFlag,
}

/// Which argv words an Allow rule must name to cover `Unread (program)`
/// (contract §4, §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgramPin {
    /// Not coverable via program pinning.
    None,
    /// Rule must name `[tool, subcommand]` (e.g. `["cargo", "test"]`).
    CommandAndSubcommand,
    /// Rule must name `[tool, operand0]` or `[tool, subcommand, operand0]`
    /// (e.g. `["python", "build.py"]`, `["sh", "x.sh"]`, `["pipx", "run", "pkg"]`).
    FirstOperand,
}

/// How a tool/subcommand executes code (contract §4, §6, §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeRunnerBehavior {
    Never,
    /// Always runs inline/arbitrary code (plain `Unread`; never pinnable).
    InlineOnly,
    /// Runs inline code when a flag with `FlagEffects::runs_code` is given.
    InlineWithFlag,
    /// Inspects script operands (`ValueRole::Code`, positional `[0]` or
    /// supplied via a `provides_script` flag such as `sed -e`/`--expression`)
    /// for the listed single-letter commands (e.g. `sed`'s `e`, `w`, `r`; §4).
    /// Only scripts containing one of those commands (or a `runs_code` flag
    /// such as `sed -f`) trigger code execution.
    ScriptCommands(&'static [char]),
    /// Runs inline code when the first operand matches one of these subargs
    /// (e.g. `git bisect run`, `git submodule foreach`; §6).
    InlineWithSubarg(&'static [&'static str]),
    /// Runs inline code with a code flag (`-c`/`-e`) OR runs a named program
    /// file (`Unread (program)` coverable under `pin`; §4, §7).
    ProgramOrInline {
        pin: ProgramPin,
    },
    /// Project build/test command (`Unread (program)` coverable under `pin`;
    /// e.g. `cargo test`; §4, §7).
    ProjectBuild {
        pin: ProgramPin,
    },
}

/// Single-line switch controlling whether Home auto-allow permits followed-link
/// searches (`rg -L`, `grep -R`) when the bounded link walk stays within budget
/// and reaches no protected path (§10 row 15; proposed §11 item 15).
///
/// Flipping this constant to `false` restores `dev`'s Home `Ask` in one line if
/// the driver rejects proposed §11 item 15.
pub(crate) const ALLOW_HOME_FOLLOWED_SEARCH: bool = true;

/// Home read-only eligibility mirrored verbatim from
/// `atrium_protocol::policy::HOME_READONLY_COMMANDS` (contract §6).
///
/// Home eligibility matches on the command word **as written** (`cat`
/// qualifies; `/bin/cat` and `CAT` do not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomeReadOnlySpec {
    pub(crate) prefix: &'static [&'static str],
    pub(crate) forbidden: &'static [&'static str],
    pub(crate) only: Option<&'static [&'static str]>,
    pub(crate) recursion: Recursion,
    /// Whether followed-symlink searches (`rg -L`, `grep -R`) are eligible for
    /// Home Allow when the link walk is within budget (§10 row 15; proposed
    /// §11 item 15).
    pub(crate) allow_followed_search: bool,
}

/// Normative row for a subcommand of a multi-subcommand tool (e.g. `git`,
/// `cargo`, `pipx`; contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubcommandSpec {
    pub(crate) name: &'static str,
    pub(crate) flag_mode: FlagMode,
    pub(crate) flags: &'static [FlagSpec],
    pub(crate) operands: OperandLayout,
    pub(crate) operand_access: OperandAccessMode,
    pub(crate) writes: WriteBehavior,
    pub(crate) derived_destinations: DerivedDestinationsBehavior,
    pub(crate) recursive: RecursiveBehavior,
    pub(crate) links: LinkPolicy,
    pub(crate) code_runner: CodeRunnerBehavior,
    /// `true` if an Allow prefix ending at `[tool, subcommand]` is Invalid at
    /// creation (§7: e.g. `["pipx", "run"]`, `["git", "merge-index"]`).
    pub(crate) code_runner_no_pin: bool,
    pub(crate) home_readonly: Option<HomeReadOnlySpec>,
    pub(crate) git_selects_by_name: bool,
    /// Contract §11 item(s) when this row changes `dev`'s default behaviour.
    pub(crate) changes_default: Option<&'static str>,
}

/// Normative row for one known tool in §6's table (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolSpec {
    /// Folded (Unicode lowercase + CR-19) tool name.
    pub(crate) name: &'static str,
    pub(crate) platforms: Platforms,
    /// Subcommand table for multi-subcommand tools (`git`, `cargo`, `pipx`).
    /// For `git`, an unlisted subcommand falls back to unknown-tool checking
    /// (every word a candidate path, possible writer, not Home read-only; §4)
    /// while still checking `GIT_GLOBAL_FLAGS`.
    pub(crate) subcommands: &'static [SubcommandSpec],
    pub(crate) flag_mode: FlagMode,
    /// `true` when long option names are spelled with a single leading dash
    /// (e.g. `find` primaries `-follow`, `-delete`, `-exec`).
    pub(crate) single_dash_long: bool,
    pub(crate) flags: &'static [FlagSpec],
    pub(crate) operands: OperandLayout,
    pub(crate) operand_access: OperandAccessMode,
    pub(crate) writes: WriteBehavior,
    pub(crate) derived_destinations: DerivedDestinationsBehavior,
    pub(crate) recursive: RecursiveBehavior,
    pub(crate) links: LinkPolicy,
    pub(crate) code_runner: CodeRunnerBehavior,
    /// `true` if an Allow prefix ending at `[name]` is Invalid at creation
    /// (§7: e.g. `["awk"]`, `["python"]`, `["git"]`).
    pub(crate) code_runner_no_pin: bool,
    /// Modeled §4 wrapper kind, if this tool is one of the 6 closed wrappers.
    pub(crate) wrapper: Option<WrapperKind>,
    /// `true` for modeled nested shells (`sh`, `bash`, `dash`; §4).
    pub(crate) nested_shell: bool,
    /// `true` for downloaders checked by `pipe-to-shell` (`deny::DOWNLOADERS`).
    pub(crate) is_downloader: bool,
    /// Always-blocked hard-deny id if the tool itself is hard-denied
    /// (`deny::PRIV` → `Some("privilege-escalation")`).
    pub(crate) hard_deny: Option<&'static str>,
    pub(crate) home_readonly: Option<HomeReadOnlySpec>,
    pub(crate) filter_dialect: Option<FilterDialect>,
    /// Contract §11 item(s) when this row changes `dev`'s default behaviour.
    pub(crate) changes_default: Option<&'static str>,
}

/// Tool or reserved word known to `dev` that the rev 4 model deliberately
/// leaves out, making its unit `Unread` (contract §3, §4, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutsideModelEntry {
    pub(crate) name: &'static str,
    pub(crate) platforms: Platforms,
    /// Whether an Allow prefix ending at `[name]` is also Invalid at creation
    /// (§7).
    pub(crate) code_runner_no_pin: bool,
    pub(crate) reason: &'static str,
    pub(crate) contract_section: &'static str,
    pub(crate) changes_default: Option<&'static str>,
}

// ---------------------------------------------------------------------------
// Git credential name guards mirrored from `engine.rs` (`GIT_CRED_*`, §6)
// ---------------------------------------------------------------------------

pub(crate) const GIT_CRED_SUFFIXES: &[&str] = &[
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
];

pub(crate) const GIT_CRED_NAMES: &[&str] = &[
    ".netrc",
    ".git-credentials",
    ".npmrc",
    ".claude.json",
    ".pypirc",
    ".pgpass",
    ".vault-token",
    ".env",
    ".bash_history",
    ".zsh_history",
    ".zhistory",
    ".sh_history",
    ".ksh_history",
    ".history",
    "fish_history",
    "accesstokens.json",
];

pub(crate) const GIT_CRED_DIRS: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".config/gcloud",
    ".gemini",
    ".aws/sso/cache",
    ".zsh_sessions",
    ".bash_sessions",
    "library/keychains",
];

// ---------------------------------------------------------------------------
// Helper constructors for static table definitions
// ---------------------------------------------------------------------------

const fn bool_s(short: char) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: None,
        kind: FlagKind::Bool,
        effects: FlagEffects::NONE,
    }
}

const fn bool_l(long: &'static str) -> FlagSpec {
    FlagSpec {
        short: None,
        long: Some(long),
        kind: FlagKind::Bool,
        effects: FlagEffects::NONE,
    }
}

const fn bool_sl(short: char, long: &'static str) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: Some(long),
        kind: FlagKind::Bool,
        effects: FlagEffects::NONE,
    }
}

const fn bool_e(short: Option<char>, long: Option<&'static str>, effects: FlagEffects) -> FlagSpec {
    FlagSpec {
        short,
        long,
        kind: FlagKind::Bool,
        effects,
    }
}

const fn val_s(short: char, role: ValueRole) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: None,
        kind: FlagKind::Value(role),
        effects: FlagEffects::NONE,
    }
}

const fn val_l(long: &'static str, role: ValueRole) -> FlagSpec {
    FlagSpec {
        short: None,
        long: Some(long),
        kind: FlagKind::Value(role),
        effects: FlagEffects::NONE,
    }
}

const fn val_sl(short: char, long: &'static str, role: ValueRole) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: Some(long),
        kind: FlagKind::Value(role),
        effects: FlagEffects::NONE,
    }
}

const fn val_e(
    short: Option<char>,
    long: Option<&'static str>,
    role: ValueRole,
    effects: FlagEffects,
) -> FlagSpec {
    FlagSpec {
        short,
        long,
        kind: FlagKind::Value(role),
        effects,
    }
}

const fn opt_val_sl(short: char, long: &'static str, role: ValueRole) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: Some(long),
        kind: FlagKind::OptionalValue(role),
        effects: FlagEffects::NONE,
    }
}

const LINKS_NONE: LinkPolicy = LinkPolicy {
    at_root: false,
    below_root: false,
};

const LINKS_ROOT_ONLY: LinkPolicy = LinkPolicy {
    at_root: true,
    below_root: false,
};

const LINKS_BOTH: LinkPolicy = LinkPolicy {
    at_root: true,
    below_root: true,
};

const fn base_tool(name: &'static str) -> ToolSpec {
    ToolSpec {
        name,
        platforms: Platforms::Both,
        subcommands: &[],
        flag_mode: FlagMode::AnyExceptForbidden,
        single_dash_long: false,
        flags: &[],
        operands: OperandLayout::all(ValueRole::Path),
        operand_access: OperandAccessMode::AllRead,
        writes: WriteBehavior::Never,
        derived_destinations: DerivedDestinationsBehavior::Never,
        recursive: RecursiveBehavior::Never,
        links: LINKS_ROOT_ONLY,
        code_runner: CodeRunnerBehavior::Never,
        code_runner_no_pin: false,
        wrapper: None,
        nested_shell: false,
        is_downloader: false,
        hard_deny: None,
        home_readonly: None,
        filter_dialect: None,
        changes_default: None,
    }
}

const fn base_subcommand(name: &'static str) -> SubcommandSpec {
    SubcommandSpec {
        name,
        flag_mode: FlagMode::AnyExceptForbidden,
        flags: &[],
        operands: OperandLayout::all(ValueRole::Path),
        operand_access: OperandAccessMode::AllRead,
        writes: WriteBehavior::Never,
        derived_destinations: DerivedDestinationsBehavior::Never,
        recursive: RecursiveBehavior::Never,
        links: LINKS_ROOT_ONLY,
        code_runner: CodeRunnerBehavior::Never,
        code_runner_no_pin: false,
        home_readonly: None,
        git_selects_by_name: false,
        changes_default: None,
    }
}

const fn home_ro(prefix: &'static [&'static str]) -> Option<HomeReadOnlySpec> {
    Some(HomeReadOnlySpec {
        prefix,
        forbidden: &[],
        only: None,
        recursion: Recursion::Never,
        allow_followed_search: false,
    })
}

const GIT_FORBIDDEN: &[&str] = &[
    "--output",
    "--ext-diff",
    "--exec-path",
    "--config-env",
    "-c",
];

const fn home_git_ro(prefix: &'static [&'static str]) -> Option<HomeReadOnlySpec> {
    Some(HomeReadOnlySpec {
        prefix,
        forbidden: GIT_FORBIDDEN,
        only: None,
        recursion: Recursion::Never,
        allow_followed_search: false,
    })
}

const fn command_runner_row(name: &'static str) -> ToolSpec {
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool(name)
    }
}

// ---------------------------------------------------------------------------
// Faithfully copied flag tables from `dev` (`engine.rs`, `gate.rs`,
// `words.rs`, `shell.rs`, `deny.rs`) and contract §4–§6
// ---------------------------------------------------------------------------

/// `rg` flags copied from `engine::RG_BOOL_S`, `RG_VAL_S`, `RG_BOOL_L`,
/// `RG_VAL_L`, plus `-L`/`--follow` and `-f`/`--file` from `gate.rs`.
const RG_FLAGS: &[FlagSpec] = &[
    // Paired short + long booleans
    bool_sl('i', "ignore-case"),
    bool_sl('S', "smart-case"),
    bool_sl('s', "case-sensitive"),
    bool_sl('n', "line-number"),
    bool_sl('N', "no-line-number"),
    bool_sl('H', "with-filename"),
    bool_sl('I', "no-filename"),
    bool_sl('l', "files-with-matches"),
    bool_sl('c', "count"),
    bool_sl('o', "only-matching"),
    bool_sl('v', "invert-match"),
    bool_sl('w', "word-regexp"),
    bool_sl('x', "line-regexp"),
    bool_sl('u', "unrestricted"),
    bool_sl('a', "text"),
    bool_sl('U', "multiline"),
    bool_sl('F', "fixed-strings"),
    bool_sl('P', "pcre2"),
    bool_sl('q', "quiet"),
    bool_sl('0', "null"),
    bool_sl('p', "pretty"),
    bool_sl('b', "byte-offset"),
    bool_e(
        Some('L'),
        Some("follow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    // Long-only booleans
    bool_l("hidden"),
    bool_l("no-ignore"),
    bool_l("no-ignore-vcs"),
    bool_l("no-ignore-dot"),
    bool_l("no-ignore-parent"),
    bool_l("no-ignore-global"),
    bool_l("no-ignore-files"),
    bool_l("no-messages"),
    bool_l("no-heading"),
    bool_l("heading"),
    bool_l("count-matches"),
    bool_l("files-without-match"),
    bool_l("multiline-dotall"),
    bool_l("binary"),
    bool_e(
        None,
        Some("files"),
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        None,
        Some("type-list"),
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    bool_l("json"),
    bool_l("column"),
    bool_l("no-column"),
    bool_l("vimgrep"),
    bool_l("trim"),
    bool_l("stats"),
    bool_l("no-config"),
    bool_l("version"),
    bool_l("help"),
    bool_l("no-pcre2"),
    bool_l("crlf"),
    bool_l("include-zero"),
    bool_l("glob-case-insensitive"),
    bool_l("no-glob-case-insensitive"),
    // Paired short + long value flags
    val_e(
        Some('e'),
        Some("regexp"),
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        Some('g'),
        Some("glob"),
        ValueRole::Pattern,
        FlagEffects {
            filter: Some((FilterDialect::RgGlob, false)),
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("iglob"),
        ValueRole::Pattern,
        FlagEffects {
            filter: Some((FilterDialect::RgGlob, true)),
            ..FlagEffects::NONE
        },
    ),
    val_e(
        Some('f'),
        Some("file"),
        ValueRole::Path,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    val_sl('t', "type", ValueRole::Text),
    val_sl('T', "type-not", ValueRole::Text),
    val_sl('m', "max-count", ValueRole::Text),
    val_sl('A', "after-context", ValueRole::Text),
    val_sl('B', "before-context", ValueRole::Text),
    val_sl('C', "context", ValueRole::Text),
    val_sl('d', "max-depth", ValueRole::Text),
    val_l("maxdepth", ValueRole::Text),
    val_sl('j', "threads", ValueRole::Text),
    val_sl('M', "max-columns", ValueRole::Text),
    val_sl('E', "encoding", ValueRole::Text),
    val_sl('r', "replace", ValueRole::Text),
    // Long-only value flags
    val_l("colors", ValueRole::Text),
    val_l("color", ValueRole::Text),
    val_l("sort", ValueRole::Text),
    val_l("sortr", ValueRole::Text),
    val_l("max-filesize", ValueRole::Text),
    val_l("path-separator", ValueRole::Text),
    val_l("engine", ValueRole::Text),
    val_l("context-separator", ValueRole::Text),
    val_l("type-add", ValueRole::Text),
    val_l("type-clear", ValueRole::Text),
    val_l("field-context-separator", ValueRole::Text),
    val_l("field-match-separator", ValueRole::Text),
    val_l("dfa-size-limit", ValueRole::Text),
    val_l("regex-size-limit", ValueRole::Text),
];

/// `grep` / `egrep` / `fgrep` / `zgrep` / `ggrep` / `rgrep` flags copied from
/// `engine::GREP_BOOL_S`, `GREP_VAL_S`, `GREP_BOOL_L`, `GREP_VAL_L`, plus
/// `-f`/`--file` from `gate.rs`.
const GREP_FLAGS: &[FlagSpec] = &[
    // Paired short + long booleans
    bool_sl('i', "ignore-case"),
    bool_l("no-ignore-case"),
    bool_sl('v', "invert-match"),
    bool_sl('n', "line-number"),
    bool_sl('c', "count"),
    bool_sl('l', "files-with-matches"),
    bool_sl('L', "files-without-match"),
    bool_sl('o', "only-matching"),
    bool_sl('q', "quiet"),
    bool_l("silent"),
    bool_sl('s', "no-messages"),
    bool_sl('w', "word-regexp"),
    bool_sl('x', "line-regexp"),
    bool_sl('h', "no-filename"),
    bool_sl('H', "with-filename"),
    bool_sl('E', "extended-regexp"),
    bool_sl('F', "fixed-strings"),
    bool_sl('G', "basic-regexp"),
    bool_sl('P', "perl-regexp"),
    bool_sl('a', "text"),
    bool_sl('z', "null-data"),
    bool_sl('Z', "null"),
    bool_sl('b', "byte-offset"),
    bool_sl('T', "initial-tab"),
    bool_e(
        Some('r'),
        Some("recursive"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('R'),
        Some("dereference-recursive"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_l("no-group-separator"),
    // Short-only booleans from GREP_BOOL_S
    bool_s('I'),
    bool_s('U'),
    bool_s('y'),
    bool_s('0'),
    bool_s('1'),
    bool_s('2'),
    bool_s('3'),
    bool_s('4'),
    bool_s('5'),
    bool_s('6'),
    bool_s('7'),
    bool_s('8'),
    bool_s('9'),
    // Value flags
    val_e(
        Some('e'),
        Some("regexp"),
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        Some('f'),
        Some("file"),
        ValueRole::Path,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    val_sl('m', "max-count", ValueRole::Text),
    val_sl('A', "after-context", ValueRole::Text),
    val_sl('B', "before-context", ValueRole::Text),
    val_sl('C', "context", ValueRole::Text),
    val_e(
        Some('d'),
        Some("directories"),
        ValueRole::Text,
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    val_sl('D', "devices", ValueRole::Text),
    val_e(
        None,
        Some("include"),
        ValueRole::Pattern,
        FlagEffects {
            filter: Some((FilterDialect::GrepInclude, false)),
            ..FlagEffects::NONE
        },
    ),
    val_l("exclude", ValueRole::Pattern),
    val_l("exclude-dir", ValueRole::Pattern),
    val_l("label", ValueRole::Text),
    val_l("color", ValueRole::Text),
    val_l("colour", ValueRole::Text),
    val_l("binary-files", ValueRole::Text),
    val_l("group-separator", ValueRole::Text),
];

/// `ag` flags copied faithfully from `gate::AG_OPTS`.
const AG_FLAGS: &[FlagSpec] = &[
    // Paired short + long booleans
    bool_sl('a', "all-types"),
    bool_sl('D', "debug"),
    bool_sl('c', "count"),
    bool_sl('F', "fixed-strings"),
    bool_e(
        Some('f'),
        Some("follow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_sl('H', "heading"),
    bool_s('h'),
    bool_sl('i', "ignore-case"),
    bool_sl('L', "files-without-matches"),
    bool_sl('l', "files-with-matches"),
    bool_e(
        Some('n'),
        Some("norecurse"),
        FlagEffects {
            recursive: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_sl('o', "only-matching"),
    bool_sl('Q', "literal"),
    bool_e(
        Some('R'),
        Some("recurse"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('r'),
        None,
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_sl('S', "smart-case"),
    bool_sl('s', "case-sensitive"),
    bool_sl('v', "invert-match"),
    bool_s('V'),
    bool_sl('t', "all-text"),
    bool_sl('u', "unrestricted"),
    bool_sl('U', "skip-vcs-ignores"),
    bool_sl('w', "word-regexp"),
    bool_sl('z', "search-zip"),
    bool_sl('0', "null"),
    // Long-only booleans
    bool_l("ackmate"),
    bool_l("affinity"),
    bool_l("noaffinity"),
    bool_l("hidden"),
    bool_l("column"),
    bool_l("nocolor"),
    bool_l("color"),
    bool_l("color-win-ansi"),
    bool_l("noheading"),
    bool_l("nobreak"),
    bool_l("break"),
    bool_e(
        None,
        Some("nofollow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_l("search-binary"),
    bool_l("silent"),
    bool_l("stats"),
    bool_l("stats-only"),
    bool_l("vimgrep"),
    bool_l("print0"),
    bool_l("one-device"),
    bool_l("filename"),
    bool_l("nofilename"),
    bool_l("numbers"),
    bool_l("nonumbers"),
    bool_l("line-numbers"),
    bool_l("group"),
    bool_l("nogroup"),
    bool_l("match"),
    bool_l("mmap"),
    bool_l("nommap"),
    bool_l("multiline"),
    bool_l("nomultiline"),
    bool_l("nopager"),
    bool_l("parallel"),
    bool_l("passthrough"),
    bool_l("passthru"),
    bool_l("print-all-files"),
    bool_l("print-long-lines"),
    // Value flags
    val_sl('G', "file-search-regex", ValueRole::Pattern),
    val_e(
        Some('g'),
        Some("filename-pattern"),
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    val_sl('m', "max-count", ValueRole::Text),
    val_sl('p', "path-to-ignore", ValueRole::Path),
    val_sl('W', "width", ValueRole::Text),
    val_l("ackmate-dir-filter", ValueRole::Pattern),
    val_l("depth", ValueRole::Text),
    val_l("ignore", ValueRole::Pattern),
    val_l("ignore-dir", ValueRole::Pattern),
    val_e(
        None,
        Some("pager"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_l("workers", ValueRole::Text),
    val_l("color-line-number", ValueRole::Text),
    val_l("color-match", ValueRole::Text),
    val_l("color-path", ValueRole::Text),
    // Optional-value flags
    opt_val_sl('A', "after", ValueRole::Text),
    opt_val_sl('B', "before", ValueRole::Text),
    opt_val_sl('C', "context", ValueRole::Text),
];

/// `ack` flags copied faithfully from `gate::ACK_OPTS`.
const ACK_FLAGS: &[FlagSpec] = &[
    // Paired short + long booleans
    bool_sl('i', "ignore-case"),
    bool_s('I'),
    bool_sl('S', "smart-case"),
    bool_sl('v', "invert-match"),
    bool_sl('w', "word-regexp"),
    bool_sl('Q', "literal"),
    bool_sl('l', "files-with-matches"),
    bool_sl('L', "files-without-matches"),
    bool_sl('c', "count"),
    bool_sl('h', "no-filename"),
    bool_sl('H', "with-filename"),
    bool_s('o'),
    bool_s('1'),
    bool_e(
        Some('n'),
        Some("no-recurse"),
        FlagEffects {
            recursive: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('r'),
        Some("recurse"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('R'),
        None,
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_s('s'),
    bool_s('x'),
    bool_e(
        Some('f'),
        None,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    bool_s('k'),
    // Long-only booleans
    bool_l("column"),
    bool_l("nocolor"),
    bool_l("color"),
    bool_l("noheading"),
    bool_l("heading"),
    bool_l("nobreak"),
    bool_l("break"),
    bool_e(
        None,
        Some("follow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        None,
        Some("nofollow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_l("sort-files"),
    bool_l("nopager"),
    // Value flags
    val_sl('A', "after-context", ValueRole::Text),
    val_sl('B', "before-context", ValueRole::Text),
    val_sl('m', "max-count", ValueRole::Text),
    val_e(
        Some('g'),
        None,
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    val_sl('t', "type", ValueRole::Text),
    val_l("ignore-dir", ValueRole::Pattern),
    val_l("noignore-dir", ValueRole::Pattern),
    val_e(
        None,
        Some("match"),
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("output"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    // Optional-value flags
    opt_val_sl('C', "context", ValueRole::Text),
];

/// Modeled nested-shell flags (§4: `-e`, `-u`, `-x`, `-c SCRIPT`).
const MODELED_SHELL_FLAGS: &[FlagSpec] = &[
    bool_s('e'),
    bool_s('u'),
    bool_s('x'),
    val_e(
        Some('c'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
];

/// `nice` flags (§4: `nice`, `nice -n N`).
const NICE_FLAGS: &[FlagSpec] = &[val_s('n', ValueRole::Text)];

/// `timeout` flags (§4: `timeout [-s SIG] [-k D] D`).
const TIMEOUT_FLAGS: &[FlagSpec] = &[val_s('s', ValueRole::Text), val_s('k', ValueRole::Text)];

/// `ln` flags (§6: `ln` and `ln -s`).
const LN_FLAGS: &[FlagSpec] = &[bool_s('s')];

/// `cp` flags known to `dev` (`gate.rs`: `-r`, `-R`, `-a`, `--recursive`,
/// `--archive`, `-L`, `--dereference`, `-P`, `-d`).
const CP_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('r'),
        None,
        FlagEffects {
            recursive: FlagToggle::Enable,
            derived_destinations: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('R'),
        Some("recursive"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            derived_destinations: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('a'),
        Some("archive"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            derived_destinations: FlagToggle::Enable,
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('L'),
        Some("dereference"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('P'),
        None,
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('d'),
        None,
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
];

/// `rsync` flags known to `dev` (`gate.rs`: `-L`, `-k`, `--copy-links`,
/// `--copy-dirlinks`, `--copy-unsafe-links`).
const RSYNC_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('L'),
        Some("copy-links"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('k'),
        Some("copy-dirlinks"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        None,
        Some("copy-unsafe-links"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
];

/// `tar` flags named in `dev` (`gate.rs`: `-h`, `--dereference`, `-c`, `-r`,
/// `-u`) and contract §5 (`-x`).
const TAR_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('x'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            derived_destinations: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('c'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('r'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('u'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('h'),
        Some("dereference"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
];

/// `cd` flags from `words.rs` (`-L`, `-P`, `-e`, `-@`).
const CD_FLAGS: &[FlagSpec] = &[bool_s('L'), bool_s('P'), bool_s('e'), bool_s('@')];

/// `find` flags known to `dev` (`shell.rs`, `gate.rs`, `deny.rs`, `words.rs`)
/// and contract §4/§5.
///
/// Note: `find` uses single-dash multi-character primary names (`-follow`,
/// `-delete`, `-exec`, …); they are stored in `FlagSpec::long` with
/// `ToolSpec::single_dash_long = true`.
const FIND_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('H'),
        None,
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('L'),
        Some("follow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('P'),
        None,
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::NONE
        },
    ),
    bool_s('E'),
    bool_s('X'),
    bool_s('d'),
    bool_s('s'),
    bool_s('x'),
    val_s('D', ValueRole::Text),
    val_s('f', ValueRole::Path),
    bool_e(
        None,
        Some("delete"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_l("depth"),
    val_l("maxdepth", ValueRole::Text),
    val_l("mindepth", ValueRole::Text),
    val_e(
        None,
        Some("exec"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("execdir"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("ok"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("okdir"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("fprint"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("fprint0"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("fprintf"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("fls"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
];

/// `rm` flags known to `dev` (`deny.rs`: `-r`, `-R`, `--recursive`).
const RM_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('r'),
        Some("recursive"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('R'),
        None,
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
];

/// `sort` / `tree` write flags known to `dev` (`words.rs`).
const OUTPUT_FILE_FLAGS: &[FlagSpec] = &[val_e(
    Some('o'),
    Some("output"),
    ValueRole::Path,
    FlagEffects {
        writes: FlagToggle::Enable,
        ..FlagEffects::NONE
    },
)];

/// `echo` flags known to `dev` (`shell.rs`).
const ECHO_FLAGS: &[FlagSpec] = &[bool_s('n'), bool_s('e'), bool_s('E')];

/// `printf` flags known to `dev` (`shell.rs`).
const PRINTF_FLAGS: &[FlagSpec] = &[val_s('v', ValueRole::Text)];

/// `codesign` flags known to `dev` (`words.rs`).
const CODESIGN_FLAGS: &[FlagSpec] = &[
    val_e(
        Some('s'),
        Some("sign"),
        ValueRole::Text,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        Some('f'),
        Some("force"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        None,
        Some("remove-signature"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_sl('d', "display"),
    bool_sl('v', "verify"),
];

/// `sed` flags known to `dev` (`words.rs`).
const SED_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('i'),
        Some("in-place"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        Some('e'),
        Some("expression"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
];

/// `awk` / `gawk` / `mawk` flags known to `dev` (`words.rs`).
const AWK_FLAGS: &[FlagSpec] = &[val_e(
    Some('i'),
    Some("include"),
    ValueRole::Code,
    FlagEffects {
        writes: FlagToggle::Enable,
        runs_code: true,
        ..FlagEffects::NONE
    },
)];

/// `perl` flags known to `dev` (`words.rs`, `deny.rs`).
const PERL_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('i'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        Some('e'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        Some('E'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
];

/// `python` / `python3` flags known to `dev` (`deny.rs`, contract §4).
const PYTHON_FLAGS: &[FlagSpec] = &[
    val_e(
        Some('c'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        Some('m'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
];

/// `node` flags known to `dev` (`deny.rs`).
const NODE_FLAGS: &[FlagSpec] = &[
    val_e(
        Some('e'),
        Some("eval"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        Some('p'),
        Some("print"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
];

/// `ruby` flags known to `dev` (`deny.rs`).
const RUBY_FLAGS: &[FlagSpec] = &[val_e(
    Some('e'),
    None,
    ValueRole::Code,
    FlagEffects {
        runs_code: true,
        ..FlagEffects::NONE
    },
)];

// ---------------------------------------------------------------------------
// `git`, `cargo`, and `pipx` subcommand tables (§5, §6, §7)
// ---------------------------------------------------------------------------

/// `git` top-level options known to `dev` (`deny.rs`, `HOME_READONLY_COMMANDS`)
/// and contract §6.
const GIT_GLOBAL_FLAGS: &[FlagSpec] = &[
    val_s('C', ValueRole::Path),
    val_e(
        Some('c'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("config-env"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("exec-path"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_l("git-dir", ValueRole::Path),
    val_l("work-tree", ValueRole::Path),
    val_l("namespace", ValueRole::Text),
    val_l("super-prefix", ValueRole::Text),
];

const GIT_LOG_FLAGS: &[FlagSpec] = &[bool_s('p'), val_sl('L', "line-range", ValueRole::Text)];

const GIT_DIFF_FLAGS: &[FlagSpec] = &[
    val_e(
        None,
        Some("output"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::NONE
        },
    ),
    bool_e(
        None,
        Some("ext-diff"),
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
];

const GIT_BRANCH_FLAGS: &[FlagSpec] = &[
    bool_sl('a', "all"),
    bool_sl('r', "remotes"),
    bool_sl('v', "verbose"),
    bool_sl('l', "list"),
    bool_l("show-current"),
];

const GIT_PUSH_FLAGS: &[FlagSpec] = &[
    bool_sl('f', "force"),
    bool_l("force-with-lease"),
    bool_l("force-if-includes"),
    bool_l("mirror"),
    bool_l("all"),
    bool_l("tags"),
    bool_l("branches"),
    bool_l("prune"),
    val_sl('o', "push-option", ValueRole::Text),
    val_e(
        None,
        Some("receive-pack"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
    val_e(
        None,
        Some("exec"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::NONE
        },
    ),
];

const GIT_REBASE_FLAGS: &[FlagSpec] = &[val_e(
    Some('x'),
    Some("exec"),
    ValueRole::Code,
    FlagEffects {
        runs_code: true,
        ..FlagEffects::NONE
    },
)];

const GIT_TOOL_CMD_FLAGS: &[FlagSpec] = &[val_e(
    Some('x'),
    Some("tool-cmd"),
    ValueRole::Code,
    FlagEffects {
        runs_code: true,
        ..FlagEffects::NONE
    },
)];

const GIT_SUBCOMMANDS: &[SubcommandSpec] = &[
    SubcommandSpec {
        home_readonly: home_git_ro(&["git", "status"]),
        changes_default: Some("§11 item 10"),
        ..base_subcommand("status")
    },
    SubcommandSpec {
        flags: GIT_LOG_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        home_readonly: home_git_ro(&["git", "log"]),
        git_selects_by_name: true,
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_subcommand("log")
    },
    SubcommandSpec {
        flags: GIT_DIFF_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        operand_access: OperandAccessMode::FlagOnlyWrite,
        writes: WriteBehavior::WithFlag,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        home_readonly: home_git_ro(&["git", "diff"]),
        git_selects_by_name: true,
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_subcommand("diff")
    },
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Ref),
        home_readonly: home_git_ro(&["git", "show"]),
        git_selects_by_name: true,
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_subcommand("show")
    },
    SubcommandSpec {
        flags: GIT_BRANCH_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        home_readonly: Some(HomeReadOnlySpec {
            prefix: &["git", "branch"],
            forbidden: GIT_FORBIDDEN,
            only: Some(&[
                "-a",
                "--all",
                "-r",
                "--remotes",
                "-v",
                "-vv",
                "--verbose",
                "--list",
                "-l",
                "--show-current",
            ]),
            recursion: Recursion::Never,
            allow_followed_search: false,
        }),
        changes_default: Some("§11 item 9"),
        ..base_subcommand("branch")
    },
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Ref),
        home_readonly: home_git_ro(&["git", "rev-parse"]),
        changes_default: Some("§11 item 9"),
        ..base_subcommand("rev-parse")
    },
    SubcommandSpec {
        home_readonly: home_git_ro(&["git", "ls-files"]),
        changes_default: Some("§11 item 10"),
        ..base_subcommand("ls-files")
    },
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Ref),
        git_selects_by_name: true,
        changes_default: Some("§11 item 9"),
        ..base_subcommand("cat-file")
    },
    SubcommandSpec {
        flags: GIT_PUSH_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        changes_default: Some("§11 item 9"),
        ..base_subcommand("push")
    },
    SubcommandSpec {
        operands: OperandLayout::fixed(&[ValueRole::Ref, ValueRole::DestinationRoot]),
        writes: WriteBehavior::Always,
        derived_destinations: DerivedDestinationsBehavior::Always,
        changes_default: Some("§11 item 10"),
        ..base_subcommand("clone")
    },
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Ref),
        writes: WriteBehavior::Always,
        derived_destinations: DerivedDestinationsBehavior::Always,
        changes_default: Some("§11 item 10"),
        ..base_subcommand("checkout")
    },
    // Program-running git subcommands (§6)
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_subcommand("merge-index")
    },
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_subcommand("filter-branch")
    },
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithSubarg(&["run"]),
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_subcommand("bisect")
    },
    SubcommandSpec {
        flags: GIT_REBASE_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_subcommand("rebase")
    },
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithSubarg(&["foreach"]),
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_subcommand("submodule")
    },
    SubcommandSpec {
        flags: GIT_TOOL_CMD_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_subcommand("difftool")
    },
    SubcommandSpec {
        flags: GIT_TOOL_CMD_FLAGS,
        operands: OperandLayout::all(ValueRole::Path),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_subcommand("mergetool")
    },
];

const fn cargo_build_subcommand(name: &'static str) -> SubcommandSpec {
    SubcommandSpec {
        operands: OperandLayout::all(ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProjectBuild {
            pin: ProgramPin::CommandAndSubcommand,
        },
        changes_default: Some("§11 item 8"),
        ..base_subcommand(name)
    }
}

const CARGO_SUBCOMMANDS: &[SubcommandSpec] = &[
    cargo_build_subcommand("test"),
    cargo_build_subcommand("build"),
    cargo_build_subcommand("run"),
    cargo_build_subcommand("check"),
    cargo_build_subcommand("bench"),
    cargo_build_subcommand("doc"),
    cargo_build_subcommand("clippy"),
];

const PIPX_SUBCOMMANDS: &[SubcommandSpec] = &[SubcommandSpec {
    operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
    writes: WriteBehavior::Always,
    code_runner: CodeRunnerBehavior::ProgramOrInline {
        pin: ProgramPin::FirstOperand,
    },
    code_runner_no_pin: true,
    changes_default: Some("§11 item 4; §11 item 8"),
    ..base_subcommand("run")
}];

// ---------------------------------------------------------------------------
// Normative tool table (§6)
// ---------------------------------------------------------------------------

pub(crate) const TOOLS: &[ToolSpec] = &[
    // --- Modeled §4 wrappers (exact closed forms) ---
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        operands: OperandLayout::NONE,
        wrapper: Some(WrapperKind::Command),
        changes_default: Some("§11 item 10"),
        ..base_tool("command")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        operands: OperandLayout::NONE,
        wrapper: Some(WrapperKind::Exec),
        changes_default: Some("§11 item 10"),
        ..base_tool("exec")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: NICE_FLAGS,
        operands: OperandLayout::NONE,
        wrapper: Some(WrapperKind::Nice),
        changes_default: Some("§11 item 10"),
        ..base_tool("nice")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: TIMEOUT_FLAGS,
        operands: OperandLayout::fixed(&[ValueRole::Text]),
        wrapper: Some(WrapperKind::Timeout),
        changes_default: Some("§11 item 10"),
        ..base_tool("timeout")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        operands: OperandLayout::NONE,
        wrapper: Some(WrapperKind::Time),
        changes_default: Some("§11 item 10"),
        ..base_tool("time")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        operands: OperandLayout::NONE,
        wrapper: Some(WrapperKind::Env),
        changes_default: Some("§11 item 10"),
        ..base_tool("env")
    },
    // --- Modeled §4 nested shells (and script-file Unread (program)) ---
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: MODELED_SHELL_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        nested_shell: true,
        changes_default: Some("§11 item 2; §11 item 4; §11 item 8"),
        ..base_tool("sh")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: MODELED_SHELL_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        nested_shell: true,
        changes_default: Some("§11 item 2; §11 item 4; §11 item 8"),
        ..base_tool("bash")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: MODELED_SHELL_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        nested_shell: true,
        changes_default: Some("§11 item 2; §11 item 4; §11 item 8"),
        ..base_tool("dash")
    },
    // --- Home read-only commands (`HOME_READONLY_COMMANDS`) ---
    ToolSpec {
        home_readonly: home_ro(&["ls"]),
        ..base_tool("ls")
    },
    ToolSpec {
        home_readonly: home_ro(&["pwd"]),
        ..base_tool("pwd")
    },
    ToolSpec {
        home_readonly: home_ro(&["cat"]),
        ..base_tool("cat")
    },
    ToolSpec {
        home_readonly: home_ro(&["head"]),
        ..base_tool("head")
    },
    ToolSpec {
        home_readonly: home_ro(&["tail"]),
        ..base_tool("tail")
    },
    ToolSpec {
        home_readonly: home_ro(&["wc"]),
        ..base_tool("wc")
    },
    ToolSpec {
        home_readonly: Some(HomeReadOnlySpec {
            prefix: &["file"],
            forbidden: &[
                "-C",
                "--compile",
                "-m",
                "--magic-file",
                "-f",
                "--files-from",
            ],
            only: None,
            recursion: Recursion::Never,
            allow_followed_search: false,
        }),
        ..base_tool("file")
    },
    ToolSpec {
        links: LINKS_NONE,
        home_readonly: home_ro(&["stat"]),
        ..base_tool("stat")
    },
    ToolSpec {
        home_readonly: home_ro(&["du"]),
        ..base_tool("du")
    },
    ToolSpec {
        home_readonly: home_ro(&["df"]),
        ..base_tool("df")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Text),
        home_readonly: home_ro(&["which"]),
        changes_default: Some("§11 item 9"),
        ..base_tool("which")
    },
    ToolSpec {
        home_readonly: home_ro(&["uname"]),
        ..base_tool("uname")
    },
    ToolSpec {
        home_readonly: Some(HomeReadOnlySpec {
            prefix: &["date"],
            forbidden: &["-s", "--set", "-f", "--file"],
            only: None,
            recursion: Recursion::Never,
            allow_followed_search: false,
        }),
        ..base_tool("date")
    },
    ToolSpec {
        home_readonly: home_ro(&["whoami"]),
        ..base_tool("whoami")
    },
    ToolSpec {
        flags: OUTPUT_FILE_FLAGS,
        operand_access: OperandAccessMode::FlagOnlyWrite,
        writes: WriteBehavior::WithFlag,
        home_readonly: Some(HomeReadOnlySpec {
            prefix: &["tree"],
            forbidden: &["-o", "--output", "-R"],
            only: None,
            recursion: Recursion::Never,
            allow_followed_search: false,
        }),
        ..base_tool("tree")
    },
    // --- Search tools with faithful `dev` flag tables ---
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: RG_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::Always,
        home_readonly: Some(HomeReadOnlySpec {
            prefix: &["rg"],
            forbidden: &[
                "--pre",
                "--pre-glob",
                "-z",
                "--search-zip",
                "--hostname-bin",
                "-f",
                "--file",
            ],
            only: None,
            recursion: Recursion::Always,
            allow_followed_search: ALLOW_HOME_FOLLOWED_SEARCH,
        }),
        filter_dialect: Some(FilterDialect::RgGlob),
        changes_default: Some("§11 item 9; §11 item 15 (proposed; §10 row 15)"),
        ..base_tool("rg")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        home_readonly: Some(HomeReadOnlySpec {
            prefix: &["grep"],
            forbidden: &["-f", "--file"],
            only: None,
            recursion: Recursion::WithFlags(&[
                "-r",
                "-R",
                "--recursive",
                "--dereference-recursive",
                "-d",
                "--directories",
            ]),
            allow_followed_search: ALLOW_HOME_FOLLOWED_SEARCH,
        }),
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 9; §11 item 15 (proposed; §10 row 15)"),
        ..base_tool("grep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 9"),
        ..base_tool("egrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 9"),
        ..base_tool("fgrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 9"),
        ..base_tool("zgrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 9"),
        ..base_tool("ggrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::Always,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 9"),
        ..base_tool("rgrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: AG_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        changes_default: Some("§11 item 9"),
        ..base_tool("ag")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: ACK_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        changes_default: Some("§11 item 9"),
        ..base_tool("ack")
    },
    // --- Multi-subcommand tools (`git`, `cargo`, `pipx`) ---
    ToolSpec {
        subcommands: GIT_SUBCOMMANDS,
        flags: GIT_GLOBAL_FLAGS,
        operands: OperandLayout::NONE,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool("git")
    },
    ToolSpec {
        subcommands: CARGO_SUBCOMMANDS,
        operands: OperandLayout::NONE,
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProjectBuild {
            pin: ProgramPin::CommandAndSubcommand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("cargo")
    },
    ToolSpec {
        subcommands: PIPX_SUBCOMMANDS,
        operands: OperandLayout::NONE,
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithSubarg(&["run"]),
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("pipx")
    },
    // --- Explicit §5/§6 file, directory, archive, and link tools ---
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: CD_FLAGS,
        operands: OperandLayout::fixed(&[ValueRole::Path]),
        ..base_tool("cd")
    },
    ToolSpec {
        single_dash_long: true,
        flags: FIND_FLAGS,
        operands: OperandLayout::all(ValueRole::Path),
        operand_access: OperandAccessMode::FlagOnlyWrite,
        writes: WriteBehavior::WithFlag,
        recursive: RecursiveBehavior::Always,
        links: LINKS_NONE,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool("find")
    },
    ToolSpec {
        flags: LN_FLAGS,
        operands: OperandLayout::fixed(&[ValueRole::Path, ValueRole::Path]),
        operand_access: OperandAccessMode::SourcesReadLastWrite,
        writes: WriteBehavior::Always,
        links: LINKS_NONE,
        ..base_tool("ln")
    },
    ToolSpec {
        flags: CP_FLAGS,
        operands: OperandLayout {
            positional: &[],
            rest: Some(ValueRole::Path),
            last: Some(ValueRole::DestinationRoot),
            after_double_dash: None,
        },
        operand_access: OperandAccessMode::SourcesReadLastWrite,
        writes: WriteBehavior::Always,
        derived_destinations: DerivedDestinationsBehavior::WithFlag,
        recursive: RecursiveBehavior::WithFlag,
        changes_default: Some("§11 item 10"),
        ..base_tool("cp")
    },
    ToolSpec {
        operands: OperandLayout {
            positional: &[],
            rest: Some(ValueRole::Path),
            last: Some(ValueRole::DestinationRoot),
            after_double_dash: None,
        },
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        derived_destinations: DerivedDestinationsBehavior::WhenSourceIsDir,
        links: LINKS_NONE,
        changes_default: Some("§11 item 10"),
        ..base_tool("mv")
    },
    ToolSpec {
        flags: RSYNC_FLAGS,
        operands: OperandLayout {
            positional: &[],
            rest: Some(ValueRole::Path),
            last: Some(ValueRole::DestinationRoot),
            after_double_dash: None,
        },
        operand_access: OperandAccessMode::SourcesReadLastWrite,
        writes: WriteBehavior::Always,
        derived_destinations: DerivedDestinationsBehavior::Always,
        recursive: RecursiveBehavior::Always,
        changes_default: Some("§11 item 10"),
        ..base_tool("rsync")
    },
    ToolSpec {
        flags: TAR_FLAGS,
        operands: OperandLayout::all(ValueRole::ArchiveMember),
        writes: WriteBehavior::WithFlag,
        derived_destinations: DerivedDestinationsBehavior::WithFlag,
        recursive: RecursiveBehavior::Always,
        links: LINKS_NONE,
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_tool("tar")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::Path], ValueRole::ArchiveMember),
        writes: WriteBehavior::Always,
        derived_destinations: DerivedDestinationsBehavior::Always,
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_tool("unzip")
    },
    // --- Remaining `words::WRITERS` ---
    ToolSpec {
        flags: RM_FLAGS,
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        recursive: RecursiveBehavior::WithFlag,
        links: LINKS_NONE,
        ..base_tool("rm")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("tee")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("touch")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::Text], ValueRole::Path),
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        changes_default: Some("§11 item 9"),
        ..base_tool("chmod")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::Text], ValueRole::Path),
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        changes_default: Some("§11 item 9"),
        ..base_tool("chown")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::Text], ValueRole::Path),
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        changes_default: Some("§11 item 9"),
        ..base_tool("chgrp")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("truncate")
    },
    ToolSpec {
        operand_access: OperandAccessMode::SourcesReadLastWrite,
        writes: WriteBehavior::Always,
        ..base_tool("install")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("mkdir")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        links: LINKS_NONE,
        ..base_tool("rmdir")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        links: LINKS_NONE,
        ..base_tool("unlink")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("shred")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("patch")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("wipefs")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("blkdiscard")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("ed")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        ..base_tool("ex")
    },
    // --- Remaining `words::READERS` (modeled readers) ---
    base_tool("diff"),
    base_tool("cmp"),
    ToolSpec {
        platforms: Platforms::MacOs,
        ..base_tool("md5")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..base_tool("md5sum")
    },
    base_tool("shasum"),
    base_tool("sha1sum"),
    base_tool("sha256sum"),
    ToolSpec {
        links: LINKS_NONE,
        ..base_tool("readlink")
    },
    ToolSpec {
        links: LINKS_BOTH,
        ..base_tool("realpath")
    },
    ToolSpec {
        links: LINKS_NONE,
        ..base_tool("basename")
    },
    ToolSpec {
        links: LINKS_NONE,
        ..base_tool("dirname")
    },
    base_tool("nl"),
    base_tool("od"),
    base_tool("hexdump"),
    base_tool("strings"),
    base_tool("bat"),
    base_tool("column"),
    base_tool("fold"),
    base_tool("expand"),
    base_tool("less"),
    base_tool("more"),
    base_tool("cut"),
    ToolSpec {
        flags: OUTPUT_FILE_FLAGS,
        operand_access: OperandAccessMode::FlagOnlyWrite,
        writes: WriteBehavior::WithFlag,
        ..base_tool("sort")
    },
    ToolSpec {
        operands: OperandLayout::fixed(&[ValueRole::Path, ValueRole::Path]),
        operand_access: OperandAccessMode::SourcesReadLastWrite,
        writes: WriteBehavior::WithOperandCount(2),
        ..base_tool("uniq")
    },
    ToolSpec {
        operands: OperandLayout::fixed(&[ValueRole::Path, ValueRole::Path]),
        operand_access: OperandAccessMode::SourcesReadLastWrite,
        writes: WriteBehavior::WithOperandCount(2),
        ..base_tool("xxd")
    },
    ToolSpec {
        flags: ECHO_FLAGS,
        operands: OperandLayout::all(ValueRole::Text),
        changes_default: Some("§11 item 9"),
        ..base_tool("echo")
    },
    ToolSpec {
        flags: PRINTF_FLAGS,
        operands: OperandLayout::all(ValueRole::Text),
        changes_default: Some("§11 item 9"),
        ..base_tool("printf")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Text),
        changes_default: Some("§11 item 9"),
        ..base_tool("whereis")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Text),
        changes_default: Some("§11 item 9"),
        ..base_tool("type")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Text),
        changes_default: Some("§11 item 9"),
        ..base_tool("tr")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Text),
        changes_default: Some("§11 item 9"),
        ..base_tool("true")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Text),
        changes_default: Some("§11 item 9"),
        ..base_tool("false")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Text),
        changes_default: Some("§11 item 9"),
        ..base_tool("id")
    },
    base_tool("test"),
    base_tool("["),
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        changes_default: Some("§11 item 9"),
        ..base_tool("jq")
    },
    ToolSpec {
        platforms: Platforms::MacOs,
        ..base_tool("mdls")
    },
    ToolSpec {
        platforms: Platforms::MacOs,
        ..base_tool("open")
    },
    ToolSpec {
        platforms: Platforms::MacOs,
        ..base_tool("spctl")
    },
    ToolSpec {
        platforms: Platforms::MacOs,
        ..base_tool("mdfind")
    },
    ToolSpec {
        platforms: Platforms::MacOs,
        flags: CODESIGN_FLAGS,
        writes: WriteBehavior::WithFlag,
        ..base_tool("codesign")
    },
    // --- Code runners & interpreters (§4, §6, §7) ---
    ToolSpec {
        flags: AWK_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Code], ValueRole::Path),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool("awk")
    },
    ToolSpec {
        flags: AWK_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Code], ValueRole::Path),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool("gawk")
    },
    ToolSpec {
        flags: AWK_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Code], ValueRole::Path),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool("mawk")
    },
    ToolSpec {
        flags: SED_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Code], ValueRole::Path),
        writes: WriteBehavior::WithFlag,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool("sed")
    },
    ToolSpec {
        flags: PERL_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("perl")
    },
    ToolSpec {
        flags: PYTHON_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("python")
    },
    ToolSpec {
        flags: PYTHON_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("python3")
    },
    ToolSpec {
        flags: NODE_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("node")
    },
    ToolSpec {
        flags: RUBY_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("ruby")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool("eval")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("source")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool(".")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Text),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("npx")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4"),
        ..base_tool("xargs")
    },
    // --- `shell::COMMAND_RUNNERS` (marked as code runners in §4, §6) ---
    command_runner_row("ssh"),
    command_runner_row("mosh"),
    command_runner_row("autossh"),
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("nsenter")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("unshare")
    },
    command_runner_row("docker"),
    command_runner_row("podman"),
    command_runner_row("nerdctl"),
    command_runner_row("kubectl"),
    command_runner_row("oc"),
    command_runner_row("lxc"),
    command_runner_row("incus"),
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("machinectl")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("systemd-run")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("systemd-nspawn")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("firejail")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("bwrap")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("flatpak-spawn")
    },
    command_runner_row("toolbox"),
    command_runner_row("distrobox"),
    command_runner_row("distrobox-enter"),
    command_runner_row("multipass"),
    command_runner_row("vagrant"),
    command_runner_row("limactl"),
    command_runner_row("colima"),
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("runuser")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..command_runner_row("setpriv")
    },
    command_runner_row("parallel"),
    // --- `deny::PRIV` (always hard-denied `privilege-escalation`, also code runners) ---
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        hard_deny: Some("privilege-escalation"),
        ..base_tool("sudo")
    },
    ToolSpec {
        writes: WriteBehavior::Always,
        hard_deny: Some("privilege-escalation"),
        ..base_tool("sudoedit")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        hard_deny: Some("privilege-escalation"),
        ..base_tool("su")
    },
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        hard_deny: Some("privilege-escalation"),
        ..base_tool("doas")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        hard_deny: Some("privilege-escalation"),
        ..base_tool("pkexec")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        operands: OperandLayout::all(ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        hard_deny: Some("privilege-escalation"),
        ..base_tool("run0")
    },
    // --- `deny::DOWNLOADERS` (pipe-to-shell sources) ---
    ToolSpec {
        writes: WriteBehavior::Always,
        is_downloader: true,
        ..base_tool("curl")
    },
    ToolSpec {
        writes: WriteBehavior::Always,
        is_downloader: true,
        ..base_tool("wget")
    },
    ToolSpec {
        writes: WriteBehavior::Always,
        is_downloader: true,
        ..base_tool("fetch")
    },
];

// ---------------------------------------------------------------------------
// Explicit OUTSIDE_MODEL list: tools `dev` knows that rev 4 leaves Unread
// ---------------------------------------------------------------------------

const fn outside_wrapper(name: &'static str, platforms: Platforms) -> OutsideModelEntry {
    OutsideModelEntry {
        name,
        platforms,
        code_runner_no_pin: true,
        reason: "Wrapper outside §4's closed list; makes the unit Unread",
        contract_section: "§4",
        changes_default: Some("§11 item 10"),
    }
}

const fn outside_shell(name: &'static str) -> OutsideModelEntry {
    OutsideModelEntry {
        name,
        platforms: Platforms::Both,
        code_runner_no_pin: true,
        reason: "Shell outside §4's modeled sh/bash/dash subset; makes the unit Unread",
        contract_section: "§4",
        changes_default: Some("§11 item 4"),
    }
}

const fn outside_reserved(name: &'static str) -> OutsideModelEntry {
    OutsideModelEntry {
        name,
        platforms: Platforms::Both,
        code_runner_no_pin: false,
        reason: "Shell reserved word or compound construct outside §3's simple-command model",
        contract_section: "§3",
        changes_default: None,
    }
}

pub(crate) const OUTSIDE_MODEL: &[OutsideModelEntry] = &[
    // --- Shells in `shell::SHELLS` other than sh/bash/dash (§4) ---
    OutsideModelEntry {
        name: "zsh",
        platforms: Platforms::Both,
        code_runner_no_pin: true,
        reason: "zsh glob qualifiers and `=cmd` run code; outside §4's nested-shell model",
        contract_section: "§4",
        changes_default: Some("§11 item 4"),
    },
    outside_shell("ksh"),
    outside_shell("fish"),
    outside_shell("ash"),
    outside_shell("csh"),
    outside_shell("tcsh"),
    outside_shell("mksh"),
    outside_shell("yash"),
    outside_shell("rbash"),
    outside_shell("posh"),
    outside_shell("elvish"),
    outside_shell("xonsh"),
    // --- Wrappers in `shell.rs` outside §4's closed list of 6 ---
    outside_wrapper("nohup", Platforms::Both),
    outside_wrapper("builtin", Platforms::Both),
    outside_wrapper("busybox", Platforms::Linux),
    outside_wrapper("toybox", Platforms::Linux),
    outside_wrapper("setsid", Platforms::Linux),
    outside_wrapper("unbuffer", Platforms::Both),
    outside_wrapper("caffeinate", Platforms::MacOs),
    outside_wrapper("sandbox-exec", Platforms::MacOs),
    outside_wrapper("chroot", Platforms::Both),
    outside_wrapper("ionice", Platforms::Linux),
    outside_wrapper("stdbuf", Platforms::Both),
    outside_wrapper("chrt", Platforms::Linux),
    outside_wrapper("taskset", Platforms::Linux),
    outside_wrapper("flock", Platforms::Linux),
    outside_wrapper("script", Platforms::Both),
    outside_wrapper("arch", Platforms::MacOs),
    // --- Reserved words in `shell::RESERVED` (§3) ---
    outside_reserved("if"),
    outside_reserved("then"),
    outside_reserved("else"),
    outside_reserved("elif"),
    outside_reserved("fi"),
    outside_reserved("for"),
    outside_reserved("while"),
    outside_reserved("until"),
    outside_reserved("do"),
    outside_reserved("done"),
    outside_reserved("case"),
    outside_reserved("esac"),
    outside_reserved("function"),
    outside_reserved("select"),
    outside_reserved("{"),
    outside_reserved("}"),
    outside_reserved("!"),
    outside_reserved("[["),
    outside_reserved("coproc"),
    // --- Directory-stack builtins outside §5's `cd` model (`words::READERS`) ---
    OutsideModelEntry {
        name: "pushd",
        platforms: Platforms::Both,
        code_runner_no_pin: false,
        reason: "Mutates shell directory stack; makes cwd unknown (§5)",
        contract_section: "§5",
        changes_default: None,
    },
    OutsideModelEntry {
        name: "popd",
        platforms: Platforms::Both,
        code_runner_no_pin: false,
        reason: "Mutates shell directory stack; makes cwd unknown (§5)",
        contract_section: "§5",
        changes_default: None,
    },
];

// ---------------------------------------------------------------------------
// Plain table-read accessors (argv parsing belongs to P3)
// ---------------------------------------------------------------------------

/// Looks up a tool row by its CR-19 folded command name.
pub(crate) fn lookup(folded_name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|t| t.name == folded_name)
}

/// Looks up an explicit `OUTSIDE_MODEL` entry by its folded name.
pub(crate) fn lookup_outside_model(folded_name: &str) -> Option<&'static OutsideModelEntry> {
    OUTSIDE_MODEL.iter().find(|e| e.name == folded_name)
}

/// Looks up a subcommand row on `tool` by exact subcommand name.
pub(crate) fn subcommand(tool: &'static ToolSpec, name: &str) -> Option<&'static SubcommandSpec> {
    tool.subcommands.iter().find(|s| s.name == name)
}

/// Looks up a short flag in `flags`.
pub(crate) fn flag_short(flags: &'static [FlagSpec], short: char) -> Option<&'static FlagSpec> {
    flags.iter().find(|f| f.short == Some(short))
}

/// Looks up a long flag (without leading `--`) in `flags`.
pub(crate) fn flag_long(flags: &'static [FlagSpec], long: &str) -> Option<&'static FlagSpec> {
    flags.iter().find(|f| f.long == Some(long))
}

/// Looks up a single flag token (`"-x"` or `"--name"`) in `flags`.
pub(crate) fn flag(flags: &'static [FlagSpec], token: &str) -> Option<&'static FlagSpec> {
    if let Some(long) = token.strip_prefix("--") {
        if long.is_empty() {
            return None;
        }
        flag_long(flags, long)
    } else if let Some(short_str) = token.strip_prefix('-') {
        let mut chars = short_str.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => flag_short(flags, c),
            _ => None,
        }
    } else {
        None
    }
}

pub(crate) const fn all_tools() -> &'static [ToolSpec] {
    TOOLS
}

pub(crate) const fn outside_model() -> &'static [OutsideModelEntry] {
    OUTSIDE_MODEL
}

// ---------------------------------------------------------------------------
// Deterministic Markdown renderer for `docs/contracts/m1-policy-rev4-tools.md`
// ---------------------------------------------------------------------------

fn fmt_role(role: ValueRole) -> &'static str {
    match role {
        ValueRole::Path => "Path",
        ValueRole::Pattern => "Pattern",
        ValueRole::ArchiveMember => "ArchiveMember",
        ValueRole::Ref => "Ref",
        ValueRole::Text => "Text",
        ValueRole::DestinationRoot => "DestinationRoot",
        ValueRole::Code => "Code",
        ValueRole::ProgramFile => "ProgramFile",
    }
}

fn fmt_operands(layout: OperandLayout) -> String {
    let mut parts: Vec<String> = layout
        .positional
        .iter()
        .enumerate()
        .map(|(i, r)| format!("[{i}] {}", fmt_role(*r)))
        .collect();
    if let Some(rest) = layout.rest {
        parts.push(format!("rest: {}", fmt_role(rest)));
    }
    if let Some(last) = layout.last {
        parts.push(format!("last: {}", fmt_role(last)));
    }
    if parts.is_empty() {
        "—".to_string()
    } else {
        parts.join(", ")
    }
}

fn fmt_flags_summary(flags: &[FlagSpec]) -> String {
    if flags.is_empty() {
        return "none (any flag → Unread)".to_string();
    }
    if flags.len() > 12 {
        let bool_n = flags
            .iter()
            .filter(|f| matches!(f.kind, FlagKind::Bool))
            .count();
        let val_n = flags.len() - bool_n;
        return format!(
            "{} flags ({bool_n} bool, {val_n} value; see flag table below)",
            flags.len()
        );
    }
    flags
        .iter()
        .map(|f| {
            let name = match (f.short, f.long) {
                (Some(s), Some(l)) => format!("-{s}/--{l}"),
                (Some(s), None) => format!("-{s}"),
                (None, Some(l)) => format!("--{l}"),
                (None, None) => String::new(),
            };
            match f.kind {
                FlagKind::Bool => format!("`{name}`"),
                FlagKind::Value(r) => format!("`{name}` ({})", fmt_role(r)),
                FlagKind::OptionalValue(r) => format!("`{name}` ([{}])", fmt_role(r)),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

struct EffectFields {
    writes: WriteBehavior,
    derived: DerivedDestinationsBehavior,
    recursive: RecursiveBehavior,
    links: LinkPolicy,
    code: CodeRunnerBehavior,
    no_pin: bool,
    filter: Option<FilterDialect>,
    git_sel: bool,
}

fn fmt_effects(e: EffectFields) -> String {
    let mut items: Vec<String> = Vec::new();
    match e.writes {
        WriteBehavior::Never => {}
        WriteBehavior::Always => items.push("writes".into()),
        WriteBehavior::WithFlag => items.push("writes (flag)".into()),
        WriteBehavior::WithOperandCount(n) => items.push(format!("writes (>= {n} args)")),
        WriteBehavior::WithFlagOrOperandCount(n) => {
            items.push(format!("writes (flag or >= {n} args unless disabled)"))
        }
    }
    match e.derived {
        DerivedDestinationsBehavior::Never => {}
        DerivedDestinationsBehavior::Always => items.push("derived-dest".into()),
        DerivedDestinationsBehavior::WithFlag => items.push("derived-dest (flag)".into()),
        DerivedDestinationsBehavior::WhenSourceIsDir => items.push("derived-dest (dir src)".into()),
    }
    match e.recursive {
        RecursiveBehavior::Never => {}
        RecursiveBehavior::Always => items.push("recursive".into()),
        RecursiveBehavior::WithFlag => items.push("recursive (flag)".into()),
    }
    items.push(format!(
        "links(root={}, below={})",
        if e.links.at_root { "Y" } else { "N" },
        if e.links.below_root { "Y" } else { "N" }
    ));
    match e.code {
        CodeRunnerBehavior::Never => {}
        CodeRunnerBehavior::InlineOnly => items.push("code: inline".into()),
        CodeRunnerBehavior::InlineWithFlag => items.push("code: flag".into()),
        CodeRunnerBehavior::ScriptCommands(cmds) => {
            let list = cmds
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join("/");
            items.push(format!("code: script commands ({list})"));
        }
        CodeRunnerBehavior::InlineWithSubarg(s) => {
            items.push(format!("code: subarg {}", s.join("/")))
        }
        CodeRunnerBehavior::ProgramOrInline { pin } => {
            items.push(format!("code: program/inline (pin: {pin:?})"))
        }
        CodeRunnerBehavior::ProjectBuild { pin } => {
            items.push(format!("code: build (pin: {pin:?})"))
        }
    }
    if e.no_pin {
        items.push("unpinned-prefix-invalid".into());
    }
    if let Some(d) = e.filter {
        items.push(format!("filter: {d:?}"));
    }
    if e.git_sel {
        items.push("git-cred-select".into());
    }
    items.join("; ")
}

fn fmt_home(h: Option<HomeReadOnlySpec>) -> String {
    let Some(h) = h else {
        return "No".to_string();
    };
    let mut s = String::from("Yes");
    if !h.forbidden.is_empty() {
        let _ = write!(s, " (forbids {})", h.forbidden.join(", "));
    }
    if let Some(only) = h.only {
        let _ = write!(s, " (only {})", only.join(", "));
    }
    match h.recursion {
        Recursion::Never => {}
        Recursion::Always => s.push_str(" [recursive]"),
        Recursion::WithFlags(_) => s.push_str(" [recursive w/ flags]"),
    }
    s
}

/// Renders the entire normative tool table as deterministic Markdown for
/// review in `docs/contracts/m1-policy-rev4-tools.md`.
pub(crate) fn render_markdown() -> String {
    let mut out = String::new();
    out.push_str("# Policy engine rev 4: normative tool table (annex to §6)\n\n");
    out.push_str(
        "Generated from `crates/atrium-core/src/policy/tools.rs`. Do not edit by hand; \
         regenerate with `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.\n\n",
    );
    out.push_str(
        "**Unknown tools vs. excluded forms (§4, §6):**\n\
         - An **unknown tool** (absent from both **Modeled tools** and **Outside the model** below) \
           stays in the model for hard-deny and store checks (§4): every word counts as a candidate `Path`, \
           the unit counts as a possible writer (§5), and it is never eligible for Home's read-only list. \
           Under a covering prefix/exact Allow rule meeting §7's simple-command conditions, Clear targets remain Clear.\n\
         - An **explicitly excluded form** makes its unit **Unread**: every entry in **Outside the model**, \
           any unlisted flag on a `ListedOnly` row, any forbidden flag on an `AnyExceptForbidden` row, \
           or any form triggering an `Unread` effect (§3–§6).\n\n",
    );

    // 1. Main tools table
    out.push_str("## Modeled tools\n\n");
    out.push_str(
        "| Tool | Platforms | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|\n");
    for t in TOOLS {
        let mut effects = fmt_effects(EffectFields {
            writes: t.writes,
            derived: t.derived_destinations,
            recursive: t.recursive,
            links: t.links,
            code: t.code_runner,
            no_pin: t.code_runner_no_pin,
            filter: t.filter_dialect,
            git_sel: false,
        });
        if let Some(w) = t.wrapper {
            let _ = write!(effects, "; wrapper: {w:?}");
        }
        if t.nested_shell {
            effects.push_str("; nested-shell");
        }
        if t.is_downloader {
            effects.push_str("; downloader");
        }
        if let Some(hd) = t.hard_deny {
            let _ = write!(effects, "; hard-deny: `{hd}`");
        }
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} | {} |",
            t.name,
            t.platforms.label(),
            fmt_flags_summary(t.flags),
            fmt_operands(t.operands),
            effects,
            fmt_home(t.home_readonly),
            t.changes_default.unwrap_or("—"),
        );
    }

    // 2. Subcommand tables
    out.push_str("\n## Subcommands\n");
    for t in TOOLS {
        if t.subcommands.is_empty() {
            continue;
        }
        let _ = writeln!(out, "\n### `{}`\n", t.name);
        out.push_str(
            "| Subcommand | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |\n",
        );
        out.push_str("|---|---|---|---|---|---|\n");
        for s in t.subcommands {
            let effects = fmt_effects(EffectFields {
                writes: s.writes,
                derived: s.derived_destinations,
                recursive: s.recursive,
                links: s.links,
                code: s.code_runner,
                no_pin: s.code_runner_no_pin,
                filter: None,
                git_sel: s.git_selects_by_name,
            });
            let _ = writeln!(
                out,
                "| `{} {}` | {} | {} | {} | {} | {} |",
                t.name,
                s.name,
                fmt_flags_summary(s.flags),
                fmt_operands(s.operands),
                effects,
                fmt_home(s.home_readonly),
                s.changes_default.unwrap_or("—"),
            );
        }
    }

    // 3. Detailed flag tables for tools with >12 flags
    out.push_str("\n## Detailed flag tables\n");
    for t in TOOLS {
        if t.flags.len() <= 12 {
            continue;
        }
        let _ = writeln!(out, "\n### `{}` ({} flags)\n", t.name, t.flags.len());
        out.push_str("| Short | Long | Kind | Effects |\n");
        out.push_str("|---|---|---|---|\n");
        for f in t.flags {
            let short = f.short.map_or_else(|| "—".into(), |c| format!("`-{c}`"));
            let long = f.long.map_or_else(|| "—".into(), |l| format!("`--{l}`"));
            let kind = match f.kind {
                FlagKind::Bool => "Bool".into(),
                FlagKind::Value(r) => format!("Value({})", fmt_role(r)),
                FlagKind::OptionalValue(r) => format!("OptionalValue({})", fmt_role(r)),
            };
            let mut eff = Vec::new();
            match f.effects.writes {
                FlagToggle::Unchanged => {}
                FlagToggle::Enable => eff.push("writes:+".to_string()),
                FlagToggle::Disable => eff.push("writes:-".to_string()),
            }
            match f.effects.derived_destinations {
                FlagToggle::Unchanged => {}
                FlagToggle::Enable => eff.push("derived-dest:+".to_string()),
                FlagToggle::Disable => eff.push("derived-dest:-".to_string()),
            }
            match f.effects.recursive {
                FlagToggle::Unchanged => {}
                FlagToggle::Enable => eff.push("recursive:+".to_string()),
                FlagToggle::Disable => eff.push("recursive:-".to_string()),
            }
            if f.effects.follows_links_at_root != FlagToggle::Unchanged
                || f.effects.follows_links_below_root != FlagToggle::Unchanged
            {
                let sym = |t: FlagToggle| match t {
                    FlagToggle::Unchanged => "=",
                    FlagToggle::Enable => "+",
                    FlagToggle::Disable => "-",
                };
                eff.push(format!(
                    "links(root={}, below={})",
                    sym(f.effects.follows_links_at_root),
                    sym(f.effects.follows_links_below_root)
                ));
            }
            if f.effects.provides_pattern {
                eff.push("provides-pattern".to_string());
            }
            if f.effects.provides_script {
                eff.push("provides-script".to_string());
            }
            if let Some(role) = f.effects.positional_role {
                eff.push(format!("positional-role({})", fmt_role(role)));
            }
            if let Some((d, ci)) = f.effects.filter {
                eff.push(format!("filter({d:?}, ci={ci})"));
            }
            if f.effects.runs_code {
                eff.push("runs-code".to_string());
            }
            let eff_str = if eff.is_empty() {
                "—".to_string()
            } else {
                eff.join("; ")
            };
            let _ = writeln!(out, "| {short} | {long} | {kind} | {eff_str} |");
        }
    }

    // 4. Outside the model
    out.push_str("\n## Outside the model (`OUTSIDE_MODEL`)\n\n");
    out.push_str(
        "| Name | Platforms | Unpinned prefix invalid | Contract | Changes default (§11) | Reason |\n",
    );
    out.push_str("|---|---|---|---|---|---|\n");
    for e in OUTSIDE_MODEL {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} | {} | {} | {} |",
            e.name,
            e.platforms.label(),
            if e.code_runner_no_pin { "Yes" } else { "No" },
            e.contract_section,
            e.changes_default.unwrap_or("—"),
            e.reason,
        );
    }

    // 5. Summary of rows changing defaults (for Pedro)
    out.push_str("\n## Summary of rows changing `dev` defaults (§11)\n\n");
    out.push_str("| Entry | §11 item(s) |\n");
    out.push_str("|---|---|\n");
    for t in TOOLS {
        if let Some(cd) = t.changes_default {
            let _ = writeln!(out, "| `{}` | {cd} |", t.name);
        }
        for s in t.subcommands {
            if let Some(cd) = s.changes_default {
                let _ = writeln!(out, "| `{} {}` | {cd} |", t.name, s.name);
            }
        }
    }
    for e in OUTSIDE_MODEL {
        if let Some(cd) = e.changes_default {
            let _ = writeln!(out, "| `{}` (outside model) | {cd} |", e.name);
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Integrity tests & Markdown snapshot test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use atrium_protocol::policy::HOME_READONLY_COMMANDS;

    use super::*;
    use crate::policy::{deny, engine, gate, shell, words};

    fn check_flag_slice_unique(context: &str, flags: &[FlagSpec]) {
        let mut shorts = HashSet::new();
        let mut longs = HashSet::new();
        for f in flags {
            assert!(
                f.short.is_some() || f.long.is_some(),
                "{context}: flag has neither short nor long name"
            );
            if let Some(s) = f.short {
                assert!(shorts.insert(s), "{context}: duplicate short flag -{s}");
            }
            if let Some(l) = f.long {
                assert!(
                    !l.starts_with('-'),
                    "{context}: long flag `{l}` must omit leading `--`"
                );
                assert!(longs.insert(l), "{context}: duplicate long flag --{l}");
            }
        }
    }

    #[test]
    fn no_duplicate_names_subcommands_or_flags() {
        let mut names = HashSet::new();
        for t in TOOLS {
            assert!(names.insert(t.name), "duplicate tool name `{}`", t.name);
            check_flag_slice_unique(t.name, t.flags);
            let mut subs = HashSet::new();
            for s in t.subcommands {
                assert!(
                    subs.insert(s.name),
                    "duplicate subcommand `{} {}`",
                    t.name,
                    s.name
                );
                check_flag_slice_unique(&format!("{} {}", t.name, s.name), s.flags);
            }
        }
        let mut outside = HashSet::new();
        for e in OUTSIDE_MODEL {
            assert!(
                outside.insert(e.name),
                "duplicate OUTSIDE_MODEL name `{}`",
                e.name
            );
            assert!(
                !names.contains(e.name),
                "`{}` appears in both TOOLS and OUTSIDE_MODEL",
                e.name
            );
        }
    }

    #[test]
    fn value_taking_flags_have_valid_roles() {
        let verify = |context: &str, flags: &[FlagSpec]| {
            for f in flags {
                if f.effects.filter.is_some() {
                    assert_eq!(
                        f.kind,
                        FlagKind::Value(ValueRole::Pattern),
                        "{context}: filter flag must take Value(Pattern)"
                    );
                }
                if let FlagKind::Value(role) | FlagKind::OptionalValue(role) = f.kind {
                    // Exhaustive match confirms every value flag assigns a concrete ValueRole.
                    let _ = fmt_role(role);
                }
            }
        };
        for t in TOOLS {
            verify(t.name, t.flags);
            for s in t.subcommands {
                verify(&format!("{} {}", t.name, s.name), s.flags);
            }
        }
    }

    #[test]
    fn code_running_rows_name_code_operand_or_pinning() {
        let has_code_operand = |layout: OperandLayout, flags: &[FlagSpec]| {
            layout.positional.contains(&ValueRole::Code)
                || layout.rest == Some(ValueRole::Code)
                || flags.iter().any(|f| {
                    f.effects.runs_code
                        || matches!(
                            f.kind,
                            FlagKind::Value(ValueRole::Code)
                                | FlagKind::OptionalValue(ValueRole::Code)
                        )
                })
        };
        let has_program_file = |layout: OperandLayout| {
            layout.positional.contains(&ValueRole::ProgramFile)
                || layout.rest == Some(ValueRole::ProgramFile)
        };

        let check_row = |label: &str,
                         code: CodeRunnerBehavior,
                         no_pin: bool,
                         layout: OperandLayout,
                         flags: &[FlagSpec]| {
            match code {
                CodeRunnerBehavior::Never => {
                    assert!(
                        !no_pin,
                        "{label}: code_runner_no_pin set on non-code-runner"
                    );
                }
                CodeRunnerBehavior::InlineOnly => {
                    assert!(
                        no_pin || has_code_operand(layout, flags),
                        "{label}: InlineOnly row must mark code operand or no-pin"
                    );
                }
                CodeRunnerBehavior::InlineWithFlag => {
                    assert!(
                        flags.iter().any(|f| f.effects.runs_code),
                        "{label}: InlineWithFlag row must have a runs_code flag"
                    );
                }
                CodeRunnerBehavior::ScriptCommands(cmds) => {
                    assert!(
                        !cmds.is_empty(),
                        "{label}: ScriptCommands must list trigger commands"
                    );
                    assert!(
                        has_code_operand(layout, flags),
                        "{label}: ScriptCommands must name a Code operand or flag"
                    );
                }
                CodeRunnerBehavior::InlineWithSubarg(subargs) => {
                    assert!(
                        !subargs.is_empty(),
                        "{label}: InlineWithSubarg must name subarg(s)"
                    );
                }
                CodeRunnerBehavior::ProgramOrInline { pin } => {
                    assert_ne!(
                        pin,
                        ProgramPin::None,
                        "{label}: ProgramOrInline must specify a ProgramPin"
                    );
                    assert!(
                        has_program_file(layout) || has_code_operand(layout, flags),
                        "{label}: ProgramOrInline must name ProgramFile or Code operand"
                    );
                }
                CodeRunnerBehavior::ProjectBuild { pin } => {
                    assert_ne!(
                        pin,
                        ProgramPin::None,
                        "{label}: ProjectBuild must specify a ProgramPin"
                    );
                }
            }
        };

        for t in TOOLS {
            check_row(
                t.name,
                t.code_runner,
                t.code_runner_no_pin,
                t.operands,
                t.flags,
            );
            for s in t.subcommands {
                check_row(
                    &format!("{} {}", t.name, s.name),
                    s.code_runner,
                    s.code_runner_no_pin,
                    s.operands,
                    s.flags,
                );
            }
        }
    }

    #[test]
    fn home_readonly_rows_equal_protocol() {
        let mut table_rows: Vec<HomeReadOnlySpec> = Vec::new();
        for t in TOOLS {
            if let Some(h) = t.home_readonly {
                table_rows.push(h);
            }
            for s in t.subcommands {
                if let Some(h) = s.home_readonly {
                    table_rows.push(h);
                }
            }
        }
        assert_eq!(
            table_rows.len(),
            HOME_READONLY_COMMANDS.len(),
            "Home read-only row count differs from HOME_READONLY_COMMANDS"
        );
        for proto in HOME_READONLY_COMMANDS {
            let matched = table_rows
                .iter()
                .find(|r| r.prefix == proto.prefix)
                .unwrap_or_else(|| {
                    panic!(
                        "HOME_READONLY_COMMANDS entry {:?} missing from tools.rs",
                        proto.prefix
                    )
                });
            assert_eq!(
                matched.forbidden, proto.forbidden,
                "forbidden mismatch for {:?}",
                proto.prefix
            );
            assert_eq!(
                matched.only, proto.only,
                "`only` mismatch for {:?}",
                proto.prefix
            );
            assert_eq!(
                matched.recursion, proto.recursion,
                "recursion mismatch for {:?}",
                proto.prefix
            );
        }
    }

    #[test]
    fn dev_lists_and_flag_tables_fully_accounted_for() {
        let accounted = |name: &str| lookup(name).is_some() || lookup_outside_model(name).is_some();

        // 1. HOME_READONLY_COMMANDS
        for entry in HOME_READONLY_COMMANDS {
            let cmd = entry.prefix[0];
            assert!(
                lookup(cmd).is_some(),
                "HOME_READONLY_COMMANDS `{cmd}` must be in TOOLS"
            );
            if entry.prefix.len() > 1 {
                let tool = lookup(cmd).unwrap();
                assert!(
                    subcommand(tool, entry.prefix[1]).is_some(),
                    "HOME_READONLY_COMMANDS subcommand {:?} missing",
                    entry.prefix
                );
            }
        }

        // 2. words.rs WRITERS & READERS
        for &w in words::WRITERS {
            assert!(accounted(w), "words::WRITERS `{w}` not accounted for");
        }
        for &r in words::READERS {
            assert!(accounted(r), "words::READERS `{r}` not accounted for");
        }

        // 3. shell.rs SHELLS, COMMAND_RUNNERS, RESERVED, and wrapper_opts names
        for &s in shell::SHELLS {
            assert!(accounted(s), "shell::SHELLS `{s}` not accounted for");
        }
        for &c in shell::COMMAND_RUNNERS {
            assert!(
                lookup(c).is_some(),
                "shell::COMMAND_RUNNERS `{c}` must be marked in TOOLS (§4)"
            );
        }
        for &r in shell::RESERVED {
            assert!(
                lookup_outside_model(r).is_some(),
                "shell::RESERVED `{r}` must be in OUTSIDE_MODEL"
            );
        }
        let dev_wrappers = [
            "command",
            "exec",
            "nice",
            "timeout",
            "time",
            "env",
            "nohup",
            "builtin",
            "busybox",
            "toybox",
            "setsid",
            "unbuffer",
            "caffeinate",
            "sandbox-exec",
            "chroot",
            "ionice",
            "stdbuf",
            "xargs",
            "chrt",
            "taskset",
            "flock",
            "script",
            "arch",
        ];
        for w in dev_wrappers {
            assert!(accounted(w), "shell.rs wrapper `{w}` not accounted for");
        }

        // 4. deny.rs DOWNLOADERS & PRIV
        for &d in deny::DOWNLOADERS {
            let t = lookup(d).unwrap_or_else(|| panic!("deny::DOWNLOADERS `{d}` missing"));
            assert!(t.is_downloader, "`{d}` must set is_downloader");
        }
        for &p in deny::PRIV {
            let t = lookup(p).unwrap_or_else(|| panic!("deny::PRIV `{p}` missing"));
            assert_eq!(t.hard_deny, Some("privilege-escalation"));
        }

        // 5. engine.rs GIT_CRED_* parity
        assert_eq!(GIT_CRED_SUFFIXES, engine::GIT_CRED_SUFFIXES);
        assert_eq!(GIT_CRED_NAMES, engine::GIT_CRED_NAMES);
        assert_eq!(GIT_CRED_DIRS, engine::GIT_CRED_DIRS);

        // 6. engine.rs RG_* & GREP_* flag table parity
        let rg = lookup("rg").unwrap();
        for c in engine::RG_BOOL_S.chars() {
            let f = flag_short(rg.flags, c).unwrap_or_else(|| panic!("rg missing -{c}"));
            assert_eq!(f.kind, FlagKind::Bool, "rg -{c} must be Bool");
        }
        for c in engine::RG_VAL_S.chars() {
            let f = flag_short(rg.flags, c).unwrap_or_else(|| panic!("rg missing -{c}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "rg -{c} must take a Value"
            );
        }
        for &l in engine::RG_BOOL_L {
            let f = flag_long(rg.flags, l).unwrap_or_else(|| panic!("rg missing --{l}"));
            assert_eq!(f.kind, FlagKind::Bool, "rg --{l} must be Bool");
        }
        for &l in engine::RG_VAL_L {
            let f = flag_long(rg.flags, l).unwrap_or_else(|| panic!("rg missing --{l}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "rg --{l} must take a Value"
            );
        }

        let grep = lookup("grep").unwrap();
        for c in engine::GREP_BOOL_S.chars() {
            let f = flag_short(grep.flags, c).unwrap_or_else(|| panic!("grep missing -{c}"));
            assert_eq!(f.kind, FlagKind::Bool, "grep -{c} must be Bool");
        }
        for c in engine::GREP_VAL_S.chars() {
            let f = flag_short(grep.flags, c).unwrap_or_else(|| panic!("grep missing -{c}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "grep -{c} must take a Value"
            );
        }
        for &l in engine::GREP_BOOL_L {
            let f = flag_long(grep.flags, l).unwrap_or_else(|| panic!("grep missing --{l}"));
            assert_eq!(f.kind, FlagKind::Bool, "grep --{l} must be Bool");
        }
        for &l in engine::GREP_VAL_L {
            let f = flag_long(grep.flags, l).unwrap_or_else(|| panic!("grep missing --{l}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "grep --{l} must take a Value"
            );
        }

        // 7. gate.rs AG_OPTS & ACK_OPTS flag table parity
        for (name, opts) in [("ag", &gate::AG_OPTS), ("ack", &gate::ACK_OPTS)] {
            let tool = lookup(name).unwrap();
            for c in opts.flags.chars() {
                let f = flag_short(tool.flags, c).unwrap_or_else(|| panic!("{name} missing -{c}"));
                assert_eq!(f.kind, FlagKind::Bool, "{name} -{c} must be Bool");
            }
            for c in opts.vals.chars() {
                let f = flag_short(tool.flags, c).unwrap_or_else(|| panic!("{name} missing -{c}"));
                assert!(
                    matches!(f.kind, FlagKind::Value(_)),
                    "{name} -{c} must take Value"
                );
            }
            for c in opts.opt_vals.chars() {
                let f = flag_short(tool.flags, c).unwrap_or_else(|| panic!("{name} missing -{c}"));
                assert!(
                    matches!(f.kind, FlagKind::OptionalValue(_)),
                    "{name} -{c} must take OptionalValue"
                );
            }
            for &l in opts.long_flags {
                let f = flag_long(tool.flags, l).unwrap_or_else(|| panic!("{name} missing --{l}"));
                assert_eq!(f.kind, FlagKind::Bool, "{name} --{l} must be Bool");
            }
            for &l in opts.long_vals {
                let f = flag_long(tool.flags, l).unwrap_or_else(|| panic!("{name} missing --{l}"));
                assert!(
                    matches!(f.kind, FlagKind::Value(_)),
                    "{name} --{l} must take Value"
                );
            }
            for &l in opts.long_opt_vals {
                let f = flag_long(tool.flags, l).unwrap_or_else(|| panic!("{name} missing --{l}"));
                assert!(
                    matches!(f.kind, FlagKind::OptionalValue(_)),
                    "{name} --{l} must take OptionalValue"
                );
            }
            for c in opts.no_pattern.chars() {
                let f = flag_short(tool.flags, c).unwrap();
                assert!(
                    f.effects.provides_pattern,
                    "{name} -{c} must set provides_pattern"
                );
            }
            for &l in opts.long_no_pattern {
                let f = flag_long(tool.flags, l).unwrap();
                assert!(
                    f.effects.provides_pattern,
                    "{name} --{l} must set provides_pattern"
                );
            }
        }
    }

    #[test]
    fn changes_default_citations_reference_valid_items() {
        let check = |label: &str, s: &str| {
            let parts: Vec<&str> = s.split(';').map(str::trim).collect();
            assert!(!parts.is_empty(), "{label}: empty changes_default");
            for p in parts {
                if p == "§11 item 15 (proposed; §10 row 15)" {
                    continue;
                }
                let num_str = p
                    .strip_prefix("§11 item ")
                    .unwrap_or_else(|| panic!("{label}: invalid citation `{p}`"));
                let n: u8 = num_str
                    .parse()
                    .unwrap_or_else(|_| panic!("{label}: non-numeric item in `{p}`"));
                assert!(
                    (1..=14).contains(&n),
                    "{label}: item {n} outside 1..=14 in `{p}`"
                );
            }
        };
        for t in TOOLS {
            if let Some(cd) = t.changes_default {
                check(t.name, cd);
            }
            for s in t.subcommands {
                if let Some(cd) = s.changes_default {
                    check(&format!("{} {}", t.name, s.name), cd);
                }
            }
        }
        for e in OUTSIDE_MODEL {
            if let Some(cd) = e.changes_default {
                check(e.name, cd);
            }
        }
    }

    #[test]
    fn accessors_read_table() {
        let rg = lookup("rg").unwrap();
        assert_eq!(flag(rg.flags, "-g").unwrap().long, Some("glob"));
        assert_eq!(flag(rg.flags, "--iglob").unwrap().short, None);
        assert!(flag(rg.flags, "--").is_none());
        assert!(flag(rg.flags, "-ab").is_none());
        let git = lookup("git").unwrap();
        assert!(subcommand(git, "show").unwrap().git_selects_by_name);
        assert!(lookup_outside_model("zsh").is_some());
    }

    #[test]
    fn tool_table_markdown_matches_doc() {
        let doc_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/contracts/m1-policy-rev4-tools.md");
        let rendered = render_markdown();
        if std::env::var_os("UPDATE_TOOL_TABLE").is_some_and(|v| v == "1") {
            std::fs::write(&doc_path, &rendered)
                .unwrap_or_else(|e| panic!("failed writing {}: {e}", doc_path.display()));
            return;
        }
        let existing = std::fs::read_to_string(&doc_path).unwrap_or_else(|e| {
            panic!(
                "failed reading {} ({e}); regenerate with: env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools",
                doc_path.display()
            )
        });
        assert_eq!(
            existing, rendered,
            "docs/contracts/m1-policy-rev4-tools.md is out of date; regenerate with: env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools"
        );
    }
}
