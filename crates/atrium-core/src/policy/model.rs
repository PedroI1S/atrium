//! Shared data model for policy engine rev 4 (contract §1–§8).
//!
//! Defines the types P1 (`v4/lex.rs`, `v4/units.rs`), P2 (`v4/paths.rs`),
//! P3 (`v4/tools.rs`, `v4/rules.rs`), and P5 (`v4/evaluate.rs`) build against.
//! Not wired into `SessionGate` until P5.

#![allow(dead_code)]

use atrium_agents::ToolCheck;
use atrium_protocol::policy::{Preset, RuleId, RuleMatcher};

use crate::PolicyVerdict;

// ---------------------------------------------------------------------------
// Words with provenance (§3)
// ---------------------------------------------------------------------------

/// How one byte of a [`Word`] was produced (contract §3).
///
/// Kept to a single byte (`#[repr(u8)]`) so provenance tracking stays linear
/// in both time and memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub(crate) enum ByteProvenance {
    /// Written directly outside quotes without a preceding backslash.
    Unquoted = 0,
    /// Written inside `'...'`.
    SingleQuoted = 1,
    /// Written inside `"..."`.
    DoubleQuoted = 2,
    /// Produced by an unquoted backslash escape or an allowed `\"` / `\\`
    /// escape inside double quotes.
    Escaped = 3,
    /// Produced by resolution or expansion (e.g. `~` or glob expansion), so it
    /// is never parsed as shell syntax again.
    Derived = 4,
}

/// A shell word carrying its bytes and per-byte [`ByteProvenance`] (contract §3).
///
/// Invariant: `bytes.len() == provenance.len()`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub(crate) struct Word {
    bytes: Vec<u8>,
    provenance: Vec<ByteProvenance>,
}

impl Word {
    /// Creates an empty word.
    pub(crate) const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            provenance: Vec::new(),
        }
    }

    /// Creates a word from pre-built byte and provenance buffers if their
    /// lengths match.
    pub(crate) fn from_parts(bytes: Vec<u8>, provenance: Vec<ByteProvenance>) -> Option<Self> {
        (bytes.len() == provenance.len()).then_some(Self { bytes, provenance })
    }

    /// Creates a word whose bytes are all marked [`ByteProvenance::Derived`].
    pub(crate) fn derived(bytes: Vec<u8>) -> Self {
        let provenance = vec![ByteProvenance::Derived; bytes.len()];
        Self { bytes, provenance }
    }

    /// Appends one byte with its provenance.
    pub(crate) fn push(&mut self, byte: u8, prov: ByteProvenance) {
        self.bytes.push(byte);
        self.provenance.push(prov);
    }

    /// Appends a slice of bytes sharing the same provenance.
    pub(crate) fn extend_with_provenance(&mut self, bytes: &[u8], prov: ByteProvenance) {
        self.bytes.extend_from_slice(bytes);
        self.provenance
            .extend(std::iter::repeat_n(prov, bytes.len()));
    }

    /// Returns the decoded literal bytes of the word (contract §3).
    pub(crate) fn literal_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the per-byte provenance slice (same length as
    /// [`Self::literal_bytes`]).
    pub(crate) fn provenance(&self) -> &[ByteProvenance] {
        &self.provenance
    }

    /// Views the literal bytes as UTF-8, if valid.
    pub(crate) fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    /// Returns the byte length of the word.
    pub(crate) fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns `true` if the word has zero bytes.
    pub(crate) fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns `true` iff any byte is an unquoted glob metacharacter
    /// (`*`, `?`, or `[`) (contract §3).
    pub(crate) fn has_unquoted_glob(&self) -> bool {
        self.bytes
            .iter()
            .zip(&self.provenance)
            .any(|(&b, &p)| p == ByteProvenance::Unquoted && matches!(b, b'*' | b'?' | b'['))
    }

    /// Returns `true` iff every byte in the word was written unquoted.
    pub(crate) fn is_all_unquoted(&self) -> bool {
        self.provenance
            .iter()
            .all(|&p| p == ByteProvenance::Unquoted)
    }

    /// Returns `true` iff the word starts with an unquoted `~` (contract §3).
    pub(crate) fn starts_with_unquoted_tilde(&self) -> bool {
        self.bytes.first() == Some(&b'~')
            && self.provenance.first() == Some(&ByteProvenance::Unquoted)
    }
}

// ---------------------------------------------------------------------------
// Modeled units and execution structure (§3, §4)
// ---------------------------------------------------------------------------

/// Zero-based index of a modeled [`CommandUnit`] within a call (contract §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct UnitIndex(pub(crate) usize);

/// Identifier grouping all stages of one shell pipeline (contract §3, §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct PipelineId(pub(crate) u32);

/// Separator immediately preceding a unit in its command text (contract §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Separator {
    /// First unit of the command line or nested `-c` script.
    Start,
    /// `;`
    Semi,
    /// `&&`
    And,
    /// `||`
    Or,
    /// `|`
    Pipe,
    /// `\n`
    Newline,
}

/// File descriptor within the closed command model: 0, 1, or 2 (contract §3).
///
/// Any other descriptor number is outside the model (`DescriptorOutsideStd`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub(crate) enum StdFd {
    Stdin = 0,
    Stdout = 1,
    Stderr = 2,
}

impl StdFd {
    pub(crate) const fn from_u8(fd: u8) -> Option<Self> {
        match fd {
            0 => Some(Self::Stdin),
            1 => Some(Self::Stdout),
            2 => Some(Self::Stderr),
            _ => None,
        }
    }
}

/// Redirect from §3's closed list (contract §3).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum ModelRedirect {
    /// `< target` (fd 0 read).
    Read { target: Word },
    /// `> target` (fd 1 write/truncate).
    Write { target: Word },
    /// `>> target` (fd 1 append).
    Append { target: Word },
    /// `2> target` (fd 2 write/truncate).
    ErrWrite { target: Word },
    /// `2>> target` (fd 2 append).
    ErrAppend { target: Word },
    /// `n>&m` with `n` and `m` in `0..=2`.
    Dup { from: StdFd, to: StdFd },
    /// `n>&-` with `n` in `0..=2`.
    Close { fd: StdFd },
}

impl ModelRedirect {
    /// Returns the literal target word for file redirects, or `None` for fd
    /// duplication/close.
    pub(crate) fn target(&self) -> Option<&Word> {
        match self {
            Self::Read { target }
            | Self::Write { target }
            | Self::Append { target }
            | Self::ErrWrite { target }
            | Self::ErrAppend { target } => Some(target),
            Self::Dup { .. } | Self::Close { .. } => None,
        }
    }

