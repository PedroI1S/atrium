# Policy engine rev 4: exact, or not established (draft)

**Status: draft, 2026-11-21.** Written by the orchestrator at rodrigo's request (D-135). Nothing is implemented, and no worker has a brief.
- **The text:** rodrigo (driver) approves it.
- **Changes to security defaults:** each one is marked **[D-013]**, and Pedro confirms it before any worker starts.
- **What it amends:** contract `m1.md` §9. Where the two disagree, this file wins once it is approved.

## Why

The M1-27 batch (M1-27 to M1-32) had 18 Codex reviews, and every one asked for rework. Each round closed its findings and the next round found a neighbouring variant. The cause is in the goals, not in the workers. Several must-fix items asked the engine to work out *exactly* what a dynamic shell line does, either to keep it allowed or to turn its Ask into a hard deny:
- K7: resolve `sh -c '"$1"' _ …`;
- K9: give long `env` lines the same verdict as the short form;
- M1-28: read `$'…'` words;
- S5, S11: reason about glob patterns against the protected set.

Shell is too large for that, so the variants never end.

This revision limits what the engine promises:
- **Exact checks** for everything it can read completely.
- **A clear "not established" outcome** for everything else, with a fixed verdict per setting.

A measurement over 3,353 shell commands from real agent sessions on rodrigo's machine shows why "not established" can't simply mean Ask everywhere:
- **Overall:** 53% use at least one construct outside the readable grammar (§3).
- **By construct:** `$VAR` 38%, loops 23%, `$(…)` 20%, here-documents 18%, inline `awk`/`python -c` 9%.

Asking for all of them would make FullAccess useless. Under FullAccess, the hard deny can't stop an agent that wants to get around it anyway: writing `sudo id` into `x.sh` and running `sh x.sh` gets past any parser, because the engine never reads file contents. Real limits for FullAccess come from the sandbox (D-044), not from a smarter shell parser.

## 1. Three outcomes

Every check of a call ends in one of three outcomes. The engine never turns doubt into a hard deny, and never turns doubt into an Allow outside the settings listed in §2.

| Outcome | Meaning |
|---|---|
| **Exact hit** | Atrium established, with no guess, that the call touches something on the hard-deny list, a credential store that asks (CR-12, CR-17) or Atrium's own paths. |
| **Exact clear** | Atrium read the whole call, and every command it runs and every path it touches is known and is none of those. |
| **Not established** | Anything else: a construct outside §3's grammar, a path or glob §4 can't resolve within budget, an unknown tool's code, or a budget that ran out (§7). |

## 2. What each setting does with them

