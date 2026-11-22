//! Normative tool table for policy engine rev 4 (contract §6, annex; D-141).
//!
//! Per D-141, the table contains **only** the six row kinds the contract needs:
//! 1. [`ToolRowKind::HomeReadOnly`]: tools and `git` subcommands in
//!    `HOME_READONLY_COMMANDS` (without a search flag table). Uses
//!    `AnyExceptForbidden` mirroring `dev`'s `forbidden_flag` matching (short
//!    clusters and long abbreviations of `>= 3` chars including `--`). Every
//!    word is a candidate `Path` with `ReadWrite` access (no operand
//!    relaxations), and a forbidden flag **only** removes Home eligibility.
//! 2. [`ToolRowKind::Search`]: `rg` and the `grep` family (`grep`, `egrep`,
//!    `fgrep`, `zgrep`, `ggrep`, `rgrep`). Keeps `dev`'s flag tables, the
//!    pattern operand (§11 item 9), modeled filter dialects and case rules,
//!    unsupported filters as `Unread`, and recursion / link-following flags
//!    with the §11 item 15 toggle (`ALLOW_HOME_FOLLOWED_SEARCH`).
//! 3. [`ToolRowKind::Wrapper`] and [`ToolRowKind::NestedShell`]: §4's closed
//!    wrappers (`command`, `exec`, `nice`, `timeout`, `time`, `env`) and nested
//!    shells (`sh`, `bash`, `dash`).
//! 4. [`ToolRowKind::CodeRunner`]: always `Unread`, naming its inline-code
//!    location or pinned program (`Unread (program)`, §11 item 8), with
//!    `code_runner_no_pin` per §7 / §11 item 4.
//! 5. [`ToolRowKind::ConditionalRunner`]: unknown-tool treatment unless a
//!    listed trigger fires (`find`, `git`, `sed`), making the unit `Unread`
//!    (or pinned `Unread (program)` for `sed -f`/`--file`). Triggers are data;
//!    no flag arities or operand roles on these rows.
//! 6. [`ToolRowKind::DerivedDestinationWriter`]: `cp` (`-R`/`-r`/`-a`), `mv`,
//!    `rsync`, `tar` (`-x`), `unzip`, `git clone`, `git checkout`. When
//!    triggered, the writer itself is `Unread(DerivedDestinations)` (§11
//!    item 10) while every word stays a candidate `Path` (`ReadWrite`). Without
//!    a trigger, the unit gets unknown-tool treatment.
//!
//! **Every other tool** (including [`UNKNOWN_BY_DESIGN`]) is an **unknown tool**
//! (§4): every word is a candidate `Path` for hard-deny, store, and
//! self-protection checks (`ReadWrite`), the unit is a possible writer (§5),
//! and it is never Home-eligible. Tools/constructs in [`OUTSIDE_MODEL`] (non-§4
//! shells, non-§4 wrappers, shell reserved words, `pushd`/`popd`) always make
//! their unit `Unread`.
//!
//! Not wired into `SessionGate` until P5; parsing argv against this table is
//! P3's job (`v4/tools.rs`).

#![allow(dead_code)]

use std::fmt::Write as _;

use atrium_protocol::policy::Recursion;

use super::model::{FilterDialect, LinkPolicy, UnreadReason, ValueRole, WrapperKind};

// ---------------------------------------------------------------------------
// Schema types for the normative tool table (§6, D-141)
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

/// Ordered effect toggle applied left-to-right across a unit's flags, where the
/// last matching flag wins (e.g. `rg --glob-case-insensitive
/// --no-glob-case-insensitive`, `grep -R -r`; contract §5, §6).
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

/// Whether a listed flag on a `Search`, `Wrapper`, or `NestedShell` row is
/// boolean or takes a single required value (contract §4, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FlagKind {
    /// Takes 0 arguments.
    Bool,
    /// Takes 1 required argument with the given [`ValueRole`].
    Value(ValueRole),
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

/// Precise `UnreadReason` triggered by a listed search flag whose effect is
/// intentionally outside the rev 4 model (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FlagUnread {
    /// Unsupported filter, exclusion, depth/size limit, or filter dialect →
    /// `UnreadReason::UnsupportedFilterForm` (§5).
    FilterForm,
    /// Other real flag effect outside the model (`grep -d`/`--directories`) →
    /// `UnreadReason::UnsupportedFlagEffect` (§5, §6).
    FlagEffect,
}

impl FlagUnread {
    pub(crate) fn into_unread_reason(self, tool: &str, flag: &str) -> UnreadReason {
        match self {
            Self::FilterForm => UnreadReason::UnsupportedFilterForm,
            Self::FlagEffect => UnreadReason::UnsupportedFlagEffect {
                tool: tool.to_string(),
                flag: flag.to_string(),
            },
        }
    }
}

/// Semantic effects triggered when a listed flag on a `Search`, `Wrapper`, or
/// `NestedShell` row is present (contract §4, §5, §6).
///
/// Toggle fields are applied left-to-right as flags appear in argv so a later
/// disabling flag overrides an earlier enabling flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FlagEffects {
    /// Ordered toggle for recursive traversal (`Enable` / `Disable`; last wins).
    pub(crate) recursive: FlagToggle,
    /// Ordered toggle for following symlinks at root operands (last wins).
    pub(crate) follows_links_at_root: FlagToggle,
    /// Ordered toggle for following symlinks below traversed roots (last wins).
    pub(crate) follows_links_below_root: FlagToggle,
    /// Flag supplies the search pattern (or disables a positional pattern,
    /// e.g. `rg -e`/`-f`/`--files`/`--type-list`, `grep -e`/`-f`).
    pub(crate) provides_pattern: bool,
    /// Modeled search filter dialect and case rule (`rg -g`/`--glob`/`--iglob`,
    /// `grep --include`).
    pub(crate) filter: Option<FilterSpec>,
    /// Global toggle for filter case-insensitivity (`rg --glob-case-insensitive`
    /// / `--no-glob-case-insensitive`; last toggle wins and applies to every
    /// `FilterCase::FollowGlobalToggle` filter on the unit).
    pub(crate) filter_case_insensitive: FlagToggle,
    /// Flag supplies inline code (`sh`/`bash`/`dash` `-c SCRIPT`).
    pub(crate) runs_code: bool,
    /// Flag has an effect outside the model, making the unit `Unread`.
    pub(crate) unread: Option<FlagUnread>,
}

impl FlagEffects {
    const EMPTY: Self = Self {
        recursive: FlagToggle::Unchanged,
        follows_links_at_root: FlagToggle::Unchanged,
        follows_links_below_root: FlagToggle::Unchanged,
        provides_pattern: false,
        filter: None,
        filter_case_insensitive: FlagToggle::Unchanged,
        runs_code: false,
        unread: None,
    };

    /// Returns `true` iff at least one modeled effect or `unread` trigger is
    /// stated on this `FlagEffects`.
    pub(crate) const fn has_any_effect(self) -> bool {
        !matches!(self.recursive, FlagToggle::Unchanged)
            || !matches!(self.follows_links_at_root, FlagToggle::Unchanged)
            || !matches!(self.follows_links_below_root, FlagToggle::Unchanged)
            || self.provides_pattern
            || self.filter.is_some()
            || !matches!(self.filter_case_insensitive, FlagToggle::Unchanged)
            || self.runs_code
            || self.unread.is_some()
    }
}

/// Explicit policy-effect declaration on every [`FlagSpec`] (contract §6, Rule A).
///
/// Every flag in a `Search`, `Wrapper`, or `NestedShell` flag table must either
/// state at least one effect in `Applied(FlagEffects)` or explicitly declare
/// `None { reason }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Flag has no policy-relevant effect, with a short reviewable explanation.
    None { reason: &'static str },
    /// Flag triggers one or more explicit effects in [`FlagEffects`].
    Applied(FlagEffects),
}

/// One flag listed on a `Search`, `Wrapper`, or `NestedShell` row (contract §4, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FlagSpec {
    pub(crate) short: Option<char>,
    /// Long option name without leading `--`.
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

/// Which argv words an Allow rule must name to cover `Unread (program)`
/// (contract §4, §7, §11 item 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProgramPin {
    /// Rule must name `[tool, subcommand]` (e.g. `["cargo", "test"]`).
    CommandAndSubcommand,
    /// Rule must name `[tool, operand0]` or `[tool, subcommand, operand0]`
    /// (e.g. `["python", "build.py"]`, `["sh", "x.sh"]`, `["pipx", "run", "pkg"]`).
    FirstOperand,
    /// Rule must name the program-file flag and its value (e.g.
    /// `["sed", "-f", "edit.sed"]`, `["awk", "-f", "prog.awk"]`).
    FlagValue,
}

/// Single-line switch controlling whether Home auto-allow permits followed-link
/// searches (`rg -L`, `grep -R`) when the bounded link walk stays within budget
/// and reaches no protected path (§10 row 15; §11 item 15, D-139).
pub(crate) const ALLOW_HOME_FOLLOWED_SEARCH: bool = true;

/// Home read-only eligibility mirrored verbatim from
/// `atrium_protocol::policy::HOME_READONLY_COMMANDS` (contract §6).
///
/// Home eligibility matches on the command word **as written** (`cat`
/// qualifies; `/bin/cat` and `CAT` do not). `forbidden` and `only` restrict
/// **Home read-only eligibility only** (`home_readonly_eligible = false`, so
/// Home asks as on `dev`); they never by themselves make the unit `Unread`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomeReadOnlySpec {
    pub(crate) prefix: &'static [&'static str],
    /// Flags that disqualify the unit from Home read-only auto-allow only.
    pub(crate) forbidden: &'static [&'static str],
    /// When `Some`, Home read-only auto-allow permits only these exact args
    /// (e.g. `git branch` listing flags).
    pub(crate) only: Option<&'static [&'static str]>,
    pub(crate) recursion: Recursion,
    /// Whether followed-symlink searches (`rg -L`, `grep -R`) are eligible for
    /// Home Allow when the link walk is within budget (§10 row 15; §11 item
    /// 15, D-139).
    pub(crate) allow_followed_search: bool,
}

// ---------------------------------------------------------------------------
// Six D-141 row-kind payloads (each holds only the fields its kind needs)
// ---------------------------------------------------------------------------

/// Payload for [`ToolRowKind::HomeReadOnly`] and
/// [`SubcommandKind::HomeReadOnly`] (D-141 kind 1).
///
/// `AnyExceptForbidden` with `dev`'s `forbidden_flag` matching mirrored
/// exactly. Every word is a candidate `Path` (`ReadWrite`, no operand
/// relaxations), and a forbidden flag only removes Home eligibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomeReadOnlyRow {
    pub(crate) home_readonly: HomeReadOnlySpec,
    /// `true` on `git log`/`diff`/`show` for CR-12 committed-credential
    /// name-selection checks (§6).
    pub(crate) git_selects_by_name: bool,
    /// `true` on `git` read subcommands that accept pathspecs (`status`,
    /// `log`, `diff`, `show`, `ls-files`), where active wildcard pathspecs
    /// become `Unread(GitWildcardPathspec)` (§6, §11 item 10).
    pub(crate) git_wildcard_pathspec: bool,
}

/// Whether a [`SearchRow`] searches recursively by default or with flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchRecursion {
    /// Always recursive (`rg`, `rgrep`).
    Always,
    /// Recursive when a `recursive: Enable` flag (`-r`, `-R`, `-d`) is active
    /// (`grep`, `egrep`, `fgrep`, `zgrep`, `ggrep`).
    WithFlag,
}

/// Payload for [`ToolRowKind::Search`] (D-141 kind 2): `rg` and the `grep`
/// family.
///
/// Positional `[0]` is `ValueRole::Pattern` unless a `provides_pattern` flag
/// (`-e`, `-f`, `--files`, `--type-list`) is present; remaining positional
/// operands and `-f`/`--file` values are `ValueRole::Path` (`Read`). Any flag
/// absent from `flags` makes the unit `Unread(UnknownFlag)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchRow {
    pub(crate) flags: &'static [FlagSpec],
    pub(crate) filter_dialect: FilterDialect,
    pub(crate) recursive: SearchRecursion,
    pub(crate) links: LinkPolicy,
    pub(crate) home_readonly: Option<HomeReadOnlySpec>,
}

/// Payload for [`ToolRowKind::Wrapper`] (D-141 kind 3a): §4's 6 closed wrapper
/// forms. Any unlisted flag or form makes the unit `Unread`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WrapperRow {
    pub(crate) wrapper: WrapperKind,
    pub(crate) flags: &'static [FlagSpec],
    /// Whether bare `--` is permitted before the wrapped command (`command [--]`).
    pub(crate) allows_double_dash: bool,
    /// Required wrapper-owned positional operands before the wrapped command
    /// (`timeout`: `[ValueRole::Text]` for duration `D`).
    pub(crate) positional_operands: &'static [ValueRole],
}