    /// Returns `true` for output redirects whose literal target is not
    /// `/dev/null` (contract §3, §5: output redirect makes the unit a writer,
    /// except one to `/dev/null`).
    pub(crate) fn writes_outside_dev_null(&self) -> bool {
        match self {
            Self::Write { target }
            | Self::Append { target }
            | Self::ErrWrite { target }
            | Self::ErrAppend { target } => target.literal_bytes() != b"/dev/null",
            Self::Read { .. } | Self::Dup { .. } | Self::Close { .. } => false,
        }
    }

    /// Returns `true` for redirects permitted under simple-command rule and
    /// Home read-only guards: fd duplication/close or output to `/dev/null`
    /// (contract §3, §6, §7).
    pub(crate) fn is_harmless_for_rules(&self) -> bool {
        match self {
            Self::Dup { .. } | Self::Close { .. } => true,
            Self::Write { target }
            | Self::Append { target }
            | Self::ErrWrite { target }
            | Self::ErrAppend { target } => target.literal_bytes() == b"/dev/null",
            Self::Read { .. } => false,
        }
    }
}

/// Closed list of transparent wrappers stripped by the command model
/// (contract §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WrapperKind {
    /// `command [--]`
    Command,
    /// `exec` (no options)
    Exec,
    /// `nice [-n N]`
    Nice,
    /// `timeout [-s SIG] [-k D] D`
    Timeout,
    /// `time` (no options)
    Time,
    /// `env` (no options and no assignments)
    Env,
}

/// One stripped wrapper recorded on a [`CommandUnit`] (contract §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemovedWrapper {
    pub(crate) kind: WrapperKind,
    /// Exact words removed from the front of the unit (command word plus any
    /// allowed wrapper options/duration).
    pub(crate) words: Vec<Word>,
}

/// A unit produced by exact lexing before wrapper removal (contract §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawCommandUnit {
    pub(crate) index: UnitIndex,
    pub(crate) argv: Vec<Word>,
    pub(crate) redirects: Vec<ModelRedirect>,
    pub(crate) separator_before: Separator,
    pub(crate) pipeline_id: Option<PipelineId>,
    /// Nesting depth (`0` for top-level, `1..=MAX_DEPTH` inside `-c` scripts).
    pub(crate) depth: u8,
    /// The `-c` unit this unit was lexed from (`None` for top-level units).
    pub(crate) parent_unit: Option<UnitIndex>,
}

/// A modeled command unit after §4 wrapper removal (contract §3, §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommandUnit {
    /// Position of this unit in the call's unit list.
    pub(crate) index: UnitIndex,
    /// Effective argv words after stripping §4 wrappers.
    pub(crate) argv: Vec<Word>,
    /// Wrappers stripped from the front of the unit, in outer-to-inner order.
    pub(crate) removed_wrappers: Vec<RemovedWrapper>,
    /// Redirects from §3's closed list.
    pub(crate) redirects: Vec<ModelRedirect>,
    /// Separator immediately preceding this unit (`;`, `&&`, `||`, `|`, `\n`,
    /// or `Start`).
    pub(crate) separator_before: Separator,
    /// Shared pipeline identifier when this unit belongs to a pipeline.
    pub(crate) pipeline_id: Option<PipelineId>,
    /// Nesting depth (`0` for top-level, `1..=MAX_DEPTH` inside `-c` scripts).
    pub(crate) depth: u8,
    /// The `-c` unit this unit originated from (`None` for top-level units).
    pub(crate) parent_unit: Option<UnitIndex>,
}

impl CommandUnit {
    /// Iterates over the original argv words as written (removed wrapper words
    /// followed by effective `argv`).
    pub(crate) fn original_argv(&self) -> impl Iterator<Item = &Word> {
        self.removed_wrappers
            .iter()
            .flat_map(|w| w.words.iter())
            .chain(self.argv.iter())
    }
}

/// Result of exact lexing of one command text (contract §3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LexExactOutput {
    pub(crate) units: Vec<RawCommandUnit>,
    pub(crate) outcomes: Vec<PartResult>,
}

/// Result of stripping §4 wrappers from one [`RawCommandUnit`] (contract §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UnwrapResult {
    pub(crate) unit: CommandUnit,
    /// Any wrapper-level `Unread` reason (e.g. `MaxWrapReached`,
    /// `UnsupportedWrapperForm`).
    pub(crate) unread: Option<UnreadReason>,
}

/// Complete output of P1 command analysis for one command line (contract §2–§4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParsedCommandUnits {
    /// Modeled command units (including units from modeled `sh`/`bash`/`dash -c`
    /// scripts).
    pub(crate) units: Vec<CommandUnit>,
    /// Unit- and line-level outcomes produced during lexing/unwrapping (e.g.
    /// §3/§4 `UnreadReason`s).
    pub(crate) outcomes: Vec<PartResult>,
    /// Separate best-effort record for §2 hard-deny and §7 Deny-rule matching.
    pub(crate) best_effort: BestEffortRecord,
}

// ---------------------------------------------------------------------------
// Best-effort pass record (§2) — kept strictly separate from exact units
// ---------------------------------------------------------------------------

/// Coarse redirect direction recorded for best-effort matching (contract §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum BestEffortRedirectKind {
    Read,
    Write,
    Dup,
}

/// One unit produced by the tolerant best-effort lexer (contract §2).
///
/// Kept separate from [`CommandUnit`] so the exact model never reads a
/// best-effort unit as `Clear`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BestEffortUnit {
    pub(crate) index: usize,
    pub(crate) raw_argv: Vec<String>,
    pub(crate) effective_argv: Vec<String>,
    pub(crate) alt_argv: Vec<Vec<String>>,
    pub(crate) assignments: Vec<(String, String)>,
    pub(crate) redirects: Vec<(BestEffortRedirectKind, String)>,
    pub(crate) pipeline_id: Option<PipelineId>,
    pub(crate) piped: bool,
    pub(crate) depth: u8,
    pub(crate) derived: bool,
}

/// Raw text and tolerant-lexer units for the best-effort pass (contract §2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BestEffortRecord {
    /// Raw command text (or strings extracted from `ToolSubject::Unknown`).
    pub(crate) raw_texts: Vec<String>,
    /// Every unit the tolerant lexer produced, including those derived from
    /// `-c` scripts and substitutions.
    pub(crate) units: Vec<BestEffortUnit>,
}

// ---------------------------------------------------------------------------
// Outcomes, hits, and closed UnreadReason enum (§1–§8)
// ---------------------------------------------------------------------------

