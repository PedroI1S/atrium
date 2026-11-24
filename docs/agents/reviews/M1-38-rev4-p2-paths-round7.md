## Verdict

**Request changes.**

Reviewed `git diff dev...HEAD` at `7f74312`. Round 6’s reported reproductions are addressed, with the Unicode qualification below. Four new defects remain.

P2 preserves the frozen interface and remains unwired from `SessionGate`. No files were modified or repository tests run. The verification log still identifies `a986038`, so it does not verify round 8.

## Findings

1. **High — Root aliases compare canonical spellings instead of identities.**  
   **Location:** [sensitive.rs:1752](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1752).  
   **Failing case:** give an empty `~/.config/google-chrome` directory another filesystem name, `/tmp/profile`, with the same device/inode but a distinct canonical spelling. This models a [bind-mounted directory](https://man7.org/linux/man-pages/man2/mount.2.html); the fake can represent it with a shared directory inode. Resolve the Read `/tmp/profile/Cookies` through `resolve_subject_paths`. P2 returns Clear: the browser collector has no members, and alias rewriting skips the root because its configured spelling already equals its canonical spelling. Dev’s `alias_fix` compares ancestor identities and rewrites this path to `~/.config/google-chrome/Cookies`, producing HardDeny.  
   **Fix:** implement identity-based protected-root rewriting in resolver-backed flows, including roots whose configured and canonical spellings coincide.

2. **High — Filter probes omit protected-root alias spellings.**  
   **Locations:** [command.rs:1635](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/command.rs:1635), [recursive.rs:1586](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/recursive.rs:1586).  
   **Failing case:** create an empty `/tmp/cache` and `~/.azure → /tmp/cache`. Call `resolve_subject_paths(["/tmp/cache"], Some("msal*"), false, Read, …)`. It returns Clear because probes use only the canonical and written roots; neither supplies `~/.azure`. The alias spelling requires the Store probe hit retained by §5.  
   Recursive checking also loses an alias when another written spelling comes first: with `~/other → /tmp/profile` and `~/.config/google-chrome → /tmp/profile`, an empty-profile recursive search rooted at `~/other`, filtered to `Cookies`, skips the browser hard-deny probe. `spellings.first()` passes only `~/other` to filter checking.  
   **Fix:** check probes against every applicable canonical, written and protected-root alias spelling.

3. **Medium — A successful alias rewrite suppresses an incomplete-table Unread.**  
   **Location:** [sensitive.rs:1831](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1831).  
   **Failing case:** create `/tmp/keys/config` and `~/.ssh → /tmp/keys`; build protection with ample separate resources. Read `/tmp/keys/config` using `FsCaps::new(1000, 1000, 1000, 1000, 2)`. Resolution consumes one canonicalization, and discovering `.ssh` consumes the second. Later alias discoveries reach the cap. Rewriting nevertheless succeeds to readable `~/.ssh/config`, then returns `None`, discarding the table’s recorded cap reason. The call returns Clear instead of Unread under §8.  
   **Fix:** retain the incomplete-table reason when a rewrite yields no definitive hit; preserve it alongside Store hits where applicable.

4. **Medium — Glob spelling reconstruction allocates before charging.**  
   **Location:** [glob.rs:904](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/glob.rs:904).  
   **Failing case:** expand relative `*` with an ordinary match `a`. After resolving the match, `rest.collect()` allocates its slice vector before the allocation charge at line 910. At an allocation-budget boundary, the allocation has already happened when that charge reports Exhausted. The subsequent `exact_matches.push(target)` also grows its vector without a corresponding storage charge. This violates §8.  
   **Fix:** charge and reserve the component buffer before collecting, or avoid it; charge target-vector growth before insertion.

## Test gaps

Add regressions for these four cases, including allocation instrumentation that checks charge ordering.

**Correction to round 6 finding 3:** `logins.json` is protected, but Unicode’s default case fold keeps dotless `ı` distinct from `i`. The [Unicode case-folding data](https://www.unicode.org/Public/UCD/latest/ucd/CaseFolding.txt) confirms this. The new test’s Unread result is acceptable conservative handling under §11 item 13; its explanation should not present `ı → i` as default Unicode conformance. Add an independent normalization or documented unlisted-fold case to exercise alias equivalence checking.

The inspected real-filesystem fixtures continue to use temporary fake homes and separate builder resources.