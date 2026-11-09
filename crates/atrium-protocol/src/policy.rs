//! Enforcement and policy types (PLAN §3.12, contract §3, §5, §9).
//!
//! # Coverage, policy, isolation (D-044)
//!
//! Three different things, never blurred in code or UI:
//! - **coverage**: which tool calls Atrium sees before they run
//!   ([`EnforcementDescriptor`]);
//! - **policy**: what Atrium does with the calls it sees ([`Rule`], [`Preset`],
//!   the hard-deny list);
//! - **isolation**: what limits code that was allowed to run. Only the sandbox
//!   (M4) provides it. An allowed `cargo test` can run a build script that does
//!   anything the user can.
//!
//! The hard-deny list takes precedence over every rule and remembered
//! decision **for calls Atrium sees**. It is not a machine-wide prohibition.

use serde::{Deserialize, Serialize};

use crate::agent::{Provider, ToolKind};
use crate::thread::ThreadId;

// ---------------------------------------------------------------------------
// Enforcement descriptor (G1)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnforcementLevel {
    /// Every tool call, of every class, passes Atrium's gate before it runs,
    /// on a tested CLI version, with every configuration source inspected.
    Full,
    /// Anything less, including anything Atrium couldn't verify.
    Partial,
}

/// The classes an adapter must account for. `Full` needs all of them
/// `Intercepted`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Fetch,
    Mcp,
    Subagent,
    /// Anything provider-specific (e.g. OpenCode Code Mode, Claude plugins).
    ProviderSpecific,
}

