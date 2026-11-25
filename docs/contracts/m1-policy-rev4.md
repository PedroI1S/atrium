# Policy engine rev 4: exact, or not established (draft 5)

**Status: draft 5, 2026-11-21, Codex approved (round 5).** Written by the orchestrator at rodrigo's request (D-135). Five Codex reviews (`docs/agents/reviews/rev4-contract.md` and `-round2.md` to `-round5.md`) found 21, 15, 5, 1 and then 0 findings. Nothing is implemented, and no worker has a brief.
- **The text:** rodrigo (driver) approves it.
- **Changes to security defaults:** each one is listed in §11, and Pedro confirms it before P0–P4 start.
- **What it amends:** contract `m1.md` §9. Where the two disagree, this file wins once it is approved.

## Why

The M1-27 batch (M1-27 to M1-32) had 18 Codex reviews, and every one asked for rework. Each round closed its findings and the next round found a neighbouring variant. The cause is in the goals, not in the workers. Several must-fix items asked the engine to work out *exactly* what a dynamic shell line does, either to keep it allowed or to turn its Ask into a hard deny:
- K7: resolve `sh -c '"$1"' _ …`;
- K9: give long `env` lines the same verdict as the short form;
- M1-28: read `$'…'` words;
- S5, S11: reason about glob patterns against the protected set.

Shell is too large for that, so the variants never end.

This revision limits what the engine promises:
- **Exact checks** within a small, closed model (§3–§6), under stated assumptions (§2).
- **A fixed verdict per setting** for everything outside that model.

Two facts shape the design:
- **A measurement:** over 3,353 shell commands from real agent sessions on rodrigo's machine, 53% use at least one construct outside the model (`$VAR` 38%, loops 23%, `$(…)` 20%, here-documents 18%, inline `awk`/`python -c` 9%). Asking for all of them would make FullAccess useless.
- **What a parser can't do:** under FullAccess, the hard deny can't stop an agent that wants to get around it. Writing `sudo id` into `x.sh` and running `sh x.sh` gets past any parser. Real limits for FullAccess come from the sandbox (D-044).

## 1. Outcomes

Each part of a call (a unit, a path, a filter) gets one outcome. The call's outcome combines its parts by the precedence below.

| Outcome | Meaning |
|---|---|
| **Hard-deny hit** | Within the model, the call touches something on the hard-deny list or Atrium's own paths. |
| **Store hit** | Within the model, the call touches a credential store that asks (CR-12, CR-17). |
| **Clear** | Every part of the call is within the model, and none of the above. |
| **Unread** | Some part is outside the model (§3–§6). The record says why. When the reason is the contents of a program the call names (a script file, a project build), it is **Unread (program)**. |
| **Exhausted** | A budget ran out (§8), or the subject is `ToolSubject::Unknown`. |
| **Too large** | Over `MAX_BYTES`, or over the structural caps (`MAX_UNITS`, `MAX_USES`, `MAX_PATHS`), as on `dev`. |

**Combining parts: the decision procedure.** Every hit and every reason for doubt found in any part is kept. The steps run in order, and the first that applies decides:
1. **Too large:** `Deny` ("too large to check"), as on `dev`.
2. **A hard-deny hit,** from the model or from the best-effort pass (§2): HardDeny. It beats every rule and setting, and nothing cancels it.
3. **Rules:** they are applied with `dev`'s precedence: thread over Home over global, and a higher-ranked rule that covers the call suppresses lower-ranked ones (`engine.rs` `apply_rules`). A Deny rule matches exact units and best-effort units (§7). A winning Deny is Deny.
4. **A multiply-linked regular file** that isn't already a hit gets Ask in every setting, as `dev` does (`gate.rs` `multiply_linked_file`, overriding Allow). That covers a file named directly, and one found by the bounded recursive walk.
5. **Any Exhausted part:** Ask, in every setting.
6. **Otherwise each part must be authorized** by §2's table under the call's setting and its covering rules. The call is Allow only if every part is, and otherwise Ask. A rule that names a program authorizes only the **Unread (program)** reason of that unit, not any other reason the same unit has.