/// Payload for [`ToolRowKind::NestedShell`] (D-141 kind 3b): `sh`, `bash`,
/// `dash` in §4's closed `-c SCRIPT` form (plus pinnable script file `sh x.sh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NestedShellRow {
    pub(crate) flags: &'static [FlagSpec],
    /// Program-pinning condition for a script file (`sh x.sh` →
    /// `Unread (program)`; §4, §7, §11 item 8).
    pub(crate) script_pin: ProgramPin,
    /// Bare shell prefix (`["sh"]`, `["bash"]`, `["dash"]`) is Invalid at
    /// creation (§7, §11 item 4).
    pub(crate) code_runner_no_pin: bool,
}

/// Where inline code appears on a [`CodeRunnerRow`] (contract §4, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InlineCodeSpec {
    /// No inline-code form (program file or project build only, e.g. `source`,
    /// `.`, `npx`, `cargo`, `pipx`).
    None,
    /// Inline code is passed via one of these value-taking flags (e.g.
    /// `python -c`/`-m`, `node -e`/`--eval`/`-p`/`--print`, `perl -e`/`-E`,
    /// `ruby -e`).
    Flags(&'static [&'static str]),
    /// Inline code is in positional operand `index` (e.g. `awk`'s `[0]`,
    /// unless a program-file flag like `-f`/`--file` is present).
    Operand(usize),
    /// Inline code / command to execute spans positional operands starting at
    /// `start` (`0` for `eval`/`xargs`/`parallel`/`nsenter`/..., `1` after
    /// target/host for `ssh`/`docker`/`kubectl`/...).
    OperandsFrom(usize),
}

/// Program-file or project-build pinning specification on a [`CodeRunnerRow`]
/// (`Unread (program)`, §4, §7, §11 item 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProgramPinSpec {
    pub(crate) pin: ProgramPin,
    /// Flag spelling(s) that supply the pinned program file when
    /// `pin == ProgramPin::FlagValue` (e.g. `awk -f`/`--file`). Empty for
    /// positional/subcommand pins.
    pub(crate) flags: &'static [&'static str],
}

/// Payload for [`ToolRowKind::CodeRunner`] and [`SubcommandKind::CodeRunner`]
/// (D-141 kind 4): always `Unread`.
///
/// `inline_code` and `program_pin` locate inline code or a pinned program file
/// without relaxing candidate-path hard-deny/store checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodeRunnerRow {
    pub(crate) inline_code: InlineCodeSpec,
    pub(crate) program_pin: Option<ProgramPinSpec>,
    /// `true` if an Allow prefix ending at this tool/subcommand is Invalid at
    /// creation (§7, §11 item 4).
    pub(crate) code_runner_no_pin: bool,
    /// Subcommands for multi-subcommand code runners (`cargo`, `pipx`).
    pub(crate) subcommands: &'static [SubcommandSpec],
}

/// One data-only trigger on a [`ConditionalRunnerRow`] (D-141 kind 5).
///
/// Triggers carry no flag arities and no operand roles; P3 applies them
/// (matching flag triggers against exact, `--flag=value`, and short-flag
/// cluster / attached-value forms as appropriate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunnerTrigger {
    /// Presence of this flag makes the unit `Unread(CodeRunnerInlineCode)`
    /// (e.g. `find`: `-exec`/`-execdir`/`-ok`/`-okdir`; `git`: `-c`,
    /// `--config-env`, `--exec-path`).
    Flag(&'static str),
    /// This subcommand unconditionally makes the unit
    /// `Unread(CodeRunnerInlineCode)` (e.g. `git merge-index`,
    /// `git filter-branch`).
    Subcommand(&'static str),
    /// This subcommand + subarg makes the unit `Unread(CodeRunnerInlineCode)`
    /// (e.g. `git bisect run`, `git submodule foreach`).
    SubcommandWithSubarg {
        subcommand: &'static str,
        subarg: &'static str,
    },
    /// This subcommand + flag makes the unit `Unread(CodeRunnerInlineCode)`
    /// (e.g. `git rebase -x`/`--exec`, `git difftool`/`mergetool`
    /// `-x`/`--tool-cmd`, `git diff --ext-diff`, `git push`
    /// `--receive-pack`/`--exec`).
    SubcommandWithFlag {
        subcommand: &'static str,
        flag: &'static str,
    },
    /// An inline script containing any of these single-letter commands makes
    /// the unit `Unread(CodeRunnerInlineCode)` (e.g. `sed`: `e`, `w`, `r`).
    ScriptCommands(&'static [char]),
    /// A flag naming a program file makes the unit pinnable
    /// `Unread (program)` (§11 item 8; e.g. `sed -f`/`--file`).
    ProgramFileFlag { flag: &'static str, pin: ProgramPin },
}

/// Payload for [`ToolRowKind::ConditionalRunner`] (D-141 kind 5): `find`,
/// `git`, `sed`.
///
/// Without a matching trigger (or modeled subcommand on `git`), the unit
/// receives unknown-tool treatment (every word a candidate `Path` with
/// `ReadWrite` access, possible writer, never Home-eligible).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConditionalRunnerRow {
    pub(crate) triggers: &'static [RunnerTrigger],
    /// `true` if an Allow prefix ending at `[tool]` is Invalid at creation
    /// (§7, §11 item 4).
    pub(crate) code_runner_no_pin: bool,
    /// Modeled subcommands (`git`: the 7 `HomeReadOnly` subcommands and 2
    /// `DerivedDestinationWriter` subcommands).
    pub(crate) subcommands: &'static [SubcommandSpec],
}

/// Trigger condition for a [`DerivedDestinationWriterRow`] (D-141 kind 6; §5,
/// §11 item 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DerivedDestinationTrigger {
    /// Unconditionally writes to derived destinations (`rsync`, `unzip`,
    /// `git clone`, `git checkout`).
    Always,
    /// Writes to derived destinations when any source operand resolves to a
    /// directory (`mv` of a directory; P2 resolves against `PathResolver`, §5).
    WhenSourceIsDir,
    /// Writes to derived destinations when any listed flag is present (short
    /// flags inside clusters, exact/prefixed long options, and for `tar`
    /// old-style bundled key letters such as `tar xf`). Without a trigger
    /// flag (plain `cp a b`, `tar -c`), the unit receives unknown-tool
    /// treatment.
    Flags(&'static [&'static str]),
}

/// Payload for [`ToolRowKind::DerivedDestinationWriter`] and
/// [`SubcommandKind::DerivedDestinationWriter`] (D-141 kind 6).
///
/// Every word remains a candidate `Path` (`ReadWrite`), so a destination hit
/// stands; when triggered, the writer itself is `Unread(DerivedDestinations)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DerivedDestinationWriterRow {
    pub(crate) trigger: DerivedDestinationTrigger,
}

/// Row kind for a subcommand in [`SubcommandSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubcommandKind {
    HomeReadOnly(HomeReadOnlyRow),
    CodeRunner(CodeRunnerRow),
    DerivedDestinationWriter(DerivedDestinationWriterRow),
}

impl SubcommandKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::HomeReadOnly(_) => "HomeReadOnly",
            Self::CodeRunner(_) => "CodeRunner",
            Self::DerivedDestinationWriter(_) => "DerivedDestinationWriter",
        }
    }
}

/// Normative row for a subcommand of a multi-subcommand tool (`git`, `cargo`,
/// `pipx`; contract §6, D-141).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SubcommandSpec {
    pub(crate) name: &'static str,
    pub(crate) kind: SubcommandKind,
    /// Contract §11 item(s) when this row changes `dev`'s default behaviour.
    pub(crate) changes_default: Option<&'static str>,
}

impl SubcommandSpec {
    pub(crate) const fn home_readonly(&self) -> Option<HomeReadOnlySpec> {
        match self.kind {
            SubcommandKind::HomeReadOnly(r) => Some(r.home_readonly),
            SubcommandKind::CodeRunner(_) | SubcommandKind::DerivedDestinationWriter(_) => None,
        }
    }

    pub(crate) const fn code_runner_no_pin(&self) -> bool {
        match self.kind {
            SubcommandKind::CodeRunner(r) => r.code_runner_no_pin,
            SubcommandKind::HomeReadOnly(_) | SubcommandKind::DerivedDestinationWriter(_) => false,
        }
    }
}

/// D-141 row kind enum for [`ToolSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolRowKind {
    HomeReadOnly(HomeReadOnlyRow),
    Search(SearchRow),
    Wrapper(WrapperRow),
    NestedShell(NestedShellRow),
    CodeRunner(CodeRunnerRow),
    ConditionalRunner(ConditionalRunnerRow),
    DerivedDestinationWriter(DerivedDestinationWriterRow),
}

impl ToolRowKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::HomeReadOnly(_) => "HomeReadOnly",
            Self::Search(_) => "Search",
            Self::Wrapper(_) => "Wrapper",
            Self::NestedShell(_) => "NestedShell",
            Self::CodeRunner(_) => "CodeRunner",
            Self::ConditionalRunner(_) => "ConditionalRunner",
            Self::DerivedDestinationWriter(_) => "DerivedDestinationWriter",
        }
    }
}

/// Normative row for one tool in §6's table (contract §6, D-141).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ToolSpec {
    /// Folded (Unicode lowercase + CR-19) tool name.
    pub(crate) name: &'static str,
    pub(crate) platforms: Platforms,
    /// D-141 row kind and kind-specific fields.
    pub(crate) kind: ToolRowKind,
    /// Contract §11 item(s) when this row changes `dev`'s default behaviour.
    pub(crate) changes_default: Option<&'static str>,
}

impl ToolSpec {
    /// Returns this row's listed flag table (`Search`, `Wrapper`,
    /// `NestedShell`), or `&[]` for kinds that have no flag table.
    pub(crate) const fn flags(&self) -> &'static [FlagSpec] {
        match self.kind {
            ToolRowKind::Search(r) => r.flags,
            ToolRowKind::Wrapper(r) => r.flags,
            ToolRowKind::NestedShell(r) => r.flags,
            ToolRowKind::HomeReadOnly(_)
            | ToolRowKind::CodeRunner(_)
            | ToolRowKind::ConditionalRunner(_)
            | ToolRowKind::DerivedDestinationWriter(_) => &[],
        }
    }

    /// Returns this row's subcommand table (`git`, `cargo`, `pipx`), or `&[]`.
    pub(crate) const fn subcommands(&self) -> &'static [SubcommandSpec] {
        match self.kind {
            ToolRowKind::CodeRunner(r) => r.subcommands,
            ToolRowKind::ConditionalRunner(r) => r.subcommands,
            ToolRowKind::HomeReadOnly(_)
            | ToolRowKind::Search(_)
            | ToolRowKind::Wrapper(_)
            | ToolRowKind::NestedShell(_)
            | ToolRowKind::DerivedDestinationWriter(_) => &[],
        }
    }

    /// Returns the §4 [`WrapperKind`] if this row is a `Wrapper`.
    pub(crate) const fn wrapper(&self) -> Option<WrapperKind> {
        match self.kind {
            ToolRowKind::Wrapper(r) => Some(r.wrapper),
            _ => None,
        }
    }

    /// Returns `true` iff this row is a modeled §4 nested shell.
    pub(crate) const fn is_nested_shell(&self) -> bool {
        matches!(self.kind, ToolRowKind::NestedShell(_))
    }

    /// Returns this row's [`HomeReadOnlySpec`], if any.
    pub(crate) const fn home_readonly(&self) -> Option<HomeReadOnlySpec> {
        match self.kind {
            ToolRowKind::HomeReadOnly(r) => Some(r.home_readonly),
            ToolRowKind::Search(r) => r.home_readonly,
            _ => None,
        }
    }

    /// Returns `true` iff an Allow prefix ending at `[name]` is Invalid at
    /// creation (§7, §11 item 4).
    pub(crate) const fn code_runner_no_pin(&self) -> bool {
        match self.kind {
            ToolRowKind::NestedShell(r) => r.code_runner_no_pin,
            ToolRowKind::CodeRunner(r) => r.code_runner_no_pin,
            ToolRowKind::ConditionalRunner(r) => r.code_runner_no_pin,
            _ => false,
        }
    }
}

/// Tool or reserved word known to `dev` that the rev 4 model deliberately
/// leaves outside the model, making its unit `Unread` (contract §3, §4, §5).
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

/// Tool listed in `dev` (`WRITERS`, `READERS`, `DOWNLOADERS`, `PRIV`) that
/// D-141 deliberately omits from [`TOOLS`] so it receives §4's unknown-tool
/// treatment (every word a candidate `Path` with `ReadWrite` access, possible
/// writer, never Home-eligible).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UnknownByDesignEntry {
    pub(crate) name: &'static str,
    pub(crate) dev_list: &'static str,
    pub(crate) reason: &'static str,
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

const LINKS_ROOT_ONLY: LinkPolicy = LinkPolicy {
    at_root: true,
    below_root: false,
};