impl ToolClass {
    pub const ALL: [ToolClass; 10] = [
        ToolClass::Read,
        ToolClass::Edit,
        ToolClass::Delete,
        ToolClass::Move,
        ToolClass::Search,
        ToolClass::Execute,
        ToolClass::Fetch,
        ToolClass::Mcp,
        ToolClass::Subagent,
        ToolClass::ProviderSpecific,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    /// Every call of this class reaches the gate before it runs.
    Intercepted,
    /// Only the calls the provider itself chooses to ask about.
    ProviderGated,
    /// Calls of this class run without Atrium seeing them first.
    NotIntercepted,
    /// Not verified for this provider, version or configuration.
    Unknown,
    /// The provider has no tools of this class in this session.
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassCoverage {
    pub class: ToolClass,
    pub coverage: Coverage,
    /// The fixture or test that shows it, e.g. `fixtures/agents/claude/gate-deny`.
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSourceKind {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    ManagedPolicy,
    /// A settings file passed explicitly (e.g. Claude `--settings`).
    ExplicitSettings,
    Plugin,
    Hooks,
    Other,
}

/// A configuration source the provider loads for this session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSource {
    pub kind: ConfigSourceKind,
    /// Sanitized path (home directory as `~`).
    pub path: String,
    /// Lowercase hex SHA-256 of the content when read.
    pub sha256: Option<String>,
    /// The adapter parsed it and accounted for what it can do (hooks, permission
    /// overrides). `false` makes the session Partial.
    pub inspected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    ClassNotIntercepted,
    CoverageUnknown,
    UntestedVersion,
    UninspectedConfig,
    /// Hooks outside Atrium's control can run commands or rewrite inputs.
    ExternalHooks,
    GateUnreachable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcementReason {
    pub code: ReasonCode,
    /// One line for the UI, e.g. "Plugin hooks are not inspected".
    pub message: String,
}

/// What Atrium can enforce for one session. Persisted with the session and
/// shown in the thread header, the provider picker and settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnforcementDescriptor {
    pub level: EnforcementLevel,
    /// Empty only when `level` is `Full`.
    pub reasons: Vec<EnforcementReason>,
    pub provider: Provider,
    pub cli_version: Option<String>,
    /// `cli_version` is in the adapter's tested set.
    pub version_tested: bool,
    pub coverage: Vec<ClassCoverage>,
    pub config_sources: Vec<ConfigSource>,
}

impl EnforcementDescriptor {
    /// The only way adapters build a descriptor: computes `level` and adds a
    /// reason for everything that isn't verified. `extra` reasons (e.g.
    /// `ExternalHooks`, `GateUnreachable`) always make it Partial. A missing
    /// `cli_version` is never Full, and when a class has several entries the
    /// weakest one counts.
    pub fn evaluate(
        provider: Provider,
        cli_version: Option<String>,
        version_tested: bool,
        coverage: Vec<ClassCoverage>,
        config_sources: Vec<ConfigSource>,
        extra: Vec<EnforcementReason>,
    ) -> Self {
        let mut reasons = extra;
        if !version_tested || cli_version.is_none() {
            reasons.push(EnforcementReason {
                code: ReasonCode::UntestedVersion,
                message: match &cli_version {
                    Some(v) => format!("CLI version {v} hasn't been tested with Atrium"),
                    None => "CLI version unknown".to_owned(),
                },
            });
        }
        for class in ToolClass::ALL {
            let entries: Vec<Coverage> = coverage
                .iter()
                .filter(|c| c.class == class)
                .map(|c| c.coverage)
                .collect();
            let weakest = if entries.is_empty() || entries.contains(&Coverage::Unknown) {
                Coverage::Unknown
            } else if let Some(bad) = entries
                .iter()
                .copied()
                .find(|c| !matches!(c, Coverage::Intercepted | Coverage::NotApplicable))
            {
                bad
            } else {
                continue;
            };
            let code = match weakest {
                Coverage::Unknown => ReasonCode::CoverageUnknown,
                _ => ReasonCode::ClassNotIntercepted,
            };
            reasons.push(EnforcementReason {
                code,
                message: format!("{class:?} calls: {weakest:?}"),
            });
        }
        for source in config_sources.iter().filter(|s| !s.inspected) {
            reasons.push(EnforcementReason {
                code: ReasonCode::UninspectedConfig,
                message: format!("{:?} not inspected: {}", source.kind, source.path),
            });
        }
        let level = if reasons.is_empty() {
            EnforcementLevel::Full
        } else {
            EnforcementLevel::Partial
        };
        Self {
            level,
            reasons,
            provider,
            cli_version,
            version_tested,
            coverage,
            config_sources,
        }
    }
}

// ---------------------------------------------------------------------------
// Presets, rules, approvals (§5, §9)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Home: read freely, approve changes (PLAN §3.3). M1's only preset in use.
    Home,
    Careful,
    Normal,
    Trusted,
    /// The provider's full-access mode, chosen by the user (D-011).
    FullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleDecision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleScope {
    Thread { thread_id: ThreadId },
    Home,
    Global,
}

/// "Always…" choices offered with an approval. The thread is implied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RememberScope {
    Thread,
    Home,
    Global,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleMatcher {
    /// Every call of the rule's tool kind (and name, if set).
    Any,
    /// A simple command whose argv starts with these tokens (contract §9).
    CommandPrefix { argv: Vec<String> },
    /// A path argument matching this glob (`**` allowed), after the path is
    /// made absolute against the thread's cwd and symlinks are resolved.
    PathGlob { glob: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuleId(pub String);

/// What a tool call acts on, extracted by the adapter so the policy engine
/// never guesses from provider-specific input shapes (review finding 3).
/// Anything the adapter can't extract is `Unknown`, which the policy treats
/// as `Ask` (after the hard-deny check on the raw input).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolSubject {
    /// A shell command line. The call's `kind` is always `Execute`, whatever
    /// the provider calls it (e.g. Codex `commandExecution` with read
    /// actions).
    Command {
        line: String,
        /// The directory it runs in, if not the thread's cwd.
        cwd: Option<String>,
    },
    /// File tools: every path the call reads or writes, as given (the gate
    /// resolves them). Searches give their root and glob.
    Paths {
        paths: Vec<String>,
        /// For searches: the glob or pattern scope, if any.
        glob: Option<String>,
        recursive: bool,
    },
    Fetch {
        url: String,
    },
    Mcp {
        server: String,
        tool: String,
    },
    /// No side effects outside the conversation (e.g. thinking, plans).
    None,
    Unknown,
}

/// A rule as a client creates it; the daemon assigns `id`, `origin` and
/// `created_at_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleDraft {
    pub scope: RuleScope,
    pub tool_kind: ToolKind,
    pub tool_name: Option<String>,
    pub matcher: RuleMatcher,
    pub path_scope: Option<String>,
    pub decision: RuleDecision,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuleOrigin {
    /// Created by "always…" on an approval.
    Approval { request_id: String },
    /// Created or edited in settings.
    Settings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    pub id: RuleId,
    pub scope: RuleScope,
    pub tool_kind: ToolKind,
    /// The provider's tool name, when the rule is that specific.
    pub tool_name: Option<String>,
    pub matcher: RuleMatcher,
    /// The rule applies only when every path the call touches is inside this
    /// glob.
    pub path_scope: Option<String>,
    pub decision: RuleDecision,
    pub origin: RuleOrigin,
    pub created_at_ms: u64,
}

/// What a policy decision was evaluated against. Bound into every approval
/// ticket (D-046).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyContext {
    /// Increases on every rule change.
    pub rules_revision: u64,
    pub preset: Preset,
}

// ---------------------------------------------------------------------------
// Hard-deny defaults and Home's auto-allowed commands (§9). Approved by
// Pedro (D-050). Changes need his approval (D-013).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardDenyDefault {
    pub id: &'static str,
    /// Shown in settings and when a call is blocked.
    pub description: &'static str,
}

/// Matching lives in `atrium-core::policy`; each entry has adversarial tests
/// (M1-13). No rule or remembered decision overrides them for calls the gate
/// sees. Applies to every tool kind whose subject names a matching command
/// or path, searches included.
pub const HARD_DENY_DEFAULTS: &[HardDenyDefault] = &[
    HardDenyDefault {
        id: "recursive-delete-outside-cwd",
        description: "Recursive delete (rm -r, find -delete, …) of a path outside the thread's folder, of the folder itself, or of ~, / or any ancestor of ~. In Home, `.` is ~, so it counts.",
    },
    HardDenyDefault {
        id: "force-push-protected",
        description: "Force-push (--force, -f, --force-with-lease, --force-if-includes, +refspec) unless every destination is an explicit branch that isn't main, master, dev, develop, trunk or release/*. A bare force-push, or one whose destination is HEAD or implicit, is blocked.",
    },
    HardDenyDefault {
        id: "credential-read",
        description: "Reading or writing credential stores: every private key in ~/.ssh (and writing ~/.ssh/config or authorized_keys), ~/.aws/credentials, ~/.netrc, ~/.git-credentials, ~/.npmrc, ~/.docker/config.json, ~/.kube/config, ~/.gnupg, ~/.config/gh/hosts.yml, ~/.config/gcloud, agent tokens (~/.claude/.credentials.json, ~/.claude.json, ~/.codex/auth.json, ~/.gemini), browser login and cookie databases, keychain dumps and lookups (security find-*-password, dump-keychain), secret-tool lookup",
    },
    HardDenyDefault {
        id: "pipe-to-shell",
        description: "Running downloaded code outside a sandbox: curl or wget piped into a shell or interpreter, or used in command or process substitution with sh, bash, zsh, eval or source (bash -c \"$(curl …)\", sh <(curl …))",
    },
    HardDenyDefault {
        id: "atrium-self",
        description: "Reading or writing Atrium's data (database, WAL, snapshots, logs: other threads' transcripts), and writing its config, socket and service files or its app",
    },
    HardDenyDefault {
        id: "privilege-escalation",
        description: "sudo, su, doas, pkexec, run0, osascript … with administrator privileges",
    },
    HardDenyDefault {
        id: "disk-wipe",
        description: "Formatting or overwriting disks (mkfs, dd of=/dev/…, diskutil erase…)",
    },
];

/// A command Home allows without asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadOnlyCommand {
    /// The argv prefix, e.g. `["git", "log"]`.
    pub prefix: &'static [&'static str],
    /// Flags that make it write, execute or configure something; their
    /// presence (as `--flag`, `--flag=…` or a combined short flag) means ask.
    pub forbidden: &'static [&'static str],
    /// If set, every further argument must be one of these flags (no
    /// positional arguments), e.g. `git branch` only lists.
    pub only: Option<&'static [&'static str]>,
    /// Whether it searches recursively (D-050: a recursive search rooted at
    /// `~` itself asks).
    pub recursion: Recursion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recursion {
    Never,
    /// Always recursive into directory arguments (`rg`).
    Always,
    /// Recursive when one of these flags is present (`grep -r`).
    WithFlags(&'static [&'static str]),
}