/// Identifies which part of a call produced a [`PartOutcome`] or [`Hit`]
/// (contract §1).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum PartId {
    /// The call as a whole (e.g. `MAX_BYTES`, `ToolSubject::Unknown`, or
    /// raw-text best-effort check).
    WholeCall,
    /// A modeled command unit.
    Unit(UnitIndex),
    /// A specific argv operand on a modeled command unit.
    Operand { unit: UnitIndex, argv_index: usize },
    /// A specific redirect on a modeled command unit.
    Redirect {
        unit: UnitIndex,
        redirect_index: usize,
    },
    /// A search filter on a unit or `ToolSubject::Paths` search.
    Filter { unit: Option<UnitIndex> },
    /// One path entry on `ToolSubject::Paths` or `ToolSubject::Fetch`.
    SubjectPath { path_index: usize },
    /// A unit from [`BestEffortRecord::units`].
    BestEffortUnit(usize),
}

/// Whether a [`Hit`] came from the exact model or the best-effort pass
/// (contract §1, §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum HitSource {
    Model,
    BestEffort,
}

/// What a [`Hit`] matched (contract §1, §2, §5, §6).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum HitMatch {
    /// A protected path (credential file/tree, Atrium data/protected path,
    /// device path, or recursive-delete target).
    ProtectedPath { path: Vec<u8> },
    /// A credential store family that asks (CR-12, CR-17).
    StoreFamily { family: &'static str },
    /// A probe name matched by a glob or search filter (e.g.
    /// `"msal_token_cache.json"`, `"id_rsa"`).
    ProbeName { probe: String },
    /// A command word or invocation (e.g. `"sudo"`, `"diskutil"`, `"security"`,
    /// `"git push --force"`).
    CommandWord { word: String },
}

/// A hard-deny or store hit (contract §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hit {
    /// The `HARD_DENY_DEFAULTS` id when this hit belongs to `HardDenyHit`, or
    /// `None` for `StoreHit`.
    pub(crate) hard_deny_id: Option<&'static str>,
    pub(crate) source: HitSource,
    pub(crate) matched: HitMatch,
    pub(crate) part: PartId,
}

/// Why a part or call is `Exhausted` (contract §1, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ExhaustReason {
    /// Per-check compute unit budget ran out (§8).
    ComputeBudget,
    /// Per-check heap allocation budget ran out (§8).
    AllocationBudget,
    /// Call subject is `ToolSubject::Unknown` (§1, §11).
    UnknownSubject,
}

/// Structural size limit exceeded, producing `TooLarge` → `Deny` (contract §1, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Cap {
    /// Input exceeded `limits::MAX_BYTES`.
    Bytes,
    /// Unit count exceeded `limits::MAX_UNITS`.
    Units,
    /// Use count exceeded `limits::MAX_USES`.
    Uses,
    /// Resolved path count exceeded `limits::MAX_PATHS`.
    Paths,
}

/// Per-check filesystem cap that ran out, turning into `Unread` rather than
/// `Exhausted` (contract §2, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FsCap {
    /// Directory entries read (`WALK_MAX_ENTRIES` or `FS_MAX_ENTRIES_READ`).
    EntriesRead,
    /// Directory opens (`FS_MAX_DIR_OPENS`).
    DirOpens,
    /// `stat` / `lstat` calls (`FS_MAX_STATS`).
    Stats,
    /// `readlink` calls (`FS_MAX_READLINKS`).
    LinkReads,
    /// Canonicalization calls (`FS_MAX_CANONICALIZATIONS`).
    Canonicalizations,
    /// Walk or glob expansion depth (`WALK_MAX_DEPTH` or `GLOB_MAX_DEPTH`).
    WalkDepth,
}

