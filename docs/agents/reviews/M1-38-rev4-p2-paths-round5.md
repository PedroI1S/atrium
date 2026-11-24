## Verdict

**Request changes.**

Reviewed `git diff 4f18a59...HEAD` against the contract, D-145 and D-147. The seven round-4 reproductions are addressed. P2 preserves the frozen interface and remains unwired from `SessionGate`.

The accepted CR-P2-3 standalone cases are excluded. Findings below are source-traced; no tests were run and no files were modified. Supplied verification at `3c850b9` is clean.

## Findings

1. **High — Recursive Unread roots skip written-name protection.**  
   **Location:** [recursive.rs:64](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/recursive.rs:64).  
   **Failing case:** make the fake `~/.gnupg` directory inaccessible, then call `resolve_subject_paths(["~/.gnupg/key"], None, true, Read, …)`. Resolution returns Unread, and this branch returns before checking the supplied written spelling. The result lacks the named HardDeny that D-147 requires and that `dev` finds lexically. This affects resolver-backed calls, independently of CR-P2-3.  
   **Fix:** check the written name before the Unread-root return, retaining both the hit and the resolution reason.

2. **High — Followed searches lose protection through aliased browser roots.**  
   **Location:** [recursive.rs:1971](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/recursive.rs:1971).  
   **Failing case:** create `~/.config/google-chrome → /tmp/profile`, an existing singly linked `/tmp/profile/Cookies`, and `proj/p → ~/.config/google-chrome`. With the built protected set, resolve `rg -L token proj` without a filter. The followed directory is checked only as `/tmp/profile`; S8 finds no browser name there, and subsequent regular-file entries are ignored. P2 returns Clear despite reaching Cookies. `dev` canonicalizes its browser roots and denies this case.  
   **Fix:** preserve protection when recognizing followed directory aliases, including browser containers and their protected descendants. Keep filtered over-approximations Unread.

3. **Medium — Written spellings omit the unlisted-equivalence check.**  
   **Location:** [sensitive.rs:1337](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/sensitive.rs:1337).  
   **Failing case:** configure Atrium data at `/h/u/Café/atrium`, leave that path absent, and create the differently spelled `/h/u/Cafe\u{0301} → /tmp/public`. Reading `/h/u/Cafe\u{0301}/atrium/db` returns Clear: canonical-name comparison misses, and `written_name_hit` checks only listed folds. The written name matches the protected name under NFC/NFD, requiring Unread under §5 and §11 item 13.  
   **Fix:** apply unlisted-equivalence detection to written spellings too, while preserving identity and listed-fold hits.

4. **Medium — Globbed `mv` sources use the wrong entry for directory detection.**  
   **Location:** [command.rs:510](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/command.rs:510).  
   **Failing case:** with cwd `/h/u`, create `link → /x/sub`, directory `/x/srcdir`, and ordinary file `/h/u/srcdir`. For `mv link/../src* dest`, expansion selects the actual directory `/x/srcdir`, but its written spelling normalizes to `/h/u/srcdir`. The source-type check sees that ordinary file and omits `Unread(DerivedDestinations)`, contrary to §5 and `WhenSourceIsDir`.  
   **Fix:** inspect the actual expanded final entry before following its final symlink. Keep the lexically normalized spelling solely for name protection.

5. **Medium — Initial cwd storage is undercharged.**  
   **Location:** [command.rs:1194](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/command.rs:1194).  
   **Failing case:** call `resolve_command_targets([], [], b"/proj", …)` with `ComputeBudget::new(100, 5)`. The initial charge covers five cwd bytes, but `CwdState::Known` allocates two five-byte copies. The call returns Ok without exhaustion after exceeding its allocation budget.  
   **Fix:** precharge both copies, or avoid constructing cwd state for an empty sequence (§8).

6. **Medium — Recursive filter checks discard Store hits when gaps apply.**  
   **Location:** [recursive.rs:1430](/home/rodrigo/atrium-wt/M1-38-rev4-p2-paths/crates/atrium-core/src/policy/v4/paths/recursive.rs:1430).  
   **Failing case:** create `.pypirc`, `proj/x.token → ~/.pypirc`, and an unlistable `.ssh`, producing an AnyTarget identity gap. Evaluate recursive `Search(paths=["proj"], glob="*.token")`. The selected entry establishes a Store hit, but `check_target_impl` replaces it with the gap’s Unread. No probe restores that hit. This violates §1’s requirement to retain every hit and reason for doubt.  
   **Fix:** use `check_target_parts` inside recursive checks and retain both records, as the nonrecursive resolver-backed flow already does.

## Test gaps

Add regressions for all six cases, especially inaccessible named roots, followed links through aliased browser roots, and normalization-equivalent written aliases.

The two real-filesystem tests now separate builder budgets from assertion budgets and remove the reported temporary-parent entry-count dependency. They continue to use temporary fake homes.