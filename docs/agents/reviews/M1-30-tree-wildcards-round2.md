# Review: M1-30-tree-wildcards
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-17 · round 2

## Verdict
**rework**: the search-glob bypass remains, and the new matchers contain additional verdict gaps, unbounded recursion, and cubic work.

## Findings

Cases use home/cwd `/h/u`, empty `resolved_paths`, and the task’s virtual environment unless stated otherwise. “Any” means Home with a matching global `Any → Allow` rule. Except where marked executed, results follow from tracing HEAD.

1. [major] **Round 1’s contained-search bypass remains open**
   - Where: `crates/atrium-core/src/policy/deny.rs:2814`, `"for probe in self.s.probes()"`; `crates/atrium-core/tests/M1-30-tree-wildcards.rs:926`, `#[ignore = "escalated: requires changing Sensitive::search_glob_hit (owned by M1-31)"]`.
   - Case: `Search { paths: ["~"], recursive: false, glob: ".g*/private-keys-v1.d/k.key" }`; also `glob: ".local/share/atr*/missing.db"`.
   - Result: Allow in Home, Any and FullAccess. Executing the ignored regression reproduces Allow for the first case in Home. Expected HardDeny (`credential-read` / `atrium-self`): the brief §18–20 covers searches, and contract §9 applies hard denies to searches. Finite probes still miss these filenames.
   - Fix: Resolve the ownership escalation and check contained filters symbolically against sensitive trees. Ignoring the reproduction does not close round 1 finding 1.

2. [major] **Literal braces cause unbounded recursion**
   - Where: `crates/atrium-core/src/policy/deny.rs:1610`, `"return !exps.is_empty() && exps.iter().all(|e| pat_is_public_ssh_name(e));"`; `crates/atrium-core/src/policy/words.rs:379`, `"None => out.push(x)"`.
   - Case: `Read { paths: ["~/.s?h/confi{g}"], recursive: false, glob: null }`, in Home, Any or FullAccess.
   - Result: No terminating verdict. `{g}` is not a brace expansion, so `brace_expand` returns the unchanged pattern. `pat_is_public_ssh_name` calls itself with that same pattern indefinitely. Expected HardDeny (`credential-read`): `.s?h` matches `.ssh`, and `confi{g}` is neither public nor an exempt configuration filename. The plain `.ssh/confi{g}` path hard-denies.
   - Fix: Detect unchanged brace-expansion results and process them as literal syntax. Apply that guard to the other recursive brace helpers at `deny.rs:1587`, `:1721` and `:1801` too.

3. [major] **Adjacent globstars produce cubic matcher work**
   - Where: `crates/atrium-core/src/policy/deny.rs:1931`, `"Comp::GlobStar => self.parts[idx..].iter().all(Comp::is_catchall)"`; `:711`, `"if tails.iter().any(|t| {"`; `:2144`, `"for &idx in reach_indices(...)"`.
   - Case: The exact command formed by `"cat ~/" + "**/".repeat(4096) + "notes.txt"`—12,303 bytes, below the command-size limit—in Home, Any or FullAccess.
   - Result: `ShellGlob::new` preserves all adjacent globstars. Home matching produces every suffix tail; each tree check then rescans every remaining catch-all suffix until `notes.txt`. Across those tails, the `.gnupg` tree check alone entails over 11 billion `is_catchall` visits: Θ(n³). The eventual ordinary verdict should remain Ask / Allow / Allow, with bounded work. This violates the brief’s no-slowdown requirement independently of the excluded timing-test failures.
   - Fix: Coalesce adjacent globstars and compute trailing catch-all information once instead of rescanning suffixes. Add a regression for this input shape. This finding is an operation-count proof, not a measured timing claim.

4. [major] **The session filename-family fix does not cover Read paths**
   - Where: `crates/atrium-core/src/policy/deny.rs:812`, `"let tx_segs = t_segs.extended_with(\"x.json\");"`; `:814`, `"t.matches_tree_segs(t_segs.as_slice())"`; `:1931`, the catch-all-only globstar leaf rule.
   - Case: `Read { paths: ["~/**/b.session"], recursive: false, glob: null }`; likewise `~/**/b.history`.
   - Result: Allow in Home, Any and FullAccess. Expected Ask in Home, Allow in Any/FullAccess: these match `.zsh_sessions/b.session` and `.bash_sessions/b.history`, which contract §9 protects. The corresponding Search filters correctly Ask after round 1 finding 5’s fix, but `ask_hit` still uses the tree/root/`x.json` checks.
   - Fix: Share the filename-family intersection logic with `ask_hit`, while preserving ordinary `~/**/*.rs` behavior.

