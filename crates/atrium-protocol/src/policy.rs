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
    /// `ExternalHooks`, `GateUnreachable`) always make it Partial.
    pub fn evaluate(
        provider: Provider,
        cli_version: Option<String>,
        version_tested: bool,
        coverage: Vec<ClassCoverage>,
        config_sources: Vec<ConfigSource>,
        extra: Vec<EnforcementReason>,
    ) -> Self {
        let mut reasons = extra;
        if !version_tested {
            reasons.push(EnforcementReason {
                code: ReasonCode::UntestedVersion,
                message: match &cli_version {
                    Some(v) => format!("CLI version {v} hasn't been tested with Atrium"),
                    None => "CLI version unknown".to_owned(),
                },
            });
        }
        for class in ToolClass::ALL {
            let found = coverage
                .iter()
                .find(|c| c.class == class)
                .map(|c| c.coverage);
            let code = match found {
                Some(Coverage::Intercepted | Coverage::NotApplicable) => continue,
                Some(Coverage::ProviderGated | Coverage::NotIntercepted) => {
                    ReasonCode::ClassNotIntercepted
                }
                Some(Coverage::Unknown) | None => ReasonCode::CoverageUnknown,
            };
            reasons.push(EnforcementReason {
                code,
                message: format!("{class:?} calls: {:?}", found.unwrap_or(Coverage::Unknown)),
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
// Hard-deny defaults (§9). NEEDS PEDRO'S EXPLICIT APPROVAL (D-013).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HardDenyDefault {
    pub id: &'static str,
    /// Shown in settings and when a call is blocked.
    pub description: &'static str,
}

/// The M1 defaults. Matching lives in `atrium-core::policy`; each entry has
/// adversarial tests (M1-13). No rule or remembered decision overrides them
/// for calls the gate sees.
pub const HARD_DENY_DEFAULTS: &[HardDenyDefault] = &[
    HardDenyDefault {
        id: "recursive-delete-outside-cwd",
        description: "Recursive delete (rm -r, find -delete, …) of a path outside the thread's folder, or of ~ or /",
    },
    HardDenyDefault {
        id: "force-push-protected",
        description: "Force-push (--force, -f, --force-with-lease, +refspec) to main, master, release/* or the remote's default branch",
    },
    HardDenyDefault {
        id: "credential-read",
        description: "Reading credential stores: ~/.ssh private keys, ~/.aws/credentials, ~/.netrc, gh/docker/kube credentials, ~/.gnupg, browser login and cookie databases, keychain and secret-tool lookups",
    },
    HardDenyDefault {
        id: "pipe-to-shell",
        description: "Downloading and piping into a shell or interpreter (curl … | sh, wget -O- … | bash, …) outside a sandbox",
    },
    HardDenyDefault {
        id: "atrium-self",
        description: "Writing to Atrium's own data, socket and app folders",
    },
    HardDenyDefault {
        id: "privilege-escalation",
        description: "sudo, su, doas",
    },
    HardDenyDefault {
        id: "disk-wipe",
        description: "Formatting or overwriting disks (mkfs, dd of=/dev/…, diskutil erase…)",
    },
];