/// Closed enumeration of every "Unread" / "outside the model" reason named in
/// contract §2–§7.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum UnreadReason {
    /// An unquoted byte outside §3's allowed character/glob/tilde set (including
    /// `$`, backtick, `{`, `}`, `#`, `!`, `^`, `(`, `)`, non-ASCII, or leading
    /// `=`) (contract §3).
    UnquotedByteOutsideModel { byte: u8 },
    /// A negated character class (`[!…]` or `[^…]`) in an unquoted glob
    /// (contract §3).
    NegatedGlobClass,
    /// An unquoted `~` not followed by `/` or end of word (e.g. `~user`, `~+`,
    /// `~-`) (contract §3).
    UnsupportedTildeForm,
    /// A quote or escape form outside §3: `$`, backtick, `!`, or non-`\"`/`\\`
    /// backslash inside double quotes; unquoted backslash before a disallowed
    /// byte or newline; or an unterminated quote (contract §3).
    UnsupportedQuoteOrEscape,
    /// A shell reserved word or compound construct: comments, here-documents,
    /// here-strings, groups, subshells, functions, `&`, reserved words, or
    /// arithmetic (contract §3).
    ReservedWordOrConstruct { construct: String },
    /// A leading `NAME=value` assignment on a unit (contract §3).
    Assignment,
    /// `~` used after an earlier unit in the line modified `HOME` (assignment,
    /// `env`, `export`, or `unset`) (contract §3).
    HomeSetEarlier,
    /// An unquoted glob character in command-word position (contract §4,
    /// §11 item 10).
    CommandWordGlob,
    /// Wrapper unwrapping reached `limits::MAX_WRAP` (8) (contract §4,
    /// §10 row 5).
    MaxWrapReached,
    /// A wrapper or wrapper option outside §4's closed list (contract §4,
    /// §11 item 10).
    UnsupportedWrapperForm { wrapper: String },
    /// A nested-shell form outside §4 (`sh`/`bash`/`dash` with unsupported
    /// flags or trailing operands, stdin shell, `zsh -c`, or any other entry in
    /// `SHELLS`) (contract §4).
    UnsupportedNestedShellForm { shell: String },
    /// Nested `sh`/`bash`/`dash -c` recursion reached `limits::MAX_DEPTH`
    /// (contract §4).
    MaxNestingDepthReached,
    /// A code runner executing inline code (`python -c`, `awk`, `sed` with
    /// `e`/`w`/`r`, `find -exec`, `eval`, `xargs`, `parallel`, `env -S`, etc.)
    /// (contract §4, §6).
    CodeRunnerInlineCode { tool: String },
    /// **Unread (program):** the unit runs a named script file or project build
    /// (`["python", "build.py"]`, `["cargo", "test"]`, `["sh", "x.sh"]`),
    /// coverable by an Allow rule naming these program words (contract §1, §4,
    /// §7, §11 item 8).
    Program { program_words: Vec<String> },
    /// A known tool invoked with a flag absent from §6's table (contract §6).
    UnknownFlag { tool: String, flag: String },
    /// A tool flag changes the base directory or path prefix for resolving
    /// later operands (`tar -C`/`--directory`, `git -C`/`--git-dir`/
    /// `--work-tree`/`--super-prefix`), which the table does not resolve
    /// per-operand (contract §5, §6).
    UnsupportedBaseDirChange { tool: String, flag: String },
    /// A known tool flag has a real effect intentionally left outside the
    /// model (`ag -z`/`--search-zip`, `ack -x`, `printf -v`,
    /// `grep -d`/`--directories`) (contract §3, §5, §6).
    UnsupportedFlagEffect { tool: String, flag: String },
    /// A tool or subcommand explicitly left outside the model in §6's table
    /// (`OUTSIDE_MODEL` or unsupported subcommand) (contract §6).
    OutsideToolModel { tool: String },
    /// A process-dependent path (`/proc/self`, `/proc/thread-self`,
    /// `/proc/<pid>`, `/dev/fd/`, `/dev/stdin`, `/dev/stdout`, `/dev/stderr`),
    /// directly or via a symlink (contract §3, §11 item 14).
    ProcessDependentPath { path: Vec<u8> },
    /// A path that could not be resolved (unreadable directory, `MAX_HOPS`
    /// exceeded, or filesystem I/O error) (contract §5).
    UnresolvablePath { path: Vec<u8> },
    /// A name matching a protected name only under an unlisted volume
    /// equivalence (unlisted Unicode case fold or NFC/NFD normalization)
    /// (contract §5, §11 item 13).
    UnlistedNameEquivalence { path: Vec<u8> },
    /// A shell glob match found only under a dialect variant (dot-matching or
    /// recursive `**`), never under the sh/bash/zsh intersection (contract §5,
    /// §11 item 10).
    VariantOnlyGlobMatch { pattern: Vec<u8> },
    /// A search filter outside §5's supported forms (negated filter, multiple
    /// filters on one call, or unsupported dialect) (contract §5).
    UnsupportedFilterForm,
    /// A followed symlink reaching a protected path during a recursive read
    /// that carries a filter (over-approximation never produces a hard-deny
    /// hit; contract §1, §5).
    FollowedLinkUnderFilter { path: Vec<u8> },
    /// A filesystem walk or operation cap was reached (contract §2, §5, §8,
    /// §11 item 11).
    WalkCapReached(FsCap),
    /// A writer whose destinations depend on contents or traversal (`cp -R`,
    /// `cp -a`, `mv` of a directory, `rsync`, `tar -x`, `unzip`, `git clone`,
    /// `git checkout`) (contract §5, §11 item 10).
    DerivedDestinations { tool: String },
    /// Paths of a unit executing after an earlier possible writer in the same
    /// call (contract §5, §11 item 6).
    AfterPossibleWriter { writer_unit: UnitIndex },
    /// Paths of any stage in a pipeline that contains a possible writer,
    /// including stages before it (contract §5, §11 item 10).
    PipelineWithPossibleWriter { writer_unit: UnitIndex },
    /// Working directory became unknown after a `cd` outside §5's model
    /// (contract §5).
    UnknownCwd,
    /// A Git pathspec containing an active wildcard (contract §6, §11 item 10).
    GitWildcardPathspec { pathspec: String },
    /// A redirect file descriptor outside `0..=2` (contract §3, §11 item 10).
    DescriptorOutsideStd { fd: u32 },
    /// Combined redirect `&>` / `&>>` (not shared by POSIX `sh`) (contract §3,
    /// §11 item 10).
    CombinedRedirect,
    /// Input fd duplication `<&` or another redirect form outside §3's closed
    /// list (`<>`, `>|`, or missing target) (contract §3).
    UnsupportedRedirect { op: String },
}

/// Outcome of one part of a call (contract §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PartOutcome {
    HardDenyHit(Hit),
    StoreHit(Hit),
    Clear,
    Unread(UnreadReason),
    Exhausted(ExhaustReason),
    TooLarge(Cap),
}

/// One part identifier paired with its [`PartOutcome`] (contract §1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PartResult {
    pub(crate) part: PartId,
    pub(crate) outcome: PartOutcome,
}

/// Aggregated outcomes across **every** part of a tool call (contract §1).
///
/// Never discards hits or reasons for doubt after finding the first one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CallOutcome {
    pub(crate) parts: Vec<PartResult>,
    /// `true` iff any directly named file or file found by the bounded
    /// hard-link walk is a regular file with `nlink > 1` (contract §1 step 4,
    /// §5).
    pub(crate) multiply_linked_file: bool,
}

impl CallOutcome {
    pub(crate) fn push(&mut self, part: PartId, outcome: PartOutcome) {
        self.parts.push(PartResult { part, outcome });
    }

    pub(crate) fn extend(&mut self, results: impl IntoIterator<Item = PartResult>) {
        self.parts.extend(results);
    }
}

// ---------------------------------------------------------------------------
// Tool effects (§6)
// ---------------------------------------------------------------------------

/// Semantic role of a positional operand or a value-taking flag's argument
/// (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ValueRole {
    /// Filesystem path candidate for §5 resolution.
    Path,
    /// Search/match pattern (not a candidate path; §11 item 9).
    Pattern,
    /// Member path inside an archive (not a host filesystem path; §6).
    ArchiveMember,
    /// VCS ref, revision, branch, or remote name (not a candidate path; §6).
    Ref,
    /// Plain text, number, mode, duration, signal, etc. (not a candidate path;
    /// §6, §11 item 9).
    Text,
    /// Destination root for copy/move/extract/clone (§5, §6).
    DestinationRoot,
    /// Inline code string (e.g. `sh -c SCRIPT`, `python -c CODE`, `awk PROG`;
    /// §4, §6).
    Code,
    /// Program/script file path executed by an interpreter (e.g. `python
    /// build.py`, `sh x.sh`; §4, §6, §7).
    ProgramFile,
}

/// Role assigned to each argv position in [`ToolEffects`] (contract §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum OperandRole {
    /// Command word (`argv[0]`) or subcommand word.
    Command,
    Path,
    Pattern,
    ArchiveMember,
    Ref,
    Text,
    DestinationRoot,
    Code,
    ProgramFile,
    /// Flag token (boolean flag, flag introducing a value, or `--`).
    Flag,
    /// Value argument consumed by a preceding value-taking flag.
    FlagValue(ValueRole),
}

