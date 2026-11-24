## Verdict

**Request changes.**

Reviewed `git diff 4f18a59...HEAD` against D-145. P2 stays within its owned paths, preserves P0’s interface, and does not wire anything into `SessionGate`.

Round 3’s reported reproductions are addressed or superseded by D-145. Finding 6’s allocation audit remains incomplete for the additional cases below. D-145’s accepted member cutoffs and direct external reads are not findings here.

## Findings

Cases below are source-traced; no tests were run.

1. **High — Aliased browser roots lose protected names that do not exist yet.**  
   **Location:** [sensitive.rs:2188](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:2188).  
   **Failing case:** make `~/.config/google-chrome → /tmp/profile`, with an empty target directory. Reading `~/.config/google-chrome/Cookies` returns Clear: resolution produces `/tmp/profile/Cookies`, while collection records neither a credential-file identity nor a suffix-aware browser-root alias. The protected spelling is lost. `~/.ssh → /tmp/keys` with an absent private-key name has the same problem.  
   **Fix:** preserve protected-name checks through these root aliases, including absent children, while retaining ordinary/public-name exceptions. Request an interface extension if the frozen representation cannot carry that information. D-145 preserves name and root-identity protection.

2. **High — S8 still skips linked browser profile directories.**  
   **Location:** [recursive.rs:884](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/recursive.rs:884).  
   **Failing case:** create `~/.config/google-chrome/Default → /tmp/profile`, containing `Cookies`. Evaluate recursive `rg -g '*.rs' x ~/.config/google-chrome`. The builder collects Cookies, but S8 descends only `FileType::Dir`; it skips `Default`. The filter and hard-link pass recover no hit, producing Clear instead of S8’s HardDeny.  
   **Fix:** traverse linked directories in the S8 browser scan within budget, with cycle detection and Unread on traversal failure or caps. D-145 explicitly retains walking linked directories.

3. **High — A known root-link identity incorrectly narrows an unknown referent gap.**  
   **Location:** [sensitive.rs:1850](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1850).  
   **Failing case:** create `~/.azure/msal_token_cache.json → /tmp/token`, where `token` is singly linked. Give the builder enough Stats for the final link’s `lstat`, but not its following `stat`. Collection succeeds with `Through(link_identity)`. Checking `/tmp/token` with fresh caps returns Clear: it carries neither that link identity nor the multiply-linked flag. Its protected-root identity was never established.  
   **Fix:** use AnyTarget when the root’s followed identity is unknown. Knowing the symlink entry’s identity does not establish its referent’s identity. This is a root gap required by D-145, not an accepted member cutoff.

4. **High — The member cutoff silently truncates discovery of named MSAL roots.**  
   **Location:** [sensitive.rs:2248](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:2248).  
   **Failing case:** put 4,096 alphabetically earlier ordinary files in `.azure`, followed by `msal_token_cache.json → /tmp/token`. The complete listing fits the filesystem caps, but `.take(WALK_MAX_ENTRIES)` omits the named store root without a gap. Reading its protected spelling canonicalizes to `/tmp/token` and returns Clear instead of StoreHit.  
   **Fix:** separate named-root discovery from bounded hard-link member collection. Establish each matching root within the filesystem/compute budgets, or record the required uncertainty. D-145’s Clear cutoff does not permit losing a protected spelling.

5. **Medium — Subject resolution still allocates before charging.**  
   **Location:** [command.rs:1316](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/command.rs:1316), [command.rs:1326](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/command.rs:1326).  
   **Failing case:** `resolve_subject_paths([], None, false, …)` with `ComputeBudget::new(1, 0)` returns Ok after allocating the one-slot `roots` buffer. A supplied relative filter additionally allocates both `Word` buffers before any allocation charge. This leaves round 3’s allocation audit incomplete under §8.  
   **Fix:** remove unnecessary eager buffers and precharge every filter buffer, target clone, and collection growth before allocation.

6. **Medium — Moving a symlink to a directory incorrectly triggers derived destinations.**  
   **Location:** [command.rs:1082](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/command.rs:1082).  
   **Failing case:** with ordinary `link → directory`, resolve `mv link destination` using the annex’s effects. Literal resolution has already followed `link`, so `lstat(canonical)` observes the directory and emits `Unread(DerivedDestinations)`. The source being moved is a symlink, not a directory.  
   **Fix:** determine the source’s final-entry type before following that final symlink. Keep canonical target checks for protection separately.

7. **Medium — Real-filesystem tests depend on unrelated temporary-directory contents.**  
   **Location:** [p2_tests.rs:2225](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/p2_tests.rs:2225), [p2_tests.rs:3554](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/p2_tests.rs:3554).  
   **Failing case:** run with the temporary folder’s parent containing more than `FS_MAX_ENTRIES_READ` entries. Discovery from `/` exhausts EntriesRead and creates required gaps. The first test’s ordinary hard-link Clear assertion fails; the second reuses exhausted caps and receives CapReached where its small-allocation test expects Exhausted. The implementation’s cap behavior is correct; these tests are not robust.  
   **Fix:** isolate discovery from unrelated host listings, or use a fixed protected fixture for resolver-only assertions. Use separate budgets/caps for independent phases.

## Test gaps

Add regressions for these seven cases, particularly absent names beneath aliased roots, filtered S8 searches through linked profiles, and a cap reached between root `lstat` and `stat`.

The supplied verification now shows fmt/clippy passing and no P2 failure. Workspace `rc=101` comes from the separately tracked bridge test, excluded here.

No files were modified.