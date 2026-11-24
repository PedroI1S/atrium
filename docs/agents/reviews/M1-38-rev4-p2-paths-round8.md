## Verdict

**Request changes.**

Reviewed P2’s changes in `git diff dev...HEAD` at `de12d00`. Round 7’s four reported failing cases are fixed. Three new defects remain.

P2 preserves the frozen interface and remains unwired from `SessionGate`. No files were modified or tests run.

## Findings

1. **High — Omitting SSH directory members loses identity aliases.**

   **Location:** [sensitive.rs:1744](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1744), [sensitive.rs:2791](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:2791).

   **Failing case:** create empty `/h/u/.ssh/keys`, then give that directory another name with `FakeFs::bind_dir(..., b"/mnt/keys")`. Build protection with ample resources. Reading `/mnt/keys/future` through `resolve_subject_paths` returns Clear.

   Dev’s `ssh_members` includes ordinary SSH subdirectories in its alias roots. Its `alias_fix` therefore rewrites this target to `/h/u/.ssh/keys/future`, producing HardDeny. P2 neither includes that directory in `RootAliases` nor records its identity in `ProtectedSet`. No collection cutoff is involved.

   **Fix:** retain dev’s SSH member-directory aliases, including their spelling and name exceptions. The handoff’s claim that member identities fully replace these alias roots is incorrect.

2. **Medium — An incomplete-table reason is cloned before its allocation charge.**

   **Location:** [sensitive.rs:1809](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1809).

   **Failing case:** populate `RootAliases` with an I/O failure that records `UnresolvablePath`, such as an unlistable `.azure` directory. Reuse this cached table in `scan` with zero allocation budget. `self.incomplete.clone()` allocates the reason’s path buffer before line 1811 reports Exhausted.

   This violates §8’s charge-before-allocation requirement; copying those bytes also needs a compute charge.

   **Fix:** inspect the borrowed reason, charge its allocation and copying work, then clone it.

3. **Medium — MSAL alias discovery undercharges lossy-name allocations.**

   **Location:** [sensitive.rs:1780](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1780).

   **Failing case:** put an entry whose name contains 64 `0xFF` bytes in the fake `.azure` directory. Discovery charges 256 allocation bytes. `String::from_utf8_lossy` produces 192 bytes of replacement characters, and `to_lowercase` allocates another 192-byte string: at least 384 bytes, exceeding the charge. Both allocations execute with only 256 bytes budgeted for them.

   **Fix:** precharge both temporary buffers using bounds that include invalid-UTF-8 expansion, and charge processing of the expanded text before running it.

## Test gaps

Add regressions for SSH subdirectory aliases and allocation accounting for path-bearing incomplete reasons and invalid-UTF-8 names. The monotonic budget sweep does not establish allocation ordering.

The handoff’s other points check out: `.azure` is excluded from wholesale followed-link protection; reading `~/.ssh/config` is exempt; non-recursive `msal*` filters use probes. The dotless-i test correctly describes conservative handling rather than default Unicode folding.

Real-filesystem fixtures use temporary fake homes and separate builder resources. The available verification log still identifies `a986038`, so it does not verify round 9.