const GIT_FORBIDDEN: &[&str] = &[
    "--output",
    "--ext-diff",
    "--exec-path",
    "--config-env",
    "-c",
];

const fn cmd(prefix: &'static [&'static str]) -> ReadOnlyCommand {
    ReadOnlyCommand {
        prefix,
        forbidden: &[],
        only: None,
        recursion: Recursion::Never,
    }
}

const fn git(sub: &'static [&'static str]) -> ReadOnlyCommand {
    ReadOnlyCommand {
        prefix: sub,
        forbidden: GIT_FORBIDDEN,
        only: None,
        recursion: Recursion::Never,
    }
}

/// Commands Home allows without asking (D-050), when the whole line is one
/// simple command (or several joined by `&&`, `||`, `;`, `|`, each allowed)
/// with no `$` expansion, globs, brace expansion or newline, no
/// `git -c`/`--config-env`/`--exec-path`, no leading `NAME=value`
/// assignment, and no redirect other than fd duplication or output to
/// `/dev/null` (in Home the cwd is `~`, so `ls > ~/.zshrc` would otherwise
/// be "inside the folder"). The hard-deny list applies on top (so
/// `cat ~/.ssh/id_ed25519` is blocked).
///
/// A recursive search (`recursion`) asks unless every search root is given
/// and resolves to a file or to a folder strictly inside `~`: `rg token`
/// with no path, or `grep -r token .`, searches all of home (D-050). A flag
/// that takes a separate value (`-e`, `-g`, `-t`, `-m`, …) consumes the
/// next word, which is not a search root; a flag the matcher doesn't know
/// makes the command ask.
///
/// Known limit, stated in settings: a repository's own `.git/config` can make
/// git run programs (`core.fsmonitor`, textconv and diff drivers), so
/// auto-allowed git reads trust the repositories already on disk.
pub const HOME_READONLY_COMMANDS: &[ReadOnlyCommand] = &[
    cmd(&["ls"]),
    cmd(&["pwd"]),
    cmd(&["cat"]),
    cmd(&["head"]),
    cmd(&["tail"]),
    cmd(&["wc"]),
    ReadOnlyCommand {
        prefix: &["file"],
        // `-f` reads a list of names from a file and echoes its lines.
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
    },
    cmd(&["stat"]),
    cmd(&["du"]),
    cmd(&["df"]),
    cmd(&["which"]),
    cmd(&["uname"]),
    ReadOnlyCommand {
        prefix: &["date"],
        forbidden: &["-s", "--set"],
        only: None,
        recursion: Recursion::Never,
    },
    cmd(&["whoami"]),
    ReadOnlyCommand {
        prefix: &["tree"],
        // `-R` with `-H` writes an HTML file into every directory.
        forbidden: &["-o", "--output", "-R"],
        only: None,
        recursion: Recursion::Never,
    },
    ReadOnlyCommand {
        prefix: &["rg"],
        // `--hostname-bin` runs a program.
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
    },
    ReadOnlyCommand {
        prefix: &["grep"],
        // `-f` reads patterns from a file the gate can't see as a search root.
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
    },
    git(&["git", "status"]),
    git(&["git", "log"]),
    git(&["git", "diff"]),
    git(&["git", "show"]),
    ReadOnlyCommand {
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
    },
    git(&["git", "rev-parse"]),
    git(&["git", "ls-files"]),
];