| Outcome | Home, Careful, Normal | Prefix, exact or scoped rules | Any rule | FullAccess |
|---|---|---|---|---|
| Exact hit (hard-deny list) | HardDeny | HardDeny | HardDeny | HardDeny |
| Exact hit (store that asks) | Ask | Allow only by a rule naming it (as today) | Allow | Allow |
| Exact clear | the preset's rules (as today) | the rule decides | Allow | Allow |
| Not established | **Ask** | **Ask** (a rule never covers a line it can't read) | **Allow**, after the best-effort check | **Allow**, after the best-effort check |

- **The best-effort check** is `dev`'s current hard-deny matching on the raw text and on every unit the lexer did produce. A hit there is a HardDeny. A miss is not a finding: the contract makes no promise about lines that aren't readable. **[D-013]** This makes D-133's second point the general rule.
- **Trusted** is an M2 preset. It treats "not established" as Ask until M2 decides otherwise.
- A subject the adapter can't extract (`ToolSubject::Unknown`) is "not established", and the best-effort check runs on its raw input (as `m1.md` §9 says today).

## 3. Commands: the readable grammar

A command line is **readable** when the lexer gets it, within budget, into units built only from what follows.

**Words:**
- unquoted literal characters, single-quoted text, double-quoted text with no `$`, backtick or `\` other than `\"`, `\\`, `\$` and `` \` ``;
- a backslash before an ASCII character;
- `~` and `~/…` at the start of a word;
- `$HOME`, `${HOME}`, `$PWD` and `${PWD}` (the gate supplies home and the tracked cwd).

Everything else that expands is outside the grammar:
- any other `$` form, including `$'…'` (ANSI-C) and `$"…"`;
- backticks, `$(…)`, `<(…)` and `>(…)`;
- arithmetic.

**Structure:**
- separators `;`, `&&`, `||`, `|` and newline;
- redirects that are fd duplication, or that go to or from a readable literal path (the path is then checked under §4).

Outside it: `&` (background), here-documents and here-strings, groups and subshells, functions, and every reserved word (`if`, `for`, `while`, `case`, …).

**Assignments:** a leading `NAME=value` whose value is a readable word. The command after it is read as usual. A value that expands makes the line not established. This closes M1-27 round 3's finding 1 by construction.

**Command words:**
- The command word is folded (CR-19).
- Wrappers are unwrapped through a closed list of known wrappers, with their known options only. An unknown option on a wrapper makes the line not established.
- The unwrap depth has a fixed bound. Reaching it makes the line not established, never "stop and check what is left" (M1-27 round 3, finding 5).

**Nested shells:** `sh`, `bash`, `zsh` and the rest of `SHELLS`, with `-c` and a script that is itself a readable line, are read recursively up to the depth bound. Any operand after the script (`$0`, `$1`, …) makes the line not established. **K7's positional resolution is removed.**

**Code runners:** a tool marked in §5 as running code it is given makes the line not established, unless §5 says how to read that code. This covers:
- `awk` and `sed` programs, `perl -e`, `python -c` and `node -e`;
- `find -exec`, `xargs`, `env -S`;
- `eval`, `source` and `.`;
- the program-running git subcommands;
- `pipx run`, `npx`;
- the tools in `COMMAND_RUNNERS`.

A readable line has exact units. Hard-deny matching, rule matching and Home's read-only list all work on those units only.

## 4. Paths, globs and links

The engine still never touches the filesystem; `SessionGate` does. Resolved paths are bytes, never patterns: a resolved name is never parsed again as glob syntax (M1-32 round 4, finding 1).

- **Literal paths** (command arguments, `Read`/`Write` paths, redirect targets): resolved as today, with the deepest existing ancestor canonicalized, symlinks followed up to `MAX_HOPS`, and the identity (device, inode) compared against the protected set. If a path can't be resolved (an unreadable directory, too many hops), it is not established (M1-32 round 4, finding 4).
- **Globs in a path:**
  - **Forward expansion:** `SessionGate` expands the glob against the filesystem, as the shell would at that cwd, within §7's budget. Every match is then checked as a literal path. A glob that matches nothing is checked as the literal word, as the shell passes it. Running out of budget makes it not established.
  - **No pattern-versus-pattern reasoning.** The engine never decides whether two patterns overlap, which ends the `ShellGlob` / variant-widening / class-complement work of M1-30.
- **Search filters** (`Search { glob }`, `rg -g`, `--include`): the filter is matched against the **concrete, existing** protected paths under each root. Each is a single path tested against the pattern with a standard matcher such as `globset`. Protected trees (`.gnupg`, browser profiles, `.zsh_sessions`, …) are listed within budget for this. Running out of budget makes it not established.
- **Recursive reads:**
  - **Protected paths:** a root that contains an existing protected path gets that path's outcome. S8 and D-133's `~/.config` exception stay as they are.
  - **Followed links:** when the reader follows links (`rg -L`, `grep -R`, `find -L`), `SessionGate` walks the root's links within budget, and a link that reaches a protected path is an exact hit. Running out of budget is not established, never a hard deny (the `rg -L` false block).
- **`cd`:** `cd` to a readable literal path moves resolution, as today. `cd` with a glob or anything not readable makes the cwd not established. This replaces M1-28's globbed `cd` candidates.
- **Hard links inside recursive roots:** `dev`'s bounded walk stays as it is, outside this contract's exactness promise (Q2).

## 5. Known tools: one table

Today, the knowledge of tool syntax is spread across several places:
- `HOME_READONLY_COMMANDS`;
- the `rg`/`grep`/`ag`/`ack` flag tables;
- `wrapper_opts`, `WRITERS`/`READERS`, `COMMAND_RUNNERS`;
- the git credential guards.

They become **one table** in `atrium-core::policy`. For each tool, the table records:
- its folded name, its subcommands, and its flags (boolean, or taking a value);
- for each operand: a **path**, a **pattern**, an **archive member** (`tar -t`), a **ref**, or **text**;
- whether it reads recursively, whether it follows links, and whether it writes;
- whether it **runs code**, and which operand that code is.

How the table is used:
- **Readability:** a known tool with an unknown flag is not established.
- **Unknown tools:** every word of an unknown tool is a candidate path, as today, so exact hits still apply.
- **Home's read-only list** becomes a property in this table ("read-only, with these flags forbidden").

## 6. Rules

- **Coverage:** a prefix or exact rule covers a unit only when the line is readable. A rule never makes a not-established line allowed (§2).
- **Rule creation:** an Allow prefix rule whose prefix ends at a code runner (§5), with no pinned program, is **Invalid** at creation, and the rules editor says why. This covers `["awk"]`, `["python"]`, `["pipx", "run"]` and `["git"]` when it would cover `merge-index`, `filter-branch` and the like. This closes M1-29 round 4's findings 4–6 by construction.
- **Path scopes:** a scope is checked against the canonical targets from §4, including write targets that §5 finds inside programs it can read. A target it can't establish makes the rule not cover the unit (M1-29 round 4, findings 2–3).
- **Matching:** `Any` rules, `CommandExact` from "always…", and rule storage are unchanged.

## 7. Budgets

- **One total budget per check** covers lexing, unwrapping, expansion, filesystem walks and matching, as the design rules already say.
- **Time:** work is linear in the input length, so no stage copies suffixes or rescans the remainder (M1-30 round 3, finding 5; M1-31 round 2, findings 5–6).
- **Exhaustion:** running out is always **not established**, never a hard deny and never an exact clear (M1-30 round 3, finding 6).
- **Size limit:** a line over `MAX_BYTES` stays `Deny` ("too large to check"), as today.

## 8. What the user sees

- **Settings, under the hard-deny list:** one line: "Always blocked for commands Atrium can read in full. Commands built at run time (variables, loops, `$(…)`) are checked on a best-effort basis under FullAccess and Any rules." **[D-013]**
- **Ask dialogs:** an Ask caused by "not established" says so ("Atrium can't read this command in full"). "Always…" is not offered for it, since no rule could cover it.

## 9. Examples

Home / prefix rule (`sh`, `awk` or `cat` as given) / Any rule / FullAccess, with `home = cwd = /h/u`.

| Call | Rev 4 verdict | Why |
|---|---|---|
| `cat README.md` | Allow / Allow / Allow / Allow | exact clear |
| `cat ~/.ssh/id_rsa` | HardDeny everywhere | exact hit |
| `SUDO id`, `/usr/bin/sudo id` | HardDeny everywhere | folded command word (CR-19) |
| `sh -c 'X="$1" sudo id' _ foo` | Ask / Ask / Allow\* / Allow\* | an operand after the script, so not established |
| `command` ×13 `sudo id` | Ask / Ask / Allow\* / Allow\* | wrapper bound reached |
| `cat $'\xff'` | Ask / Ask / Allow / Allow | ANSI-C word |
| `for f in *.rs; do wc -l "$f"; done` | Ask / Ask / Allow / Allow | loop |
| `cat proj/key*` (`proj` links to a store whose `key` links to `.ssh/id_rsa`) | HardDeny everywhere | forward expansion finds the real match |
| `Read /h/u/[!é]trium/secret` (`/h/u/átrium` is an Atrium data path) | HardDeny everywhere | forward expansion |
| `Search ~, glob **/ms?l_*.custom` (`.azure/msal_token_cache.custom` exists) | Ask / … / Allow / Allow | concrete store matched by the filter |
| `awk 'BEGIN{system("touch v")}'` with an Allow `["awk"]` | the rule is Invalid at creation; the call asks | code runner |
| `rg -L x ~/proj` with no link reaching a protected path | Allow | link walk within budget |
| `rg x ~/.aws` / `rg x ~/.config` | HardDeny / Ask | S8 / D-133, unchanged |

\* unless the best-effort check matches the raw text.

## 10. Changes against `dev` that need Pedro **[D-013]**

1. **Not established allows under Any rules and FullAccess.** The hard deny is best-effort for lines that aren't readable. This generalizes D-133's second point, and the batch's reviews no longer count those misses.
2. **Not established always asks in Home and under prefix, exact and scoped rules,** including lines that `dev` allows by resolving them, such as K7's exact positional cases (D-130).
3. **Code-runner prefixes are Invalid at creation** (§6).
4. **The Settings text** in §8.

Everything else is what `dev` and `m1.md` §9 already promise, implemented more simply.

## 11. What happens to the M1-27 batch

None of the six branches is merged. Their tests are kept as cases wherever §9-style verdicts still apply.

| Task | Kept | Dropped or replaced |
|---|---|---|
| M1-27 engine-name-fold | CR-19 folding (needed by §3); `deny.rs` name comparisons; bounded code scans where §3 still reads code | K7 resolution (replaced by "not established"); K9 parity (round 3, finding 7, no longer required) |
| M1-28 ansi-c-words | **Non-policy work, split into its own branch now:** N10 (daemon lane), N12 (daemon socket races, `gate_withdraw`), the late orphan withdrawal (D-122), fish PATH discovery, the quadratic markdown parser | the `$'…'` reader; globbed `cd` candidates (finding 11's `cd_effect` half) |
| M1-29 cr17-stores | CR-17's four stores; the rules-editor UI (`app/src/chat/rule-form.ts`) | the per-tool prefix guards (replaced by §5–§6) |
| M1-30 tree-wildcards | its test cases | the replacement matcher (replaced by §4) |
| M1-31 credential-folder-search | **N9 and N11, split now;** S8's verdicts (§4) | its root parser (replaced by §5's table) |
| M1-32 link-globs | its test cases; the `rg -L` false block fix as a verdict (§4) | the link-glob expansion code (replaced by §4) |

## 12. Tasks after approval

Each task follows PLAN §8: a brief, Neon `high` writers (D-134), the orchestrator's verification (D-123), Codex review (D-124) and the 100% gate (D-120).

- **Now, without waiting for Pedro:** `M1-33-nonpolicy-fixes`, cherry-picked from M1-28 (N10, N12, orphan withdrawal, fish, markdown), and `M1-34-linux-test-fixes`, from M1-31 (N9, N11). These are verification and review only, with no new design.
- **After Pedro's confirmation**, in order:
  1. **`P1-readable-grammar`** (`shell.rs`, `words.rs`, the outcome type in `deny.rs`, the §2 table in `engine.rs`). It removes the resolvers.
  2. **`P2-paths-exact`** (`gate.rs`, `Sensitive` in `deny.rs`): §4.
  3. **`P3-known-tools`** (the §5 table, `engine.rs` rules, `atrium-protocol` `ReadOnlyCommand`, the rules editor): §5–§6.

  P2 and P3 start after P1's outcome type lands. They can run in parallel, because their owned paths don't overlap.
- **Tests:**
  - A property test: under Home and prefix rules, a random line containing any construct outside §3 is never Allow.
  - A property test: every exact hit is HardDeny in every setting.
  - The current adversarial suites, with expected verdicts updated from §9 and the list in §10.

## 13. Open questions

- **Q1.** Trusted (M2): Ask or best-effort Allow for not established? This draft says Ask until M2 decides.
- **Q2.** Hard links inside recursive roots: `dev`'s walk stops at 4,096 entries and then treats the tree as clear. Keep it outside the exactness promise (this draft), or make running out of budget not established, which would ask for most searches of large repositories in Home?
- **Q3.** `$HOME` and `$PWD` are readable (§3). Should any other variable be, such as `$TMPDIR` or `$USER`? This draft says no.
