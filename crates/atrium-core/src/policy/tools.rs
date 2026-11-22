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

use super::model::{FilterDialect, LinkPolicy, UnreadReason, ValueRole, WrapperKind};

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
    /// the unit `Unread(UnknownFlag)` (§6) and triggers conservative
    /// `AccessMode::ReadWrite` fallback on unclassified words so hard-deny /
    /// `atrium-self` write checks are never lost. Used for every row outside
    /// `HOME_READONLY_COMMANDS` as well as search tools with complete `dev`
    /// flag tables (`rg`, `grep`, `ag`, `ack`).
    ListedOnly,
    /// Home-parity mode: allowed **only** on tools and git subcommands in
    /// `HOME_READONLY_COMMANDS` that lack a complete `dev` flag table.
    ///
    /// - Any flag matching `home_readonly.forbidden` (via [`is_forbidden_flag`],
    ///   mirroring `dev`'s `engine::forbidden_flag`: short flags inside
    ///   clusters and long-option abbreviations of `>= 3` chars including `--`)
    ///   is forbidden in Home and rejected.
    /// - Any listed flag (or long abbreviation of `>= 3` chars matching a
    ///   listed writing flag via [`flag_long_or_writing_abbrev`]) applies its
    ///   exact `FlagKind` and `Effect`.
    /// - Any other unlisted flag is accepted rather than `Unread`, keeping
    ///   `dev`'s conservative treatment: every word (flag values and attached
    ///   `--opt=value` included) is a candidate `Path` checked with
    ///   `AccessMode::ReadWrite` (both read and write checks, including
    ///   Atrium's self-protection), except operands the table marks as
    ///   `Pattern`/`Text`/`Ref`/`ArchiveMember` (§11 item 9).
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
/// `cp -LP`, `ag -r -n`, `rg --glob-case-insensitive --no-glob-case-insensitive`;
/// contract §5, §6).
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

/// How a path/destination/program-file value taken by a flag is accessed
/// (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FlagValueAccess {
    /// Value path is read (e.g. `rg -f`, `grep -f`, `ag -p`, `find -f`,
    /// `sed -f`, `awk -f`, `git -C`, `git --git-dir`, `git --work-tree`).
    Read,
    /// Value path is written (e.g. `sort -o`, `tree -o`, `git diff --output`,
    /// `find -fprint`/`-fprint0`/`-fprintf`/`-fls`, `unzip -d`).
    Write,
    /// Value path is written when the unit writes and read otherwise (e.g.
    /// `tar -f` written in `-c`/`-r`/`-u`, read in `-t`/`-x`; `tar -C`).
    WriteWhenWriting,
}

/// Case-sensitivity rule for a modeled search filter flag (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FilterCase {
    /// Always case-sensitive (e.g. `grep --include`).
    AlwaysSensitive,
    /// Always case-insensitive regardless of global toggles (e.g. `rg --iglob`).
    AlwaysInsensitive,
    /// Follows the final value of the unit's global filter case-insensitivity
    /// toggle (`FlagEffects::filter_case_insensitive`, last toggle wins; e.g.
    /// `rg -g`/`--glob` governed by `--glob-case-insensitive` /
    /// `--no-glob-case-insensitive`).
    FollowGlobalToggle,
}

/// Modeled search filter specification attached to a flag (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FilterSpec {
    pub(crate) dialect: FilterDialect,
    pub(crate) case: FilterCase,
}

/// Force-push / implicit-target semantics for `git push` flags (§6,
/// `force-push-protected`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GitPushEffect {
    /// Forces non-fast-forward ref update (`-f`/`--force`,
    /// `--force-with-lease`, `--force-if-includes`, `--mirror`).
    Force,
    /// Selects implicit push targets (`--all`, `--tags`, `--branches`,
    /// `--prune`), blocking force-push unless targets are explicit.
    ImplicitTargets,
}

/// Precise `UnreadReason` triggered by a flag whose effect is intentionally
/// outside the rev 4 model (contract §3, §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FlagUnread {
    /// Unsupported filter, exclusion, ignore file, depth/size/mount limit, or
    /// filter dialect → `UnreadReason::UnsupportedFilterForm` (§5).
    FilterForm,
    /// Changes the base directory or path prefix for resolving later operands
    /// (`tar -C`/`--directory`, `git -C`/`--git-dir`/`--work-tree`/
    /// `--super-prefix`) → `UnreadReason::UnsupportedBaseDirChange` (§5, §6).
    BaseDirChange,
    /// Other real flag effect outside the model (`ag -z`/`--search-zip`,
    /// `ack -x`, `printf -v`, `grep -d`/`--directories`) →
    /// `UnreadReason::UnsupportedFlagEffect` (§3, §5, §6).
    FlagEffect,
}

impl FlagUnread {
    pub(crate) fn into_unread_reason(self, tool: &str, flag: &str) -> UnreadReason {
        match self {
            Self::FilterForm => UnreadReason::UnsupportedFilterForm,
            Self::BaseDirChange => UnreadReason::UnsupportedBaseDirChange {
                tool: tool.to_string(),
                flag: flag.to_string(),
            },
            Self::FlagEffect => UnreadReason::UnsupportedFlagEffect {
                tool: tool.to_string(),
                flag: flag.to_string(),
            },
        }
    }
}

/// Semantic effects triggered when a flag is present on a unit (contract §5, §6).
///
/// Toggle fields are applied left-to-right as flags appear in argv so a later
/// disabling flag overrides an earlier enabling flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FlagEffects {
    /// Ordered toggle for write behaviour (`Enable` / `Disable`; last wins).
    pub(crate) writes: FlagToggle,
    /// Access mode for a path/destination/program-file value taken by this
    /// flag (`None` for bool or non-path value flags).
    pub(crate) value_access: Option<FlagValueAccess>,
    /// Ordered toggle for derived destinations (`cp -r`/`-R`/`-a`, `tar -x`,
    /// `find -delete`).
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
    /// script operand (e.g. `sed -e`/`--expression`, `sed -f`/`--file`,
    /// `awk -f`/`--file`).
    pub(crate) provides_script: bool,
    /// Overrides the role of positional operands when present (last wins; e.g.
    /// `tar -c`/`-r`/`-u` → `Path`, `tar -x`/`-t` → `ArchiveMember`).
    pub(crate) positional_role: Option<ValueRole>,
    /// Modeled search filter dialect and case rule (`rg -g`/`--glob`/`--iglob`,
    /// `grep --include`).
    pub(crate) filter: Option<FilterSpec>,
    /// Global toggle for filter case-insensitivity (`rg --glob-case-insensitive`
    /// / `--no-glob-case-insensitive`; last toggle wins and applies to every
    /// `FilterCase::FollowGlobalToggle` filter on the unit).
    pub(crate) filter_case_insensitive: FlagToggle,
    /// Flag causes the unit to execute inline code (plain `Unread`; e.g.
    /// `sh -c`, `python -c`, `find -exec`, `git -c`, `git rebase -x`).
    pub(crate) runs_code: bool,
    /// Flag names a program file to execute (`Unread (program)` coverable
    /// under `pin`; e.g. `sed -f FILE`, `awk -f FILE`; §4, §7, §11 item 8).
    pub(crate) runs_program: Option<ProgramPin>,
    /// Force-push / implicit-target effect for `git push` flags.
    pub(crate) git_push: Option<GitPushEffect>,
    /// Flag has an effect outside the model, making the unit `Unread` with
    /// this precise reason category (while preserving any `value_access` or
    /// `provides_pattern` semantics for hard-deny checks).
    pub(crate) unread: Option<FlagUnread>,
}

impl FlagEffects {
    const EMPTY: Self = Self {
        writes: FlagToggle::Unchanged,
        value_access: None,
        derived_destinations: FlagToggle::Unchanged,
        recursive: FlagToggle::Unchanged,
        follows_links_at_root: FlagToggle::Unchanged,
        follows_links_below_root: FlagToggle::Unchanged,
        provides_pattern: false,
        provides_script: false,
        positional_role: None,
        filter: None,
        filter_case_insensitive: FlagToggle::Unchanged,
        runs_code: false,
        runs_program: None,
        git_push: None,
        unread: None,
    };

    /// Returns `true` iff at least one modeled effect or `unread` trigger is
    /// stated on this `FlagEffects`.
    pub(crate) const fn has_any_effect(self) -> bool {
        !matches!(self.writes, FlagToggle::Unchanged)
            || self.value_access.is_some()
            || !matches!(self.derived_destinations, FlagToggle::Unchanged)
            || !matches!(self.recursive, FlagToggle::Unchanged)
            || !matches!(self.follows_links_at_root, FlagToggle::Unchanged)
            || !matches!(self.follows_links_below_root, FlagToggle::Unchanged)
            || self.provides_pattern
            || self.provides_script
            || self.positional_role.is_some()
            || self.filter.is_some()
            || !matches!(self.filter_case_insensitive, FlagToggle::Unchanged)
            || self.runs_code
            || self.runs_program.is_some()
            || self.git_push.is_some()
            || self.unread.is_some()
    }
}

/// Explicit policy-effect declaration on every [`FlagSpec`] (contract §6).
///
/// Every flag in the table must either state at least one effect in
/// `Applied(FlagEffects)` or explicitly declare `None { reason }`; an integrity
/// test fails if any flag has neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Flag has no policy-relevant effect (path resolution, writing,
    /// recursion, symlink following, filtering, code/program execution, or
    /// base-directory changes), with a short reviewable explanation.
    None { reason: &'static str },
    /// Flag triggers one or more explicit effects in [`FlagEffects`].
    Applied(FlagEffects),
}

/// One flag accepted by a tool or subcommand (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FlagSpec {
    pub(crate) short: Option<char>,
    /// Long option name without leading `--` (or without leading `-` when
    /// `ToolSpec::single_dash_long` is `true`, as for `find` primaries).
    pub(crate) long: Option<&'static str>,
    pub(crate) kind: FlagKind,
    pub(crate) effect: Effect,
}

impl FlagSpec {
    /// Returns the flag's [`FlagEffects`] (`FlagEffects::EMPTY` when
    /// `Effect::None`).
    pub(crate) const fn effects(&self) -> FlagEffects {
        match self.effect {
            Effect::None { .. } => FlagEffects::EMPTY,
            Effect::Applied(e) => e,
        }
    }
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
    /// Rule must name the program-file flag and its value (e.g.
    /// `["sed", "-f", "edit.sed"]`, `["awk", "-f", "prog.awk"]`; contract §4,
    /// §7, §11 item 8).
    FlagValue,
}

/// How a tool/subcommand executes code (contract §4, §6, §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeRunnerBehavior {
    Never,
    /// Always runs inline/arbitrary code (plain `Unread`; never pinnable).
    InlineOnly,
    /// Runs inline code when a flag with `FlagEffects::runs_code` is given.
    InlineWithFlag,
    /// Inspects inline script operands (`ValueRole::Code`, positional `[0]` or
    /// supplied via a `provides_script` flag such as `sed -e`/`--expression`)
    /// for the listed single-letter commands (e.g. `sed`'s `e`, `w`, `r`; §4).
    /// Only scripts containing one of those commands trigger inline code
    /// (`UnreadReason::CodeRunnerInlineCode`). Flag-held program files
    /// (`sed -f FILE` with `FlagEffects::runs_program`) instead produce
    /// pinnable `UnreadReason::Program` (§7, §11 item 8).
    ScriptCommands(&'static [char]),
    /// Runs inline code when the first operand matches one of these subargs
    /// (e.g. `git bisect run`, `git submodule foreach`; §6).
    InlineWithSubarg(&'static [&'static str]),
    /// Runs inline code with a code flag (`-c`/`-e`) or inline operand OR runs
    /// a named program file (`Unread (program)` coverable under `pin`; §4, §7).
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
/// and reaches no protected path (§10 row 15; §11 item 15, D-139).
///
/// Flipping this constant to `false` restores `dev`'s Home `Ask` in one line if
/// the driver had rejected §11 item 15 (approved, D-139).
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

const fn bool_none_s(short: char, reason: &'static str) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: None,
        kind: FlagKind::Bool,
        effect: Effect::None { reason },
    }
}

const fn bool_none_l(long: &'static str, reason: &'static str) -> FlagSpec {
    FlagSpec {
        short: None,
        long: Some(long),
        kind: FlagKind::Bool,
        effect: Effect::None { reason },
    }
}

const fn bool_none_sl(short: char, long: &'static str, reason: &'static str) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: Some(long),
        kind: FlagKind::Bool,
        effect: Effect::None { reason },
    }
}

const fn bool_e(short: Option<char>, long: Option<&'static str>, effects: FlagEffects) -> FlagSpec {
    FlagSpec {
        short,
        long,
        kind: FlagKind::Bool,
        effect: Effect::Applied(effects),
    }
}

const fn bool_unread(
    short: Option<char>,
    long: Option<&'static str>,
    unread: FlagUnread,
) -> FlagSpec {
    FlagSpec {
        short,
        long,
        kind: FlagKind::Bool,
        effect: Effect::Applied(FlagEffects {
            unread: Some(unread),
            ..FlagEffects::EMPTY
        }),
    }
}

const fn val_none_s(short: char, role: ValueRole, reason: &'static str) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: None,
        kind: FlagKind::Value(role),
        effect: Effect::None { reason },
    }
}

const fn val_none_l(long: &'static str, role: ValueRole, reason: &'static str) -> FlagSpec {
    FlagSpec {
        short: None,
        long: Some(long),
        kind: FlagKind::Value(role),
        effect: Effect::None { reason },
    }
}

const fn val_none_sl(
    short: char,
    long: &'static str,
    role: ValueRole,
    reason: &'static str,
) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: Some(long),
        kind: FlagKind::Value(role),
        effect: Effect::None { reason },
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
        effect: Effect::Applied(effects),
    }
}