**Over-approximation never produces a hard-deny hit.** Where the model checks more than the real call can reach (dialect variants, dot-matching, a filter it can't apply), a protected path found only that way makes the part Unread, never a hit. One exception: a filter that matches `dev`'s probe names for a family of stores that ask counts as a **Store hit**, as on `dev` (§5). Hits from the model come only from the precise reading. This is the old rule "never a hard deny on doubt", stated precisely. The best-effort pass is different: it keeps `dev`'s raw-text matching, which can block text that isn't an accessed path (§10, row 17), as `dev` does today.

## 2. Settings, and what is outside the promise

| Outcome | Home, Careful, Normal | Prefix or exact rules | Any rule | FullAccess |
|---|---|---|---|---|
| Store hit | Ask | Allow by a covering Allow rule (as `dev`, D-082) | Allow | Allow |
| Clear | the preset (as `dev`) | the rule decides | Allow | Allow |
| Unread | Ask | Ask; **Unread (program)** is covered by a rule that names that program (§7) | Allow\* if the Any rule is **unscoped**; a scoped Any or `PathGlob` rule needs established targets, so Ask | Allow\* |
| Exhausted | Ask | Ask | Ask | Ask |

\* **The best-effort pass:** `dev`'s hard-deny matching on the raw text and on every unit the lexer produced, including units derived from `-c` scripts. It also includes `dev`'s **hard-deny probe matching** for shell globs and search filters (`Sensitive::glob_hit` and `search_glob_hit`'s probes). So `Search(paths=["~/.aws"], glob="credentia?s")` on an empty `~/.aws` stays HardDeny, as on `dev`. A hit there is a HardDeny. A miss is not a finding, because the contract makes no promise about Unread parts.
- **Trusted** (an M2 preset) treats Unread as Ask until M2 decides otherwise.
- **Exhausted asks everywhere,** as on `dev` for unbounded globs and K9. Padding a line to exhaust the budget must not buy an Allow.
- **Filesystem walks with a cap:** a walk that stops at its entry cap (§5, §8) is **Unread**, not Exhausted. The size of a tree is not something the line controls. Making it Exhausted would ask under FullAccess for every search of a large repository, and under FullAccess an agent could create the files anyway.

**Assumptions: what the promise doesn't cover.** These are stated in Settings (§9), and reviews don't count them as findings:
- **Command names:** a command named `cat` or `git` behaves as that system tool, and the model doesn't follow `PATH` shadowing.
- **The user's own setup:** the user's shell configuration (aliases, functions, options) and repository configuration (`git config`, build scripts) are the user's own. `m1.md` §9 already lists git config as a known limit.
- **The contents of repository objects:** what a commit or diff contains, beyond the paths a git read names explicitly (§6).
- **Other processes:** see "Time" below.
- **Hard links inside recursive roots,** beyond `dev`'s bounded walk (§5).
- **Inherited file descriptors** other than 0, 1 and 2. They are outside the model anyway (§3).

**Time.** Classification reads the filesystem with non-atomic observations. Changes made by other processes after any observation, including during the check, are outside the promise. Approved calls are re-evaluated at `redeem` (as `m1.md` §9 says). Automatic Allows are not. Changes made by **the same call** are inside the promise (§5, "Effects within one call").

## 3. The command model: words

The model reads a command line as the **intersection** of what POSIX `sh`, `bash` and `zsh` agree on. Anything whose meaning differs between them is outside the model.

**Words keep provenance.** Each byte records whether it was unquoted, single-quoted, double-quoted or escaped. Only unquoted glob characters expand. Bytes that come out of a quote, an escape, a resolution or an expansion are never parsed as syntax again.

**Unquoted text:**
- **Allowed characters:** the bytes `A–Z a–z 0–9 _ . / , : @ % + -` plus `=` (but not as a word's first byte, because of zsh's `=cmd`).
- **Glob characters:** `*`, `?` and `[…]`, with no `!` or `^` inside the class.
- **`~`:** only as a word's first byte, followed by `/` or the end of the word.
- **`~` anywhere else in a word is a literal byte,** as bash and zsh read it (`HEAD~1`, `a~b`). The exception is a `~` right after a `=` or a `:` (`a=~/x`, `--opt=~/x`, `PATH=x:~/bin`): shells may tilde-expand it there, so the line is outside the model. **[§11, item 16; D-176]**
- Any other unquoted byte puts the line outside the model. That covers `$`, backtick, `{`, `}`, `#`, `!`, `^`, `(`, `)`, `<` and `>` except in the redirects listed below, `&` other than `&&`, and non-ASCII.

**Quotes and escapes:**
- **Single quotes:** any bytes.
- **Double quotes:** any bytes except `$`, backtick and `!`. A `\` is allowed only in `\"` and `\\`. So `"\q"`, which keeps its backslash, is outside the model.
- **Outside quotes:** a backslash before one of the allowed characters or glob characters, which makes it literal. Backslash-newline is outside the model.

**Words become units.** Separators are `;`, `&&`, `||`, `|` and newline. Comments, here-documents, here-strings, groups, subshells, functions, `&`, reserved words and arithmetic are all outside the model.

**Redirects, from this closed list:**
- `<`, `>`, `>>`, `2>`, `2>>`, followed by a literal target;
- `n>&m` with `n` and `m` in 0–2, and `>&m` with `m` in 0–2, which is the same as `1>&m`;
- `n>&-` with `n` in 0–2, and `>&-`, which is the same as `1>&-`. A `>&` followed by anything else is a file redirect in bash, so it is outside the model. **[§11, item 16; D-176]**

`&>` (not shared by POSIX `sh`), `<&` and every other descriptor number are outside the model. So `cat <&3` is Unread.
- **Literal targets** are checked under §5, input as a read and output as a write. An output redirect makes the unit a writer, except one to `/dev/null`.
- **Process-dependent paths,** as operands or redirect targets, directly or through a symlink, are Unread: `/proc/self`, `/proc/thread-self`, `/proc/<pid>`, `/dev/fd/`, `/dev/stdin`, `/dev/stdout`, `/dev/stderr`. The gate resolves them in its own process, not in the one that runs the call.
- **Rule coverage** keeps `m1.md` §9's exclusion (§7).

**Assignments:**
- A leading `NAME=value` puts the unit outside the model. `dev`'s rules and Home guards exclude it already.
- **`$HOME` and `$PWD` aren't readable.** `~` is resolved by the gate only if no earlier unit of the line sets `HOME` (an assignment, `env`, `export`, `unset`). Otherwise the unit is Unread.

## 4. The command model: command words, wrappers and nested shells

**Command words:**
- The command word is folded (CR-19), then looked up in §6's table.
- **A command word with an unquoted glob character** (`/usr/bin/sud[o] id`) is Unread. The model does not expand command words, and the best-effort pass still runs on them.
- An unknown tool is in the model only for hard-deny and store checks: every word counts as a candidate path, as on `dev`. It is never Clear for Home's read-only list.

**Wrappers.** A closed list. The model removes a wrapper only in the forms shown, and any other form or option puts the unit outside the model:
- `command` (with `--` at most);
- `exec` (no options);
- `nice`, `nice -n N`;
- `timeout [-s SIG] [-k D] D`;
- `time` (no options);
- `env` (no options and no assignments; so `env -C dir`, `env -i`, `env -S` and `env X=1` are outside the model).

A wrapper's own effects stay with the unit: none of these forms reads or writes a file. The unwrap depth is bounded (`MAX_WRAP` = 8). Reaching it makes the unit Unread, never "check what is left" (M1-27 round 3, finding 5).

**Nested shells:**
- **Supported form:** only `sh`, `bash` and `dash`, as `NAME -c SCRIPT`, optionally with `-e`, `-u` or `-x` before `-c`, and with nothing after `SCRIPT`. `SCRIPT` is read recursively by §3 and §4, up to `MAX_DEPTH`.
- **Outside the model** (Unread):
  - operands after the script (`$0`, `$1`, …): **K7's positional resolution is removed**;
  - a script file, or a shell reading stdin;
  - `-i`, `-l`, `--rcfile`, `--login`, `-o …`;
  - `zsh -c` (glob qualifiers and `=cmd` run code);
  - every other entry in `SHELLS`.
- **A script file, `sh x.sh`,** is Unread (program).

**Code runners.** These are marked in §6 and make the unit Unread: `awk`, `sed` scripts with `e`, `w` or `r`, `perl`, `python`, `node`, `ruby`, `find -exec`/`-execdir`/`-ok`, `xargs`, `parallel`, `eval`, `source`, `.`, `env -S`, the program-running git forms (§6), `pipx run`, `npx`, and the tools in `COMMAND_RUNNERS`.
- **Unread (program)** applies when the unit names a program file or a project build, e.g. `python build.py` or `cargo test`.
- **Plain Unread** applies to inline code, such as `python -c`, an `awk` program or `find -exec`.

## 5. Paths, globs and links

The engine still never touches the filesystem; `SessionGate` does. Resolved paths are bytes, never patterns (M1-32 round 4, finding 1).

**Literal paths** come from:
- command operands that §6 marks as paths, plus every word of an unknown tool;
- the paths of `Read`/`Write`/`Edit` tools, which are always literal (M1-32 round 4, finding 3);
- redirect targets.

Each is resolved as on `dev`: the deepest existing ancestor is canonicalized, symlinks are followed up to `MAX_HOPS`, dangling link entries count by their own name, and the identities (device, inode) of the path **and of its existing ancestors** are compared against the protected set. A path that can't be resolved (an unreadable directory, too many hops) is Unread (M1-32 round 4, finding 4).

**Names:**
- **The listed folds define the protected spellings,** whatever equivalence the volume has: CR-19's folds plus `deny.rs`'s path folds (lowercase, `ſ`→`s`, K→k), exactly as on `dev`, with no Unicode normalization added. They apply **in addition to** identity, as on `dev`, so an existing `~/.SSH/ID_RSA` or `~/.ſsh/id_rsa` on a case-sensitive volume is still a hit.
- **Other equivalences:** a name that would match a protected name only under some *other* equivalence a volume might apply (an unlisted Unicode case fold, or NFC/NFD normalization such as `Café` spelled with `e` + U+0301) is Unread. This never downgrades a hit from the listed folds. **[§11, item 13]**

**Shell globs** (unquoted, in a unit within the model):
- `SessionGate` expands them against the filesystem at the unit's cwd, within budget.
- Each match is then a literal path.
- A glob that matches nothing is checked as its literal word.
- **Matches under all three dialects** (`sh`, `bash`, `zsh` defaults; no dotglob; `**` as `*`) can be hits.
- **Matches found only under a variant** (dot-matching, a recursive `**`) are Unread, never hits (§1).
- **No pattern-versus-pattern reasoning,** which ends M1-30's `ShellGlob`, variant-widening and class-complement work.

**Search tools** (`Search { paths, glob, recursive }`, and the `rg`/`grep`/`ag`/`ack` forms that §6 supports):
- **Selectors and filters are different things:**
  - A *selector* names what to read. It can contain `..` or be absolute, and can escape its root.
  - A *filter* only narrows a traversal under the root.
- **Selectors:** `Search`/`Glob` globs with a `..` component, or absolute ones, are selectors. They are expanded like shell globs, and D-100's escaping-glob verdicts stay.
- **Filters,** in the dialects §6 lists:
  - Search-tool globs: a relative path with `**`.
  - `rg -g`/`--glob`/`--iglob`: gitignore-style, relative path.
  - `grep --include`: basename.

  Checking a filter takes two steps:
  1. **Selected entries:** the entries the filter selects under each root are enumerated within the walk cap, and each goes through literal-path resolution, so a selected alias such as `proj/key → ~/.ssh/id_rsa` is a hit. Running into the cap is Unread (§2).
  2. **Probe names:** the filter is also matched against `dev`'s probe names for protected families that may not exist yet (`msal_token_cache.*`, `*.session`, …). A probe match is a **Store hit**, as on `dev`: Ask in Home, and Allow under a covering rule. It is also an independent part, so it still counts when another selected entry is a Store hit for a different name.

  A root with no selected hit and no probe match is Clear for that filter.
- **Link behaviour:** each file tool's behaviour with links (followed or not, at the root and below it) is stated in §6's table.
- **Outside the model** (Unread): negated filters, several filters on one call, and other dialects.

**Recursive reads:**
- **S8, protected-root policy:** a recursive read whose root contains an existing hard-deny path is a hard-deny hit whatever its filters. This is a policy rule, not a reachability claim, so it can block a search whose filter excludes the credential. It doesn't apply to home itself, to an ancestor of home, or to D-133's `~/.config`; those keep `dev`'s behaviour (`rg token ~` asks in Home, as `m1.md` §9 says). It does apply to ancestors of Atrium data paths outside home (M1-31 round 2, finding 4).
- **Followed links:** when the reader follows links (`rg -L`, `grep -R`, `find -L`), `SessionGate` walks the root's links within budget.
  - A followed link that reaches a protected path is a hit when the read has no filter, and Unread when it has one.
  - When the walk cap is reached, the part is Unread, never a hit (the `rg -L` false block).
- **Hard links:** `dev`'s bounded walk stays as it is: 4,096 entries, depth 8, `node_modules` and `.git` skipped, and the tree counts as clear when the walk runs out. That is the stated exception in §2. **[§11, item 7]**

**`cd`:** `cd` to a literal path in the model moves resolution, as on `dev`. Any other `cd` makes the cwd unknown, so later units are Unread.

**Effects within one call:**
- **Writers:** a unit is a possible writer when:
  - §6 marks the tool as writing;
  - the tool is unknown;
  - the unit runs a program whose effects aren't read (Unread (program), or a code runner);
  - or the unit has an output redirect other than to `/dev/null`.
- **Which units it affects:** the paths of every unit that runs after a possible writer are Unread, and so are those of **every stage of a pipeline that contains one, including stages before it** (pipelines run concurrently). Their hard-deny hits still stand.
  - So `cat a | python mutate.py` makes `cat`'s targets Unread, while `cat README.md >/dev/null; cat LICENSE` stays Clear.
  - **In Home** this mostly changes nothing, because a possible writer already asks. **Under prefix and exact rules,** a compound line that writes and then reads now asks.
- **Derived destinations:** a writer whose destinations depend on contents or on traversal is a **possible writer to unknown targets**, which makes **the writer itself Unread**. This covers recursive copies and moves (`cp -R`, `cp -a`, `mv` of a directory, `rsync`), archive extraction (`tar -x`, `unzip`) and `git clone`/`checkout`. A destination root inside a protected tree, or equal to a protected path, is still a hit. The model doesn't derive destinations, because existing links below the root (`proj/plain → ~/.ssh/id_rsa`), absolute archive names and member links can all escape a root.

## 6. Known tools: one table

Today, the knowledge of tool syntax is spread across several places:
- `HOME_READONLY_COMMANDS`;
- the `rg`/`grep`/`ag`/`ack` flag tables;
- `wrapper_opts`, `WRITERS`/`READERS`, `COMMAND_RUNNERS`;
- the git credential guards.

They become **one normative table** in `atrium-core::policy` (`tools.rs`). It is the annex of this contract: a form that isn't in the table is outside the model.

**The table is approved before any behaviour work starts.** P0 writes it as data, reviewed and approved like this text, with the items that change defaults marked for Pedro. It records the supported forms, operand roles, platforms, program-pinning conditions and link behaviour. A later change to the table is a change to this contract.

For each tool, the table records:
- its folded name, its subcommands, and its flags (boolean, or taking a value);
- for each operand: **path**, **pattern**, **archive member**, **ref**, **text**, or **destination root** (§5);
- whether it reads recursively, follows links, writes, or **runs code**, and which operand holds the code (§4);
- the filter dialect, for search tools (§5).

How the table is used:
- **Unknown flags:** a known tool with an unknown flag is Unread.
- **Pattern, text, ref and member operands** are not candidate paths. So `grep .npmrc README.md` no longer names `~/.npmrc`. **[§11, item 9]**
- **Git selects by name:** CR-12's rule stays for `git show`/`git cat-file`/`git log -p`/`git diff`. A path whose name matches a credential asks in Home. So does a pathspec that is equal to or an ancestor of a credential-shaped name (`git show HEAD -- .gem`, M1-29 round 4, finding 1), and so does any pathspec magic or exclusion (`:(…)`, `:!`). A pathspec with an active wildcard (`git show HEAD -- '.gem/*'`) is Unread. Pattern operands are excluded from filesystem-path extraction, but still go through these git selection checks. What a commit contains beyond the names selected is outside the promise (§2).
- **Links:** `ln` and `ln -s` treat their target operand as a path that is read. A link to a protected path is a hit, which settles round 1's Q1.
- **Git subcommands that run programs:** `merge-index`, `filter-branch`, `bisect run`, `rebase --exec`/`-x`, `submodule foreach`, `difftool`/`mergetool` with `--tool-cmd`/`-x`, and any `-c`/`--config-env`/`--exec-path` (already excluded by `m1.md` §9). These count as code runners.
- **Home's read-only list** becomes a property in this table, with its forbidden flags, and keeps `m1.md` §9's guards (no globs, redirects only fd duplication or to `/dev/null`). Home eligibility keeps `dev`'s matching on the command word **as written**: `cat` qualifies, but `/bin/cat` and `CAT` do not. Folded lookup is used for hard-deny, store and code-runner checks only.

## 7. Rules

- **Deny rules** match exact units **and** best-effort units, and apply before §2's table. With a Deny `["cat"]`, `cat "$X"` is Deny, as on `dev` (Codex finding 16).
- **What an Allow rule covers:** a prefix or exact Allow rule covers a unit only when:
  - the unit is Clear, a Store hit, or Unread (program) whose program is named in the rule's argv (`CommandExact ["python","build.py"]`, `CommandPrefix ["cargo","test"]`);
  - and the unit meets `m1.md` §9's simple-command conditions: no globs, no assignments, and no redirects except fd duplication and `/dev/null`. So `CommandExact` keeps matching argv only (Codex finding 5).

  A rule never covers plain Unread or Exhausted. Naming a program allows that program's own uninspected effects (the user's choice), and nothing else.
- **Store hits:** an Allow rule that covers the unit allows the store read, as on `dev` (D-082).
- **Invalid at creation:** an Allow prefix rule that ends at a code runner with no pinned program, such as `["awk"]`, `["python"]`, `["pipx","run"]` or `["git"]` (which would cover `merge-index`). The rules editor says why. **[§11, item 4]**
  - **Rules already stored** that new validation rejects stop covering anything. They stay listed, marked invalid, so the user can edit or delete them. **[§11, item 5]**
- **Path scopes** are checked against §5's canonical targets. A target that is Unread makes the rule not cover the unit (M1-29 round 4, findings 2–3).
- **Unchanged:** `Any` rules and "always…" creating `CommandExact`. "Always…" is not offered when the unit is Unread or Exhausted.

## 8. Budgets

- **One compute budget per check,** shared by lexing, unwrapping, expansion, matching and the best-effort pass. Every stage charges it **before** doing the work, with one unit per byte scanned or copied and per matcher step.
- **Allocations** are charged by size before they are made (M1-31 round 2, finding 5).
- **Running out of compute** is Exhausted.
- **Filesystem work has its own caps:** entries read, directory opens, `stat`/`lstat` calls, link reads and canonicalizations. Each operation is charged before it is made. Reaching a cap is Unread (§2).
- **The hard-link walk** keeps its own local cutoff, which counts as clear (§2).
- **P0 freezes the numbers** in `limits` before behaviour work. The starting values are `dev`'s: 4,096 entries per walk and 12 levels for glob expansion. The compute budget is set from the current suites.
- **Complexity:** input processing is linear in the line length, plus a bounded amount of charged work. No stage copies suffixes or rescans the remainder (M1-30 round 3, finding 5; M1-31 round 2, finding 6).
- **Exhaustion:** Ask everywhere, unless a higher step of §1 already decided. A hit found before the budget ran out still stands, and exhaustion never creates a hit (M1-30 round 3, finding 6).
- **Size limits:** `MAX_BYTES` and the structural caps stay `Deny` ("too large to check").

## 9. What the user sees

- **Settings, under the hard-deny list:** "Always blocked for commands Atrium can read in full. Commands built at run time (variables, loops, `$(…)`, inline scripts) are checked on a best-effort basis under FullAccess and Any rules. What Atrium can't check: …" (§2's assumptions, in plain words). **[§11, item 3]**
- **Ask dialogs:** an Ask caused by Unread or Exhausted says so ("Atrium can't read this command in full"). "Always…" is not offered. This needs a reason field in `ApprovalRequested` and UI text (P4).

## 10. Examples

The columns are **Home / prefix or exact rule / Any rule / FullAccess**, with `home = cwd = /h/u` and no rules unless stated. "Prefix" means an Allow rule whose argv covers the unit's first word, where such a rule would be valid.

| # | Call and fixture | Verdict | Why |
|---|---|---|---|
| 1 | `cat README.md` | Allow / Allow / Allow / Allow | Clear |
| 2 | `cat ~/.ssh/id_rsa` | HardDeny everywhere | hit |
| 3 | `SUDO id`, `/usr/bin/sudo id` | HardDeny everywhere | folded command word (CR-19) |
| 4 | `sh -c 'X="$1" sudo id' _ foo` | HardDeny everywhere | the best-effort pass checks the `-c` script's derived units |
| 5 | `command` ×13 `sudo id` | Ask / Ask / Allow\* / Allow\* | `MAX_WRAP` reached, so Unread; \* the best-effort pass may still hit on `sudo` |
| 6 | `cat $'\xff'` | Ask / Ask / Allow / Allow | Unread (`$`) |
| 7 | `for f in *.rs; do wc -l "$f"; done` | Ask / Ask / Allow / Allow | Unread (reserved word) |
| 8 | `cat proj/key*`: `proj → store[1]`, `store[1]/key → ../.ssh/id_rsa` | HardDeny everywhere | expansion finds `proj/key`, and resolution reaches `.ssh/id_rsa` |
| 9 | `cat 'proj/key*'`: a literal file `proj/key*` exists, and `proj/key1 → ~/.ssh/id_rsa` | Allow / Allow / Allow / Allow | quoted, so not a glob (provenance) |
| 10 | `cat /h/u/[!é]trium/secret` | Ask / Ask / Allow / Allow | non-ASCII unquoted byte, so Unread |
| 11 | `Read /h/u/[!é]trium/secret` | the literal path's outcome (Allow if `[!é]trium` is an ordinary name) | Read paths are literal |
| 12 | `Search ~/.azure, glob msal*` with `msal_token_cache.json` present | Ask / Allow by a covering rule / Allow / Allow | filter matches an existing store |
| 13 | `awk 'BEGIN{system("touch v")}'` | Ask / (`["awk"]` is Invalid) / Allow\* / Allow\* | inline code, so Unread |
| 14 | `python build.py` with `CommandExact ["python","build.py"]` | Ask / Allow / Allow / Allow | Unread (program), covered by the rule naming it |
| 15 | `rg -L x proj`, no link out of `proj` | Allow / Allow / Allow / Allow | link walk within budget |
| 16 | with `~/.aws/credentials` present: `rg x ~/.aws` / `rg x ~/.config` / `rg x ~` | HardDeny everywhere / Ask, Allow, Allow, Allow / Ask, Allow, Allow, Allow | S8 / D-133 / home root, unchanged |
| 17 | `ln -s ~/.ssh/id_rsa x; cat x` | HardDeny everywhere | `ln`'s target is a read (§6), and `dev`'s raw matcher also hits |
| 18 | `cp -R src/. proj`, with `proj/plain → ~/.ssh/id_rsa` | Ask / Ask / Allow\* / Allow\* | derived destinations, so the writer is Unread |
| 21 | `cat a \| python mutate.py`, with rules covering both | Ask / Ask / Allow / Allow | a pipeline with a possible writer, so every stage is Unread |
| 22 | `cat README.md >/dev/null; cat LICENSE` | Allow / Allow / Allow / Allow | `/dev/null` is not a write |
| 23 | `Glob(paths=["proj"], glob="key*")`, with `proj/key → ~/.ssh/id_rsa` | HardDeny everywhere | the selected entry is resolved |
| 24 | `Search(paths=["~/.azure"], glob="msal*")`, no cache file present | Ask / Allow by a covering rule / Allow / Allow | probe match, so a Store hit (as `dev`) |
| 25 | `cat <&3` | Ask / Ask / Allow / Allow | descriptor 3 is outside the model |
| 26 | global Deny `["cat"]` and thread Allow `["cat"]`: `cat README.md` | Allow | `dev`'s rule precedence |
| 19 | with a Deny `["cat"]` and no higher-ranked Allow: `cat "$X"` | Deny everywhere | Deny rules match best-effort units |
| 20 | `cat ` ×600 segments (over `MAX_UNITS`) | Deny everywhere | too large |

\* unless the best-effort pass hits.

## 11. Changes against `dev` and `m1.md` §9

**Approved earlier** (listed for completeness): CR-17's four stores (D-111); CR-19's name folds (D-118); D-133's `~/.config` exception.

**For Pedro [D-013]:** confirmed, all 15 items (D-157, 2026-11-24).
1. **Unread allows under Any rules and FullAccess,** after the best-effort pass. This generalizes D-133's second point: the hard deny is promised only within the model, and reviews no longer count misses on Unread parts.
2. **Unread always asks in Home and under prefix and exact rules,** including lines `dev` allows by resolving them, such as K7's exact positional cases (D-130), long readable `env` lines (K9) and ANSI-C words.
3. **The Settings text** and the stated assumptions (§2, §9).
4. **Code-runner prefixes are Invalid at creation** (§7).
5. **Stored rules that new validation rejects stop covering anything** (§7).
6. **A compound line that writes and then reads asks under prefix and exact rules** (§5).
7. **Hard links inside recursive roots** are outside the promise beyond `dev`'s walk (§2, §5). The alternative is to make an exhausted walk Exhausted, which would ask for most searches of large repositories in every setting.
8. **Unread (program) is coverable** by a rule that names the program (§7).
9. **Pattern, text, ref and member operands of known tools stop counting as paths** (§6). This relaxes `dev`'s all-arguments extraction: `grep .npmrc README.md` stops being blocked.
10. **The new Unread parts that `dev` allows under rules:**
    - variant-only glob matches (§5);
    - writers with derived destinations (§5);
    - the wrapper forms outside §4's list;
    - command-word globs;
    - `&>`;
    - descriptor duplication beyond 0–2;
    - pipelines with a possible writer;
    - Git wildcard pathspecs.
11. **Filesystem walk caps become Unread** (§2): they ask in Home and under rules, and Allow under unscoped Any and FullAccess. On `dev`, some cap overflows ask everywhere.
12. **S8's hard deny of roots holding a credential,** with its exceptions (home, ancestors of home, `~/.config`) and its extension to ancestors of Atrium data paths outside home (§5).
13. **Names that match a protected name only under an unlisted equivalence** (Unicode normalization, other case folds) become Unread, so Home asks where `dev` allows (§5).
14. **Process-dependent paths** (`/proc/self/…`, `/dev/fd/…`, `/dev/std*`) become Unread, so Home asks where `dev` may allow (§3).
15. **Followed-link searches can be Allow in Home** (`rg -L`, `grep -R`, `find -L`, `ack --follow`). If `SessionGate`'s walk of the root's links finds no link to a protected path within budget, the search is Clear, so Home allows it (§5, §10 row 15). On `dev`, Home asks for every followed-link search. Unchanged:
    - a link that reaches a protected path is a hit when the search has no filter, and Unread when it has one;
    - reaching the walk cap is Unread.

    Added on 2026-11-22 (D-139): P0's review found that row 15 had no item here.
16. **Two common spellings come back into the model** (§3): a `~` inside a word, other than right after `=` or `:`, is literal, so `git diff HEAD~1` is Allow in Home as on `dev`; and `>&m`/`>&-` without an explicit `n` are the same as `1>&m`/`1>&-`. Without this change, both were outside §3 and asked in Home.

    Added on 2026-11-25 (D-176): P5's review 1, findings 6 and 7.
17. **A folder linked from inside a protected tree is itself protected** (D-145 item 3). So `ls vault/work`, where `~/.ssh/work → vault/work`, is HardDeny, where `dev` allowed it (M1-03 finding 5).

    Added on 2026-11-25 (D-176): P5's review 1, finding 7.

**Unchanged on purpose:**
- `ToolSubject::Unknown` asks everywhere.
- The size limits are Deny.
- Unbounded work asks everywhere.
- Home's read-only guards.
- Rules never cover globs, assignments or path redirects.
- S8 and the folded-name protection, with no normalization added.
- Probe matches for families of stores that ask: Store hit, coverable by rules. Hard-deny probe matches for globs and search filters: HardDeny, through the best-effort pass.
- The multiply-linked-file Ask.
- Home eligibility by the command word as written.

## 12. What happens to the M1-27 batch

None of the six branches is merged. Each P task's brief carries a **hunk and test disposition list** from the source branches:
- **Hunks:** each hunk is kept, rewritten or dropped.
- **Tests:** each test is kept as is, kept with a new expected verdict (citing §10 or §11), or dropped with the reason.

"Tests kept" never means keeping an expectation this contract changes.

| Task (files; +/−) | Goes to M1-33 or M1-34 now | Goes into P0–P4 | Dropped |
|---|---|---|---|
| M1-27 (4; +1,835/−223) | — | CR-19 folding (P1); `deny.rs` name comparisons (P1); each bounded code scan the best-effort pass still uses, listed one by one in P1's brief | K7 resolution; K9 parity (round 3, finding 7) |
| M1-28 (23; +4,814/−266) | N10 (daemon lane); N12 (socket races, `gate_withdraw`); the late orphan withdrawal (D-122); fish PATH discovery and the login-shell PATH merge (M1-22); client batch-error logging (M1-24); the markdown parser; the M1-25 export test, moved to its own file | — | the `$'…'` reader; globbed `cd` candidates |
| M1-29 (15; +6,232/−97) | — | CR-17's four stores (P2); `Security.tsx` and its tests, daemon rule validation and its tests, the core validator wiring, canonical-scope semantics (P3); `app/src/chat/rule-form.ts` (P3 or P4) | the per-tool prefix guards (replaced by §6–§7) |
| M1-30 (4; +3,208/−83) | — | its test cases, migrated one by one (absent targets especially) (P2) | the replacement matcher |
| M1-31 (8; +3,876/−205) | N9: `bce1a5a` (M1-34) | S8's verdicts (P2); **N11:** bounded brace expansion in `words.rs` (P1, now best-effort only), the lexer memo in `shell.rs` (P1), the shared `Sensitive` in `engine.rs` (P2) | its root parser; the glob normalizer |
| M1-32 (4; +2,807/−212) | — | as obligations in P2: ordered resolution, dangling targets, I/O uncertainty, identity, archive operand roles (P3), and the `rg -L` verdict | the link-glob expansion code |

## 13. Tasks

Each task follows PLAN §8: a brief, Neon `high` writers (D-134), the orchestrator's verification (D-123), Codex review (D-124) and the 100% gate (D-120).

**Now, with rodrigo's go and no wait for Pedro:**
- **`M1-33-nonpolicy-fixes`** takes the hunks listed in §12 from M1-28, not whole commits, because some commits mix policy and non-policy work. They are applied to `dev` and verified against `dev`'s policy. They touch approval lifecycle (the adapters' tests use a real `SessionGate`), so the review checks that path.
- **`M1-34-n9-fixture`** is M1-31's `bce1a5a`.

**After Pedro's confirmation:**
1. **P0, the shared interface** (`policy/model.rs`, `policy/tools.rs` skeleton, `limits`). It defines:
   - units with word provenance and execution structure;
   - the outcome type, with hit dispositions and Unread reasons;
   - the tool-effects record (operand roles, writer, recursive, follows links, runs code, filter dialect);
   - the canonical per-operand targets and the budget object.

   It contains no behaviour change. It is reviewed alone.
2. **After P0 lands, in parallel behind its interface.** The new evaluator is built **alongside** `dev`'s, in `policy/v4/`, and is not wired into `SessionGate`. `dev`'s evaluator stays live until P5, so a form left incomplete can never Allow in the meantime. P0 freezes function-level ownership:
   - **P1, the command model** (`v4/lex.rs`, `v4/units.rs`): §3 and §4.
   - **P2, paths** (`v4/paths.rs`, filesystem access through the gate's resolver): §5, including the shared `Sensitive`.
   - **P3, tools and rules** (the content of `v4/tools.rs`, `v4/rules.rs`, `atrium-protocol` `ReadOnlyCommand`, daemon rule validation, `Security.tsx`, `rule-form.ts`): §6 and §7.
3. **P4, UI and ledger:** the Unread reason in `ApprovalRequested`, no "always…" for it, and the Settings text (§9).
4. **P5, integration (one owner):**
   - `v4/evaluate.rs` implements §1's decision procedure and §2's table;
   - it wires P1–P3 into `SessionGate`;
   - it removes `dev`'s evaluator (`shell.rs` resolvers, `words.rs`, `deny.rs` and `engine.rs` paths no longer used);
   - it migrates the suites by the disposition lists.
5. **An integrated policy review** (Codex) of P5, against this contract as a whole, before the merge.

**Tests:**
- **The §10 table** as a test suite, with its fixtures.
- **Property tests** that check the boundaries independently of the implementation's classifier, with a separate small generator for each construct outside §3 and §4:
  - in Home and under prefix rules, a line containing such a construct is never Allow;
  - a fixture-planted hard-deny target that a **modeled** part touches is HardDeny in every setting;
  - compute exhaustion with no earlier decision is Ask in every setting;
  - exhaustion never creates a hit.
- **The current adversarial suites,** with expected verdicts migrated by the disposition lists.

## 14. Open questions

- **Q1.** Settled: `ln`'s target is a read (§6).
- **Q2.** The numbers in `limits`, frozen by P0 (§8).