const GIT_FORBIDDEN: &[&str] = &[
    "--output",
    "--ext-diff",
    "--exec-path",
    "--config-env",
    "-c",
];

const fn home_ro_tool(
    name: &'static str,
    prefix: &'static [&'static str],
    forbidden: &'static [&'static str],
) -> ToolSpec {
    ToolSpec {
        name,
        platforms: Platforms::Both,
        kind: ToolRowKind::HomeReadOnly(HomeReadOnlyRow {
            home_readonly: HomeReadOnlySpec {
                prefix,
                forbidden,
                only: None,
                recursion: Recursion::Never,
                allow_followed_search: false,
            },
            git_selects_by_name: false,
            git_wildcard_pathspec: false,
        }),
        changes_default: None,
    }
}

const fn home_git_sub(
    name: &'static str,
    prefix: &'static [&'static str],
    only: Option<&'static [&'static str]>,
    git_selects_by_name: bool,
    git_wildcard_pathspec: bool,
    changes_default: Option<&'static str>,
) -> SubcommandSpec {
    SubcommandSpec {
        name,
        kind: SubcommandKind::HomeReadOnly(HomeReadOnlyRow {
            home_readonly: HomeReadOnlySpec {
                prefix,
                forbidden: GIT_FORBIDDEN,
                only,
                recursion: Recursion::Never,
                allow_followed_search: false,
            },
            git_selects_by_name,
            git_wildcard_pathspec,
        }),
        changes_default,
    }
}

const fn direct_code_runner(name: &'static str, platforms: Platforms) -> ToolSpec {
    ToolSpec {
        name,
        platforms,
        kind: ToolRowKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::OperandsFrom(0),
            program_pin: None,
            code_runner_no_pin: true,
            subcommands: &[],
        }),
        changes_default: Some("§11 item 4"),
    }
}

const fn target_then_code_runner(name: &'static str, platforms: Platforms) -> ToolSpec {
    ToolSpec {
        name,
        platforms,
        kind: ToolRowKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::OperandsFrom(1),
            program_pin: None,
            code_runner_no_pin: true,
            subcommands: &[],
        }),
        changes_default: Some("§11 item 4"),
    }
}

const fn interpreter_code_runner(
    name: &'static str,
    inline_flags: &'static [&'static str],
) -> ToolSpec {
    ToolSpec {
        name,
        platforms: Platforms::Both,
        kind: ToolRowKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::Flags(inline_flags),
            program_pin: Some(ProgramPinSpec {
                pin: ProgramPin::FirstOperand,
                flags: &[],
            }),
            code_runner_no_pin: true,
            subcommands: &[],
        }),
        changes_default: Some("§11 item 4; §11 item 8"),
    }
}

const fn awk_code_runner(name: &'static str) -> ToolSpec {
    ToolSpec {
        name,
        platforms: Platforms::Both,
        kind: ToolRowKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::Operand(0),
            program_pin: Some(ProgramPinSpec {
                pin: ProgramPin::FlagValue,
                flags: &["-f", "--file"],
            }),
            code_runner_no_pin: true,
            subcommands: &[],
        }),
        changes_default: Some("§11 item 4; §11 item 8"),
    }
}

const fn program_only_code_runner(name: &'static str) -> ToolSpec {
    ToolSpec {
        name,
        platforms: Platforms::Both,
        kind: ToolRowKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::None,
            program_pin: Some(ProgramPinSpec {
                pin: ProgramPin::FirstOperand,
                flags: &[],
            }),
            code_runner_no_pin: true,
            subcommands: &[],
        }),
        changes_default: Some("§11 item 4; §11 item 8"),
    }
}

const fn grep_sibling(name: &'static str, recursive: SearchRecursion) -> ToolSpec {
    ToolSpec {
        name,
        platforms: Platforms::Both,
        kind: ToolRowKind::Search(SearchRow {
            flags: GREP_FLAGS,
            filter_dialect: FilterDialect::GrepInclude,
            recursive,
            links: LINKS_ROOT_ONLY,
            home_readonly: None,
        }),
        changes_default: Some("§11 item 2; §11 item 9"),
    }
}

// ---------------------------------------------------------------------------
// Faithfully copied flag tables (`Search`, `Wrapper`, `NestedShell` only)
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
            provides_pattern: true,
            ..FlagEffects::EMPTY
        },
    ),
    // Unsupported file-type / depth / size filters (§5, Rule A)
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
    // Unsupported grep filters (§5, Rule A)
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

// ---------------------------------------------------------------------------
// Subcommand tables (`git`, `cargo`, `pipx`)
// ---------------------------------------------------------------------------

const GIT_SUBCOMMANDS: &[SubcommandSpec] = &[
    // Home read-only git subcommands (the exact 7 in HOME_READONLY_COMMANDS).
    // No operand relaxations: every word is a candidate Path (ReadWrite), so
    // `git diff --no-index a b` checks both `a` and `b` as paths.
    home_git_sub(
        "status",
        &["git", "status"],
        None,
        false,
        true,
        Some("§11 item 10"),
    ),
    home_git_sub(
        "log",
        &["git", "log"],
        None,
        true,
        true,
        Some("§11 item 10"),
    ),
    home_git_sub(
        "diff",
        &["git", "diff"],
        None,
        true,
        true,
        Some("§11 item 10"),
    ),
    home_git_sub(
        "show",
        &["git", "show"],
        None,
        true,
        true,
        Some("§11 item 10"),
    ),
    home_git_sub(
        "branch",
        &["git", "branch"],
        Some(&[
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
        false,
        false,
        None,
    ),
    home_git_sub("rev-parse", &["git", "rev-parse"], None, false, false, None),
    home_git_sub(
        "ls-files",
        &["git", "ls-files"],
        None,
        false,
        true,
        Some("§11 item 10"),
    ),
    // Derived-destination writers (§5, §11 item 10)
    SubcommandSpec {
        name: "clone",
        kind: SubcommandKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
            trigger: DerivedDestinationTrigger::Always,
        }),
        changes_default: Some("§11 item 10"),
    },
    SubcommandSpec {
        name: "checkout",
        kind: SubcommandKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
            trigger: DerivedDestinationTrigger::Always,
        }),
        changes_default: Some("§11 item 10"),
    },
];

const fn cargo_build_subcommand(name: &'static str) -> SubcommandSpec {
    SubcommandSpec {
        name,
        kind: SubcommandKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::None,
            program_pin: Some(ProgramPinSpec {
                pin: ProgramPin::CommandAndSubcommand,
                flags: &[],
            }),
            code_runner_no_pin: false,
            subcommands: &[],
        }),
        changes_default: Some("§11 item 8"),
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
    name: "run",
    kind: SubcommandKind::CodeRunner(CodeRunnerRow {
        inline_code: InlineCodeSpec::None,
        program_pin: Some(ProgramPinSpec {
            pin: ProgramPin::FirstOperand,
            flags: &[],
        }),
        code_runner_no_pin: true,
        subcommands: &[],
    }),
    changes_default: Some("§11 item 4; §11 item 8"),
}];

// ---------------------------------------------------------------------------
// Conditional-runner trigger lists (data only; P3 applies them)
// ---------------------------------------------------------------------------

const FIND_TRIGGERS: &[RunnerTrigger] = &[
    RunnerTrigger::Flag("-exec"),
    RunnerTrigger::Flag("-execdir"),
    RunnerTrigger::Flag("-ok"),
    RunnerTrigger::Flag("-okdir"),
];

const GIT_TRIGGERS: &[RunnerTrigger] = &[
    RunnerTrigger::Flag("-c"),
    RunnerTrigger::Flag("--config-env"),
    RunnerTrigger::Flag("--exec-path"),
    RunnerTrigger::Subcommand("merge-index"),
    RunnerTrigger::Subcommand("filter-branch"),
    RunnerTrigger::SubcommandWithSubarg {
        subcommand: "bisect",
        subarg: "run",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "rebase",
        flag: "-x",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "rebase",
        flag: "--exec",
    },
    RunnerTrigger::SubcommandWithSubarg {
        subcommand: "submodule",
        subarg: "foreach",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "difftool",
        flag: "-x",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "difftool",
        flag: "--tool-cmd",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "mergetool",
        flag: "-x",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "mergetool",
        flag: "--tool-cmd",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "diff",
        flag: "--ext-diff",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "push",
        flag: "--receive-pack",
    },
    RunnerTrigger::SubcommandWithFlag {
        subcommand: "push",
        flag: "--exec",
    },
];

const SED_TRIGGERS: &[RunnerTrigger] = &[
    RunnerTrigger::ScriptCommands(&['e', 'w', 'r']),
    RunnerTrigger::ProgramFileFlag {
        flag: "-f",
        pin: ProgramPin::FlagValue,
    },
    RunnerTrigger::ProgramFileFlag {
        flag: "--file",
        pin: ProgramPin::FlagValue,
    },
];

// ---------------------------------------------------------------------------
// Normative tool table (§6, D-141)
// ---------------------------------------------------------------------------