const fn val_unread(
    short: Option<char>,
    long: Option<&'static str>,
    role: ValueRole,
    unread: FlagUnread,
) -> FlagSpec {
    FlagSpec {
        short,
        long,
        kind: FlagKind::Value(role),
        effect: Effect::Applied(FlagEffects {
            unread: Some(unread),
            ..FlagEffects::EMPTY
        }),
    }
}

const fn opt_val_none_sl(
    short: char,
    long: &'static str,
    role: ValueRole,
    reason: &'static str,
) -> FlagSpec {
    FlagSpec {
        short: Some(short),
        long: Some(long),
        kind: FlagKind::OptionalValue(role),
        effect: Effect::None { reason },
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
        flag_mode: FlagMode::ListedOnly,
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
        flag_mode: FlagMode::ListedOnly,
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

const fn target_then_code_runner(name: &'static str) -> ToolSpec {
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::Text], ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineOnly,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 9"),
        ..base_tool(name)
    }
}

const fn direct_code_runner(name: &'static str) -> ToolSpec {
    ToolSpec {
        operands: OperandLayout::all(ValueRole::Code),
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
    bool_none_sl('i', "ignore-case", "content match case-insensitivity only"),
    bool_none_sl('S', "smart-case", "content match smart-case only"),
    bool_none_sl('s', "case-sensitive", "content match case-sensitivity only"),
    bool_none_sl('n', "line-number", "output formatting: print line numbers"),
    bool_none_sl(
        'N',
        "no-line-number",
        "output formatting: omit line numbers",
    ),
    bool_none_sl('H', "with-filename", "output formatting: print filename"),
    bool_none_sl('I', "no-filename", "output formatting: omit filename"),
    bool_none_sl(
        'l',
        "files-with-matches",
        "output formatting: print matching filenames only",
    ),
    bool_none_sl('c', "count", "output formatting: print match count"),
    bool_none_sl(
        'o',
        "only-matching",
        "output formatting: print matched text only",
    ),
    bool_none_sl('v', "invert-match", "content match inversion only"),
    bool_none_sl('w', "word-regexp", "content match word boundaries only"),
    bool_none_sl('x', "line-regexp", "content match whole lines only"),
    bool_none_sl(
        'u',
        "unrestricted",
        "disables default ignore/hidden skipping; model checks all entries",
    ),
    bool_none_sl('a', "text", "treat binary files as text during match"),
    bool_none_sl('U', "multiline", "multiline content match only"),
    bool_none_sl('F', "fixed-strings", "literal content match only"),
    bool_none_sl('P', "pcre2", "PCRE2 content regex engine only"),
    bool_none_sl('q', "quiet", "suppress output"),
    bool_none_sl('0', "null", "output formatting: NUL filename separator"),
    bool_none_sl('p', "pretty", "output formatting: pretty display"),
    bool_none_sl('b', "byte-offset", "output formatting: print byte offset"),
    bool_e(
        Some('L'),
        Some("follow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    // Long-only booleans
    bool_none_l(
        "hidden",
        "searches dotfiles; model already checks all entries",
    ),
    bool_none_l(
        "no-ignore",
        "disables ignore files; model already checks all entries",
    ),
    bool_none_l(
        "no-ignore-vcs",
        "disables VCS ignore files; model already checks all entries",
    ),
    bool_none_l(
        "no-ignore-dot",
        "disables .ignore files; model already checks all entries",
    ),
    bool_none_l(
        "no-ignore-parent",
        "disables parent ignore files; model already checks all entries",
    ),
    bool_none_l(
        "no-ignore-global",
        "disables global ignore files; model already checks all entries",
    ),
    bool_none_l(
        "no-ignore-files",
        "disables --ignore-file entries; model already checks all entries",
    ),
    bool_none_l("no-messages", "suppress error messages"),
    bool_none_l("no-heading", "output formatting: omit file headings"),
    bool_none_l("heading", "output formatting: print file headings"),
    bool_none_l(
        "count-matches",
        "output formatting: print total match count",
    ),
    bool_none_l(
        "files-without-match",
        "output formatting: print non-matching filenames",
    ),
    bool_none_l("multiline-dotall", "multiline regex dotall mode only"),
    bool_none_l("binary", "search binary files for content matches"),
    bool_e(
        None,
        Some("files"),
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("type-list"),
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_l("json", "output formatting: JSON lines"),
    bool_none_l("column", "output formatting: print column number"),
    bool_none_l("no-column", "output formatting: omit column number"),
    bool_none_l("vimgrep", "output formatting: vimgrep format"),
    bool_none_l("trim", "output formatting: trim leading whitespace"),
    bool_none_l("stats", "output formatting: print search statistics"),
    bool_none_l("no-config", "ignore ripgrep config file"),
    bool_none_l("version", "print version and exit"),
    bool_none_l("help", "print help and exit"),
    bool_none_l("no-pcre2", "disable PCRE2 content regex engine"),
    bool_none_l("crlf", "CRLF line terminator handling only"),
    bool_none_l("include-zero", "output formatting: include zero counts"),
    bool_e(
        None,
        Some("glob-case-insensitive"),
        FlagEffects {
            filter_case_insensitive: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("no-glob-case-insensitive"),
        FlagEffects {
            filter_case_insensitive: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    // Paired short + long value flags
    val_e(
        Some('e'),
        Some("regexp"),
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('g'),
        Some("glob"),
        ValueRole::Pattern,
        FlagEffects {
            filter: Some(FilterSpec {
                dialect: FilterDialect::RgGlob,
                case: FilterCase::FollowGlobalToggle,
            }),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("iglob"),
        ValueRole::Pattern,
        FlagEffects {
            filter: Some(FilterSpec {
                dialect: FilterDialect::RgGlob,
                case: FilterCase::AlwaysInsensitive,
            }),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('f'),
        Some("file"),
        ValueRole::Path,
        FlagEffects {
            value_access: Some(FlagValueAccess::Read),
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    // Unsupported file-type / depth / size filters (§5: other dialects / narrowing)
    val_unread(
        Some('t'),
        Some("type"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_unread(
        Some('T'),
        Some("type-not"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_none_sl(
        'm',
        "max-count",
        ValueRole::Text,
        "limits matches printed per file",
    ),
    val_none_sl(
        'A',
        "after-context",
        ValueRole::Text,
        "prints trailing context lines",
    ),
    val_none_sl(
        'B',
        "before-context",
        ValueRole::Text,
        "prints leading context lines",
    ),
    val_none_sl(
        'C',
        "context",
        ValueRole::Text,
        "prints surrounding context lines",
    ),
    val_unread(
        Some('d'),
        Some("max-depth"),
        ValueRole::Text,
        FlagUnread::FilterForm,
    ),
    val_unread(
        None,
        Some("maxdepth"),
        ValueRole::Text,
        FlagUnread::FilterForm,
    ),
    val_none_sl('j', "threads", ValueRole::Text, "sets worker thread count"),
    val_none_sl(
        'M',
        "max-columns",
        ValueRole::Text,
        "truncates long output lines",
    ),
    val_none_sl(
        'E',
        "encoding",
        ValueRole::Text,
        "selects text decoding for content match",
    ),
    val_none_sl(
        'r',
        "replace",
        ValueRole::Text,
        "formats matched output text",
    ),
    // Long-only value flags
    val_none_l("colors", ValueRole::Text, "output color specification"),
    val_none_l("color", ValueRole::Text, "output color mode"),
    val_none_l("sort", ValueRole::Text, "sorts output order"),
    val_none_l("sortr", ValueRole::Text, "reverse-sorts output order"),
    val_unread(
        None,
        Some("max-filesize"),
        ValueRole::Text,
        FlagUnread::FilterForm,
    ),
    val_none_l(
        "path-separator",
        ValueRole::Text,
        "output path display separator only",
    ),
    val_none_l("engine", ValueRole::Text, "selects regex engine"),
    val_none_l(
        "context-separator",
        ValueRole::Text,
        "output context separator string",
    ),
    val_unread(
        None,
        Some("type-add"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_unread(
        None,
        Some("type-clear"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_none_l(
        "field-context-separator",
        ValueRole::Text,
        "output field separator string",
    ),
    val_none_l(
        "field-match-separator",
        ValueRole::Text,
        "output field separator string",
    ),
    val_none_l("dfa-size-limit", ValueRole::Text, "regex DFA memory limit"),
    val_none_l(
        "regex-size-limit",
        ValueRole::Text,
        "regex compilation memory limit",
    ),
];

/// `grep` / `egrep` / `fgrep` / `zgrep` / `ggrep` / `rgrep` flags copied from
/// `engine::GREP_BOOL_S`, `GREP_VAL_S`, `GREP_BOOL_L`, `GREP_VAL_L`, plus
/// `-f`/`--file` from `gate.rs`.
const GREP_FLAGS: &[FlagSpec] = &[
    // Paired short + long booleans
    bool_none_sl('i', "ignore-case", "content match case-insensitivity only"),
    bool_none_l("no-ignore-case", "content match case-sensitivity only"),
    bool_none_sl('v', "invert-match", "content match inversion only"),
    bool_none_sl('n', "line-number", "output formatting: print line numbers"),
    bool_none_sl('c', "count", "output formatting: print match count"),
    bool_none_sl(
        'l',
        "files-with-matches",
        "output formatting: print matching filenames only",
    ),
    bool_none_sl(
        'L',
        "files-without-match",
        "output formatting: print non-matching filenames",
    ),
    bool_none_sl(
        'o',
        "only-matching",
        "output formatting: print matched text only",
    ),
    bool_none_sl('q', "quiet", "suppress output"),
    bool_none_l("silent", "suppress output"),
    bool_none_sl('s', "no-messages", "suppress error messages"),
    bool_none_sl('w', "word-regexp", "content match word boundaries only"),
    bool_none_sl('x', "line-regexp", "content match whole lines only"),
    bool_none_sl('h', "no-filename", "output formatting: omit filename"),
    bool_none_sl('H', "with-filename", "output formatting: print filename"),
    bool_none_sl('E', "extended-regexp", "ERE content regex dialect only"),
    bool_none_sl('F', "fixed-strings", "literal content match only"),
    bool_none_sl('G', "basic-regexp", "BRE content regex dialect only"),
    bool_none_sl('P', "perl-regexp", "PCRE content regex dialect only"),
    bool_none_sl('a', "text", "treat binary files as text during match"),
    bool_none_sl('z', "null-data", "NUL-delimited input lines only"),
    bool_none_sl('Z', "null", "output formatting: NUL filename separator"),
    bool_none_sl('b', "byte-offset", "output formatting: print byte offset"),
    bool_none_sl('T', "initial-tab", "output formatting: align tab"),
    bool_e(
        Some('r'),
        Some("recursive"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('R'),
        Some("dereference-recursive"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_l(
        "no-group-separator",
        "output formatting: omit context group separator",
    ),
    // Short-only booleans from GREP_BOOL_S
    bool_none_s('I', "skip binary files in content match only"),
    bool_none_s('U', "binary CR handling on MS-DOS/Windows only"),
    bool_none_s('y', "obsolete alias for -i ignore-case"),
    bool_none_s('0', "digit context-line count shorthand"),
    bool_none_s('1', "digit context-line count shorthand"),
    bool_none_s('2', "digit context-line count shorthand"),
    bool_none_s('3', "digit context-line count shorthand"),
    bool_none_s('4', "digit context-line count shorthand"),
    bool_none_s('5', "digit context-line count shorthand"),
    bool_none_s('6', "digit context-line count shorthand"),
    bool_none_s('7', "digit context-line count shorthand"),
    bool_none_s('8', "digit context-line count shorthand"),
    bool_none_s('9', "digit context-line count shorthand"),
    // Value flags
    val_e(
        Some('e'),
        Some("regexp"),
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('f'),
        Some("file"),
        ValueRole::Path,
        FlagEffects {
            value_access: Some(FlagValueAccess::Read),
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_none_sl(
        'm',
        "max-count",
        ValueRole::Text,
        "limits matches printed per file",
    ),
    val_none_sl(
        'A',
        "after-context",
        ValueRole::Text,
        "prints trailing context lines",
    ),
    val_none_sl(
        'B',
        "before-context",
        ValueRole::Text,
        "prints leading context lines",
    ),
    val_none_sl(
        'C',
        "context",
        ValueRole::Text,
        "prints surrounding context lines",
    ),
    val_e(
        Some('d'),
        Some("directories"),
        ValueRole::Text,
        FlagEffects {
            recursive: FlagToggle::Enable,
            unread: Some(FlagUnread::FlagEffect),
            ..FlagEffects::EMPTY
        },
    ),
    val_none_sl(
        'D',
        "devices",
        ValueRole::Text,
        "device/FIFO handling only; does not change file or directory selection",
    ),
    val_e(
        None,
        Some("include"),
        ValueRole::Pattern,
        FlagEffects {
            filter: Some(FilterSpec {
                dialect: FilterDialect::GrepInclude,
                case: FilterCase::AlwaysSensitive,
            }),
            ..FlagEffects::EMPTY
        },
    ),
    // Unsupported grep filters (§5: negated / directory filters → Unread)
    val_unread(
        None,
        Some("exclude"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_unread(
        None,
        Some("exclude-dir"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_unread(
        None,
        Some("include-dir"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_none_l("label", ValueRole::Text, "stdin display label in output"),
    val_none_l("color", ValueRole::Text, "output color mode"),
    val_none_l("colour", ValueRole::Text, "output color mode"),
    val_none_l(
        "binary-files",
        ValueRole::Text,
        "binary file content match mode only",
    ),
    val_none_l(
        "group-separator",
        ValueRole::Text,
        "output context group separator string",
    ),
];

/// `ag` flags copied faithfully from `gate::AG_OPTS`.
const AG_FLAGS: &[FlagSpec] = &[
    // Paired short + long booleans
    bool_none_sl(
        'a',
        "all-types",
        "disables default type skipping; model already checks all entries",
    ),
    bool_none_sl('D', "debug", "prints debug diagnostics only"),
    bool_none_sl('c', "count", "output formatting: print match count"),
    bool_none_sl('F', "fixed-strings", "literal content match only"),
    bool_e(
        Some('f'),
        Some("follow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_sl('H', "heading", "output formatting: print file headings"),
    bool_none_s('h', "print help and exit"),
    bool_none_sl('i', "ignore-case", "content match case-insensitivity only"),
    bool_none_sl(
        'L',
        "files-without-matches",
        "output formatting: print non-matching filenames",
    ),
    bool_none_sl(
        'l',
        "files-with-matches",
        "output formatting: print matching filenames only",
    ),
    bool_e(
        Some('n'),
        Some("norecurse"),
        FlagEffects {
            recursive: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_sl(
        'o',
        "only-matching",
        "output formatting: print matched text only",
    ),
    bool_none_sl('Q', "literal", "literal content match only"),
    bool_e(
        Some('R'),
        Some("recurse"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('r'),
        None,
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_sl('S', "smart-case", "content match smart-case only"),
    bool_none_sl('s', "case-sensitive", "content match case-sensitivity only"),
    bool_none_sl('v', "invert-match", "content match inversion only"),
    bool_none_s('V', "print version and exit"),
    bool_none_sl(
        't',
        "all-text",
        "disables ignore skipping for text files; model checks all entries",
    ),
    bool_none_sl(
        'u',
        "unrestricted",
        "disables ignore/hidden skipping; model checks all entries",
    ),
    bool_none_sl(
        'U',
        "skip-vcs-ignores",
        "disables VCS ignore files; model checks all entries",
    ),
    bool_none_sl('w', "word-regexp", "content match word boundaries only"),
    bool_unread(Some('z'), Some("search-zip"), FlagUnread::FlagEffect),
    bool_none_sl('0', "null", "output formatting: NUL filename separator"),
    // Long-only booleans
    bool_none_l("ackmate", "output formatting: AckMate format"),
    bool_none_l("affinity", "CPU thread affinity setting only"),
    bool_none_l("noaffinity", "disables CPU thread affinity only"),
    bool_none_l(
        "hidden",
        "searches dotfiles; model already checks all entries",
    ),
    bool_none_l("column", "output formatting: print column number"),
    bool_none_l("nocolor", "output formatting: disable color"),
    bool_none_l("color", "output formatting: enable color"),
    bool_none_l(
        "color-win-ansi",
        "output formatting: ANSI colors on Windows",
    ),
    bool_none_l("noheading", "output formatting: omit file headings"),
    bool_none_l(
        "nobreak",
        "output formatting: omit blank lines between files",
    ),
    bool_none_l(
        "break",
        "output formatting: print blank lines between files",
    ),
    bool_e(
        None,
        Some("nofollow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_l(
        "search-binary",
        "search binary files; model already checks all entries",
    ),
    bool_none_l("silent", "suppress error messages"),
    bool_none_l("stats", "output formatting: print search statistics"),
    bool_none_l(
        "stats-only",
        "output formatting: print search statistics only",
    ),
    bool_none_l("vimgrep", "output formatting: vimgrep format"),
    bool_none_l("print0", "output formatting: NUL filename separator"),
    bool_unread(None, Some("one-device"), FlagUnread::FilterForm),
    bool_none_l("filename", "output formatting: print filename"),
    bool_none_l("nofilename", "output formatting: omit filename"),
    bool_none_l("numbers", "output formatting: print line numbers"),
    bool_none_l("nonumbers", "output formatting: omit line numbers"),
    bool_none_l("line-numbers", "output formatting: print line numbers"),
    bool_none_l("group", "output formatting: group matches by file"),
    bool_none_l("nogroup", "output formatting: do not group matches by file"),
    bool_none_l("match", "disables filename-only search mode"),
    bool_none_l("mmap", "use mmap for file reading only"),
    bool_none_l("nommap", "disable mmap for file reading only"),
    bool_none_l("multiline", "multiline content match only"),
    bool_none_l("nomultiline", "disable multiline content match"),
    bool_none_l("nopager", "do not pipe output to pager"),
    bool_none_l("parallel", "enable parallel search threads only"),
    bool_none_l("passthrough", "output formatting: print all lines of file"),
    bool_none_l("passthru", "output formatting: print all lines of file"),
    bool_none_l(
        "print-all-files",
        "output formatting: print all file headers",
    ),
    bool_none_l(
        "print-long-lines",
        "output formatting: do not truncate lines",
    ),
    // Value flags (unsupported filter / ignore options → Unread)
    val_unread(
        Some('G'),
        Some("file-search-regex"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_e(
        Some('g'),
        Some("filename-pattern"),
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            unread: Some(FlagUnread::FilterForm),
            ..FlagEffects::EMPTY
        },
    ),
    val_none_sl(
        'm',
        "max-count",
        ValueRole::Text,
        "limits matches printed per file",
    ),
    val_e(
        Some('p'),
        Some("path-to-ignore"),
        ValueRole::Path,
        FlagEffects {
            value_access: Some(FlagValueAccess::Read),
            unread: Some(FlagUnread::FilterForm),
            ..FlagEffects::EMPTY
        },
    ),
    val_none_sl(
        'W',
        "width",
        ValueRole::Text,
        "output formatting: truncate lines to width",
    ),
    val_unread(
        None,
        Some("ackmate-dir-filter"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_unread(None, Some("depth"), ValueRole::Text, FlagUnread::FilterForm),
    val_unread(
        None,
        Some("ignore"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_unread(
        None,
        Some("ignore-dir"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_e(
        None,
        Some("pager"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_none_l("workers", ValueRole::Text, "sets worker thread count"),
    val_none_l(
        "color-line-number",
        ValueRole::Text,
        "output color for line numbers",
    ),
    val_none_l("color-match", ValueRole::Text, "output color for matches"),
    val_none_l("color-path", ValueRole::Text, "output color for paths"),
    // Optional-value flags
    opt_val_none_sl(
        'A',
        "after",
        ValueRole::Text,
        "prints trailing context lines",
    ),
    opt_val_none_sl(
        'B',
        "before",
        ValueRole::Text,
        "prints leading context lines",
    ),
    opt_val_none_sl(
        'C',
        "context",
        ValueRole::Text,
        "prints surrounding context lines",
    ),
];

/// `ack` flags copied faithfully from `gate::ACK_OPTS`.
const ACK_FLAGS: &[FlagSpec] = &[
    // Paired short + long booleans
    bool_none_sl('i', "ignore-case", "content match case-insensitivity only"),
    bool_none_s('I', "content match case-sensitivity only"),
    bool_none_sl('S', "smart-case", "content match smart-case only"),
    bool_none_sl('v', "invert-match", "content match inversion only"),
    bool_none_sl('w', "word-regexp", "content match word boundaries only"),
    bool_none_sl('Q', "literal", "literal content match only"),
    bool_none_sl(
        'l',
        "files-with-matches",
        "output formatting: print matching filenames only",
    ),
    bool_none_sl(
        'L',
        "files-without-matches",
        "output formatting: print non-matching filenames",
    ),
    bool_none_sl('c', "count", "output formatting: print match count"),
    bool_none_sl('h', "no-filename", "output formatting: omit filename"),
    bool_none_sl('H', "with-filename", "output formatting: print filename"),
    bool_none_s('o', "output formatting: print matched text only"),
    bool_none_s('1', "stop after first match"),
    bool_e(
        Some('n'),
        Some("no-recurse"),
        FlagEffects {
            recursive: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('r'),
        Some("recurse"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('R'),
        None,
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_s('s', "suppress error messages"),
    bool_unread(Some('x'), None, FlagUnread::FlagEffect),
    bool_e(
        Some('f'),
        None,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    bool_unread(Some('k'), None, FlagUnread::FilterForm),
    // Long-only booleans
    bool_none_l("column", "output formatting: print column number"),
    bool_none_l("nocolor", "output formatting: disable color"),
    bool_none_l("color", "output formatting: enable color"),
    bool_none_l("noheading", "output formatting: omit file headings"),
    bool_none_l("heading", "output formatting: print file headings"),
    bool_none_l(
        "nobreak",
        "output formatting: omit blank lines between files",
    ),
    bool_none_l(
        "break",
        "output formatting: print blank lines between files",
    ),
    bool_e(
        None,
        Some("follow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("nofollow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_l("sort-files", "sorts output file order only"),
    bool_none_l("nopager", "do not pipe output to pager"),
    // Value flags
    val_none_sl(
        'A',
        "after-context",
        ValueRole::Text,
        "prints trailing context lines",
    ),
    val_none_sl(
        'B',
        "before-context",
        ValueRole::Text,
        "prints leading context lines",
    ),
    val_none_sl(
        'm',
        "max-count",
        ValueRole::Text,
        "limits matches printed per file",
    ),
    val_e(
        Some('g'),
        None,
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            unread: Some(FlagUnread::FilterForm),
            ..FlagEffects::EMPTY
        },
    ),
    val_unread(
        Some('t'),
        Some("type"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_unread(
        None,
        Some("ignore-dir"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_unread(
        None,
        Some("noignore-dir"),
        ValueRole::Pattern,
        FlagUnread::FilterForm,
    ),
    val_e(
        None,
        Some("match"),
        ValueRole::Pattern,
        FlagEffects {
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("output"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    // Optional-value flags
    opt_val_none_sl(
        'C',
        "context",
        ValueRole::Text,
        "prints surrounding context lines",
    ),
];

/// Modeled nested-shell flags (§4: `-e`, `-u`, `-x`, `-c SCRIPT`).
const MODELED_SHELL_FLAGS: &[FlagSpec] = &[
    bool_none_s('e', "shell errexit option; allowed before -c (§4)"),
    bool_none_s('u', "shell nounset option; allowed before -c (§4)"),
    bool_none_s('x', "shell xtrace option; allowed before -c (§4)"),
    val_e(
        Some('c'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
];

/// `nice` flags (§4: `nice`, `nice -n N`).
const NICE_FLAGS: &[FlagSpec] = &[val_none_s(
    'n',
    ValueRole::Text,
    "scheduling priority increment; allowed wrapper option (§4)",
)];

/// `timeout` flags (§4: `timeout [-s SIG] [-k D] D`).
const TIMEOUT_FLAGS: &[FlagSpec] = &[
    val_none_s(
        's',
        ValueRole::Text,
        "termination signal name/number; allowed wrapper option (§4)",
    ),
    val_none_s(
        'k',
        ValueRole::Text,
        "kill-after duration; allowed wrapper option (§4)",
    ),
];

/// `ln` flags (§6: `ln` and `ln -s`).
const LN_FLAGS: &[FlagSpec] = &[bool_none_s(
    's',
    "creates symbolic link instead of hard link; target stays Read and link stays Write (§6)",
)];

/// `cp` flags known to `dev` (`gate.rs`: `-r`, `-R`, `-a`, `--recursive`,
/// `--archive`, `-L`, `--dereference`, `-P`, `-d`).
const CP_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('r'),
        None,
        FlagEffects {
            recursive: FlagToggle::Enable,
            derived_destinations: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('R'),
        Some("recursive"),
        FlagEffects {
            recursive: FlagToggle::Enable,
            derived_destinations: FlagToggle::Enable,
            ..FlagEffects::EMPTY
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
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('L'),
        Some("dereference"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('P'),
        None,
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('d'),
        None,
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::EMPTY
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
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('k'),
        Some("copy-dirlinks"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("copy-unsafe-links"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
];

/// `tar` flags distinguishing creation/update source paths (`-c`/`-r`/`-u`)
/// from extraction/listing archive member selectors (`-x`/`-t`), plus `-h`/
/// `--dereference` (`gate.rs`), `-f`/`--file` (archive path), and `-C`/
/// `--directory` (base-directory change for later operands → `Unread`).
///
/// For old-style bundled key arguments (`tar chf out.tar proj/key`), P3 reads
/// the same operation letters (`c`/`r`/`u` → `Path`, `x`/`t` → `ArchiveMember`,
/// `h` → follow links, `f` → next operand is archive `Path`).
const TAR_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('c'),
        Some("create"),
        FlagEffects {
            writes: FlagToggle::Enable,
            positional_role: Some(ValueRole::Path),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('r'),
        Some("append"),
        FlagEffects {
            writes: FlagToggle::Enable,
            positional_role: Some(ValueRole::Path),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('u'),
        Some("update"),
        FlagEffects {
            writes: FlagToggle::Enable,
            positional_role: Some(ValueRole::Path),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('x'),
        Some("extract"),
        FlagEffects {
            writes: FlagToggle::Enable,
            derived_destinations: FlagToggle::Enable,
            positional_role: Some(ValueRole::ArchiveMember),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('t'),
        Some("list"),
        FlagEffects {
            positional_role: Some(ValueRole::ArchiveMember),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('h'),
        Some("dereference"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('f'),
        Some("file"),
        ValueRole::Path,
        FlagEffects {
            value_access: Some(FlagValueAccess::WriteWhenWriting),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('C'),
        Some("directory"),
        ValueRole::DestinationRoot,
        FlagEffects {
            value_access: Some(FlagValueAccess::WriteWhenWriting),
            unread: Some(FlagUnread::BaseDirChange),
            ..FlagEffects::EMPTY
        },
    ),
];

/// `unzip` destination-directory flag (`-d DIR` writes to destination root).
const UNZIP_FLAGS: &[FlagSpec] = &[val_e(
    Some('d'),
    None,
    ValueRole::DestinationRoot,
    FlagEffects {
        writes: FlagToggle::Enable,
        value_access: Some(FlagValueAccess::Write),
        ..FlagEffects::EMPTY
    },
)];

/// `cd` flags from `words.rs` (`-L`, `-P`, `-e`, `-@`).
const CD_FLAGS: &[FlagSpec] = &[
    bool_none_s(
        'L',
        "logical symlink handling for cd; target resolved by P2 (§5)",
    ),
    bool_none_s(
        'P',
        "physical symlink handling for cd; target resolved by P2 (§5)",
    ),
    bool_none_s('e', "exit non-zero if physical cwd cannot be determined"),
    bool_none_s(
        '@',
        "extended-attribute directory flag accepted by dev's cd parser",
    ),
];

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
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('L'),
        Some("follow"),
        FlagEffects {
            follows_links_at_root: FlagToggle::Enable,
            follows_links_below_root: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('P'),
        None,
        FlagEffects {
            follows_links_at_root: FlagToggle::Disable,
            follows_links_below_root: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_s('E', "extended regex syntax for -regex predicate only"),
    bool_none_s('X', "xargs-safe path warning mode only"),
    bool_none_s(
        'd',
        "BSD depth-first traversal order only (same files visited)",
    ),
    bool_none_s(
        's',
        "BSD lexicographical traversal order only (same files visited)",
    ),
    bool_unread(Some('x'), None, FlagUnread::FilterForm),
    val_none_s('D', ValueRole::Text, "GNU find debug output category only"),
    val_e(
        Some('f'),
        None,
        ValueRole::Path,
        FlagEffects {
            value_access: Some(FlagValueAccess::Read),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("delete"),
        FlagEffects {
            writes: FlagToggle::Enable,
            derived_destinations: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_l(
        "depth",
        "depth-first traversal order only (same files visited)",
    ),
    val_unread(
        None,
        Some("maxdepth"),
        ValueRole::Text,
        FlagUnread::FilterForm,
    ),
    val_unread(
        None,
        Some("mindepth"),
        ValueRole::Text,
        FlagUnread::FilterForm,
    ),
    val_e(
        None,
        Some("exec"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("execdir"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("ok"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("okdir"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("fprint"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            value_access: Some(FlagValueAccess::Write),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("fprint0"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            value_access: Some(FlagValueAccess::Write),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("fprintf"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            value_access: Some(FlagValueAccess::Write),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("fls"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            value_access: Some(FlagValueAccess::Write),
            ..FlagEffects::EMPTY
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
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('R'),
        None,
        FlagEffects {
            recursive: FlagToggle::Enable,
            ..FlagEffects::EMPTY
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
        value_access: Some(FlagValueAccess::Write),
        ..FlagEffects::EMPTY
    },
)];

/// `echo` flags known to `dev` (`shell.rs`).
const ECHO_FLAGS: &[FlagSpec] = &[
    bool_none_s('n', "omits trailing newline in stdout output only"),
    bool_none_s('e', "interprets backslash escapes in stdout output only"),
    bool_none_s(
        'E',
        "disables backslash escape interpretation in stdout output only",
    ),
];

/// `printf` flags known to `dev` (`shell.rs`).
///
/// `-v VAR` assigns a shell variable in bash (including `HOME`), outside §3's
/// assignment model → `Unread(UnsupportedFlagEffect)`.
const PRINTF_FLAGS: &[FlagSpec] = &[val_unread(
    Some('v'),
    None,
    ValueRole::Text,
    FlagUnread::FlagEffect,
)];

/// `codesign` flags known to `dev` (`words.rs`).
const CODESIGN_FLAGS: &[FlagSpec] = &[
    val_e(
        Some('s'),
        Some("sign"),
        ValueRole::Text,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('f'),
        Some("force"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("remove-signature"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_sl(
        'd',
        "display",
        "displays code signature metadata (read-only)",
    ),
    bool_none_sl('v', "verify", "verifies code signature (read-only)"),
];

/// `sed` flags (`words.rs`, contract §4, §7): `-i`/`--in-place` writes input
/// files, `-e`/`--expression` supplies an inline script inspected for
/// `e`/`w`/`r` (omitting positional `[0]` script), and `-f`/`--file` names a
/// pinnable program file (`Unread (program)` coverable under
/// `ProgramPin::FlagValue`; §11 item 8).
const SED_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('i'),
        Some("in-place"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('e'),
        Some("expression"),
        ValueRole::Code,
        FlagEffects {
            provides_script: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('f'),
        Some("file"),
        ValueRole::ProgramFile,
        FlagEffects {
            writes: FlagToggle::Enable,
            value_access: Some(FlagValueAccess::Read),
            provides_script: true,
            runs_program: Some(ProgramPin::FlagValue),
            ..FlagEffects::EMPTY
        },
    ),
];

/// `awk` / `gawk` / `mawk` flags (`words.rs`, contract §4, §7): `-f`/`--file`
/// names a pinnable program file (`Unread (program)` coverable under
/// `ProgramPin::FlagValue`; §11 item 8), while `-i`/`--include` loads an
/// extension/library (`inline` code runner).
const AWK_FLAGS: &[FlagSpec] = &[
    val_e(
        Some('f'),
        Some("file"),
        ValueRole::ProgramFile,
        FlagEffects {
            writes: FlagToggle::Enable,
            value_access: Some(FlagValueAccess::Read),
            provides_script: true,
            runs_program: Some(ProgramPin::FlagValue),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('i'),
        Some("include"),
        ValueRole::Code,
        FlagEffects {
            writes: FlagToggle::Enable,
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
];

/// `perl` flags known to `dev` (`words.rs`, `deny.rs`).
const PERL_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('i'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('e'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('E'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
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
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('m'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
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
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('p'),
        Some("print"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
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
        ..FlagEffects::EMPTY
    },
)];

// ---------------------------------------------------------------------------
// `git`, `cargo`, and `pipx` subcommand tables (§5, §6, §7)
// ---------------------------------------------------------------------------

/// `git` top-level options known to `dev` (`deny.rs`, `HOME_READONLY_COMMANDS`)
/// and contract §6.
///
/// Base-directory / repository-root changing flags (`-C`, `--git-dir`,
/// `--work-tree`, `--super-prefix`) make the unit `Unread` with
/// `UnsupportedBaseDirChange` while still reading their path values for
/// hard-deny checks.
const GIT_GLOBAL_FLAGS: &[FlagSpec] = &[
    val_e(
        Some('C'),
        None,
        ValueRole::Path,
        FlagEffects {
            value_access: Some(FlagValueAccess::Read),
            unread: Some(FlagUnread::BaseDirChange),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('c'),
        None,
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("config-env"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("exec-path"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("git-dir"),
        ValueRole::Path,
        FlagEffects {
            value_access: Some(FlagValueAccess::Read),
            unread: Some(FlagUnread::BaseDirChange),
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("work-tree"),
        ValueRole::Path,
        FlagEffects {
            value_access: Some(FlagValueAccess::Read),
            unread: Some(FlagUnread::BaseDirChange),
            ..FlagEffects::EMPTY
        },
    ),
    val_none_l(
        "namespace",
        ValueRole::Text,
        "selects git ref namespace only; does not change host filesystem paths",
    ),
    val_unread(
        None,
        Some("super-prefix"),
        ValueRole::Text,
        FlagUnread::BaseDirChange,
    ),
];

const GIT_LOG_FLAGS: &[FlagSpec] = &[
    bool_none_s(
        'p',
        "shows patch diffs in git log output; CR-12 git selection still checks operands",
    ),
    val_none_sl(
        'L',
        "line-range",
        ValueRole::Text,
        "selects line range in git log; file part checked by CR-12 git selection (§6)",
    ),
];

const GIT_DIFF_FLAGS: &[FlagSpec] = &[
    val_e(
        None,
        Some("output"),
        ValueRole::Path,
        FlagEffects {
            writes: FlagToggle::Enable,
            value_access: Some(FlagValueAccess::Write),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("ext-diff"),
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
];

const GIT_BRANCH_FLAGS: &[FlagSpec] = &[
    bool_none_sl(
        'a',
        "all",
        "lists both local and remote-tracking branches (read-only)",
    ),
    bool_none_sl('r', "remotes", "lists remote-tracking branches (read-only)"),
    bool_none_sl(
        'v',
        "verbose",
        "prints commit hash and subject in branch listing (read-only)",
    ),
    bool_e(
        Some('l'),
        Some("list"),
        FlagEffects {
            writes: FlagToggle::Disable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_none_l(
        "show-current",
        "prints current branch name only (read-only)",
    ),
    bool_e(
        Some('d'),
        Some("delete"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('D'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('m'),
        Some("move"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('M'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('c'),
        Some("copy"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        Some('C'),
        None,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('u'),
        Some("set-upstream-to"),
        ValueRole::Ref,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("unset-upstream"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("edit-description"),
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
];

const GIT_CHECKOUT_FLAGS: &[FlagSpec] = &[
    val_e(
        Some('b'),
        None,
        ValueRole::Ref,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        Some('B'),
        None,
        ValueRole::Ref,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("orphan"),
        ValueRole::Ref,
        FlagEffects {
            writes: FlagToggle::Enable,
            ..FlagEffects::EMPTY
        },
    ),
];

const GIT_PUSH_FLAGS: &[FlagSpec] = &[
    bool_e(
        Some('f'),
        Some("force"),
        FlagEffects {
            git_push: Some(GitPushEffect::Force),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("force-with-lease"),
        FlagEffects {
            git_push: Some(GitPushEffect::Force),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("force-if-includes"),
        FlagEffects {
            git_push: Some(GitPushEffect::Force),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("mirror"),
        FlagEffects {
            git_push: Some(GitPushEffect::Force),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("all"),
        FlagEffects {
            git_push: Some(GitPushEffect::ImplicitTargets),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("tags"),
        FlagEffects {
            git_push: Some(GitPushEffect::ImplicitTargets),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("branches"),
        FlagEffects {
            git_push: Some(GitPushEffect::ImplicitTargets),
            ..FlagEffects::EMPTY
        },
    ),
    bool_e(
        None,
        Some("prune"),
        FlagEffects {
            git_push: Some(GitPushEffect::ImplicitTargets),
            ..FlagEffects::EMPTY
        },
    ),
    val_none_sl(
        'o',
        "push-option",
        ValueRole::Text,
        "transmits string option to remote server only; does not touch host paths",
    ),
    val_e(
        None,
        Some("receive-pack"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
    val_e(
        None,
        Some("exec"),
        ValueRole::Code,
        FlagEffects {
            runs_code: true,
            ..FlagEffects::EMPTY
        },
    ),
];

const GIT_REBASE_FLAGS: &[FlagSpec] = &[val_e(
    Some('x'),
    Some("exec"),
    ValueRole::Code,
    FlagEffects {
        runs_code: true,
        ..FlagEffects::EMPTY
    },
)];

const GIT_TOOL_CMD_FLAGS: &[FlagSpec] = &[val_e(
    Some('x'),
    Some("tool-cmd"),
    ValueRole::Code,
    FlagEffects {
        runs_code: true,
        ..FlagEffects::EMPTY
    },
)];

const GIT_SUBCOMMANDS: &[SubcommandSpec] = &[
    SubcommandSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_git_ro(&["git", "status"]),
        changes_default: Some("§11 item 10"),
        ..base_subcommand("status")
    },
    SubcommandSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        flags: GIT_LOG_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        home_readonly: home_git_ro(&["git", "log"]),
        git_selects_by_name: true,
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_subcommand("log")
    },
    SubcommandSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        flags: GIT_DIFF_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        operand_access: OperandAccessMode::FlagOnlyWrite,
        writes: WriteBehavior::WithFlag,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        home_readonly: home_git_ro(&["git", "diff"]),
        git_selects_by_name: true,
        changes_default: Some("§11 item 4; §11 item 9; §11 item 10"),
        ..base_subcommand("diff")
    },
    SubcommandSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        operands: OperandLayout::all(ValueRole::Ref),
        home_readonly: home_git_ro(&["git", "show"]),
        git_selects_by_name: true,
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_subcommand("show")
    },
    SubcommandSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        flags: GIT_BRANCH_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        writes: WriteBehavior::WithFlagOrOperandCount(1),
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
        flag_mode: FlagMode::AnyExceptForbidden,
        operands: OperandLayout::all(ValueRole::Ref),
        home_readonly: home_git_ro(&["git", "rev-parse"]),
        changes_default: Some("§11 item 9"),
        ..base_subcommand("rev-parse")
    },
    SubcommandSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
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
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 9"),
        ..base_subcommand("push")
    },
    SubcommandSpec {
        operands: OperandLayout::fixed(&[ValueRole::Ref, ValueRole::DestinationRoot]),
        operand_access: OperandAccessMode::SourcesReadLastWrite,
        writes: WriteBehavior::Always,
        derived_destinations: DerivedDestinationsBehavior::Always,
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_subcommand("clone")
    },
    SubcommandSpec {
        flags: GIT_CHECKOUT_FLAGS,
        operands: OperandLayout {
            positional: &[ValueRole::Ref],
            rest: Some(ValueRole::DestinationRoot),
            last: None,
            after_double_dash: Some(ValueRole::DestinationRoot),
        },
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        derived_destinations: DerivedDestinationsBehavior::Always,
        changes_default: Some("§11 item 9; §11 item 10"),
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
        operands: OperandLayout::prefix_then(&[ValueRole::Text], ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithSubarg(&["run"]),
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 9"),
        ..base_subcommand("bisect")
    },
    SubcommandSpec {
        flags: GIT_REBASE_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 9"),
        ..base_subcommand("rebase")
    },
    SubcommandSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::Text], ValueRole::Code),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithSubarg(&["foreach"]),
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 9"),
        ..base_subcommand("submodule")
    },
    SubcommandSpec {
        flags: GIT_TOOL_CMD_FLAGS,
        operands: OperandLayout::all(ValueRole::Ref),
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 9"),
        ..base_subcommand("difftool")
    },
    SubcommandSpec {
        flags: GIT_TOOL_CMD_FLAGS,
        operands: OperandLayout::all(ValueRole::Path),
        operand_access: OperandAccessMode::AllWrite,
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
        changes_default: Some("§11 item 8; §11 item 9"),
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
    operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
        changes_default: Some("§11 item 9; §11 item 10"),
        ..base_tool("nice")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: TIMEOUT_FLAGS,
        operands: OperandLayout::fixed(&[ValueRole::Text]),
        wrapper: Some(WrapperKind::Timeout),
        changes_default: Some("§11 item 9; §11 item 10"),
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
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["ls"]),
        ..base_tool("ls")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["pwd"]),
        ..base_tool("pwd")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["cat"]),
        ..base_tool("cat")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["head"]),
        ..base_tool("head")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["tail"]),
        ..base_tool("tail")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["wc"]),
        ..base_tool("wc")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
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
        flag_mode: FlagMode::AnyExceptForbidden,
        links: LINKS_NONE,
        home_readonly: home_ro(&["stat"]),
        ..base_tool("stat")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["du"]),
        ..base_tool("du")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["df"]),
        ..base_tool("df")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        operands: OperandLayout::all(ValueRole::Text),
        home_readonly: home_ro(&["which"]),
        changes_default: Some("§11 item 9"),
        ..base_tool("which")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["uname"]),
        ..base_tool("uname")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
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
        flag_mode: FlagMode::AnyExceptForbidden,
        home_readonly: home_ro(&["whoami"]),
        ..base_tool("whoami")
    },
    ToolSpec {
        flag_mode: FlagMode::AnyExceptForbidden,
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
        changes_default: Some("§11 item 2; §11 item 9; §11 item 15"),
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
        changes_default: Some("§11 item 2; §11 item 9; §11 item 15"),
        ..base_tool("grep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 2; §11 item 9"),
        ..base_tool("egrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 2; §11 item 9"),
        ..base_tool("fgrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 2; §11 item 9"),
        ..base_tool("zgrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::WithFlag,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 2; §11 item 9"),
        ..base_tool("ggrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: GREP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::Always,
        filter_dialect: Some(FilterDialect::GrepInclude),
        changes_default: Some("§11 item 2; §11 item 9"),
        ..base_tool("rgrep")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: AG_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 2; §11 item 4; §11 item 9"),
        ..base_tool("ag")
    },
    ToolSpec {
        flag_mode: FlagMode::ListedOnly,
        flags: ACK_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Pattern], ValueRole::Path),
        recursive: RecursiveBehavior::Always,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 2; §11 item 4; §11 item 9"),
        ..base_tool("ack")
    },
    // --- Multi-subcommand tools (`git`, `cargo`, `pipx`) ---
    ToolSpec {
        subcommands: GIT_SUBCOMMANDS,
        flags: GIT_GLOBAL_FLAGS,
        operands: OperandLayout::NONE,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 2; §11 item 4; §11 item 9"),
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
        derived_destinations: DerivedDestinationsBehavior::WithFlag,
        recursive: RecursiveBehavior::Always,
        links: LINKS_NONE,
        code_runner: CodeRunnerBehavior::InlineWithFlag,
        code_runner_no_pin: true,
        changes_default: Some("§11 item 2; §11 item 4; §11 item 9; §11 item 10"),
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
        operands: OperandLayout::all(ValueRole::Path),
        operand_access: OperandAccessMode::FlagOnlyWrite,
        writes: WriteBehavior::WithFlag,
        derived_destinations: DerivedDestinationsBehavior::WithFlag,
        recursive: RecursiveBehavior::Always,
        links: LINKS_NONE,
        changes_default: Some("§11 item 2; §11 item 9; §11 item 10"),
        ..base_tool("tar")
    },
    ToolSpec {
        flags: UNZIP_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Path], ValueRole::ArchiveMember),
        operand_access: OperandAccessMode::FlagOnlyWrite,
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
        changes_default: Some("§11 item 2; §11 item 9"),
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
        operand_access: OperandAccessMode::AllWriteWhenWriting,
        writes: WriteBehavior::WithFlag,
        changes_default: Some("§11 item 9"),
        ..base_tool("codesign")
    },
    // --- Code runners & interpreters (§4, §6, §7) ---
    ToolSpec {
        flags: AWK_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Code], ValueRole::Path),
        operand_access: OperandAccessMode::AllWriteWhenWriting,
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FlagValue,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("awk")
    },
    ToolSpec {
        flags: AWK_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Code], ValueRole::Path),
        operand_access: OperandAccessMode::AllWriteWhenWriting,
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FlagValue,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("gawk")
    },
    ToolSpec {
        flags: AWK_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Code], ValueRole::Path),
        operand_access: OperandAccessMode::AllWriteWhenWriting,
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FlagValue,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("mawk")
    },
    ToolSpec {
        flags: SED_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::Code], ValueRole::Path),
        operand_access: OperandAccessMode::AllWriteWhenWriting,
        writes: WriteBehavior::WithFlag,
        code_runner: CodeRunnerBehavior::ScriptCommands(&['e', 'w', 'r']),
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("sed")
    },
    ToolSpec {
        flags: PERL_FLAGS,
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
        operand_access: OperandAccessMode::AllWriteWhenWriting,
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
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool("source")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
        writes: WriteBehavior::Always,
        code_runner: CodeRunnerBehavior::ProgramOrInline {
            pin: ProgramPin::FirstOperand,
        },
        code_runner_no_pin: true,
        changes_default: Some("§11 item 4; §11 item 8"),
        ..base_tool(".")
    },
    ToolSpec {
        operands: OperandLayout::prefix_then(&[ValueRole::ProgramFile], ValueRole::Path),
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
    target_then_code_runner("ssh"),
    target_then_code_runner("mosh"),
    target_then_code_runner("autossh"),
    ToolSpec {
        platforms: Platforms::Linux,
        ..direct_code_runner("nsenter")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..direct_code_runner("unshare")
    },
    target_then_code_runner("docker"),
    target_then_code_runner("podman"),
    target_then_code_runner("nerdctl"),
    target_then_code_runner("kubectl"),
    target_then_code_runner("oc"),
    target_then_code_runner("lxc"),
    target_then_code_runner("incus"),
    ToolSpec {
        platforms: Platforms::Linux,
        ..target_then_code_runner("machinectl")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..direct_code_runner("systemd-run")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..direct_code_runner("systemd-nspawn")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..direct_code_runner("firejail")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..direct_code_runner("bwrap")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..direct_code_runner("flatpak-spawn")
    },
    target_then_code_runner("toolbox"),
    target_then_code_runner("distrobox"),
    target_then_code_runner("distrobox-enter"),
    target_then_code_runner("multipass"),
    target_then_code_runner("vagrant"),
    target_then_code_runner("limactl"),
    target_then_code_runner("colima"),
    ToolSpec {
        platforms: Platforms::Linux,
        ..target_then_code_runner("runuser")
    },
    ToolSpec {
        platforms: Platforms::Linux,
        ..direct_code_runner("setpriv")
    },
    direct_code_runner("parallel"),
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
        operand_access: OperandAccessMode::AllWrite,
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
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        is_downloader: true,
        ..base_tool("curl")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
        writes: WriteBehavior::Always,
        is_downloader: true,
        ..base_tool("wget")
    },
    ToolSpec {
        operand_access: OperandAccessMode::AllWrite,
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

/// Looks up a long flag (without leading `--`) in `flags`:
/// 1. Exact match always wins.
/// 2. Otherwise, if `long` is non-empty (so `--<long>` has length >= 3,
///    mirroring `dev`'s `engine::forbidden_flag` and `words::is_writer`),
///    matches a listed writing flag (`effects().writes == FlagToggle::Enable`)
///    whose long name starts with `long` (e.g. `sort --out=x` → `--output`,
///    `tree --out=x` → `--output`, `git diff --out=x` → `--output`).
pub(crate) fn flag_long_or_writing_abbrev(
    flags: &'static [FlagSpec],
    long: &str,
) -> Option<&'static FlagSpec> {
    if let Some(exact) = flag_long(flags, long) {
        return Some(exact);
    }
    if long.is_empty() {
        return None;
    }
    flags.iter().find(|f| {
        f.effects().writes == FlagToggle::Enable && f.long.is_some_and(|l| l.starts_with(long))
    })
}

/// Checks whether a single flag token matches any entry in `forbidden`,
/// mirroring `dev`'s `engine::forbidden_flag` (`engine.rs:249`) exactly:
/// - short flags match anywhere inside a short-flag cluster (`-xo` matches `-o`);
/// - long options match exact names or prefixes of length `>= 3` including `--`
///   (so `--o`, `--ou`, `--out`, and `--out=x` all match `--output`).
pub(crate) fn is_forbidden_flag(forbidden: &[&str], token: &str) -> bool {
    if token == "--" {
        return false;
    }
    if let Some(rest) = token.strip_prefix("--") {
        let name_part = rest.split('=').next().unwrap_or(rest);
        if name_part.is_empty() {
            return false;
        }
        forbidden.iter().any(|&f| {
            f.strip_prefix("--")
                .is_some_and(|l| l == name_part || l.starts_with(name_part))
        })
    } else if let Some(cluster) = token.strip_prefix('-')
        && !cluster.is_empty()
    {
        cluster.chars().any(|c| {
            forbidden
                .iter()
                .any(|&f| f.len() == 2 && !f.starts_with("--") && f.chars().nth(1) == Some(c))
        })
    } else {
        false
    }
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

/// Looks up a single flag token against `tool`, respecting
/// `tool.single_dash_long` (e.g. `find`'s `-follow`, `-delete`, `-exec`).
pub(crate) fn flag_for_tool(tool: &'static ToolSpec, token: &str) -> Option<&'static FlagSpec> {
    if tool.single_dash_long {
        let rest = token.strip_prefix('-')?;
        if rest.is_empty() || rest.starts_with('-') {
            return None;
        }
        let mut chars = rest.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => flag_short(tool.flags, c),
            (Some(_), Some(_)) => flag_long(tool.flags, rest),
            (None, _) => None,
        }
    } else {
        flag(tool.flags, token)
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
    if let Some(after_dd) = layout.after_double_dash {
        parts.push(format!("after `--`: {}", fmt_role(after_dd)));
    }
    if parts.is_empty() {
        "—".to_string()
    } else {
        parts.join(", ")
    }
}

fn fmt_flag_effects(effect: Effect, include_none_reason: bool) -> String {
    let effects = match effect {
        Effect::None { reason } => {
            return if include_none_reason {
                format!("none ({reason})")
            } else {
                String::new()
            };
        }
        Effect::Applied(e) => e,
    };
    let mut eff = Vec::new();
    match effects.writes {
        FlagToggle::Unchanged => {}
        FlagToggle::Enable => eff.push("writes:+".to_string()),
        FlagToggle::Disable => eff.push("writes:-".to_string()),
    }
    if let Some(access) = effects.value_access {
        eff.push(format!("val-access:{access:?}"));
    }
    match effects.derived_destinations {
        FlagToggle::Unchanged => {}
        FlagToggle::Enable => eff.push("derived-dest:+".to_string()),
        FlagToggle::Disable => eff.push("derived-dest:-".to_string()),
    }
    match effects.recursive {
        FlagToggle::Unchanged => {}
        FlagToggle::Enable => eff.push("recursive:+".to_string()),
        FlagToggle::Disable => eff.push("recursive:-".to_string()),
    }
    if effects.follows_links_at_root != FlagToggle::Unchanged
        || effects.follows_links_below_root != FlagToggle::Unchanged
    {
        let sym = |t: FlagToggle| match t {
            FlagToggle::Unchanged => "=",
            FlagToggle::Enable => "+",
            FlagToggle::Disable => "-",
        };
        eff.push(format!(
            "links(root={}, below={})",
            sym(effects.follows_links_at_root),
            sym(effects.follows_links_below_root)
        ));
    }
    if effects.provides_pattern {
        eff.push("provides-pattern".to_string());
    }
    if effects.provides_script {
        eff.push("provides-script".to_string());
    }
    if let Some(role) = effects.positional_role {
        eff.push(format!("positional-role({})", fmt_role(role)));
    }
    if let Some(spec) = effects.filter {
        let case = match spec.case {
            FilterCase::AlwaysSensitive => "sensitive",
            FilterCase::AlwaysInsensitive => "insensitive",
            FilterCase::FollowGlobalToggle => "global-toggle",
        };
        eff.push(format!("filter({:?}, {case})", spec.dialect));
    }
    match effects.filter_case_insensitive {
        FlagToggle::Unchanged => {}
        FlagToggle::Enable => eff.push("filter-ci:+".to_string()),
        FlagToggle::Disable => eff.push("filter-ci:-".to_string()),
    }
    if effects.runs_code {
        eff.push("runs-code".to_string());
    }
    if let Some(pin) = effects.runs_program {
        eff.push(format!("runs-program({pin:?})"));
    }
    if let Some(push) = effects.git_push {
        eff.push(format!("git-push({push:?})"));
    }
    if let Some(unread) = effects.unread {
        eff.push(format!("unread({unread:?})"));
    }
    eff.join("; ")
}

fn fmt_flag_long_spelling(long: &str, single_dash_long: bool) -> String {
    if single_dash_long {
        format!("-{long}")
    } else {
        format!("--{long}")
    }
}

fn fmt_flags_summary(mode: FlagMode, single_dash_long: bool, flags: &[FlagSpec]) -> String {
    let mode_label = mode.label();
    if flags.is_empty() {
        return match mode {
            FlagMode::ListedOnly => format!("{mode_label}: none (any flag → Unread)"),
            FlagMode::AnyExceptForbidden => format!("{mode_label}: any non-forbidden flag"),
        };
    }
    if flags.len() > 12 {
        let bool_n = flags
            .iter()
            .filter(|f| matches!(f.kind, FlagKind::Bool))
            .count();
        let val_n = flags.len() - bool_n;
        return format!(
            "{mode_label}: {} listed flags ({bool_n} bool, {val_n} value; see flag table below)",
            flags.len()
        );
    }
    let rendered = flags
        .iter()
        .map(|f| {
            let name = match (f.short, f.long) {
                (Some(s), Some(l)) => {
                    format!("-{s}/{}", fmt_flag_long_spelling(l, single_dash_long))
                }
                (Some(s), None) => format!("-{s}"),
                (None, Some(l)) => fmt_flag_long_spelling(l, single_dash_long),
                (None, None) => String::new(),
            };
            let base = match f.kind {
                FlagKind::Bool => format!("`{name}`"),
                FlagKind::Value(r) => format!("`{name}` ({})", fmt_role(r)),
                FlagKind::OptionalValue(r) => format!("`{name}` ([{}])", fmt_role(r)),
            };
            let eff = fmt_flag_effects(f.effect, false);
            if eff.is_empty() {
                base
            } else {
                format!("{base} [{eff}]")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{mode_label}: {rendered}")
}

struct EffectFields {
    access: OperandAccessMode,
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
    let mut items: Vec<String> = vec![format!("access: {}", e.access.label())];
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
    if h.allow_followed_search {
        s.push_str(" [allows followed search (§11 item 15)]");
    }
    s
}

fn write_flag_table(
    out: &mut String,
    heading: &str,
    mode: FlagMode,
    single_dash_long: bool,
    flags: &[FlagSpec],
) {
    let _ = writeln!(
        out,
        "\n### `{heading}` ({} flags, {})\n",
        flags.len(),
        mode.label()
    );
    out.push_str("| Short | Long | Kind | Effects |\n");
    out.push_str("|---|---|---|---|\n");
    for f in flags {
        let short = f.short.map_or_else(|| "—".into(), |c| format!("`-{c}`"));
        let long = f.long.map_or_else(
            || "—".into(),
            |l| format!("`{}`", fmt_flag_long_spelling(l, single_dash_long)),
        );
        let kind = match f.kind {
            FlagKind::Bool => "Bool".into(),
            FlagKind::Value(r) => format!("Value({})", fmt_role(r)),
            FlagKind::OptionalValue(r) => format!("OptionalValue({})", fmt_role(r)),
        };
        let eff = fmt_flag_effects(f.effect, true);
        let eff_str = if eff.is_empty() {
            "—".to_string()
        } else {
            eff
        };
        let _ = writeln!(out, "| {short} | {long} | {kind} | {eff_str} |");
    }
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
           stays in the model for hard-deny and store checks (§4): every word counts as a candidate `Path` \
           with conservative `ReadWrite` access, the unit counts as a possible writer (§5), and it \
           is never eligible for Home's read-only list. \
           Under a covering prefix/exact Allow rule meeting §7's simple-command conditions, Clear targets remain Clear.\n\
         - **`AnyExceptForbidden`** applies **only** to `HOME_READONLY_COMMANDS` rows without complete `dev` flag tables. \
           Forbidden flags mirror `dev`'s `engine::forbidden_flag` (short clusters and long abbreviations of `>= 3` chars including `--`). \
           Listed flags (and long abbreviations matching a listed writing flag) keep their stated effects; \
           any unlisted non-forbidden flag keeps `dev`'s conservative treatment (`ReadWrite` candidate-path check on every unclassified word and attached `--opt=value`).\n\
         - **Rows outside `HOME_READONLY_COMMANDS`** (and search tools with complete `dev` flag tables) use **`ListedOnly`**: \
           an unlisted flag makes the unit `Unread(UnknownFlag)` and falls back to conservative `ReadWrite` candidate-path checks on unclassified words.\n\
         - An **explicitly excluded form** makes its unit **Unread**: every entry in **Outside the model**, \
           any unlisted flag on a `ListedOnly` row, any forbidden flag on an `AnyExceptForbidden` row, \
           or any modeled flag/form triggering an explicit `Unread` effect (§3–§6).\n\n",
    );

    // 1. Main tools table
    out.push_str("## Modeled tools\n\n");
    out.push_str(
        "| Tool | Platforms | Flags | Operands | Effects & pinning | Home read-only | Changes default (§11) |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|\n");
    for t in TOOLS {
        let mut effects = fmt_effects(EffectFields {
            access: t.operand_access,
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
            fmt_flags_summary(t.flag_mode, t.single_dash_long, t.flags),
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
                access: s.operand_access,
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
                fmt_flags_summary(s.flag_mode, false, s.flags),
                fmt_operands(s.operands),
                effects,
                fmt_home(s.home_readonly),
                s.changes_default.unwrap_or("—"),
            );
        }
    }

    // 3. Detailed flag tables for EVERY tool and subcommand that defines flags
    out.push_str("\n## Detailed flag tables\n");
    for t in TOOLS {
        if !t.flags.is_empty() {
            write_flag_table(&mut out, t.name, t.flag_mode, t.single_dash_long, t.flags);
        }
        for s in t.subcommands {
            if !s.flags.is_empty() {
                let heading = format!("{} {}", t.name, s.name);
                write_flag_table(&mut out, &heading, s.flag_mode, false, s.flags);
            }
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
                if f.effects().filter.is_some() {
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
                || layout.last == Some(ValueRole::Code)
                || flags.iter().any(|f| {
                    f.effects().runs_code
                        || matches!(
                            f.kind,
                            FlagKind::Value(ValueRole::Code)
                                | FlagKind::OptionalValue(ValueRole::Code)
                        )
                })
        };
        let has_program_file = |layout: OperandLayout, flags: &[FlagSpec]| {
            layout.positional.contains(&ValueRole::ProgramFile)
                || layout.rest == Some(ValueRole::ProgramFile)
                || flags.iter().any(|f| {
                    f.effects().runs_program.is_some()
                        || matches!(
                            f.kind,
                            FlagKind::Value(ValueRole::ProgramFile)
                                | FlagKind::OptionalValue(ValueRole::ProgramFile)
                        )
                })
        };

        let check_row = |label: &str,
                         code: CodeRunnerBehavior,
                         no_pin: bool,
                         hard_deny: Option<&'static str>,
                         changes_default: Option<&'static str>,
                         layout: OperandLayout,
                         flags: &[FlagSpec],
                         subcommands: &[SubcommandSpec],
                         is_subcommand: bool| {
            match code {
                CodeRunnerBehavior::Never => {
                    assert!(
                        !no_pin,
                        "{label}: code_runner_no_pin set on non-code-runner"
                    );
                }
                CodeRunnerBehavior::InlineOnly => {
                    assert!(
                        has_code_operand(layout, flags),
                        "{label}: InlineOnly row must identify an actual Code operand or flag (prefix invalidity is not a substitute)"
                    );
                    assert!(
                        no_pin,
                        "{label}: InlineOnly code runner must set code_runner_no_pin"
                    );
                }
                CodeRunnerBehavior::InlineWithFlag => {
                    assert!(
                        flags.iter().any(|f| f.effects().runs_code),
                        "{label}: InlineWithFlag row must have a runs_code flag"
                    );
                    assert!(
                        no_pin,
                        "{label}: InlineWithFlag code runner must set code_runner_no_pin (§7)"
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
                    assert!(
                        no_pin,
                        "{label}: ScriptCommands code runner must set code_runner_no_pin (§7)"
                    );
                }
                CodeRunnerBehavior::InlineWithSubarg(subargs) => {
                    assert!(
                        !subargs.is_empty(),
                        "{label}: InlineWithSubarg must name subarg(s)"
                    );
                    let subcmd_has_code = subcommands.iter().any(|s| {
                        subargs.contains(&s.name)
                            && (has_program_file(s.operands, s.flags)
                                || has_code_operand(s.operands, s.flags))
                    });
                    assert!(
                        has_code_operand(layout, flags) || subcmd_has_code,
                        "{label}: InlineWithSubarg must identify code/program location"
                    );
                    assert!(
                        no_pin,
                        "{label}: InlineWithSubarg code runner must set code_runner_no_pin (§7)"
                    );
                }
                CodeRunnerBehavior::ProgramOrInline { pin } => {
                    assert_ne!(
                        pin,
                        ProgramPin::None,
                        "{label}: ProgramOrInline must specify a ProgramPin"
                    );
                    assert!(
                        has_program_file(layout, flags) || has_code_operand(layout, flags),
                        "{label}: ProgramOrInline must name ProgramFile or Code operand"
                    );
                    assert!(
                        no_pin,
                        "{label}: ProgramOrInline unpinned prefix must set code_runner_no_pin (§7)"
                    );
                }
                CodeRunnerBehavior::ProjectBuild { pin } => {
                    assert_ne!(
                        pin,
                        ProgramPin::None,
                        "{label}: ProjectBuild must specify a ProgramPin"
                    );
                    if is_subcommand && pin == ProgramPin::CommandAndSubcommand {
                        assert!(
                            !no_pin,
                            "{label}: pinned [tool, subcommand] build row must not reject its own prefix"
                        );
                    } else {
                        assert!(
                            no_pin,
                            "{label}: unpinned build tool prefix must set code_runner_no_pin"
                        );
                    }
                }
            }
            if no_pin && hard_deny.is_none() {
                assert!(
                    changes_default.is_some_and(|cd| cd.contains("§11 item 4")),
                    "{label}: code_runner_no_pin row must cite §11 item 4 in changes_default"
                );
            }
        };

        for t in TOOLS {
            check_row(
                t.name,
                t.code_runner,
                t.code_runner_no_pin,
                t.hard_deny,
                t.changes_default,
                t.operands,
                t.flags,
                t.subcommands,
                false,
            );
            for s in t.subcommands {
                check_row(
                    &format!("{} {}", t.name, s.name),
                    s.code_runner,
                    s.code_runner_no_pin,
                    t.hard_deny,
                    s.changes_default,
                    s.operands,
                    s.flags,
                    &[],
                    true,
                );
            }
        }
    }

    /// Test-only helper modeling how the table's Home read-only + `FlagMode`
    /// specification accepts or rejects a concrete argv slice in Home.
    fn table_accepts_in_home(argv: &[&str]) -> bool {
        let Some(&cmd) = argv.first() else {
            return false;
        };
        let Some(tool) = lookup(cmd) else {
            return false;
        };
        let (flag_mode, flags, home, args) = if !tool.subcommands.is_empty() {
            let Some(&sub_name) = argv.get(1) else {
                return false;
            };
            let Some(sub) = subcommand(tool, sub_name) else {
                return false;
            };
            (sub.flag_mode, sub.flags, sub.home_readonly, &argv[2..])
        } else {
            (tool.flag_mode, tool.flags, tool.home_readonly, &argv[1..])
        };
        let Some(home) = home else {
            return false;
        };
        // Enforce `home.only` (e.g. `git branch` listing-only flags).
        if let Some(only) = home.only
            && !args.iter().all(|a| only.contains(a))
        {
            return false;
        }
        let mut i = 0;
        let mut dd = false;
        let mut follows_below = false;
        while i < args.len() {
            let tok = args[i];
            i += 1;
            if dd || !tok.starts_with('-') || tok == "-" {
                continue;
            }
            if tok == "--" {
                dd = true;
                continue;
            }
            if is_forbidden_flag(home.forbidden, tok) {
                return false;
            }
            if let Some(rest) = tok.strip_prefix("--") {
                let (name, has_eq) = match rest.split_once('=') {
                    Some((n, _)) => (n, true),
                    None => (rest, false),
                };
                if let Some(spec) = flag_long_or_writing_abbrev(flags, name) {
                    let eff = spec.effects();
                    if eff.runs_code || eff.unread.is_some() || eff.writes == FlagToggle::Enable {
                        return false;
                    }
                    follows_below = eff.follows_links_below_root.apply(follows_below);
                    if matches!(spec.kind, FlagKind::Value(_)) && !has_eq {
                        i += 1;
                    }
                } else if flag_mode == FlagMode::ListedOnly {
                    return false;
                }
            } else {
                let chars: Vec<char> = tok[1..].chars().collect();
                for (k, &c) in chars.iter().enumerate() {
                    if let Some(spec) = flag_short(flags, c) {
                        let eff = spec.effects();
                        if eff.runs_code || eff.unread.is_some() || eff.writes == FlagToggle::Enable
                        {
                            return false;
                        }
                        follows_below = eff.follows_links_below_root.apply(follows_below);
                        if matches!(spec.kind, FlagKind::Value(_)) {
                            if k + 1 == chars.len() {
                                i += 1;
                            }
                            break;
                        }
                    } else if flag_mode == FlagMode::ListedOnly {
                        return false;
                    }
                }
            }
        }
        if follows_below && !home.allow_followed_search {
            return false;
        }
        true
    }

    /// Test-only helper checking whether a command's flags are accepted under a
    /// covering rule (not `UnknownFlag`, no `FlagUnread`, and not `OUTSIDE_MODEL`).
    fn table_accepts_flags_under_rule(argv: &[&str]) -> bool {
        let Some(&cmd) = argv.first() else {
            return false;
        };
        if lookup_outside_model(cmd).is_some() {
            return false;
        }
        let Some(tool) = lookup(cmd) else {
            // Unknown tool stays in model under covering rules (§4).
            return true;
        };
        let (flag_mode, flags, args) = if !tool.subcommands.is_empty() {
            let Some(&sub_name) = argv.get(1) else {
                return true;
            };
            match subcommand(tool, sub_name) {
                Some(sub) => (sub.flag_mode, sub.flags, &argv[2..]),
                None => return true,
            }
        } else {
            (tool.flag_mode, tool.flags, &argv[1..])
        };
        let mut i = 0;
        let mut dd = false;
        while i < args.len() {
            let tok = args[i];
            i += 1;
            if dd || !tok.starts_with('-') || tok == "-" {
                continue;
            }
            if tok == "--" {
                dd = true;
                continue;
            }
            if let Some(rest) = tok.strip_prefix("--") {
                let (name, has_eq) = match rest.split_once('=') {
                    Some((n, _)) => (n, true),
                    None => (rest, false),
                };
                if let Some(spec) = flag_long_or_writing_abbrev(flags, name) {
                    if spec.effects().unread.is_some() {
                        return false;
                    }
                    if matches!(spec.kind, FlagKind::Value(_)) && !has_eq {
                        i += 1;
                    }
                } else if flag_mode == FlagMode::ListedOnly {
                    return false;
                }
            } else {
                let chars: Vec<char> = tok[1..].chars().collect();
                for (k, &c) in chars.iter().enumerate() {
                    if let Some(spec) = flag_short(flags, c) {
                        if spec.effects().unread.is_some() {
                            return false;
                        }
                        if matches!(spec.kind, FlagKind::Value(_)) {
                            if k + 1 == chars.len() {
                                i += 1;
                            }
                            break;
                        }
                    } else if flag_mode == FlagMode::ListedOnly {
                        return false;
                    }
                }
            }
        }
        true
    }

    #[test]
    fn home_readonly_effective_forms_and_metadata_parity() {
        // 1. Metadata parity with HOME_READONLY_COMMANDS
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

        // 2. Effective accepted forms in Home (finding 1 & finding 2)
        let home_accepted: &[&[&str]] = &[
            &["ls", "-la"],
            &["ls", "-la", "README.md"],
            &["pwd"],
            &["pwd", "-P"],
            &["cat", "-n", "README.md"],
            &["head", "-n", "5", "README.md"],
            &["tail", "-n", "20", "README.md"],
            &["wc", "-l", "README.md"],
            &["file", "-i", "README.md"],
            &["stat", "-c", "%s", "README.md"],
            &["du", "-sh", "src"],
            &["df", "-h"],
            &["which", "-a", "git"],
            &["uname", "-a"],
            &["date", "+%Y-%m-%d"],
            &["whoami"],
            &["tree", "-L", "2", "src"],
            &["rg", "-n", "--hidden", "needle", "src"],
            &["grep", "-rn", "needle", "src"],
            &["git", "status", "--short"],
            &["git", "log", "-n", "5", "--oneline"],
            &["git", "diff", "--stat", "HEAD~1"],
            &["git", "show", "--stat", "HEAD"],
            &["git", "branch", "-a", "-v"],
            &["git", "branch", "--show-current"],
            &["git", "rev-parse", "--show-toplevel"],
            &["git", "ls-files", "--others"],
        ];
        for cmd in home_accepted {
            assert!(
                table_accepts_in_home(cmd),
                "Home read-only must accept {cmd:?}"
            );
        }

        // §11 item 15 (§10 row 15, D-139): controlled by ALLOW_HOME_FOLLOWED_SEARCH
        assert_eq!(
            table_accepts_in_home(&["rg", "-L", "x", "proj"]),
            ALLOW_HOME_FOLLOWED_SEARCH
        );
        assert_eq!(
            table_accepts_in_home(&["grep", "-R", "x", "proj"]),
            ALLOW_HOME_FOLLOWED_SEARCH
        );

        // 3. Effective rejected forms in Home
        let home_rejected: &[&[&str]] = &[
            &["file", "-f", "list.txt"],
            &["file", "--compile", "magic"],
            &["file", "-m", "magic", "README.md"],
            &["date", "-s", "2026-01-01"],
            &["date", "--file", "dates.txt"],
            &["tree", "-o", "out.txt"],
            &["tree", "--out=out.txt"],
            &["tree", "-R", "src"],
            &["rg", "--pre", "cat", "x", "proj"],
            &["rg", "--unknown-flag", "x", "proj"],
            &["rg", "-f", "pats", "proj"],
            &["rg", "-t", "rust", "x", "proj"],
            &["grep", "-f", "pats", "proj"],
            &["grep", "--exclude=key", "x", "proj"],
            &["grep", "--exclude-dir=sub", "x", "proj"],
            &["grep", "--include-dir=sub", "x", "proj"],
            &["grep", "--unknown-flag", "x", "proj"],
            &["git", "status", "-c", "core.fsmonitor=x"],
            &["git", "diff", "--output", "out.patch"],
            &["git", "diff", "--out=out.patch"],
            &["git", "diff", "--ext-diff"],
            &["git", "branch", "topic"],
            &["git", "branch", "-d", "topic"],
        ];
        for cmd in home_rejected {
            assert!(
                !table_accepts_in_home(cmd),
                "Home read-only must reject {cmd:?}"
            );
        }

        // 4. Forms accepted vs. rejected under covering rules (Rules B & C)
        let rule_accepted: &[&[&str]] = &[
            &["rm", "-r", "build"],
            &["cp", "-L", "a", "b"],
            &["sed", "-e", "p", "README.md"],
            &["cargo", "test"],
            &["my-custom-tool", "--flag", "arg"],
        ];
        for cmd in rule_accepted {
            assert!(
                table_accepts_flags_under_rule(cmd),
                "covering rule must accept modeled form {cmd:?}"
            );
        }
        // Unlisted flags on rows outside HOME_READONLY_COMMANDS are Unread (Rule C).
        let rule_rejected: &[&[&str]] = &[
            &["cp", "-t", "vault", "README.md"],
            &["tar", "-chf", "out.tar", "-C", "sub", "../key"],
            &["grep", "-R", "--exclude=key", "x", "proj"],
            &["ag", "-f", "--ignore=key", "x", "proj"],
        ];
        for cmd in rule_rejected {
            assert!(
                !table_accepts_flags_under_rule(cmd),
                "covering rule must mark unlisted/unsupported form {cmd:?} Unread"
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

        // 6. Bidirectional parity: engine.rs RG_* & GREP_* (+ gate.rs follow/file)
        let rg = lookup("rg").unwrap();
        let rg_dev_bool_s: HashSet<char> = engine::RG_BOOL_S
            .chars()
            .chain(std::iter::once('L'))
            .collect();
        let rg_dev_val_s: HashSet<char> = engine::RG_VAL_S
            .chars()
            .chain(std::iter::once('f'))
            .collect();
        let rg_dev_bool_l: HashSet<&str> = engine::RG_BOOL_L
            .iter()
            .copied()
            .chain(std::iter::once("follow"))
            .collect();
        let rg_dev_val_l: HashSet<&str> = engine::RG_VAL_L
            .iter()
            .copied()
            .chain(std::iter::once("file"))
            .collect();
        for &c in &rg_dev_bool_s {
            let f = flag_short(rg.flags, c).unwrap_or_else(|| panic!("rg missing -{c}"));
            assert_eq!(f.kind, FlagKind::Bool, "rg -{c} must be Bool");
        }
        for &c in &rg_dev_val_s {
            let f = flag_short(rg.flags, c).unwrap_or_else(|| panic!("rg missing -{c}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "rg -{c} must take a Value"
            );
        }
        for &l in &rg_dev_bool_l {
            let f = flag_long(rg.flags, l).unwrap_or_else(|| panic!("rg missing --{l}"));
            assert_eq!(f.kind, FlagKind::Bool, "rg --{l} must be Bool");
        }
        for &l in &rg_dev_val_l {
            let f = flag_long(rg.flags, l).unwrap_or_else(|| panic!("rg missing --{l}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "rg --{l} must take a Value"
            );
        }
        for f in rg.flags {
            if let Some(c) = f.short {
                match f.kind {
                    FlagKind::Bool => assert!(
                        rg_dev_bool_s.contains(&c),
                        "rg invented short bool flag -{c} beyond dev's table"
                    ),
                    FlagKind::Value(_) => assert!(
                        rg_dev_val_s.contains(&c),
                        "rg invented short value flag -{c} beyond dev's table"
                    ),
                    FlagKind::OptionalValue(_) => {
                        panic!("rg invented short optional-value flag -{c}")
                    }
                }
            }
            if let Some(l) = f.long {
                match f.kind {
                    FlagKind::Bool => assert!(
                        rg_dev_bool_l.contains(l),
                        "rg invented long bool flag --{l} beyond dev's table"
                    ),
                    FlagKind::Value(_) => assert!(
                        rg_dev_val_l.contains(l),
                        "rg invented long value flag --{l} beyond dev's table"
                    ),
                    FlagKind::OptionalValue(_) => {
                        panic!("rg invented long optional-value flag --{l}")
                    }
                }
            }
        }

        let grep = lookup("grep").unwrap();
        let grep_dev_bool_s: HashSet<char> = engine::GREP_BOOL_S.chars().collect();
        let grep_dev_val_s: HashSet<char> = engine::GREP_VAL_S
            .chars()
            .chain(std::iter::once('f'))
            .collect();
        let grep_dev_bool_l: HashSet<&str> = engine::GREP_BOOL_L.iter().copied().collect();
        let grep_dev_val_l: HashSet<&str> = engine::GREP_VAL_L
            .iter()
            .copied()
            .chain(["file", "include-dir"])
            .collect();
        for &c in &grep_dev_bool_s {
            let f = flag_short(grep.flags, c).unwrap_or_else(|| panic!("grep missing -{c}"));
            assert_eq!(f.kind, FlagKind::Bool, "grep -{c} must be Bool");
        }
        for &c in &grep_dev_val_s {
            let f = flag_short(grep.flags, c).unwrap_or_else(|| panic!("grep missing -{c}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "grep -{c} must take a Value"
            );
        }
        for &l in &grep_dev_bool_l {
            let f = flag_long(grep.flags, l).unwrap_or_else(|| panic!("grep missing --{l}"));
            assert_eq!(f.kind, FlagKind::Bool, "grep --{l} must be Bool");
        }
        for &l in &grep_dev_val_l {
            let f = flag_long(grep.flags, l).unwrap_or_else(|| panic!("grep missing --{l}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "grep --{l} must take a Value"
            );
        }
        for f in grep.flags {
            if let Some(c) = f.short {
                match f.kind {
                    FlagKind::Bool => assert!(
                        grep_dev_bool_s.contains(&c),
                        "grep invented short bool flag -{c} beyond dev's table"
                    ),
                    FlagKind::Value(_) => assert!(
                        grep_dev_val_s.contains(&c),
                        "grep invented short value flag -{c} beyond dev's table"
                    ),
                    FlagKind::OptionalValue(_) => {
                        panic!("grep invented short optional-value flag -{c}")
                    }
                }
            }
            if let Some(l) = f.long {
                match f.kind {
                    FlagKind::Bool => assert!(
                        grep_dev_bool_l.contains(l),
                        "grep invented long bool flag --{l} beyond dev's table"
                    ),
                    FlagKind::Value(_) => assert!(
                        grep_dev_val_l.contains(l),
                        "grep invented long value flag --{l} beyond dev's table"
                    ),
                    FlagKind::OptionalValue(_) => {
                        panic!("grep invented long optional-value flag --{l}")
                    }
                }
            }
        }

        // 7. Bidirectional parity: gate.rs AG_OPTS & ACK_OPTS
        for (name, opts) in [("ag", &gate::AG_OPTS), ("ack", &gate::ACK_OPTS)] {
            let tool = lookup(name).unwrap();
            let dev_bool_s: HashSet<char> = opts.flags.chars().collect();
            let dev_val_s: HashSet<char> = opts.vals.chars().collect();
            let dev_opt_s: HashSet<char> = opts.opt_vals.chars().collect();
            let dev_bool_l: HashSet<&str> = opts.long_flags.iter().copied().collect();
            let dev_val_l: HashSet<&str> = opts.long_vals.iter().copied().collect();
            let dev_opt_l: HashSet<&str> = opts.long_opt_vals.iter().copied().collect();

            for &c in &dev_bool_s {
                let f = flag_short(tool.flags, c).unwrap_or_else(|| panic!("{name} missing -{c}"));
                assert_eq!(f.kind, FlagKind::Bool, "{name} -{c} must be Bool");
            }
            for &c in &dev_val_s {
                let f = flag_short(tool.flags, c).unwrap_or_else(|| panic!("{name} missing -{c}"));
                assert!(
                    matches!(f.kind, FlagKind::Value(_)),
                    "{name} -{c} must take Value"
                );
            }
            for &c in &dev_opt_s {
                let f = flag_short(tool.flags, c).unwrap_or_else(|| panic!("{name} missing -{c}"));
                assert!(
                    matches!(f.kind, FlagKind::OptionalValue(_)),
                    "{name} -{c} must take OptionalValue"
                );
            }
            for &l in &dev_bool_l {
                let f = flag_long(tool.flags, l).unwrap_or_else(|| panic!("{name} missing --{l}"));
                assert_eq!(f.kind, FlagKind::Bool, "{name} --{l} must be Bool");
            }
            for &l in &dev_val_l {
                let f = flag_long(tool.flags, l).unwrap_or_else(|| panic!("{name} missing --{l}"));
                assert!(
                    matches!(f.kind, FlagKind::Value(_)),
                    "{name} --{l} must take Value"
                );
            }
            for &l in &dev_opt_l {
                let f = flag_long(tool.flags, l).unwrap_or_else(|| panic!("{name} missing --{l}"));
                assert!(
                    matches!(f.kind, FlagKind::OptionalValue(_)),
                    "{name} --{l} must take OptionalValue"
                );
            }
            for f in tool.flags {
                if let Some(c) = f.short {
                    match f.kind {
                        FlagKind::Bool => assert!(
                            dev_bool_s.contains(&c),
                            "{name} invented short bool flag -{c} beyond dev's table"
                        ),
                        FlagKind::Value(_) => assert!(
                            dev_val_s.contains(&c),
                            "{name} invented short value flag -{c} beyond dev's table"
                        ),
                        FlagKind::OptionalValue(_) => assert!(
                            dev_opt_s.contains(&c),
                            "{name} invented short optional-value flag -{c} beyond dev's table"
                        ),
                    }
                }
                if let Some(l) = f.long {
                    match f.kind {
                        FlagKind::Bool => assert!(
                            dev_bool_l.contains(l),
                            "{name} invented long bool flag --{l} beyond dev's table"
                        ),
                        FlagKind::Value(_) => assert!(
                            dev_val_l.contains(l),
                            "{name} invented long value flag --{l} beyond dev's table"
                        ),
                        FlagKind::OptionalValue(_) => assert!(
                            dev_opt_l.contains(l),
                            "{name} invented long optional-value flag --{l} beyond dev's table"
                        ),
                    }
                }
            }
            for c in opts.no_pattern.chars() {
                let f = flag_short(tool.flags, c).unwrap();
                assert!(
                    f.effects().provides_pattern,
                    "{name} -{c} must set provides_pattern"
                );
            }
            for &l in opts.long_no_pattern {
                let f = flag_long(tool.flags, l).unwrap();
                assert!(
                    f.effects().provides_pattern,
                    "{name} --{l} must set provides_pattern"
                );
            }
        }
    }

    #[test]
    fn non_path_operand_relaxations_cite_item_9() {
        let is_relaxed = |r: ValueRole| {
            matches!(
                r,
                ValueRole::Pattern | ValueRole::Text | ValueRole::Ref | ValueRole::ArchiveMember
            )
        };
        let has_relaxed = |layout: OperandLayout, flags: &[FlagSpec]| {
            layout.positional.iter().copied().any(is_relaxed)
                || layout.rest.is_some_and(is_relaxed)
                || layout.last.is_some_and(is_relaxed)
                || layout.after_double_dash.is_some_and(is_relaxed)
                || flags.iter().any(|f| {
                    matches!(f.kind, FlagKind::Value(r) | FlagKind::OptionalValue(r) if is_relaxed(r))
                        || f.effects().positional_role.is_some_and(is_relaxed)
                })
        };
        for t in TOOLS {
            if has_relaxed(t.operands, t.flags) {
                assert!(
                    t.changes_default
                        .is_some_and(|cd| cd.contains("§11 item 9")),
                    "`{}` relaxes operands or flag values to non-Path without citing §11 item 9",
                    t.name
                );
            }
            for s in t.subcommands {
                if has_relaxed(s.operands, s.flags) {
                    assert!(
                        s.changes_default
                            .is_some_and(|cd| cd.contains("§11 item 9")),
                        "`{} {}` relaxes operands or flag values to non-Path without citing §11 item 9",
                        t.name,
                        s.name
                    );
                }
            }
        }
        for name in [
            "true", "false", "id", "codesign", "git", "find", "nice", "timeout",
        ] {
            let t = lookup(name).unwrap();
            assert!(
                t.changes_default
                    .is_some_and(|cd| cd.contains("§11 item 9")),
                "`{name}` must cite §11 item 9"
            );
        }
    }

    #[test]
    fn operand_access_tar_modes_conditional_writers_and_code_runners() {
        // Finding 5 (round 1) + Finding 4 (round 2): tar creation/update sources are Paths;
        // extract/list operands are ArchiveMembers; -C triggers UnsupportedBaseDirChange.
        let tar = lookup("tar").unwrap();
        assert_eq!(tar.operands, OperandLayout::all(ValueRole::Path));
        for c in ['c', 'r', 'u'] {
            let f = flag_short(tar.flags, c).unwrap();
            assert_eq!(f.effects().positional_role, Some(ValueRole::Path));
            assert_eq!(f.effects().writes, FlagToggle::Enable);
        }
        let tar_x = flag_short(tar.flags, 'x').unwrap();
        assert_eq!(
            tar_x.effects().positional_role,
            Some(ValueRole::ArchiveMember)
        );
        assert_eq!(tar_x.effects().derived_destinations, FlagToggle::Enable);
        let tar_t = flag_short(tar.flags, 't').unwrap();
        assert_eq!(
            tar_t.effects().positional_role,
            Some(ValueRole::ArchiveMember)
        );
        assert_eq!(tar_t.effects().writes, FlagToggle::Unchanged);
        let tar_f = flag_short(tar.flags, 'f').unwrap();
        assert_eq!(tar_f.kind, FlagKind::Value(ValueRole::Path));
        assert_eq!(
            tar_f.effects().value_access,
            Some(FlagValueAccess::WriteWhenWriting)
        );
        let tar_c = flag_short(tar.flags, 'C').unwrap();
        assert_eq!(tar_c.kind, FlagKind::Value(ValueRole::DestinationRoot));
        assert_eq!(tar_c.effects().unread, Some(FlagUnread::BaseDirChange));

        // Finding 6 (round 1): destination write access for git clone, git checkout, codesign, sed, unzip, etc.
        let git = lookup("git").unwrap();
        let clone = subcommand(git, "clone").unwrap();
        assert_eq!(
            clone.operand_access,
            OperandAccessMode::SourcesReadLastWrite
        );
        assert_eq!(
            clone.operands.positional,
            &[ValueRole::Ref, ValueRole::DestinationRoot]
        );
        let checkout = subcommand(git, "checkout").unwrap();
        assert_eq!(checkout.operand_access, OperandAccessMode::AllWrite);
        assert_eq!(checkout.operands.positional, &[ValueRole::Ref]);
        assert_eq!(checkout.operands.rest, Some(ValueRole::DestinationRoot));
        assert_eq!(
            checkout.operands.after_double_dash,
            Some(ValueRole::DestinationRoot)
        );
        let codesign = lookup("codesign").unwrap();
        assert_eq!(
            codesign.operand_access,
            OperandAccessMode::AllWriteWhenWriting
        );
        assert_eq!(
            flag_short(codesign.flags, 's').unwrap().effects().writes,
            FlagToggle::Enable
        );
        assert_eq!(
            flag_long(codesign.flags, "remove-signature")
                .unwrap()
                .effects()
                .writes,
            FlagToggle::Enable
        );

        // Finding 7 (round 1): git branch writes on creation/mutation, not on listing.
        let branch = subcommand(git, "branch").unwrap();
        assert_eq!(branch.writes, WriteBehavior::WithFlagOrOperandCount(1));
        assert_eq!(
            flag_short(branch.flags, 'l').unwrap().effects().writes,
            FlagToggle::Disable
        );
        assert_eq!(
            flag_short(branch.flags, 'd').unwrap().effects().writes,
            FlagToggle::Enable
        );

        // Finding 9 (round 1) + Finding 5 (round 2): sed -e inspects e/w/r; sed -f & awk -f pin ProgramFile.
        let sed = lookup("sed").unwrap();
        assert_eq!(
            sed.code_runner,
            CodeRunnerBehavior::ScriptCommands(&['e', 'w', 'r'])
        );
        assert_eq!(sed.operand_access, OperandAccessMode::AllWriteWhenWriting);
        assert!(sed.changes_default.unwrap().contains("§11 item 8"));
        let sed_e = flag_short(sed.flags, 'e').unwrap();
        assert_eq!(sed_e.kind, FlagKind::Value(ValueRole::Code));
        assert!(sed_e.effects().provides_script);
        assert!(!sed_e.effects().runs_code);
        assert_eq!(sed_e.effects().runs_program, None);
        let sed_f = flag_short(sed.flags, 'f').unwrap();
        assert_eq!(sed_f.kind, FlagKind::Value(ValueRole::ProgramFile));
        assert!(sed_f.effects().provides_script);
        assert!(!sed_f.effects().runs_code);
        assert_eq!(sed_f.effects().runs_program, Some(ProgramPin::FlagValue));
        assert_eq!(sed_f.effects().value_access, Some(FlagValueAccess::Read));

        // Finding 10 (round 1): ag, ack, git diff, git push all reject unpinned Allow prefixes.
        for name in ["ag", "ack"] {
            let t = lookup(name).unwrap();
            assert!(t.code_runner_no_pin);
            assert!(t.changes_default.unwrap().contains("§11 item 4"));
        }
        for sub_name in ["diff", "push"] {
            let s = subcommand(git, sub_name).unwrap();
            assert!(s.code_runner_no_pin);
            assert!(s.changes_default.unwrap().contains("§11 item 4"));
        }

        // Finding 2 (round 1): rg and grep carry §11 item 15 and single toggle flag.
        for name in ["rg", "grep"] {
            let t = lookup(name).unwrap();
            assert!(t.changes_default.unwrap().contains("§11 item 15"));
            assert_eq!(
                t.home_readonly.unwrap().allow_followed_search,
                ALLOW_HOME_FOLLOWED_SEARCH
            );
        }
    }

    #[test]
    fn disabling_flags_override_earlier_effects_in_order() {
        let apply_links =
            |flags: &'static [FlagSpec], init: LinkPolicy, tokens: &[&str], single_dash: bool| {
                let mut at_root = init.at_root;
                let mut below = init.below_root;
                for tok in tokens {
                    let spec = if single_dash {
                        let rest = tok.strip_prefix('-').unwrap();
                        if rest.len() == 1 {
                            flag_short(flags, rest.chars().next().unwrap()).unwrap()
                        } else {
                            flag_long(flags, rest).unwrap()
                        }
                    } else {
                        flag(flags, tok).unwrap()
                    };
                    at_root = spec.effects().follows_links_at_root.apply(at_root);
                    below = spec.effects().follows_links_below_root.apply(below);
                }
                (at_root, below)
            };

        let ack = lookup("ack").unwrap();
        assert_eq!(
            apply_links(ack.flags, ack.links, &["--follow", "--nofollow"], false),
            (false, false)
        );
        assert_eq!(
            apply_links(ack.flags, ack.links, &["--nofollow", "--follow"], false),
            (true, true)
        );

        let find = lookup("find").unwrap();
        assert_eq!(
            apply_links(find.flags, find.links, &["-L", "-P"], true),
            (false, false)
        );
        assert_eq!(
            apply_links(find.flags, find.links, &["-L", "-H"], true),
            (true, false)
        );

        let cp = lookup("cp").unwrap();
        assert_eq!(
            apply_links(cp.flags, cp.links, &["-L", "-P"], false),
            (false, false)
        );

        let ag = lookup("ag").unwrap();
        let mut rec = true;
        for tok in ["-r", "-n"] {
            rec = flag(ag.flags, tok).unwrap().effects().recursive.apply(rec);
        }
        assert!(!rec);
        rec = flag(ag.flags, "-R").unwrap().effects().recursive.apply(rec);
        assert!(rec);

        let git = lookup("git").unwrap();
        let branch = subcommand(git, "branch").unwrap();
        let mut writes = false;
        for tok in ["--list", "-d"] {
            writes = flag(branch.flags, tok)
                .unwrap()
                .effects()
                .writes
                .apply(writes);
        }
        assert!(writes);
        writes = flag(branch.flags, "--list")
            .unwrap()
            .effects()
            .writes
            .apply(writes);
        assert!(!writes);
    }

    #[test]
    fn markdown_renders_access_modes_flag_effects_and_find_primaries() {
        let md = render_markdown();
        assert!(md.contains("Unknown tools vs. excluded forms"));
        assert!(md.contains("AnyExceptForbidden"));
        assert!(md.contains("ListedOnly"));
        assert!(md.contains("access: AllWriteWhenWriting"));
        assert!(md.contains("access: SourcesReadLastWrite"));
        assert!(md.contains("### `codesign`"));
        assert!(md.contains("### `git branch`"));
        assert!(md.contains("### `git diff`"));
        // find primaries must render with single-dash spelling, not double-dash.
        let find_start = md
            .find("### `find`")
            .expect("detailed flag table for `find` missing");
        let after_find = &md[find_start + "### `find`".len()..];
        let find_end = after_find.find("\n### ").unwrap_or(after_find.len());
        let find_section = &after_find[..find_end];
        assert!(find_section.contains("`-follow`"));
        assert!(find_section.contains("`-delete`"));
        assert!(find_section.contains("`-exec`"));
        assert!(!find_section.contains("`--follow`"));
        assert!(!find_section.contains("`--delete`"));
        assert!(!find_section.contains("`--exec`"));
    }

    #[test]
    fn changes_default_citations_reference_valid_items() {
        let check = |label: &str, s: &str| {
            let parts: Vec<&str> = s.split(';').map(str::trim).collect();
            assert!(!parts.is_empty(), "{label}: empty changes_default");
            for p in parts {
                let num_str = p
                    .strip_prefix("§11 item ")
                    .unwrap_or_else(|| panic!("{label}: invalid citation `{p}`"));
                let n: u8 = num_str
                    .parse()
                    .unwrap_or_else(|_| panic!("{label}: non-numeric item in `{p}`"));
                assert!(
                    (1..=15).contains(&n),
                    "{label}: item {n} outside 1..=15 in `{p}`"
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
        let find = lookup("find").unwrap();
        assert_eq!(flag_for_tool(find, "-follow").unwrap().short, Some('L'));
        assert_eq!(flag_for_tool(find, "-delete").unwrap().long, Some("delete"));
        assert!(flag_for_tool(find, "--delete").is_none());
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