impl From<ValueRole> for OperandRole {
    fn from(role: ValueRole) -> Self {
        match role {
            ValueRole::Path => Self::Path,
            ValueRole::Pattern => Self::Pattern,
            ValueRole::ArchiveMember => Self::ArchiveMember,
            ValueRole::Ref => Self::Ref,
            ValueRole::Text => Self::Text,
            ValueRole::DestinationRoot => Self::DestinationRoot,
            ValueRole::Code => Self::Code,
            ValueRole::ProgramFile => Self::ProgramFile,
        }
    }
}

/// Symlink-following behaviour at root operands and below traversed roots
/// (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) struct LinkPolicy {
    pub(crate) at_root: bool,
    pub(crate) below_root: bool,
}

/// Whether and how a classified unit runs code (contract §4, §6, §7).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) enum CodeExecution {
    #[default]
    None,
    /// Executes inline code held in `code_operands` (plain `Unread`).
    Inline { code_operands: Vec<usize> },
    /// Executes a named program file or project build (`Unread (program)`),
    /// coverable by an Allow rule naming `program_words` (§7).
    Program {
        program_operand: Option<usize>,
        program_words: Vec<String>,
    },
}

/// Supported search-filter syntax dialects (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FilterDialect {
    /// `Search`/`Glob` tool globs: relative path with `**`.
    SearchGlob,
    /// `rg -g`/`--glob`/`--iglob`: gitignore-style relative path.
    RgGlob,
    /// `grep --include`: basename glob.
    GrepInclude,
}

/// One extracted search filter on a unit or search call (contract §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchFilter {
    pub(crate) dialect: FilterDialect,
    pub(crate) pattern: Word,
    pub(crate) case_insensitive: bool,
    pub(crate) argv_index: Option<usize>,
}

/// Whether a classified unit writes to derived destinations, including
/// filesystem-dependent cases that P2 resolves (contract §5).
///
/// Carried from P3 (`v4/tools.rs`, which has no [`PathResolver`]) into P2
/// (`v4/paths.rs`) so P2 can resolve `mv` of a directory against the
/// filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub(crate) enum DerivedDestinations {
    /// Destinations are explicit operands only.
    #[default]
    None,
    /// Unconditionally writes to derived destinations (`cp -R`/`-a`, `rsync`,
    /// `tar -x`, `unzip`, `git clone`, `git checkout`), making the writer
    /// itself `Unread(DerivedDestinations)`.
    Always,
    /// Writes to derived destinations when any source operand resolves to a
    /// directory (`mv` of a directory; contract §5). P2 resolves source
    /// operands and emits `Unread(DerivedDestinations)` when a source is a
    /// directory (or cannot be established as non-directory).
    WhenSourceIsDir,
}

/// Tool-table classification of one modeled [`CommandUnit`] (contract §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolEffects {
    /// Folded name matched in §6's table (`None` for unknown tools).
    pub(crate) tool_name: Option<&'static str>,
    /// Matched subcommand name, if any.
    pub(crate) subcommand: Option<&'static str>,
    /// Per-argv-position role (same length as `unit.argv`).
    pub(crate) roles: Vec<OperandRole>,
    /// Per-argv-position filesystem access mode (same length as `unit.argv`),
    /// distinguishing e.g. `ln`/`cp` source reads from destination writes (§6).
    pub(crate) operand_access: Vec<AccessMode>,
    /// Whether the unit is a possible writer under §5 (making later units and
    /// pipeline siblings `Unread`), separate from per-operand `operand_access`
    /// (e.g. `sed -f print.sed input` has `writes = true` because the program
    /// is uninspected, while `print.sed` and `input` remain `AccessMode::Read`).
    pub(crate) writes: bool,
    /// Whether the unit writes to derived destinations, including conditional
    /// cases (`mv` of a directory) that P2 resolves against the filesystem (§5).
    pub(crate) derived_destinations: DerivedDestinations,
    /// Whether the unit reads directory trees recursively.
    pub(crate) reads_recursively: bool,
    /// Symlink-following behaviour at root and below root.
    pub(crate) links: LinkPolicy,
    /// Whether and how the unit runs code.
    pub(crate) code: CodeExecution,
    /// Filter dialect for search tools, if any.
    pub(crate) filter_dialect: Option<FilterDialect>,
    /// Extracted search filters (multiple or negated filters produce
    /// `UnreadReason::UnsupportedFilterForm`).
    pub(crate) filters: Vec<SearchFilter>,
    /// Eligible for Home's read-only list (matched on command word as written,
    /// without forbidden flags; §6).
    pub(crate) home_readonly_eligible: bool,
    /// Argv index of `cd`'s literal target operand, if this unit is a modeled
    /// `cd` (§5).
    pub(crate) cd_target_operand: Option<usize>,
    /// Whether this unit needs CR-12 Git object/pathspec checks (§6).
    pub(crate) git_selects_by_name: bool,
    /// Tool-level `UnreadReason`s established while classifying the unit.
    pub(crate) unread: Vec<UnreadReason>,
}

// ---------------------------------------------------------------------------
// Canonical targets & filesystem resolver DTOs (§5)
// ---------------------------------------------------------------------------

/// Unix `(st_dev, st_ino)` identity of a file or directory (contract §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct FileIdentity {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
}

/// Coarse file type returned by `v4::paths::PathResolver` (contract §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FileType {
    File,
    Dir,
    Symlink,
    Other,
}

/// Metadata returned by `v4::paths::PathResolver::stat` / `lstat` (contract §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct PathMetadata {
    pub(crate) file_type: FileType,
    pub(crate) identity: Option<FileIdentity>,
    /// Hard-link count (`st_nlink`); regular files with `nlink > 1` trigger
    /// §1 step 4's multiply-linked Ask.
    pub(crate) nlink: u64,
}

/// One directory entry returned by `v4::paths::PathResolver::read_dir`
/// (contract §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirEntry {
    /// Entry name as raw bytes (not a full path).
    pub(crate) name: Vec<u8>,
    pub(crate) file_type: FileType,
}

/// Non-absence filesystem I/O failure category surfaced by
/// `v4::paths::PathResolver` so P2 can distinguish unreadable directories and
/// OS errors requiring [`UnreadReason::UnresolvablePath`] from legitimate
/// absence (`ResolveError::NotFound`) (contract §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FsIoError {
    /// Permission denied (`EACCES` / `EPERM`).
    PermissionDenied,
    /// Expected a directory (`ENOTDIR`).
    NotADirectory,
    /// Expected a symbolic link (`EINVAL` on `readlink`).
    NotASymlink,
    /// Too many symbolic links (`ELOOP`).
    SymlinkLoop,
    /// Any other OS I/O error.
    Other,
}

