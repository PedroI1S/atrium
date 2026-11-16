# M1-20-gate-audit: does `dev` deliver everything M1 promises?

Model: Gemini 4 Hydrogen (`gemini-4-hydrogen-high`, `scripts/agent-run agy`; D-104) · Reviewer: the orchestrator reads the report and checks a sample · Depends on: nothing (it audits `dev` as it is)
Go: D-104 (Pedro, 2026-11-16) · Branch: `agent/hydrogen/M1-20-gate-audit` · Worktree: `../atrium-wt/M1-20-gate-audit`

## Goal
Before the M1 gate review (PLAN §8, Argon + Astra), produce an evidence-backed list of what `dev` delivers and what is missing. The list covers PLAN's M1 scope, contract rev 3 (`docs/contracts/m1.md`, with rev 3.1) and the M0 gate review's promotion additions. This is an audit: you change no code. The gate reviewers and the drivers will work from your report.

## Context
- `PLAN.md` "M1: Daemon + Home + attention" (around line 727), §8 (gate), §14-related performance text.
- `docs/contracts/m1.md`, every section, including the "(rev 3.1)" paragraphs.
- `docs/agents/m1-must-fix.md`: "Where M1 stands", and "Promotion blockers and additions" under the M0 gate review.
- `docs/agents/status.md`, `docs/agents/decisions.md` (the D-numbers the contract cites), `docs/agents/change-requests.md`.
- The code: `crates/*`, `app/src/**`, `app/src-tauri/**`, `scripts/`.
- Already in flight, so don't report these as gaps; note them as "in flight":
  - M1-18 (CR-12, P1, K1–K4; on its own branch);
  - M1-19 (N3, N6, N10);
  - M1-08L (live Codex checks).

## Owned paths
- `docs/agents/reviews/M1-20-gate-audit.md` (your report), `docs/agents/handoffs/M1-20-gate-audit.md` (a short handoff).
- Nothing else. A throwaway test file is fine if you delete it before you finish.

## What to do
1. **List every promise.** Make one row for:
   - each PLAN M1 bullet item (workspace, protocol and TS generation, store, `atriumd` lifecycle with detached start, single instance, crash reaping and the launchd/systemd option, `atrium-client`, Tauri shell, tray, Home chat with Claude Code **and** Codex, the read-only and approval policy, resume, the thread state machine, inbox v1, approval queue, question forms, native notifications, `⌘J`, approval rules, the hard-deny list, native session ids, Markdown/JSONL export, daily DB snapshots);
   - each contract section's normative "must"/"never" statement;
   - each promotion addition that applies to M1.
2. **Find the evidence for each row.** Cite:
   - the code that implements it (`file:line`);
   - the test that proves it (test name), or a check script (`app/scripts/*.py`);
   - where the evidence is only a review or a report, that file.
   
   Run tests where that settles a doubt: `cargo test -p <crate> <name>`, `pnpm test`.
3. **Give each row a verdict:**
   - **delivered**: code and a test;
   - **delivered, untested**: code but no test;
   - **partial**: say what is missing;
   - **missing**;
   - **in flight**: the task named above;
   - **out of M1**: if the contract or a decision moved it, cite which.
4. **Also check the gate's process items** (M0 gate review, "Process (for M1)"):
   - ignored tests are listed and visible: list every `#[ignore]` and skipped vitest with its reason;
   - measurement evidence is pinned (which reports hold §14's numbers on macOS and Linux);
   - `scripts/doctor` covers D-030's preflight.
5. **Report.** Write `docs/agents/reviews/M1-20-gate-audit.md`:
   - a summary table (promise, verdict, evidence) grouped by area;
   - every gap with a severity (blocker / major / minor) and the smallest change that would close it;
   - a short "for the gate reviewers" section: where to look hardest.

## Definition of Done
- [ ] Every PLAN M1 bullet and every contract section appears in the table.
- [ ] Every "delivered" row cites a test or a check script, not only code.
- [ ] `git status` is clean apart from your two files, and they are committed on your branch.

## Rules
- Work only in your worktree, and commit on your branch only. Never push, merge or rebase.
- Don't change code, tests or other docs.
- Don't run the GUI check scripts (`live_check.py`, `chat_check.py`, `screenshots.py`, the benches). M1-21 owns the end-to-end runs. Cite their reports instead.

## Escalate if
You find something that reaches Allow on a credential or deletes outside a workspace. Put it at the top of the report, marked **security**, and finish the rest.