pub(crate) const TOOLS: &[ToolSpec] = &[
    // --- 1. Home read-only tools (`HOME_READONLY_COMMANDS` minus `rg`/`grep`/`git *`) ---
    home_ro_tool("ls", &["ls"], &[]),
    home_ro_tool("pwd", &["pwd"], &[]),
    home_ro_tool("cat", &["cat"], &[]),
    home_ro_tool("head", &["head"], &[]),
    home_ro_tool("tail", &["tail"], &[]),
    home_ro_tool("wc", &["wc"], &[]),
    home_ro_tool(
        "file",
        &["file"],
        &[
            "-C",
            "--compile",
            "-m",
            "--magic-file",
            "-f",
            "--files-from",
        ],
    ),
    home_ro_tool("stat", &["stat"], &[]),
    home_ro_tool("du", &["du"], &[]),
    home_ro_tool("df", &["df"], &[]),
    home_ro_tool("which", &["which"], &[]),
    home_ro_tool("uname", &["uname"], &[]),
    home_ro_tool("date", &["date"], &["-s", "--set", "-f", "--file"]),
    home_ro_tool("whoami", &["whoami"], &[]),
    home_ro_tool("tree", &["tree"], &["-o", "--output", "-R"]),
    // --- 2. Search tools (`rg` and the `grep` family) ---
    ToolSpec {
        name: "rg",
        platforms: Platforms::Both,
        kind: ToolRowKind::Search(SearchRow {
            flags: RG_FLAGS,
            filter_dialect: FilterDialect::RgGlob,
            recursive: SearchRecursion::Always,
            links: LINKS_ROOT_ONLY,
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
        }),
        changes_default: Some("§11 item 2; §11 item 9; §11 item 15"),
    },
    ToolSpec {
        name: "grep",
        platforms: Platforms::Both,
        kind: ToolRowKind::Search(SearchRow {
            flags: GREP_FLAGS,
            filter_dialect: FilterDialect::GrepInclude,
            recursive: SearchRecursion::WithFlag,
            links: LINKS_ROOT_ONLY,
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
        }),
        changes_default: Some("§11 item 2; §11 item 9; §11 item 15"),
    },
    grep_sibling("egrep", SearchRecursion::WithFlag),
    grep_sibling("fgrep", SearchRecursion::WithFlag),
    grep_sibling("zgrep", SearchRecursion::WithFlag),
    grep_sibling("ggrep", SearchRecursion::WithFlag),
    grep_sibling("rgrep", SearchRecursion::Always),
    // --- 3a. Wrappers (§4 exact closed forms) ---
    ToolSpec {
        name: "command",
        platforms: Platforms::Both,
        kind: ToolRowKind::Wrapper(WrapperRow {
            wrapper: WrapperKind::Command,
            flags: &[],
            allows_double_dash: true,
            positional_operands: &[],
        }),
        changes_default: Some("§11 item 10"),
    },
    ToolSpec {
        name: "exec",
        platforms: Platforms::Both,
        kind: ToolRowKind::Wrapper(WrapperRow {
            wrapper: WrapperKind::Exec,
            flags: &[],
            allows_double_dash: false,
            positional_operands: &[],
        }),
        changes_default: Some("§11 item 10"),
    },
    ToolSpec {
        name: "nice",
        platforms: Platforms::Both,
        kind: ToolRowKind::Wrapper(WrapperRow {
            wrapper: WrapperKind::Nice,
            flags: NICE_FLAGS,
            allows_double_dash: false,
            positional_operands: &[],
        }),
        changes_default: Some("§11 item 9; §11 item 10"),
    },
    ToolSpec {
        name: "timeout",
        platforms: Platforms::Both,
        kind: ToolRowKind::Wrapper(WrapperRow {
            wrapper: WrapperKind::Timeout,
            flags: TIMEOUT_FLAGS,
            allows_double_dash: false,
            positional_operands: &[ValueRole::Text],
        }),
        changes_default: Some("§11 item 9; §11 item 10"),
    },
    ToolSpec {
        name: "time",
        platforms: Platforms::Both,
        kind: ToolRowKind::Wrapper(WrapperRow {
            wrapper: WrapperKind::Time,
            flags: &[],
            allows_double_dash: false,
            positional_operands: &[],
        }),
        changes_default: Some("§11 item 10"),
    },
    ToolSpec {
        name: "env",
        platforms: Platforms::Both,
        kind: ToolRowKind::Wrapper(WrapperRow {
            wrapper: WrapperKind::Env,
            flags: &[],
            allows_double_dash: false,
            positional_operands: &[],
        }),
        changes_default: Some("§11 item 2; §11 item 10"),
    },
    // --- 3b. Nested shells (§4 exact closed forms + script-file Unread (program)) ---
    ToolSpec {
        name: "sh",
        platforms: Platforms::Both,
        kind: ToolRowKind::NestedShell(NestedShellRow {
            flags: MODELED_SHELL_FLAGS,
            script_pin: ProgramPin::FirstOperand,
            code_runner_no_pin: true,
        }),
        changes_default: Some("§11 item 2; §11 item 4; §11 item 8"),
    },
    ToolSpec {
        name: "bash",
        platforms: Platforms::Both,
        kind: ToolRowKind::NestedShell(NestedShellRow {
            flags: MODELED_SHELL_FLAGS,
            script_pin: ProgramPin::FirstOperand,
            code_runner_no_pin: true,
        }),
        changes_default: Some("§11 item 2; §11 item 4; §11 item 8"),
    },
    ToolSpec {
        name: "dash",
        platforms: Platforms::Both,
        kind: ToolRowKind::NestedShell(NestedShellRow {
            flags: MODELED_SHELL_FLAGS,
            script_pin: ProgramPin::FirstOperand,
            code_runner_no_pin: true,
        }),
        changes_default: Some("§11 item 2; §11 item 4; §11 item 8"),
    },
    // --- 4. Code runners (§4, §7: always Unread; inline code or pinned program) ---
    awk_code_runner("awk"),
    awk_code_runner("gawk"),
    awk_code_runner("mawk"),
    interpreter_code_runner("perl", &["-e", "-E"]),
    interpreter_code_runner("python", &["-c", "-m"]),
    interpreter_code_runner("python3", &["-c", "-m"]),
    interpreter_code_runner("node", &["-e", "--eval", "-p", "--print"]),
    interpreter_code_runner("ruby", &["-e"]),
    direct_code_runner("eval", Platforms::Both),
    direct_code_runner("xargs", Platforms::Both),
    program_only_code_runner("source"),
    program_only_code_runner("."),
    program_only_code_runner("npx"),
    ToolSpec {
        name: "cargo",
        platforms: Platforms::Both,
        kind: ToolRowKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::None,
            program_pin: Some(ProgramPinSpec {
                pin: ProgramPin::CommandAndSubcommand,
                flags: &[],
            }),
            code_runner_no_pin: true,
            subcommands: CARGO_SUBCOMMANDS,
        }),
        changes_default: Some("§11 item 4; §11 item 8"),
    },
    ToolSpec {
        name: "pipx",
        platforms: Platforms::Both,
        kind: ToolRowKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::None,
            program_pin: None,
            code_runner_no_pin: true,
            subcommands: PIPX_SUBCOMMANDS,
        }),
        changes_default: Some("§11 item 4"),
    },
    // `shell::COMMAND_RUNNERS` (all 29 entries)
    target_then_code_runner("ssh", Platforms::Both),
    target_then_code_runner("mosh", Platforms::Both),
    target_then_code_runner("autossh", Platforms::Both),
    direct_code_runner("nsenter", Platforms::Linux),
    direct_code_runner("unshare", Platforms::Linux),
    target_then_code_runner("docker", Platforms::Both),
    target_then_code_runner("podman", Platforms::Both),
    target_then_code_runner("nerdctl", Platforms::Both),
    target_then_code_runner("kubectl", Platforms::Both),
    target_then_code_runner("oc", Platforms::Both),
    target_then_code_runner("lxc", Platforms::Both),
    target_then_code_runner("incus", Platforms::Both),
    target_then_code_runner("machinectl", Platforms::Linux),
    direct_code_runner("systemd-run", Platforms::Linux),
    direct_code_runner("systemd-nspawn", Platforms::Linux),
    direct_code_runner("firejail", Platforms::Linux),
    direct_code_runner("bwrap", Platforms::Linux),
    direct_code_runner("flatpak-spawn", Platforms::Linux),
    target_then_code_runner("toolbox", Platforms::Both),
    target_then_code_runner("distrobox", Platforms::Both),
    target_then_code_runner("distrobox-enter", Platforms::Both),
    target_then_code_runner("multipass", Platforms::Both),
    target_then_code_runner("vagrant", Platforms::Both),
    target_then_code_runner("limactl", Platforms::Both),
    target_then_code_runner("colima", Platforms::Both),
    // Also in `deny::PRIV` (always HardDeny `privilege-escalation` on dev and rev 4)
    ToolSpec {
        name: "su",
        platforms: Platforms::Both,
        kind: ToolRowKind::CodeRunner(CodeRunnerRow {
            inline_code: InlineCodeSpec::OperandsFrom(0),
            program_pin: None,
            code_runner_no_pin: true,
            subcommands: &[],
        }),
        changes_default: None,
    },
    target_then_code_runner("runuser", Platforms::Linux),
    direct_code_runner("setpriv", Platforms::Linux),
    direct_code_runner("parallel", Platforms::Both),
    // --- 5. Conditional runners (`find`, `git`, `sed`) ---
    ToolSpec {
        name: "find",
        platforms: Platforms::Both,
        kind: ToolRowKind::ConditionalRunner(ConditionalRunnerRow {
            triggers: FIND_TRIGGERS,
            code_runner_no_pin: true,
            subcommands: &[],
        }),
        changes_default: Some("§11 item 4"),
    },
    ToolSpec {
        name: "git",
        platforms: Platforms::Both,
        kind: ToolRowKind::ConditionalRunner(ConditionalRunnerRow {
            triggers: GIT_TRIGGERS,
            code_runner_no_pin: true,
            subcommands: GIT_SUBCOMMANDS,
        }),
        changes_default: Some("§11 item 4"),
    },
    ToolSpec {
        name: "sed",
        platforms: Platforms::Both,
        kind: ToolRowKind::ConditionalRunner(ConditionalRunnerRow {
            triggers: SED_TRIGGERS,
            code_runner_no_pin: true,
            subcommands: &[],
        }),
        changes_default: Some("§11 item 4; §11 item 8"),
    },
    // --- 6. Derived-destination writers (§5, §11 item 10; plus `git clone`/`checkout`) ---
    ToolSpec {
        name: "cp",
        platforms: Platforms::Both,
        kind: ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
            trigger: DerivedDestinationTrigger::Flags(&[
                "-R",
                "-r",
                "-a",
                "--recursive",
                "--archive",
            ]),
        }),
        changes_default: Some("§11 item 10"),
    },
    ToolSpec {
        name: "mv",
        platforms: Platforms::Both,
        kind: ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
            trigger: DerivedDestinationTrigger::WhenSourceIsDir,
        }),
        changes_default: Some("§11 item 10"),
    },
    ToolSpec {
        name: "rsync",
        platforms: Platforms::Both,
        kind: ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
            trigger: DerivedDestinationTrigger::Always,
        }),
        changes_default: Some("§11 item 10"),
    },
    ToolSpec {
        name: "tar",
        platforms: Platforms::Both,
        kind: ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
            trigger: DerivedDestinationTrigger::Flags(&["-x", "--extract", "--get"]),
        }),
        changes_default: Some("§11 item 10"),
    },
    ToolSpec {
        name: "unzip",
        platforms: Platforms::Both,
        kind: ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
            trigger: DerivedDestinationTrigger::Always,
        }),
        changes_default: Some("§11 item 10"),
    },
];

// ---------------------------------------------------------------------------
// Explicit OUTSIDE_MODEL list: shells, wrappers, reserved words, pushd/popd
// ---------------------------------------------------------------------------

const fn outside_wrapper(name: &'static str, platforms: Platforms) -> OutsideModelEntry {
    OutsideModelEntry {
        name,
        platforms,
        code_runner_no_pin: true,
        reason: "Wrapper outside §4's closed list; makes the unit Unread",
        contract_section: "§4",
        changes_default: Some("§11 item 4; §11 item 10"),
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
    // Shells in `shell::SHELLS` other than sh/bash/dash (§4)
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
    // Wrappers in `shell.rs` outside §4's closed list of 6
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
    // Reserved words in `shell::RESERVED` (§3)
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
    // Directory-stack builtins outside §5's `cd` model (`words::READERS`)
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
// Explicit UNKNOWN_BY_DESIGN list (D-141: tools `dev` lists that become §4
// unknown tools — every word a ReadWrite candidate path, possible writer,
// never Home-eligible)
// ---------------------------------------------------------------------------

const fn unknown_tool(
    name: &'static str,
    dev_list: &'static str,
    reason: &'static str,
) -> UnknownByDesignEntry {
    UnknownByDesignEntry {
        name,
        dev_list,
        reason,
    }
}

pub(crate) const UNKNOWN_BY_DESIGN: &[UnknownByDesignEntry] = &[
    // `words::WRITERS` not in `DerivedDestinationWriter` (18)
    unknown_tool(
        "rm",
        "words::WRITERS",
        "Not Home-eligible; recursive delete checked by hard-deny pass, otherwise unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "tee",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "touch",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "chmod",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "chown",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "chgrp",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "ln",
        "words::WRITERS",
        "Every word checked as ReadWrite candidate path (as on dev), so protected link targets remain hits (§4, §6)",
    ),
    unknown_tool(
        "truncate",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "install",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "mkdir",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "rmdir",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "unlink",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "shred",
        "words::WRITERS",
        "Writer with no derived destinations or code triggers; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "patch",
        "words::WRITERS",
        "Removed per D-141; unknown-tool treatment (possible writer, every word a ReadWrite candidate path)",
    ),
    unknown_tool(
        "ed",
        "words::WRITERS",
        "Removed per D-141; never Home-eligible, possible writer, every word a ReadWrite candidate path",
    ),
    unknown_tool(
        "ex",
        "words::WRITERS",
        "Removed per D-141; never Home-eligible, possible writer, every word a ReadWrite candidate path",
    ),
    unknown_tool(
        "wipefs",
        "words::WRITERS",
        "Checked by destructive-disk hard-deny pass; otherwise unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "blkdiscard",
        "words::WRITERS",
        "Checked by destructive-disk hard-deny pass; otherwise unknown-tool treatment (§4)",
    ),
    // `words::READERS` outside HomeReadOnly / Search / CodeRunner / ConditionalRunner / OUTSIDE_MODEL (44)
    unknown_tool(
        "less",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "more",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "ag",
        "words::READERS",
        "Outside rg/grep search family with no modeled filter dialect; unknown-tool treatment (D-141)",
    ),
    unknown_tool(
        "ack",
        "words::READERS",
        "Outside rg/grep search family with no modeled filter dialect; unknown-tool treatment (D-141)",
    ),
    unknown_tool(
        "diff",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "cmp",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "md5",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "md5sum",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "shasum",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "sha1sum",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "sha256sum",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "echo",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "printf",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "whereis",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "type",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "readlink",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "realpath",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "basename",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "dirname",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "test",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "[",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "sort",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "uniq",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "cut",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "tr",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "nl",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "od",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "xxd",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "hexdump",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "strings",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "bat",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "jq",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "column",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "fold",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "expand",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "true",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "false",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "id",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "cd",
        "words::READERS",
        "Handled directly by P2's §5 cwd tracker, not a D-141 tool-table row",
    ),
    unknown_tool(
        "open",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "spctl",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "mdls",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "mdfind",
        "words::READERS",
        "Not in HOME_READONLY_COMMANDS; unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "codesign",
        "words::READERS",
        "Removed per D-141; not in HOME_READONLY_COMMANDS, unknown-tool treatment (§4)",
    ),
    // `deny::DOWNLOADERS` (3)
    unknown_tool(
        "curl",
        "deny::DOWNLOADERS",
        "Checked by pipe-to-shell hard-deny pass; otherwise unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "wget",
        "deny::DOWNLOADERS",
        "Checked by pipe-to-shell hard-deny pass; otherwise unknown-tool treatment (§4)",
    ),
    unknown_tool(
        "fetch",
        "deny::DOWNLOADERS",
        "Checked by pipe-to-shell hard-deny pass; otherwise unknown-tool treatment (§4)",
    ),
    // `deny::PRIV` other than `su` (which is also in `shell::COMMAND_RUNNERS`) (5)
    unknown_tool(
        "sudo",
        "deny::PRIV",
        "Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise",
    ),
    unknown_tool(
        "sudoedit",
        "deny::PRIV",
        "Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise",
    ),
    unknown_tool(
        "doas",
        "deny::PRIV",
        "Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise",
    ),
    unknown_tool(
        "pkexec",
        "deny::PRIV",
        "Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise",
    ),
    unknown_tool(
        "run0",
        "deny::PRIV",
        "Always HardDeny (privilege-escalation) before tool classification; unknown-tool treatment otherwise",
    ),
];

// ---------------------------------------------------------------------------
// Plain table-read accessors (signatures frozen for P1/P2; argv parsing is P3)
// ---------------------------------------------------------------------------

/// Looks up a tool row by its CR-19 folded command name.
pub(crate) fn lookup(folded_name: &str) -> Option<&'static ToolSpec> {
    TOOLS.iter().find(|t| t.name == folded_name)
}

/// Looks up an explicit `OUTSIDE_MODEL` entry by its folded name.
pub(crate) fn lookup_outside_model(folded_name: &str) -> Option<&'static OutsideModelEntry> {
    OUTSIDE_MODEL.iter().find(|e| e.name == folded_name)
}