/// Error returned by `v4::paths::PathResolver` methods (contract §2, §5, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ResolveError {
    /// The path (or an ancestor) does not exist (`ENOENT`), distinct from I/O
    /// failures so P2 can safely resolve a nonexistent suffix against its
    /// deepest existing ancestor.
    NotFound,
    /// A non-absence filesystem I/O failure (e.g. unreadable directory,
    /// permission denied, symlink loop).
    Io(FsIoError),
    /// A per-check filesystem cap in [`FsCaps`] was reached (callers convert
    /// this into `Unread(WalkCapReached)`, never `Exhausted`).
    CapReached(CapReached),
    /// The shared [`ComputeBudget`] (compute units or allocation bytes) ran
    /// out (callers propagate `Exhausted` → `Ask` everywhere).
    Exhausted(Exhausted),
}

impl From<CapReached> for ResolveError {
    fn from(value: CapReached) -> Self {
        Self::CapReached(value)
    }
}

impl From<Exhausted> for ResolveError {
    fn from(value: Exhausted) -> Self {
        Self::Exhausted(value)
    }
}

/// Result of checking one recursive read root (S8 protected-root policy,
/// filter enumeration + store probes, followed-link walk, and bounded
/// hard-link walk) (contract §1 step 4, §2, §5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RecursiveRootResult {
    pub(crate) outcomes: Vec<PartResult>,
    /// `true` iff the bounded hard-link walk found a regular file with
    /// `nlink > 1` under the root (forces `Ask` in every setting, including
    /// `FullAccess`, per §1 step 4).
    pub(crate) multiply_linked_file: bool,
}

/// Where a [`Target`] originated (contract §5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum TargetOrigin {
    Operand {
        unit: UnitIndex,
        argv_index: usize,
    },
    Redirect {
        unit: UnitIndex,
        redirect_index: usize,
    },
    SubjectPath {
        path_index: usize,
    },
    ImplicitCwd {
        unit: Option<UnitIndex>,
    },
    ExpandedFrom {
        source: PartId,
        /// `true` when matched only by a dialect variant (dot-match or
        /// recursive `**`), never by the sh/bash/zsh intersection (§5).
        variant_only: bool,
    },
}

/// Whether a [`Target`] is read, written, or conservatively checked for both
/// (contract §5, §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AccessMode {
    Read,
    Write,
    /// Conservative candidate path checked for **both** read and write
    /// protections (including `atrium-self` write-protected paths): used for
    /// unlisted flags under `FlagMode::AnyExceptForbidden`, unknown tools
    /// (§4), and unclassified words after an `UnknownFlag` fallback.
    ReadWrite,
}

/// Resolution result for one [`Target`] (contract §5).
///
/// Resolved paths are raw bytes, never glob patterns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TargetResolution {
    /// Canonicalized bytes and `(dev, ino)` identities of the path and of every
    /// existing ancestor. A dangling symlink counts under its own name.
    Resolved {
        canonical: Vec<u8>,
        identities: Vec<FileIdentity>,
        dangling_symlink: bool,
        multiply_linked: bool,
    },
    /// Resolution could not be established within §5's model.
    Unread(UnreadReason),
}

/// One resolved (or unread) filesystem target (contract §5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) origin: TargetOrigin,
    pub(crate) access: AccessMode,
    pub(crate) resolution: TargetResolution,
}

/// Result of expanding one shell glob against the filesystem (contract §5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GlobExpansion {
    /// Matches agreed on by all three dialects (`sh`, `bash`, `zsh` defaults),
    /// or the literal fallback when nothing matched.
    pub(crate) exact_matches: Vec<Target>,
    /// Matches found only under a dialect variant (dot-matching, recursive
    /// `**`): can only produce `Unread(VariantOnlyGlobMatch)`, never a hit.
    pub(crate) variant_only_matches: Vec<Target>,
    /// Any cap or resolution unread encountered during expansion.
    pub(crate) unread: Vec<UnreadReason>,
}

/// Output of P2 path resolution and checking for a call (contract §5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolvedCallPaths {
    pub(crate) targets: Vec<Target>,
    pub(crate) outcomes: Vec<PartResult>,
    pub(crate) multiply_linked_file: bool,
}

// ---------------------------------------------------------------------------
// Shared policy context data passed into P1–P3 (§4, §5)
// ---------------------------------------------------------------------------

/// Listed name-folding rules shared by P1, P2, and P3 (CR-19, contract §4, §5).
///
/// Exact folds: Unicode lowercase (`str::to_lowercase()`, matching `dev`'s
/// `deny.rs`/`engine.rs` path folds — which already maps Kelvin sign `K`
/// (U+212A) → `k` and folds e.g. `CAFÉ` → `café`) plus `ſ` (U+017F) → `s`,
/// with no Unicode normalization (NFC/NFD) added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FoldRules {
    /// Fold `ſ` (U+017F) to `s`.
    pub(crate) fold_long_s: bool,
    /// Fold `K` (U+212A) to `k` (implied by Unicode `to_lowercase()`, kept
    /// explicit for non-UTF-8 / byte-oriented paths).
    pub(crate) fold_kelvin: bool,
}

impl FoldRules {
    /// The normative rev 4 fold rules (CR-19 + `deny.rs`; contract §4, §5).
    pub(crate) const REV4: Self = Self {
        fold_long_s: true,
        fold_kelvin: true,
    };
}

/// Immutable sensitive/protected path configuration for one check, passed as a
/// parameter to P2/P3/P5 rather than imported ad hoc (contract §5, §13).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProtectedSet {
    /// Canonical home directory bytes.
    pub(crate) home: Vec<u8>,
    /// Canonical starting working directory bytes.
    pub(crate) cwd: Vec<u8>,
    /// Atrium data paths (neither read nor written; `atrium-self`).
    pub(crate) atrium_data_paths: Vec<Vec<u8>>,
    /// Atrium protected paths (never written; `atrium-self`).
    pub(crate) atrium_protected_paths: Vec<Vec<u8>>,
    /// Identities `(dev, ino)` of existing hard-deny files/trees and `~/.ssh`
    /// members, mapped to their `HARD_DENY_DEFAULTS` id.
    pub(crate) hard_deny_identities: Vec<(FileIdentity, &'static str)>,
    /// Identities `(dev, ino)` of existing credential stores that ask (CR-12,
    /// CR-17), mapped to their store family name.
    pub(crate) store_identities: Vec<(FileIdentity, &'static str)>,
    /// Canonical paths of directories symlinked into `~/.ssh`.
    pub(crate) ssh_linked_dirs: Vec<Vec<u8>>,
    /// Shared CR-19 / `deny.rs` fold rules.
    pub(crate) folds: FoldRules,
}

// ---------------------------------------------------------------------------
// Rule evaluation & call evaluation records (§1, §2, §7)
// ---------------------------------------------------------------------------

/// Reason a rule matcher is invalid under §7 (contract §7, §11 items 4–5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuleValidationError {
    /// Prefix/exact argv list is empty.
    EmptyArgv,
    /// An Allow prefix ends at a code runner with no pinned program (e.g.
    /// `["awk"]`, `["python"]`, `["pipx", "run"]`, `["git"]`).
    UnpinnedCodeRunner {
        tool: String,
        required_pin: &'static str,
    },
    /// Invalid glob syntax in `PathGlob` or `path_scope`.
    InvalidGlob { glob: String },
}