5. [major] **The globstar fallback omits Atrium’s actual socket filename**
   - Where: `crates/atrium-core/src/policy/deny.rs:580`, `"\"atrium.sock\","`, in `PROTECTED_PROBE_FILES`; `crates/atrium-host/src/lifecycle.rs:503`, `"socket: runtime_dir.join(\"daemon.sock\"),"`.
   - Case: With `atrium_protected_paths = ["/run/user/1000/atrium"]`, execute `rm /run/user/1000/**/daemon.sock`.
   - Result: Ask in Home; Allow in Any and FullAccess. Expected HardDeny (`atrium-self`) everywhere: contract §9 says Atrium’s socket is never written. The plain `rm /run/user/1000/atrium/daemon.sock` hard-denies. The globstar leaf rule rejects the tree match, and the fallback includes `atrium.sock` but omits `daemon.sock`.
   - Fix: Include the real socket basename in the protected-artifact fallback and test it with the production runtime-path shape.

6. [major] **The source-extension exception suppresses explicit Azure cache matches**
   - Where: `crates/atrium-core/src/policy/deny.rs:1807`, `"&& NON_CACHE_EXTS.contains(&ext)"`; `:1808`, `"&& [\"a\", \"main\", \"mod\", \"lib\", \"readme\"]"`; `:1812`, `"return false;"`.
   - Case: `Read { paths: ["~/.a?ure/m*.rs"], recursive: false, glob: null }`.
   - Result: Allow in Home, Any and FullAccess. Expected Ask in Home, Allow in Any/FullAccess: the pattern matches `.azure/msal_token_cache.rs`, whose plain read asks under contract §9’s `msal_token_cache.*` rule. Matching the unrelated stem `main` causes the helper to discard the valid cache intersection.
   - Fix: Restrict any ordinary-glob exception needed for K8’s broad home searches to that context. An explicit Azure-directory match must retain arbitrary-extension protection.

7. [minor] **Ordinary-glob preservation depends on spelling `*` exactly**
   - Where: `crates/atrium-core/src/policy/deny.rs:1511`, `"matches!(self, Self::Pattern { raw: \"*\", .. })"`; `:1939`, `"!comp.is_bare_star() || self.parts[idx + 1..].iter().all(Comp::is_catchall)"`.
   - Case: `cat ~/.config/***/settings.json`.
   - Result: HardDeny (`credential-read`) in Home, Any and FullAccess. Expected Ask / Allow / Allow, as on `dev`: `***` is equivalent to `*` within this component. HEAD preserves the explicitly required `cat ~/.config/*/settings.json` but hard-denies this equivalent ordinary spelling. The brief §20 and §29 forbid that new hard deny.
   - Fix: Recognize equivalent single-component catch-all patterns when applying the ordinary-glob exception.

8. [nit] **Round 1’s stale handoff summary remains**
   - Where: `docs/agents/handoffs/M1-30-tree-wildcards.md:18`, `"**/*.toml"` is still listed under `"false (Allow)"`, and `"**/*.json"` under `"true (Ask)"`.
   - Case: Non-recursive `Search ["~"]` with `**/*.toml` or `**/*.json`, Home.
   - Result: HEAD correctly returns Ask and HardDeny (`credential-read`), respectively, contrary to that summary. The later test inventory is corrected, but the opening explanation still names the removed `ASK_TREE_FILES` implementation.
   - Fix: Update the opening K8 explanation to the final implementation and verdicts.

## Suspected, not shown

None beyond the findings above. No wall-clock result is claimed for finding 3.

## Test gaps

- Round 1’s exact cases for findings 3–8 are covered and pass. Finding 1 remains open; finding 9 remains partly stale. Finding 2’s `cd` behavior is unchanged: the existing ignored fuzz test still fails at iteration 28 on `timeout 5 cd ~/.a?s; xxd credentials;` with Ask. It remains M1-28’s responsibility.
- The changed expectations in `d87710d` are correct: `**/*.toml` and `**/*.{rs,toml}` Ask because `deny.rs:446` lists `".cargo/credentials.toml"`; `**/*.json`, `**/*`, `**` and `**/` hard-deny because they reach credential probes such as `deny.rs:403`, `".docker/config.json"`. The original expectations were wrong.
- Recursive home-root searches correctly remain Ask: `engine.rs:757` requires `"p != h"`. Existing tests preserve `**/*.rs`, `src/**/mod.rs` and `~/**/*.rs`.
- `finding_11`’s fixture cases are already blocked on `dev`: `adversarial_gate.rs:24–26` creates the targets, and `gate.rs:327–330` promotes filesystem reachability to HardDeny. The new absent-on-disk test at `M1-30-tree-wildcards.rs:903` appropriately pins this task’s additional behavior.
- D-119 scope is respected. The actual source diff is +1,414/−73 lines, all within the owned region. The separate path/search matching mechanisms still produce the inconsistencies above; line count alone is not a finding.
- Ran 11 existing lexical tests successfully, explicitly reproduced the ignored search bypass, and reproduced the unchanged `cd` fuzz failure. Cargo could not open its build lock on the read-only filesystem. No files were changed.