/// Looks up an explicit `UNKNOWN_BY_DESIGN` entry by its folded name.
pub(crate) fn lookup_unknown_by_design(folded_name: &str) -> Option<&'static UnknownByDesignEntry> {
    UNKNOWN_BY_DESIGN.iter().find(|e| e.name == folded_name)
}

/// Looks up a subcommand row on `tool` by exact subcommand name.
pub(crate) fn subcommand(tool: &'static ToolSpec, name: &str) -> Option<&'static SubcommandSpec> {
    tool.subcommands().iter().find(|s| s.name == name)
}

/// Looks up a short flag in `flags`.
pub(crate) fn flag_short(flags: &'static [FlagSpec], short: char) -> Option<&'static FlagSpec> {
    flags.iter().find(|f| f.short == Some(short))
}

/// Looks up a long flag (without leading `--`) in `flags`.
pub(crate) fn flag_long(flags: &'static [FlagSpec], long: &str) -> Option<&'static FlagSpec> {
    flags.iter().find(|f| f.long == Some(long))
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

/// Looks up a single flag token against `tool`'s flag table (`Search`,
/// `Wrapper`, `NestedShell`; returns `None` on kinds without a flag table).
pub(crate) fn flag_for_tool(tool: &'static ToolSpec, token: &str) -> Option<&'static FlagSpec> {
    flag(tool.flags(), token)
}

pub(crate) const fn all_tools() -> &'static [ToolSpec] {
    TOOLS
}

pub(crate) const fn outside_model() -> &'static [OutsideModelEntry] {
    OUTSIDE_MODEL
}

pub(crate) const fn unknown_by_design() -> &'static [UnknownByDesignEntry] {
    UNKNOWN_BY_DESIGN
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
    if let Some(unread) = effects.unread {
        eff.push(format!("unread({unread:?})"));
    }
    eff.join("; ")
}

fn fmt_flags_inline(flags: &[FlagSpec]) -> String {
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
            "{} listed flags ({bool_n} bool, {val_n} value; see flag table below)",
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
            let base = match f.kind {
                FlagKind::Bool => format!("`{name}`"),
                FlagKind::Value(r) => format!("`{name}` ({})", fmt_role(r)),
            };
            let eff = fmt_flag_effects(f.effect, false);
            if eff.is_empty() {
                base
            } else {
                format!("{base} [{eff}]")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn fmt_home_spec(h: HomeReadOnlySpec) -> String {
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

fn fmt_inline_code(spec: InlineCodeSpec) -> String {
    match spec {
        InlineCodeSpec::None => "—".to_string(),
        InlineCodeSpec::Flags(fs) => fs
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", "),
        InlineCodeSpec::Operand(idx) => format!("operand `[{idx}]`"),
        InlineCodeSpec::OperandsFrom(start) => format!("operands `[{start}..]`"),
    }
}

fn fmt_program_pin(spec: Option<ProgramPinSpec>) -> String {
    let Some(p) = spec else {
        return "—".to_string();
    };
    if p.flags.is_empty() {
        format!("{:?}", p.pin)
    } else {
        let fs = p
            .flags
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join("/");
        format!("{:?} ({fs})", p.pin)
    }
}

fn fmt_runner_trigger(t: RunnerTrigger) -> String {
    match t {
        RunnerTrigger::Flag(f) => format!("flag `{f}`"),
        RunnerTrigger::Subcommand(s) => format!("subcommand `{s}`"),
        RunnerTrigger::SubcommandWithSubarg { subcommand, subarg } => {
            format!("`{subcommand} {subarg}`")
        }
        RunnerTrigger::SubcommandWithFlag { subcommand, flag } => {
            format!("`{subcommand} {flag}`")
        }
        RunnerTrigger::ScriptCommands(cmds) => {
            let list = cmds
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("script commands {list}")
        }
        RunnerTrigger::ProgramFileFlag { flag, pin } => {
            format!("program file `{flag}` (pin: `{pin:?}`)")
        }
    }
}

fn fmt_derived_trigger(trigger: DerivedDestinationTrigger) -> String {
    match trigger {
        DerivedDestinationTrigger::Always => "always".to_string(),
        DerivedDestinationTrigger::WhenSourceIsDir => "when source is a directory".to_string(),
        DerivedDestinationTrigger::Flags(fs) => fs
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

fn write_flag_table(out: &mut String, heading: &str, flags: &[FlagSpec]) {
    let _ = writeln!(out, "\n### `{heading}` ({} listed flags)\n", flags.len());
    out.push_str("| Short | Long | Kind | Effects |\n");
    out.push_str("|---|---|---|---|\n");
    for f in flags {
        let short = f.short.map_or_else(|| "—".into(), |c| format!("`-{c}`"));
        let long = f.long.map_or_else(|| "—".into(), |l| format!("`--{l}`"));
        let kind = match f.kind {
            FlagKind::Bool => "Bool".into(),
            FlagKind::Value(r) => format!("Value({})", fmt_role(r)),
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
    out.push_str("# Policy engine rev 4: normative tool table (annex to §6, D-141)\n\n");
    out.push_str(
        "Generated from `crates/atrium-core/src/policy/tools.rs`. Do not edit by hand; \
         regenerate with `env UPDATE_TOOL_TABLE=1 cargo test -p atrium-core tools`.\n\n",
    );
    out.push_str(
        "**D-141 structure — six row kinds plus unknown-tool fallback (§4, §6):**\n\
         - **`HomeReadOnly`:** tools and `git` subcommands in `HOME_READONLY_COMMANDS` (without a search flag table). \
           Uses `AnyExceptForbidden` mirroring `dev`'s `forbidden_flag` matching (short clusters and long abbreviations \
           of `>= 3` chars including `--`). Every word is a candidate `Path` (`ReadWrite`, no operand relaxations), and \
           a forbidden flag **only** removes Home read-only eligibility (never makes the unit `Unread` on its own).\n\
         - **`Search`:** `rg` and the `grep` family, keeping `dev`'s flag tables (listed flags only), the pattern \
           operand (§11 item 9), modeled filter dialects and case rules, unsupported filters as `Unread`, and recursion \
           / link-following flags with the §11 item 15 toggle.\n\
         - **`Wrapper` and `NestedShell`:** §4's closed wrapper forms and `sh`/`bash`/`dash` nested shells.\n\
         - **`CodeRunner`:** always `Unread`, naming its inline-code location or pinned program (`Unread (program)`, \
           §11 item 8), with `code_runner_no_pin` per §7 / §11 item 4.\n\
         - **`ConditionalRunner`:** unknown-tool treatment unless a listed trigger fires (`find`, `git`, `sed`), \
           making the unit `Unread` (or pinnable `Unread (program)` for `sed -f`/`--file`). No flag arities or operand roles.\n\
         - **`DerivedDestinationWriter`:** `cp` (`-R`/`-r`/`-a`), `mv`, `rsync`, `tar` (`-x`), `unzip`, `git clone`, \
           `git checkout`. When triggered, the writer itself is `Unread(DerivedDestinations)` (§11 item 10) while every \
           word stays a candidate `Path` (`ReadWrite`). Without a trigger, the unit receives unknown-tool treatment.\n\
         - **Every other tool** (including **Unknown tools by design** below) is an **unknown tool** (§4): every word \
           is a candidate `Path` (`ReadWrite`) for hard-deny, store, and self-protection checks, the unit is a possible \
           writer (§5), and it is never Home-eligible.\n\
         - **Outside the model (`OUTSIDE_MODEL`):** non-§4 shells, non-§4 wrappers, shell reserved words, and \
           `pushd`/`popd` always make their unit `Unread`.\n\n",
    );

    // 1. HomeReadOnly
    out.push_str("## 1. `HomeReadOnly` rows\n\n");
    out.push_str(
        "| Command | Kind | Platforms | Forbidden flags (Home only) | Only args (Home only) | Git checks | Changes default (§11) |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|\n");
    for t in TOOLS {
        if let ToolRowKind::HomeReadOnly(r) = t.kind {
            let forb = if r.home_readonly.forbidden.is_empty() {
                "—".to_string()
            } else {
                r.home_readonly
                    .forbidden
                    .iter()
                    .map(|f| format!("`{f}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let only = r.home_readonly.only.map_or_else(
                || "—".to_string(),
                |o| {
                    o.iter()
                        .map(|a| format!("`{a}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            );
            let _ = writeln!(
                out,
                "| `{}` | `HomeReadOnly` | {} | {forb} | {only} | — | {} |",
                t.name,
                t.platforms.label(),
                t.changes_default.unwrap_or("—"),
            );
        }
        for s in t.subcommands() {
            if let SubcommandKind::HomeReadOnly(r) = s.kind {
                let forb = if r.home_readonly.forbidden.is_empty() {
                    "—".to_string()
                } else {
                    r.home_readonly
                        .forbidden
                        .iter()
                        .map(|f| format!("`{f}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let only = r.home_readonly.only.map_or_else(
                    || "—".to_string(),
                    |o| {
                        o.iter()
                            .map(|a| format!("`{a}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    },
                );
                let mut checks = Vec::new();
                if r.git_selects_by_name {
                    checks.push("CR-12 git-cred-select");
                }
                if r.git_wildcard_pathspec {
                    checks.push("wildcard pathspec → Unread");
                }
                let checks_str = if checks.is_empty() {
                    "—".to_string()
                } else {
                    checks.join("; ")
                };
                let _ = writeln!(
                    out,
                    "| `{} {}` | `HomeReadOnly` | {} | {forb} | {only} | {checks_str} | {} |",
                    t.name,
                    s.name,
                    t.platforms.label(),
                    s.changes_default.unwrap_or("—"),
                );
            }
        }
    }

    // 2. Search
    out.push_str("\n## 2. `Search` rows\n\n");
    out.push_str(
        "| Tool | Kind | Platforms | Flags | Filter dialect | Recursion | Default links | Home read-only | Changes default (§11) |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for t in TOOLS {
        if let ToolRowKind::Search(r) = t.kind {
            let rec = match r.recursive {
                SearchRecursion::Always => "always",
                SearchRecursion::WithFlag => "with flags (`-r`/`-R`/`-d`)",
            };
            let links = format!(
                "root={}, below={}",
                if r.links.at_root { "Y" } else { "N" },
                if r.links.below_root { "Y" } else { "N" }
            );
            let home = r
                .home_readonly
                .map_or_else(|| "No".to_string(), fmt_home_spec);
            let _ = writeln!(
                out,
                "| `{}` | `Search` | {} | {} | `{:?}` | {rec} | {links} | {home} | {} |",
                t.name,
                t.platforms.label(),
                fmt_flags_inline(r.flags),
                r.filter_dialect,
                t.changes_default.unwrap_or("—"),
            );
        }
    }

    // 3. Wrapper & NestedShell
    out.push_str("\n## 3. `Wrapper` and `NestedShell` rows\n\n");
    out.push_str("### `Wrapper` (§4)\n\n");
    out.push_str(
        "| Tool | Kind | Platforms | Wrapper | Allowed flags | `--` allowed | Wrapper operands | Changes default (§11) |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for t in TOOLS {
        if let ToolRowKind::Wrapper(r) = t.kind {
            let ops = if r.positional_operands.is_empty() {
                "—".to_string()
            } else {
                r.positional_operands
                    .iter()
                    .enumerate()
                    .map(|(i, role)| format!("[{i}] {}", fmt_role(*role)))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(
                out,
                "| `{}` | `Wrapper` | {} | `{:?}` | {} | {} | {ops} | {} |",
                t.name,
                t.platforms.label(),
                r.wrapper,
                fmt_flags_inline(r.flags),
                if r.allows_double_dash { "Yes" } else { "No" },
                t.changes_default.unwrap_or("—"),
            );
        }
    }

    out.push_str("\n### `NestedShell` (§4)\n\n");
    out.push_str(
        "| Tool | Kind | Platforms | Allowed flags | Script pin (§11 item 8) | Unpinned prefix invalid (§11 item 4) | Changes default (§11) |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|\n");
    for t in TOOLS {
        if let ToolRowKind::NestedShell(r) = t.kind {
            let _ = writeln!(
                out,
                "| `{}` | `NestedShell` | {} | {} | `{:?}` | {} | {} |",
                t.name,
                t.platforms.label(),
                fmt_flags_inline(r.flags),
                r.script_pin,
                if r.code_runner_no_pin { "Yes" } else { "No" },
                t.changes_default.unwrap_or("—"),
            );
        }
    }

    // 4. CodeRunner
    out.push_str("\n## 4. `CodeRunner` rows\n\n");
    out.push_str(
        "| Command | Kind | Platforms | Inline code | Program pin (§11 item 8) | Unpinned prefix invalid (§11 item 4) | Changes default (§11) |\n",
    );
    out.push_str("|---|---|---|---|---|---|---|\n");
    for t in TOOLS {
        if let ToolRowKind::CodeRunner(r) = t.kind {
            let _ = writeln!(
                out,
                "| `{}` | `CodeRunner` | {} | {} | {} | {} | {} |",
                t.name,
                t.platforms.label(),
                fmt_inline_code(r.inline_code),
                fmt_program_pin(r.program_pin),
                if r.code_runner_no_pin { "Yes" } else { "No" },
                t.changes_default.unwrap_or("—"),
            );
            for s in r.subcommands {
                if let SubcommandKind::CodeRunner(sr) = s.kind {
                    let _ = writeln!(
                        out,
                        "| `{} {}` | `CodeRunner` | {} | {} | {} | {} | {} |",
                        t.name,
                        s.name,
                        t.platforms.label(),
                        fmt_inline_code(sr.inline_code),
                        fmt_program_pin(sr.program_pin),
                        if sr.code_runner_no_pin { "Yes" } else { "No" },
                        s.changes_default.unwrap_or("—"),
                    );
                }
            }
        }
    }

    // 5. ConditionalRunner
    out.push_str("\n## 5. `ConditionalRunner` rows\n\n");
    out.push_str(
        "| Tool | Kind | Platforms | Triggers (make unit Unread) | Unpinned prefix invalid (§11 item 4) | Changes default (§11) |\n",
    );
    out.push_str("|---|---|---|---|---|---|\n");
    for t in TOOLS {
        if let ToolRowKind::ConditionalRunner(r) = t.kind {
            let triggers = r
                .triggers
                .iter()
                .copied()
                .map(fmt_runner_trigger)
                .collect::<Vec<_>>()
                .join("; ");
            let _ = writeln!(
                out,
                "| `{}` | `ConditionalRunner` | {} | {triggers} | {} | {} |",
                t.name,
                t.platforms.label(),
                if r.code_runner_no_pin { "Yes" } else { "No" },
                t.changes_default.unwrap_or("—"),
            );
        }
    }

    // 6. DerivedDestinationWriter
    out.push_str("\n## 6. `DerivedDestinationWriter` rows\n\n");
    out.push_str("| Command | Kind | Platforms | Trigger | Changes default (§11) |\n");
    out.push_str("|---|---|---|---|---|\n");
    for t in TOOLS {
        if let ToolRowKind::DerivedDestinationWriter(r) = t.kind {
            let _ = writeln!(
                out,
                "| `{}` | `DerivedDestinationWriter` | {} | {} | {} |",
                t.name,
                t.platforms.label(),
                fmt_derived_trigger(r.trigger),
                t.changes_default.unwrap_or("—"),
            );
        }
        for s in t.subcommands() {
            if let SubcommandKind::DerivedDestinationWriter(r) = s.kind {
                let _ = writeln!(
                    out,
                    "| `{} {}` | `DerivedDestinationWriter` | {} | {} | {} |",
                    t.name,
                    s.name,
                    t.platforms.label(),
                    fmt_derived_trigger(r.trigger),
                    s.changes_default.unwrap_or("—"),
                );
            }
        }
    }

    // 7. Detailed flag tables (Search, Wrapper, NestedShell only)
    out.push_str("\n## Detailed flag tables\n");
    write_flag_table(&mut out, "rg", RG_FLAGS);
    write_flag_table(
        &mut out,
        "grep / egrep / fgrep / zgrep / ggrep / rgrep",
        GREP_FLAGS,
    );
    write_flag_table(&mut out, "nice", NICE_FLAGS);
    write_flag_table(&mut out, "timeout", TIMEOUT_FLAGS);
    write_flag_table(&mut out, "sh / bash / dash", MODELED_SHELL_FLAGS);

    // 8. Outside the model
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

    // 9. Unknown tools by design
    out.push_str("\n## Unknown tools by design (`UNKNOWN_BY_DESIGN`)\n\n");
    out.push_str(
        "Every tool below leaves the table under D-141 and receives §4's unknown-tool \
         treatment: every word is a candidate `Path` (`ReadWrite`), the unit is a possible \
         writer (§5), and it is never Home-eligible.\n\n",
    );
    out.push_str("| Tool | `dev` list | Reason |\n");
    out.push_str("|---|---|---|\n");
    for e in UNKNOWN_BY_DESIGN {
        let _ = writeln!(out, "| `{}` | `{}` | {} |", e.name, e.dev_list, e.reason);
    }

    // 10. Summary of rows changing defaults (for Pedro)
    out.push_str("\n## Summary of rows changing `dev` defaults (§11)\n\n");
    out.push_str("| Entry | Kind | §11 item(s) |\n");
    out.push_str("|---|---|---|\n");
    for t in TOOLS {
        if let Some(cd) = t.changes_default {
            let _ = writeln!(out, "| `{}` | `{}` | {cd} |", t.name, t.kind.label());
        }
        for s in t.subcommands() {
            if let Some(cd) = s.changes_default {
                let _ = writeln!(
                    out,
                    "| `{} {}` | `{}` | {cd} |",
                    t.name,
                    s.name,
                    s.kind.label()
                );
            }
        }
    }
    for e in OUTSIDE_MODEL {
        if let Some(cd) = e.changes_default {
            let _ = writeln!(out, "| `{}` | `OutsideModel` | {cd} |", e.name);
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Integrity tests, D-141 fixtures & Markdown snapshot test
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::Path;

    use atrium_protocol::policy::HOME_READONLY_COMMANDS;

    use super::*;
    use crate::policy::{deny, engine, shell, words};

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
            check_flag_slice_unique(t.name, t.flags());
            let mut subs = HashSet::new();
            for s in t.subcommands() {
                assert!(
                    subs.insert(s.name),
                    "duplicate subcommand `{} {}`",
                    t.name,
                    s.name
                );
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
        let mut unknown = HashSet::new();
        for e in UNKNOWN_BY_DESIGN {
            assert!(
                unknown.insert(e.name),
                "duplicate UNKNOWN_BY_DESIGN name `{}`",
                e.name
            );
            assert!(
                !names.contains(e.name),
                "`{}` appears in both TOOLS and UNKNOWN_BY_DESIGN",
                e.name
            );
            assert!(
                !outside.contains(e.name),
                "`{}` appears in both OUTSIDE_MODEL and UNKNOWN_BY_DESIGN",
                e.name
            );
            assert!(
                !e.reason.trim().is_empty(),
                "UNKNOWN_BY_DESIGN `{}` must provide a one-line reason",
                e.name
            );
        }
    }

    #[test]
    fn every_flag_declares_explicit_effect_or_none_reason() {
        for t in TOOLS {
            for f in t.flags() {
                let flag_name = match (f.short, f.long) {
                    (Some(s), Some(l)) => format!("-{s}/--{l}"),
                    (Some(s), None) => format!("-{s}"),
                    (None, Some(l)) => format!("--{l}"),
                    (None, None) => "<unnamed>".to_string(),
                };
                match f.effect {
                    Effect::None { reason } => {
                        assert!(
                            !reason.trim().is_empty(),
                            "{} {flag_name}: Effect::None must provide a non-empty reason",
                            t.name
                        );
                    }
                    Effect::Applied(e) => {
                        assert!(
                            e.has_any_effect(),
                            "{} {flag_name}: Effect::Applied must declare at least one effect",
                            t.name
                        );
                    }
                }
                let eff = f.effects();
                if eff.filter.is_some() {
                    assert_eq!(
                        f.kind,
                        FlagKind::Value(ValueRole::Pattern),
                        "{} {flag_name}: modeled filter flag must take Value(Pattern)",
                        t.name
                    );
                    assert!(
                        eff.unread.is_none(),
                        "{} {flag_name}: modeled filter flag must not also set unread",
                        t.name
                    );
                }
            }
        }
    }

    #[test]
    fn every_code_runner_names_code_location_or_pin() {
        let check_runner = |label: &str,
                            row: CodeRunnerRow,
                            changes_default: Option<&'static str>,
                            is_priv: bool| {
            let has_inline = match row.inline_code {
                InlineCodeSpec::None => false,
                InlineCodeSpec::Flags(fs) => {
                    assert!(!fs.is_empty(), "{label}: InlineCodeSpec::Flags is empty");
                    true
                }
                InlineCodeSpec::Operand(_) | InlineCodeSpec::OperandsFrom(_) => true,
            };
            let has_pin = match row.program_pin {
                None => false,
                Some(p) => {
                    if p.pin == ProgramPin::FlagValue {
                        assert!(
                            !p.flags.is_empty(),
                            "{label}: ProgramPin::FlagValue must list its flag(s)"
                        );
                    }
                    assert!(
                        changes_default.is_some_and(|cd| cd.contains("§11 item 8")),
                        "{label}: pinned CodeRunner must cite §11 item 8"
                    );
                    true
                }
            };
            let sub_pins = row.subcommands.iter().any(|s| {
                matches!(
                    s.kind,
                    SubcommandKind::CodeRunner(sr)
                        if !matches!(sr.inline_code, InlineCodeSpec::None)
                            || sr.program_pin.is_some()
                )
            });
            assert!(
                has_inline || has_pin || sub_pins,
                "{label}: CodeRunner must name its inline-code location or program pin"
            );
            if row.code_runner_no_pin && !is_priv {
                assert!(
                    changes_default.is_some_and(|cd| cd.contains("§11 item 4")),
                    "{label}: code_runner_no_pin must cite §11 item 4"
                );
            }
        };

        for t in TOOLS {
            match t.kind {
                ToolRowKind::CodeRunner(r) => {
                    let is_priv = deny::PRIV.contains(&t.name);
                    check_runner(t.name, r, t.changes_default, is_priv);
                    for s in r.subcommands {
                        if let SubcommandKind::CodeRunner(sr) = s.kind {
                            check_runner(
                                &format!("{} {}", t.name, s.name),
                                sr,
                                s.changes_default,
                                false,
                            );
                        }
                    }
                }
                ToolRowKind::NestedShell(r) => {
                    assert!(r.code_runner_no_pin);
                    assert!(
                        t.changes_default.is_some_and(
                            |cd| cd.contains("§11 item 4") && cd.contains("§11 item 8")
                        ),
                        "NestedShell `{}` must cite §11 item 4 and item 8",
                        t.name
                    );
                }
                ToolRowKind::ConditionalRunner(r) => {
                    assert!(r.code_runner_no_pin);
                    assert!(
                        t.changes_default
                            .is_some_and(|cd| cd.contains("§11 item 4")),
                        "ConditionalRunner `{}` must cite §11 item 4",
                        t.name
                    );
                    if r.triggers
                        .iter()
                        .any(|trig| matches!(trig, RunnerTrigger::ProgramFileFlag { .. }))
                    {
                        assert!(
                            t.changes_default
                                .is_some_and(|cd| cd.contains("§11 item 8")),
                            "ConditionalRunner `{}` with ProgramFileFlag must cite §11 item 8",
                            t.name
                        );
                    }
                }
                _ => {}
            }
        }
        for e in OUTSIDE_MODEL {
            if e.code_runner_no_pin {
                assert!(
                    e.changes_default
                        .is_some_and(|cd| cd.contains("§11 item 4")),
                    "OUTSIDE_MODEL `{}` with code_runner_no_pin must cite §11 item 4",
                    e.name
                );
            }
        }
    }

    #[test]
    fn every_trigger_list_is_non_empty() {
        for t in TOOLS {
            match t.kind {
                ToolRowKind::ConditionalRunner(r) => {
                    assert!(
                        !r.triggers.is_empty(),
                        "ConditionalRunner `{}` must have at least one trigger",
                        t.name
                    );
                    for trig in r.triggers {
                        if let RunnerTrigger::ScriptCommands(cmds) = trig {
                            assert!(
                                !cmds.is_empty(),
                                "ConditionalRunner `{}` has empty ScriptCommands trigger",
                                t.name
                            );
                        }
                    }
                }
                ToolRowKind::DerivedDestinationWriter(r) => {
                    if let DerivedDestinationTrigger::Flags(fs) = r.trigger {
                        assert!(
                            !fs.is_empty(),
                            "DerivedDestinationWriter `{}` has empty trigger flag list",
                            t.name
                        );
                    }
                }
                _ => {}
            }
            for s in t.subcommands() {
                if let SubcommandKind::DerivedDestinationWriter(r) = s.kind
                    && let DerivedDestinationTrigger::Flags(fs) = r.trigger
                {
                    assert!(
                        !fs.is_empty(),
                        "DerivedDestinationWriter `{} {}` has empty trigger flag list",
                        t.name,
                        s.name
                    );
                }
            }
        }
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
            for s in t.subcommands() {
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

    /// Test-only helper modeling Home read-only acceptance against D-141 rows.
    fn table_accepts_in_home(argv: &[&str]) -> bool {
        let Some(&cmd) = argv.first() else {
            return false;
        };
        let Some(tool) = lookup(cmd) else {
            return false;
        };
        match tool.kind {
            ToolRowKind::HomeReadOnly(row) => {
                let home = row.home_readonly;
                let args = &argv[1..];
                if let Some(only) = home.only
                    && !args.iter().all(|a| only.contains(a))
                {
                    return false;
                }
                for &tok in args {
                    if tok == "--" {
                        break;
                    }
                    if is_forbidden_flag(home.forbidden, tok) {
                        return false;
                    }
                }
                true
            }
            ToolRowKind::ConditionalRunner(_) => {
                let Some(&sub_name) = argv.get(1) else {
                    return false;
                };
                let Some(sub) = subcommand(tool, sub_name) else {
                    return false;
                };
                let SubcommandKind::HomeReadOnly(row) = sub.kind else {
                    return false;
                };
                let home = row.home_readonly;
                let args = &argv[2..];
                if let Some(only) = home.only
                    && !args.iter().all(|a| only.contains(a))
                {
                    return false;
                }
                for &tok in args {
                    if tok == "--" {
                        break;
                    }
                    if is_forbidden_flag(home.forbidden, tok) {
                        return false;
                    }
                }
                true
            }
            ToolRowKind::Search(row) => {
                let Some(home) = row.home_readonly else {
                    return false;
                };
                let args = &argv[1..];
                let mut i = 0;
                let mut dd = false;
                let mut follows_below = row.links.below_root;
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
                        let Some(spec) = flag_long(row.flags, name) else {
                            return false;
                        };
                        let eff = spec.effects();
                        if eff.unread.is_some() {
                            return false;
                        }
                        follows_below = eff.follows_links_below_root.apply(follows_below);
                        if matches!(spec.kind, FlagKind::Value(_)) && !has_eq {
                            i += 1;
                        }
                    } else {
                        let chars: Vec<char> = tok[1..].chars().collect();
                        for (k, &c) in chars.iter().enumerate() {
                            let Some(spec) = flag_short(row.flags, c) else {
                                return false;
                            };
                            let eff = spec.effects();
                            if eff.unread.is_some() {
                                return false;
                            }
                            follows_below = eff.follows_links_below_root.apply(follows_below);
                            if matches!(spec.kind, FlagKind::Value(_)) {
                                if k + 1 == chars.len() {
                                    i += 1;
                                }
                                break;
                            }
                        }
                    }
                }
                !(follows_below && !home.allow_followed_search)
            }
            _ => false,
        }
    }

    #[test]
    fn home_readonly_effective_forms_and_metadata_parity() {
        // 1. Metadata parity with HOME_READONLY_COMMANDS
        let mut table_rows: Vec<HomeReadOnlySpec> = Vec::new();
        for t in TOOLS {
            if let Some(h) = t.home_readonly() {
                assert!(
                    matches!(
                        t.kind,
                        ToolRowKind::HomeReadOnly(_) | ToolRowKind::Search(_)
                    ),
                    "only HomeReadOnly or Search rows may carry home_readonly: `{}`",
                    t.name
                );
                table_rows.push(h);
            }
            for s in t.subcommands() {
                if let Some(h) = s.home_readonly() {
                    assert!(
                        matches!(s.kind, SubcommandKind::HomeReadOnly(_)),
                        "only HomeReadOnly subcommands may carry home_readonly: `{} {}`",
                        t.name,
                        s.name
                    );
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
            assert_eq!(matched.forbidden, proto.forbidden);
            assert_eq!(matched.only, proto.only);
            assert_eq!(matched.recursion, proto.recursion);
        }

        // 2. Effective accepted forms in Home
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
            &["git", "diff", "--no-index", "a", "b"],
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
        // `-R` overridden by later `-r` disables following links below root, so
        // Home accepts regardless of ALLOW_HOME_FOLLOWED_SEARCH.
        assert!(table_accepts_in_home(&["grep", "-R", "-r", "x", "proj"]));

        // 3. Effective rejected forms in Home
        let home_rejected: &[&[&str]] = &[
            &["file", "-f", "list.txt"],
            &["file", "-if", "list.txt"],
            &["file", "--compile", "magic"],
            &["file", "--comp", "magic"],
            &["file", "-m", "magic", "README.md"],
            &["date", "-s", "2026-01-01"],
            &["date", "--file", "dates.txt"],
            &["tree", "-o", "out.txt"],
            &["tree", "-Lo", "out.txt"],
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
            // Non-Home tools must never qualify
            &["patch", "target.txt", "patch.diff"],
            &["codesign", "-v", "app"],
            &["sed", "-e", "p", "README.md"],
            &["cp", "a", "b"],
        ];
        for cmd in home_rejected {
            assert!(
                !table_accepts_in_home(cmd),
                "Home read-only must reject {cmd:?}"
            );
        }
    }

    #[test]
    fn dev_lists_and_flag_tables_fully_accounted_for() {
        // 1. HOME_READONLY_COMMANDS
        for entry in HOME_READONLY_COMMANDS {
            let cmd = entry.prefix[0];
            let tool =
                lookup(cmd).unwrap_or_else(|| panic!("HOME_READONLY_COMMANDS `{cmd}` missing"));
            if entry.prefix.len() > 1 {
                let sub = subcommand(tool, entry.prefix[1]).unwrap_or_else(|| {
                    panic!(
                        "HOME_READONLY_COMMANDS subcommand {:?} missing",
                        entry.prefix
                    )
                });
                assert!(
                    matches!(sub.kind, SubcommandKind::HomeReadOnly(_)),
                    "{:?} must be HomeReadOnly",
                    entry.prefix
                );
            } else {
                assert!(
                    matches!(
                        tool.kind,
                        ToolRowKind::HomeReadOnly(_) | ToolRowKind::Search(_)
                    ),
                    "`{cmd}` must be HomeReadOnly or Search"
                );
            }
        }

        // 2. shell::SHELLS
        for &s in shell::SHELLS {
            if matches!(s, "sh" | "bash" | "dash") {
                assert!(
                    matches!(lookup(s).map(|t| t.kind), Some(ToolRowKind::NestedShell(_))),
                    "shell `{s}` must be NestedShell"
                );
            } else {
                assert!(
                    lookup_outside_model(s).is_some(),
                    "non-modeled shell `{s}` must be in OUTSIDE_MODEL"
                );
            }
        }

        // 3. shell::COMMAND_RUNNERS
        for &c in shell::COMMAND_RUNNERS {
            assert!(
                matches!(lookup(c).map(|t| t.kind), Some(ToolRowKind::CodeRunner(_))),
                "shell::COMMAND_RUNNERS `{c}` must be CodeRunner"
            );
        }

        // 4. shell::RESERVED
        for &r in shell::RESERVED {
            assert!(
                lookup_outside_model(r).is_some(),
                "shell::RESERVED `{r}` must be in OUTSIDE_MODEL"
            );
        }

        // 5. shell.rs wrappers
        for w in ["command", "exec", "nice", "timeout", "time", "env"] {
            assert!(
                matches!(lookup(w).map(|t| t.kind), Some(ToolRowKind::Wrapper(_))),
                "§4 wrapper `{w}` must be Wrapper"
            );
        }
        assert!(matches!(
            lookup("xargs").map(|t| t.kind),
            Some(ToolRowKind::CodeRunner(_))
        ));
        for w in [
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
            "chrt",
            "taskset",
            "flock",
            "script",
            "arch",
        ] {
            assert!(
                lookup_outside_model(w).is_some(),
                "non-§4 wrapper `{w}` must be in OUTSIDE_MODEL"
            );
        }

        // 6. words::WRITERS, words::READERS, deny::DOWNLOADERS, deny::PRIV
        for &w in words::WRITERS {
            if matches!(w, "cp" | "mv" | "rsync") {
                assert!(
                    matches!(
                        lookup(w).map(|t| t.kind),
                        Some(ToolRowKind::DerivedDestinationWriter(_))
                    ),
                    "words::WRITERS `{w}` must be DerivedDestinationWriter"
                );
            } else {
                assert!(
                    lookup_unknown_by_design(w).is_some(),
                    "words::WRITERS `{w}` must be in UNKNOWN_BY_DESIGN"
                );
            }
        }
        for &r in words::READERS {
            if matches!(r, "pushd" | "popd") {
                assert!(
                    lookup_outside_model(r).is_some(),
                    "words::READERS `{r}` must be in OUTSIDE_MODEL"
                );
            } else if let Some(t) = lookup(r) {
                assert!(
                    matches!(
                        t.kind,
                        ToolRowKind::HomeReadOnly(_)
                            | ToolRowKind::Search(_)
                            | ToolRowKind::CodeRunner(_)
                            | ToolRowKind::ConditionalRunner(_)
                    ),
                    "words::READERS `{r}` has unexpected kind {:?}",
                    t.kind.label()
                );
            } else {
                assert!(
                    lookup_unknown_by_design(r).is_some(),
                    "words::READERS `{r}` must be in TOOLS or UNKNOWN_BY_DESIGN"
                );
            }
        }
        for &d in deny::DOWNLOADERS {
            assert!(
                lookup_unknown_by_design(d).is_some(),
                "deny::DOWNLOADERS `{d}` must be in UNKNOWN_BY_DESIGN"
            );
        }
        for &p in deny::PRIV {
            if p == "su" {
                assert!(
                    matches!(lookup(p).map(|t| t.kind), Some(ToolRowKind::CodeRunner(_))),
                    "deny::PRIV `su` must be CodeRunner"
                );
            } else {
                assert!(
                    lookup_unknown_by_design(p).is_some(),
                    "deny::PRIV `{p}` must be in UNKNOWN_BY_DESIGN"
                );
            }
        }
        // Two-way check: no stale UNKNOWN_BY_DESIGN entries outside dev's lists.
        for e in UNKNOWN_BY_DESIGN {
            let in_dev = words::WRITERS.contains(&e.name)
                || words::READERS.contains(&e.name)
                || deny::DOWNLOADERS.contains(&e.name)
                || deny::PRIV.contains(&e.name);
            assert!(
                in_dev,
                "UNKNOWN_BY_DESIGN `{}` is not in any dev list",
                e.name
            );
        }
        // Verify grep siblings share GREP_FLAGS.
        for name in ["egrep", "fgrep", "zgrep", "ggrep", "rgrep"] {
            let t = lookup(name).unwrap();
            assert!(
                matches!(t.kind, ToolRowKind::Search(_)),
                "`{name}` must be Search"
            );
            assert_eq!(t.flags(), GREP_FLAGS);
        }

        // 7. engine.rs GIT_CRED_* parity
        assert_eq!(GIT_CRED_SUFFIXES, engine::GIT_CRED_SUFFIXES);
        assert_eq!(GIT_CRED_NAMES, engine::GIT_CRED_NAMES);
        assert_eq!(GIT_CRED_DIRS, engine::GIT_CRED_DIRS);

        // 8. Two-way flag parity: engine.rs RG_* & GREP_* (+ gate.rs follow/file)
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
            let f = flag_short(rg.flags(), c).unwrap_or_else(|| panic!("rg missing -{c}"));
            assert_eq!(f.kind, FlagKind::Bool, "rg -{c} must be Bool");
        }
        for &c in &rg_dev_val_s {
            let f = flag_short(rg.flags(), c).unwrap_or_else(|| panic!("rg missing -{c}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "rg -{c} must take a Value"
            );
        }
        for &l in &rg_dev_bool_l {
            let f = flag_long(rg.flags(), l).unwrap_or_else(|| panic!("rg missing --{l}"));
            assert_eq!(f.kind, FlagKind::Bool, "rg --{l} must be Bool");
        }
        for &l in &rg_dev_val_l {
            let f = flag_long(rg.flags(), l).unwrap_or_else(|| panic!("rg missing --{l}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "rg --{l} must take a Value"
            );
        }
        for f in rg.flags() {
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
            .chain(std::iter::once("file"))
            .collect();
        for &c in &grep_dev_bool_s {
            let f = flag_short(grep.flags(), c).unwrap_or_else(|| panic!("grep missing -{c}"));
            assert_eq!(f.kind, FlagKind::Bool, "grep -{c} must be Bool");
        }
        for &c in &grep_dev_val_s {
            let f = flag_short(grep.flags(), c).unwrap_or_else(|| panic!("grep missing -{c}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "grep -{c} must take a Value"
            );
        }
        for &l in &grep_dev_bool_l {
            let f = flag_long(grep.flags(), l).unwrap_or_else(|| panic!("grep missing --{l}"));
            assert_eq!(f.kind, FlagKind::Bool, "grep --{l} must be Bool");
        }
        for &l in &grep_dev_val_l {
            let f = flag_long(grep.flags(), l).unwrap_or_else(|| panic!("grep missing --{l}"));
            assert!(
                matches!(f.kind, FlagKind::Value(_)),
                "grep --{l} must take a Value"
            );
        }
        for f in grep.flags() {
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
                }
            }
        }
    }

    #[test]
    fn accessors_preserve_p1_signatures_and_read_table() {
        let rg = lookup("rg").unwrap();
        assert_eq!(flag(rg.flags(), "-g").unwrap().long, Some("glob"));
        assert_eq!(flag_for_tool(rg, "--iglob").unwrap().short, None);
        assert!(flag(rg.flags(), "--").is_none());
        assert!(flag(rg.flags(), "-ab").is_none());

        let nice = lookup("nice").unwrap();
        assert_eq!(nice.wrapper(), Some(WrapperKind::Nice));
        assert_eq!(
            flag_for_tool(nice, "-n").unwrap().kind,
            FlagKind::Value(ValueRole::Text)
        );

        let timeout = lookup("timeout").unwrap();
        assert_eq!(timeout.wrapper(), Some(WrapperKind::Timeout));
        assert!(flag_for_tool(timeout, "-s").is_some());
        assert!(flag_for_tool(timeout, "-k").is_some());

        let sh = lookup("sh").unwrap();
        assert!(sh.is_nested_shell());
        assert!(flag_for_tool(sh, "-c").unwrap().effects().runs_code);
        assert!(flag_for_tool(sh, "-e").is_some());

        let git = lookup("git").unwrap();
        let show = subcommand(git, "show").unwrap();
        assert!(matches!(
            show.kind,
            SubcommandKind::HomeReadOnly(HomeReadOnlyRow {
                git_selects_by_name: true,
                ..
            })
        ));
        let clone = subcommand(git, "clone").unwrap();
        assert!(matches!(
            clone.kind,
            SubcommandKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
                trigger: DerivedDestinationTrigger::Always,
            })
        ));

        assert!(lookup_outside_model("zsh").is_some());
        assert!(lookup_unknown_by_design("patch").is_some());
        assert!(lookup_unknown_by_design("codesign").is_some());
        assert!(lookup("patch").is_none());
        assert!(lookup("codesign").is_none());
    }

    #[test]
    fn d141_row_kinds_and_triggers_behave_as_specified() {
        // 1. HomeReadOnly forbidden matching (short clusters & 3-char long prefixes)
        assert!(is_forbidden_flag(GIT_FORBIDDEN, "--out=patch.diff"));
        assert!(is_forbidden_flag(GIT_FORBIDDEN, "--o=x"));
        assert!(is_forbidden_flag(GIT_FORBIDDEN, "-xc"));
        assert!(is_forbidden_flag(&["-o"], "-o"));
        assert!(!is_forbidden_flag(GIT_FORBIDDEN, "--=x"));
        assert!(!is_forbidden_flag(GIT_FORBIDDEN, "-"));
        assert!(!is_forbidden_flag(GIT_FORBIDDEN, "--no-index"));
        assert!(!is_forbidden_flag(GIT_FORBIDDEN, "--"));

        // HomeReadOnly / ConditionalRunner / DerivedDestinationWriter carry no FlagSpec tables.
        for t in TOOLS {
            if matches!(
                t.kind,
                ToolRowKind::HomeReadOnly(_)
                    | ToolRowKind::CodeRunner(_)
                    | ToolRowKind::ConditionalRunner(_)
                    | ToolRowKind::DerivedDestinationWriter(_)
            ) {
                assert!(
                    t.flags().is_empty(),
                    "`{}` ({}) must have no FlagSpec table",
                    t.name,
                    t.kind.label()
                );
            }
        }

        // `git diff` is HomeReadOnly with no operand relaxations (resolves round 4 finding 1).
        let git = lookup("git").unwrap();
        let git_diff = subcommand(git, "diff").unwrap();
        assert!(matches!(
            git_diff.kind,
            SubcommandKind::HomeReadOnly(HomeReadOnlyRow {
                git_selects_by_name: true,
                ..
            })
        ));
        assert!(!git_diff.code_runner_no_pin());
        assert_eq!(git_diff.changes_default, Some("§11 item 10"));

        // 2. Search: rg filter case toggle ordering (Rule E) & grep -r/-R link toggle ordering
        let rg = lookup("rg").unwrap();
        let mut ci = false;
        for tok in ["--glob-case-insensitive", "--no-glob-case-insensitive"] {
            ci = flag_for_tool(rg, tok)
                .unwrap()
                .effects()
                .filter_case_insensitive
                .apply(ci);
        }
        assert!(!ci);
        ci = flag_for_tool(rg, "--glob-case-insensitive")
            .unwrap()
            .effects()
            .filter_case_insensitive
            .apply(ci);
        assert!(ci);
        assert_eq!(
            flag_for_tool(rg, "-g").unwrap().effects().filter,
            Some(FilterSpec {
                dialect: FilterDialect::RgGlob,
                case: FilterCase::FollowGlobalToggle,
            })
        );
        assert_eq!(
            flag_for_tool(rg, "--iglob").unwrap().effects().filter,
            Some(FilterSpec {
                dialect: FilterDialect::RgGlob,
                case: FilterCase::AlwaysInsensitive,
            })
        );
        for tok in [
            "-t",
            "-T",
            "-d",
            "--maxdepth",
            "--max-filesize",
            "--type-add",
            "--type-clear",
        ] {
            assert_eq!(
                flag_for_tool(rg, tok).unwrap().effects().unread,
                Some(FlagUnread::FilterForm),
                "rg {tok} must be Unread(UnsupportedFilterForm)"
            );
        }

        let grep = lookup("grep").unwrap();
        let mut below = false;
        for tok in ["-R", "-r"] {
            below = flag_for_tool(grep, tok)
                .unwrap()
                .effects()
                .follows_links_below_root
                .apply(below);
        }
        assert!(!below);
        below = flag_for_tool(grep, "-R")
            .unwrap()
            .effects()
            .follows_links_below_root
            .apply(below);
        assert!(below);
        assert_eq!(
            flag_for_tool(grep, "--include").unwrap().effects().filter,
            Some(FilterSpec {
                dialect: FilterDialect::GrepInclude,
                case: FilterCase::AlwaysSensitive,
            })
        );
        for tok in ["--exclude", "--exclude-dir"] {
            assert_eq!(
                flag_for_tool(grep, tok).unwrap().effects().unread,
                Some(FlagUnread::FilterForm)
            );
        }
        assert_eq!(
            flag_for_tool(grep, "-d").unwrap().effects().unread,
            Some(FlagUnread::FlagEffect)
        );
        assert!(flag_for_tool(grep, "--include-dir").is_none());

        // 3. CodeRunner: awk -f FILE, python FILE, cargo SUBCOMMAND, pipx run PKG
        let awk = lookup("awk").unwrap();
        assert!(matches!(
            awk.kind,
            ToolRowKind::CodeRunner(CodeRunnerRow {
                inline_code: InlineCodeSpec::Operand(0),
                program_pin: Some(ProgramPinSpec {
                    pin: ProgramPin::FlagValue,
                    flags: &["-f", "--file"],
                }),
                code_runner_no_pin: true,
                ..
            })
        ));
        let cargo = lookup("cargo").unwrap();
        assert!(cargo.code_runner_no_pin());
        assert!(!subcommand(cargo, "test").unwrap().code_runner_no_pin());
        let pipx = lookup("pipx").unwrap();
        assert!(pipx.code_runner_no_pin());
        assert!(subcommand(pipx, "run").unwrap().code_runner_no_pin());

        // 4. ConditionalRunner: find, git, sed have triggers only (no flag tables)
        for name in ["find", "git", "sed"] {
            let t = lookup(name).unwrap();
            assert!(t.flags().is_empty(), "`{name}` must have no FlagSpec table");
            assert!(matches!(t.kind, ToolRowKind::ConditionalRunner(_)));
        }
        if let ToolRowKind::ConditionalRunner(sed) = lookup("sed").unwrap().kind {
            assert!(
                sed.triggers
                    .contains(&RunnerTrigger::ScriptCommands(&['e', 'w', 'r']))
            );
            assert!(sed.triggers.contains(&RunnerTrigger::ProgramFileFlag {
                flag: "-f",
                pin: ProgramPin::FlagValue,
            }));
        }
        if let ToolRowKind::ConditionalRunner(find) = lookup("find").unwrap().kind {
            for f in ["-exec", "-execdir", "-ok", "-okdir"] {
                assert!(find.triggers.contains(&RunnerTrigger::Flag(f)));
            }
        }
        if let ToolRowKind::ConditionalRunner(git_r) = git.kind {
            for f in ["-c", "--config-env", "--exec-path"] {
                assert!(git_r.triggers.contains(&RunnerTrigger::Flag(f)));
            }
            assert!(
                git_r
                    .triggers
                    .contains(&RunnerTrigger::Subcommand("merge-index"))
            );
            assert!(
                git_r
                    .triggers
                    .contains(&RunnerTrigger::Subcommand("filter-branch"))
            );
            assert!(git_r.triggers.contains(&RunnerTrigger::SubcommandWithFlag {
                subcommand: "diff",
                flag: "--ext-diff",
            }));
        }

        // 5. DerivedDestinationWriter: cp (-R/-r/-a), mv, rsync, tar (-x), unzip, git clone/checkout
        assert!(matches!(
            lookup("cp").unwrap().kind,
            ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
                trigger: DerivedDestinationTrigger::Flags(fs),
            }) if fs.contains(&"-R") && fs.contains(&"-r") && fs.contains(&"-a")
        ));
        assert!(matches!(
            lookup("mv").unwrap().kind,
            ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
                trigger: DerivedDestinationTrigger::WhenSourceIsDir,
            })
        ));
        assert!(matches!(
            lookup("rsync").unwrap().kind,
            ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
                trigger: DerivedDestinationTrigger::Always,
            })
        ));
        assert!(matches!(
            lookup("tar").unwrap().kind,
            ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
                trigger: DerivedDestinationTrigger::Flags(fs),
            }) if fs.contains(&"-x") && fs.contains(&"--extract") && fs.contains(&"--get")
        ));
        assert!(matches!(
            lookup("unzip").unwrap().kind,
            ToolRowKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
                trigger: DerivedDestinationTrigger::Always,
            })
        ));
        for sub in ["clone", "checkout"] {
            assert!(matches!(
                subcommand(git, sub).unwrap().kind,
                SubcommandKind::DerivedDestinationWriter(DerivedDestinationWriterRow {
                    trigger: DerivedDestinationTrigger::Always,
                })
            ));
        }

        // 6. Removed tools are unknown by design
        for removed in [
            "patch", "codesign", "ag", "ack", "rm", "ln", "sort", "curl", "sudo",
        ] {
            assert!(
                lookup(removed).is_none(),
                "`{removed}` must not be in TOOLS"
            );
            assert!(
                lookup_outside_model(removed).is_none(),
                "`{removed}` must not be in OUTSIDE_MODEL"
            );
            assert!(
                lookup_unknown_by_design(removed).is_some(),
                "`{removed}` must be in UNKNOWN_BY_DESIGN"
            );
        }
    }

    #[test]
    fn markdown_renders_kinds_triggers_and_unknown_by_design() {
        let md = render_markdown();
        assert!(md.contains("## 1. `HomeReadOnly` rows"));
        assert!(md.contains("## 2. `Search` rows"));
        assert!(md.contains("## 3. `Wrapper` and `NestedShell` rows"));
        assert!(md.contains("## 4. `CodeRunner` rows"));
        assert!(md.contains("## 5. `ConditionalRunner` rows"));
        assert!(md.contains("## 6. `DerivedDestinationWriter` rows"));
        assert!(md.contains("## Outside the model (`OUTSIDE_MODEL`)"));
        assert!(md.contains("## Unknown tools by design (`UNKNOWN_BY_DESIGN`)"));
        assert!(md.contains("| `patch` | `words::WRITERS` |"));
        assert!(md.contains("| `codesign` | `words::READERS` |"));
        assert!(md.contains("script commands `e`, `w`, `r`"));
        assert!(md.contains("program file `-f` (pin: `FlagValue`)"));
        assert!(md.contains("flag `-exec`"));
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