/// Borrowed view of a call passed to P3's rule evaluator (contract §7).
pub(crate) struct RuleEvalContext<'a> {
    pub(crate) check: &'a ToolCheck,
    pub(crate) thread_id: &'a atrium_protocol::thread::ThreadId,
    pub(crate) preset: Preset,
    pub(crate) units: &'a [CommandUnit],
    pub(crate) effects: &'a [ToolEffects],
    pub(crate) targets: &'a [Target],
    pub(crate) best_effort: &'a BestEffortRecord,
    pub(crate) folds: &'a FoldRules,
}

/// Rule coverage for one modeled unit (contract §1, §2, §7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct UnitRuleCoverage {
    /// Winning Allow rule covering `Clear` / `StoreHit` parts of this unit.
    pub(crate) allow_rule: Option<RuleId>,
    /// `true` iff covered by an unscoped `RuleMatcher::Any` Allow rule (§2).
    pub(crate) unscoped_any_allow: bool,
    /// `true` iff a covering prefix/exact Allow rule pins the program words of
    /// this unit's `UnreadReason::Program` (authorizes ONLY that reason, not
    /// any other `UnreadReason` on the same unit; §1 step 6, §7).
    pub(crate) covers_pinned_program: bool,
}

/// Aggregate rule evaluation result for a call (contract §1 step 3, §2, §7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RuleCoverage {
    /// First winning Deny rule (after thread > Home > global precedence).
    pub(crate) deny: Option<(String, RuleId)>,
    /// Per-unit Allow coverage (indexed by `UnitIndex`, or 1 entry for
    /// non-command calls).
    pub(crate) per_unit: Vec<UnitRuleCoverage>,
    /// Whole-call unscoped `Any` Allow rule, if one matched.
    pub(crate) unscoped_any_rule: Option<RuleId>,
}

/// End-to-end result of rev 4 call evaluation (contract §1, §2, §9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallEvaluation {
    pub(crate) verdict: PolicyVerdict,
    pub(crate) outcome: CallOutcome,
    /// "Always…" matcher to attach to an Ask ticket (`None` when any part is
    /// `Unread` or `Exhausted`; §7, §9).
    pub(crate) remember_matcher: Option<RuleMatcher>,
}

// ---------------------------------------------------------------------------
// Compute budget & filesystem caps (§2, §8)
// ---------------------------------------------------------------------------

/// Error returned when [`ComputeBudget`] runs out (contract §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Exhausted {
    pub(crate) reason: ExhaustReason,
}

impl From<Exhausted> for ExhaustReason {
    fn from(value: Exhausted) -> Self {
        value.reason
    }
}

/// One compute + allocation budget per check, charged **before** doing work or
/// allocating (contract §8).
///
/// Once exhausted, it stays exhausted for all subsequent calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComputeBudget {
    remaining_units: u64,
    remaining_alloc_bytes: usize,
    exhausted: Option<ExhaustReason>,
}

impl ComputeBudget {
    pub(crate) const fn new(max_units: u64, max_alloc_bytes: usize) -> Self {
        Self {
            remaining_units: max_units,
            remaining_alloc_bytes: max_alloc_bytes,
            exhausted: None,
        }
    }

    pub(crate) const fn with_default_limits() -> Self {
        Self::new(
            super::limits::COMPUTE_BUDGET_UNITS,
            super::limits::COMPUTE_BUDGET_ALLOC_BYTES,
        )
    }

    pub(crate) const fn is_exhausted(&self) -> bool {
        self.exhausted.is_some()
    }

    pub(crate) const fn exhausted_reason(&self) -> Option<ExhaustReason> {
        self.exhausted
    }

    pub(crate) const fn remaining_units(&self) -> u64 {
        self.remaining_units
    }

    pub(crate) const fn remaining_alloc_bytes(&self) -> usize {
        self.remaining_alloc_bytes
    }

    /// Charges `units` before performing work. Returns `Err(Exhausted)` if the
    /// budget is already exhausted or lacks `units`.
    pub(crate) fn charge(&mut self, units: u64) -> Result<(), Exhausted> {
        if let Some(reason) = self.exhausted {
            return Err(Exhausted { reason });
        }
        match self.remaining_units.checked_sub(units) {
            Some(rem) => {
                self.remaining_units = rem;
                Ok(())
            }
            None => {
                self.remaining_units = 0;
                self.exhausted = Some(ExhaustReason::ComputeBudget);
                Err(Exhausted {
                    reason: ExhaustReason::ComputeBudget,
                })
            }
        }
    }

    /// Charges `bytes` before performing a heap allocation. Returns
    /// `Err(Exhausted)` if the budget is already exhausted or lacks `bytes`.
    pub(crate) fn charge_alloc(&mut self, bytes: usize) -> Result<(), Exhausted> {
        if let Some(reason) = self.exhausted {
            return Err(Exhausted { reason });
        }
        match self.remaining_alloc_bytes.checked_sub(bytes) {
            Some(rem) => {
                self.remaining_alloc_bytes = rem;
                Ok(())
            }
            None => {
                self.remaining_alloc_bytes = 0;
                self.exhausted = Some(ExhaustReason::AllocationBudget);
                Err(Exhausted {
                    reason: ExhaustReason::AllocationBudget,
                })
            }
        }
    }
}

/// Error returned when a per-check filesystem cap is reached (contract §2, §8).
///
/// Callers convert this into [`UnreadReason::WalkCapReached`], never
/// `Exhausted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CapReached {
    pub(crate) cap: FsCap,
}

impl From<CapReached> for UnreadReason {
    fn from(value: CapReached) -> Self {
        Self::WalkCapReached(value.cap)
    }
}

/// Separate per-check filesystem operation caps (contract §2, §8).
///
/// Each operation is charged before it runs; reaching a cap returns
/// [`CapReached`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FsCaps {
    entries_read_remaining: usize,
    dir_opens_remaining: usize,
    stats_remaining: usize,
    link_reads_remaining: usize,
    canonicalizations_remaining: usize,
}

