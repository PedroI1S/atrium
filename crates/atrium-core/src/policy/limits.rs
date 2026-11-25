//! Frozen numeric caps and budgets for policy engine rev 4 (contract §8, Q2).
//!
//! Not wired into `SessionGate` until P5; `dev`'s evaluator stays live in the
//! meantime. Values shared with `dev` are asserted equal in tests below.

#![allow(dead_code)]

// ---------------------------------------------------------------------------
// Structural size limits (§1, §8) — exceeding these produces TooLarge -> Deny.
// ---------------------------------------------------------------------------

/// Maximum command line length (or total string bytes for `ToolSubject::Unknown`),
/// matching `deny::MAX_BYTES` (128 KiB; contract §1, §8).
pub(crate) const MAX_BYTES: usize = 128 * 1024;

/// Maximum number of command units per check, matching `shell::MAX_UNITS`
/// (contract §1, §8).
pub(crate) const MAX_UNITS: usize = 512;

/// Maximum number of path/word uses per check, matching `words::MAX_USES`
/// (contract §1, §8).
pub(crate) const MAX_USES: usize = 4096;

/// Maximum number of resolved paths per check, matching `words::MAX_PATHS`
/// (contract §1, §8).
pub(crate) const MAX_PATHS: usize = 16384;

/// Maximum nesting depth for modeled `sh`/`bash`/`dash -c` scripts, matching
/// `shell::MAX_DEPTH` (contract §4, §8).
pub(crate) const MAX_DEPTH: u8 = 6;

/// Maximum symlink resolution hops per path, matching `gate::MAX_HOPS`
/// (contract §5, §8).
pub(crate) const MAX_HOPS: u8 = 40;

// ---------------------------------------------------------------------------
// Command-model limits (§4) — reaching these produces Unread.
// ---------------------------------------------------------------------------

/// Maximum number of consecutive §4 wrappers stripped from a unit's argv
/// before the unit becomes `Unread(MaxWrapReached)` (contract §4, §10 row 5).
pub(crate) const MAX_WRAP: usize = 8;

// ---------------------------------------------------------------------------
// Filesystem walk & glob limits (§2, §5, §8) — reaching these produces Unread
// (except the hard-link walk, whose local cutoff counts as clear per §2, §5).
// ---------------------------------------------------------------------------

/// Maximum directory entries inspected by a single recursive filter or
/// followed-link walk (contract §5, §8; matches `gate.rs` `expand_glob` /
/// `links_below`).
pub(crate) const WALK_MAX_ENTRIES: usize = 4096;

/// Maximum directory recursion depth for a followed-link walk (contract §5, §8;
/// matches `gate.rs` `links_below`).
pub(crate) const WALK_MAX_DEPTH: u8 = 16;

/// Maximum directory depth for shell glob expansion (contract §5, §8; matches
/// `gate.rs` `expand_glob`).
pub(crate) const GLOB_MAX_DEPTH: usize = 12;

/// Maximum directory entries inspected by the bounded hard-link walk
/// (contract §2, §5, §8, §11 item 7; matches `gate.rs` `tree_has_multiply_linked`).
pub(crate) const HARD_LINK_WALK_ENTRIES: usize = 4096;

/// Maximum directory depth for the bounded hard-link walk (contract §2, §5, §8;
/// matches `gate.rs` `reaches_multiply_linked`).
pub(crate) const HARD_LINK_WALK_DEPTH: u8 = 8;

/// Directory names skipped by the bounded hard-link walk (contract §5, §8;
/// matches `gate.rs` `tree_has_multiply_linked`).
pub(crate) const HARD_LINK_WALK_SKIP_DIRS: &[&str] = &["node_modules", ".git"];

// ---------------------------------------------------------------------------
// Per-check filesystem operation caps (§2, §8) — charged before each syscall;
// running out returns `CapReached`, which callers turn into Unread.
// ---------------------------------------------------------------------------

/// Total directory entries read across all walks in a single check (4×
/// `WALK_MAX_ENTRIES`, covering multi-root searches and multi-unit commands
/// while keeping total I/O bounded; contract §8).
pub(crate) const FS_MAX_ENTRIES_READ: usize = 16_384;

/// Total directory opens (`opendir` / `read_dir`) across a single check
/// (contract §8).
pub(crate) const FS_MAX_DIR_OPENS: usize = 1_024;

/// Total `stat` / `lstat` calls across a single check (covers operand
/// resolution, ancestor identity walks, and glob/filter entries; contract §8).
pub(crate) const FS_MAX_STATS: usize = 16_384;