impl FsCaps {
    pub(crate) const fn new(
        entries_read: usize,
        dir_opens: usize,
        stats: usize,
        link_reads: usize,
        canonicalizations: usize,
    ) -> Self {
        Self {
            entries_read_remaining: entries_read,
            dir_opens_remaining: dir_opens,
            stats_remaining: stats,
            link_reads_remaining: link_reads,
            canonicalizations_remaining: canonicalizations,
        }
    }

    pub(crate) const fn with_default_limits() -> Self {
        Self::new(
            super::limits::FS_MAX_ENTRIES_READ,
            super::limits::FS_MAX_DIR_OPENS,
            super::limits::FS_MAX_STATS,
            super::limits::FS_MAX_READLINKS,
            super::limits::FS_MAX_CANONICALIZATIONS,
        )
    }

    pub(crate) fn charge_entries_read(&mut self, count: usize) -> Result<(), CapReached> {
        match self.entries_read_remaining.checked_sub(count) {
            Some(rem) => {
                self.entries_read_remaining = rem;
                Ok(())
            }
            None => {
                self.entries_read_remaining = 0;
                Err(CapReached {
                    cap: FsCap::EntriesRead,
                })
            }
        }
    }

    pub(crate) fn charge_dir_open(&mut self) -> Result<(), CapReached> {
        match self.dir_opens_remaining.checked_sub(1) {
            Some(rem) => {
                self.dir_opens_remaining = rem;
                Ok(())
            }
            None => {
                self.dir_opens_remaining = 0;
                Err(CapReached {
                    cap: FsCap::DirOpens,
                })
            }
        }
    }

    pub(crate) fn charge_stat(&mut self) -> Result<(), CapReached> {
        match self.stats_remaining.checked_sub(1) {
            Some(rem) => {
                self.stats_remaining = rem;
                Ok(())
            }
            None => {
                self.stats_remaining = 0;
                Err(CapReached { cap: FsCap::Stats })
            }
        }
    }

    pub(crate) fn charge_link_read(&mut self) -> Result<(), CapReached> {
        match self.link_reads_remaining.checked_sub(1) {
            Some(rem) => {
                self.link_reads_remaining = rem;
                Ok(())
            }
            None => {
                self.link_reads_remaining = 0;
                Err(CapReached {
                    cap: FsCap::LinkReads,
                })
            }
        }
    }

    pub(crate) fn charge_canonicalize(&mut self) -> Result<(), CapReached> {
        match self.canonicalizations_remaining.checked_sub(1) {
            Some(rem) => {
                self.canonicalizations_remaining = rem;
                Ok(())
            }
            None => {
                self.canonicalizations_remaining = 0;
                Err(CapReached {
                    cap: FsCap::Canonicalizations,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_is_one_byte_and_glob_respects_quotes() {
        assert_eq!(std::mem::size_of::<ByteProvenance>(), 1);

        let mut unquoted = Word::new();
        unquoted.extend_with_provenance(b"proj/key*", ByteProvenance::Unquoted);
        assert!(unquoted.has_unquoted_glob());
        assert_eq!(unquoted.literal_bytes(), b"proj/key*");

        for prov in [
            ByteProvenance::SingleQuoted,
            ByteProvenance::DoubleQuoted,
            ByteProvenance::Escaped,
            ByteProvenance::Derived,
        ] {
            let mut w = Word::new();
            w.extend_with_provenance(b"proj/*?[a]", prov);
            assert!(!w.has_unquoted_glob(), "glob in {prov:?} must not expand");
            assert_eq!(w.literal_bytes(), b"proj/*?[a]");
        }
    }

    #[test]
    fn compute_budget_is_sticky_once_exhausted() {
        let mut b = ComputeBudget::new(10, 100);
        assert!(b.charge(6).is_ok());
        assert!(b.charge_alloc(40).is_ok());
        assert_eq!(
            b.charge(5),
            Err(Exhausted {
                reason: ExhaustReason::ComputeBudget
            })
        );
        assert!(b.is_exhausted());
        // Even 0-unit and allocation charges stay exhausted.
        assert!(b.charge(0).is_err());
        assert!(b.charge_alloc(0).is_err());

        let mut b2 = ComputeBudget::new(100, 10);
        assert_eq!(
            b2.charge_alloc(11),
            Err(Exhausted {
                reason: ExhaustReason::AllocationBudget
            })
        );
        assert!(b2.charge(0).is_err());
    }

    #[test]
    fn fs_caps_report_cap_reached_and_convert_to_unread() {
        let mut caps = FsCaps::new(2, 1, 1, 1, 1);
        assert!(caps.charge_entries_read(2).is_ok());
        let err = caps.charge_entries_read(1).unwrap_err();
        assert_eq!(
            UnreadReason::from(err),
            UnreadReason::WalkCapReached(FsCap::EntriesRead)
        );
        assert!(caps.charge_dir_open().is_ok());
        assert_eq!(
            caps.charge_dir_open(),
            Err(CapReached {
                cap: FsCap::DirOpens
            })
        );
        assert!(caps.charge_stat().is_ok());
        assert_eq!(caps.charge_stat(), Err(CapReached { cap: FsCap::Stats }));
        assert!(caps.charge_link_read().is_ok());
        assert_eq!(
            caps.charge_link_read(),
            Err(CapReached {
                cap: FsCap::LinkReads
            })
        );
        assert!(caps.charge_canonicalize().is_ok());
        assert_eq!(
            caps.charge_canonicalize(),
            Err(CapReached {
                cap: FsCap::Canonicalizations
            })
        );
    }

    #[test]
    fn model_redirect_harmless_and_write_checks() {
        let dev_null = Word::derived(b"/dev/null".to_vec());
        let out_file = Word::derived(b"out.txt".to_vec());

        assert!(
            ModelRedirect::Dup {
                from: StdFd::Stderr,
                to: StdFd::Stdout
            }
            .is_harmless_for_rules()
        );
        assert!(ModelRedirect::Close { fd: StdFd::Stderr }.is_harmless_for_rules());
        assert!(
            ModelRedirect::Write {
                target: dev_null.clone()
            }
            .is_harmless_for_rules()
        );
        assert!(!ModelRedirect::Write { target: dev_null }.writes_outside_dev_null());
        assert!(
            ModelRedirect::Write {
                target: out_file.clone()
            }
            .writes_outside_dev_null()
        );
        assert!(
            !ModelRedirect::Write {
                target: out_file.clone()
            }
            .is_harmless_for_rules()
        );
        assert!(!ModelRedirect::Read { target: out_file }.is_harmless_for_rules());
    }
}