/// Total `readlink` calls across a single check (covers up to `MAX_HOPS` = 40
/// hops across ~25 symlink-heavy paths or link walks; contract §8).
pub(crate) const FS_MAX_READLINKS: usize = 1_024;

/// Total path canonicalizations across a single check (matches `MAX_USES` =
/// 4,096 so large argument lists do not trip prematurely; contract §8).
pub(crate) const FS_MAX_CANONICALIZATIONS: usize = 4_096;

// ---------------------------------------------------------------------------
// Per-check compute & allocation budget (§8) — charged before work/alloc;
// running out produces Exhausted -> Ask everywhere.
// ---------------------------------------------------------------------------

/// Per-check compute budget in work units, set as a function of `MAX_BYTES`
/// (`128 * MAX_BYTES` = 16,777,216 units; contract §8).
///
/// Rationale:
/// - Exact lexing + per-byte provenance construction scans and copies the line
///   in linear time (~4 × `MAX_BYTES`).
/// - Recursive `-c` lexing up to `MAX_DEPTH` = 6 scans strictly smaller
///   substrings (~12 × `MAX_BYTES` worst case).
/// - Wrapper unwrapping (≤ `MAX_WRAP` = 8), CR-19 folding, and tool-table
///   classification are linear in argv bytes (~4 × `MAX_BYTES`).
/// - Path canonicalization, fold comparison, and sensitive-root/probe matching
///   across up to `MAX_USES` (4,096) operands or `WALK_MAX_ENTRIES` (4,096)
///   walk entries take ~32 × `MAX_BYTES`.
/// - The best-effort pass (raw-text normalization + ~60 credential/data form
///   scans + tolerant lexing) takes ~64 × `MAX_BYTES` on a full `MAX_BYTES`
///   input.
///
/// Summed (~116 × `MAX_BYTES`), `128 * MAX_BYTES` comfortably covers every row
/// of contract §10 (all small; row 20 hits `MAX_UNITS` before compute) and the
/// longest lines in `crates/atrium-core/tests/adversarial*.rs` (including
/// 4,000-byte deeply nested paths in `adversarial_gate.rs` and `MAX_BYTES`-scale
/// padded inputs), while still halting any quadratic or exponential blowup
/// (`O(MAX_BYTES^2)` ≈ 1.7 × 10^10 units) well within interactive latency.
pub(crate) const COMPUTE_BUDGET_UNITS: u64 = (MAX_BYTES as u64) * 128;

/// Per-check heap allocation budget in bytes, set as a function of `MAX_BYTES`
/// (`64 * MAX_BYTES` = 8 MiB; contract §8).
///
/// Covers exact `Word` + provenance buffers, best-effort unit copies, up to
/// `MAX_PATHS` (16,384) resolved path buffers, and `WALK_MAX_ENTRIES` (4,096)
/// walk entries, while preventing memory exhaustion from adversarial inputs.
pub(crate) const COMPUTE_BUDGET_ALLOC_BYTES: usize = MAX_BYTES * 64;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_limits_match_dev() {
        use super::super::dev_parity::{deny, shell, words};
        assert_eq!(MAX_BYTES, deny::MAX_BYTES);
        assert_eq!(MAX_UNITS, shell::MAX_UNITS);
        assert_eq!(MAX_USES, words::MAX_USES);
        assert_eq!(MAX_PATHS, words::MAX_PATHS);
        assert_eq!(MAX_DEPTH, shell::MAX_DEPTH);
        assert_eq!(MAX_HOPS, super::super::gate::MAX_HOPS);
    }

    #[test]
    fn frozen_rev4_values_match_contract() {
        assert_eq!(MAX_WRAP, 8);
        assert_eq!(WALK_MAX_ENTRIES, 4096);
        assert_eq!(GLOB_MAX_DEPTH, 12);
        assert_eq!(HARD_LINK_WALK_ENTRIES, 4096);
        assert_eq!(HARD_LINK_WALK_DEPTH, 8);
        assert_eq!(HARD_LINK_WALK_SKIP_DIRS, &["node_modules", ".git"]);
        const {
            assert!(FS_MAX_ENTRIES_READ >= WALK_MAX_ENTRIES);
            assert!(FS_MAX_DIR_OPENS > 0);
            assert!(FS_MAX_STATS >= WALK_MAX_ENTRIES);
            assert!(FS_MAX_READLINKS >= MAX_HOPS as usize);
            assert!(FS_MAX_CANONICALIZATIONS >= MAX_USES);
        }
        assert_eq!(COMPUTE_BUDGET_UNITS, (MAX_BYTES as u64) * 128);
        assert_eq!(COMPUTE_BUDGET_ALLOC_BYTES, MAX_BYTES * 64);
    }
}
