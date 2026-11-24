# M1-40-rev4-p4-ui-ledger: rev 4's user-facing text and the Unread reason (P4)

Model: Gemini 4 Neon (`gemini-4-neon-high`, `scripts/agent-run agy`, D-134) · Reviewer: Codex GPT-6.1 Sol `high`, read-only (D-124) · Depends on: P0 (`M1-36-rev4-p0-interface`, branched from its approved tip `a5b9b3b`)
Go: D-138, D-151 · Branch: `agent/neon/M1-40-rev4-p4-ui-ledger` · Worktree: `../atrium-wt/M1-40-rev4-p4-ui-ledger`
Contract: `docs/contracts/m1-policy-rev4.md`. Read all of it. P4 implements **§9**, with §2's stated assumptions and §11 item 3. It also takes `Security.tsx` from the M1-29 batch (§12), reassigned from P3 to P4 so that each file has one owner (orchestrator, D-151).

## Goal
When the policy asks because it **can't read a call in full** (Unread or Exhausted), the user sees that:
- the Ask dialog says so;
- it offers no "Always…";
- Settings states plainly what the hard-deny promise covers and what it doesn't.

**Nothing is wired into `SessionGate`.** That's P5. P4 adds the field and the text, and shows them when the field is set. P5 makes the gate set it. Until then the field is always absent, so behaviour doesn't change.

## What it must do

### 1. The reason in `ApprovalRequested` (`atrium-protocol`, the ledger, the TypeScript mirror)
- **The field:** add a typed reason to `ApprovalRequested` in `crates/atrium-protocol/src/agent.rs`, next to the existing free-text `reason` and `rememberable`. For example `unread: Option<UnreadNotice>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`, where `UnreadNotice` says whether it is **Unread** or **Exhausted**.
  - Keep it small and stable. No P0 types in the protocol: `atrium-protocol` must not depend on `atrium-core`.
  - An old client or a stored record without the field still parses.
- **Persistence:** follow how `reason` and `rememberable` travel and persist, from the gate's ticket through the approval ledger (`crates/atrium-core/src/policy/ledger.rs`, `RequestStore`, the daemon's persistence), out to the client and into restart recovery.
  - Carry the new field the same way, so a pending approval shows the same notice after a daemon restart.
  - If the ledger or ticket types need a field, add it with a default of absent.
- **The constructor:** add the code that builds an `ApprovalTicket` with the notice, so P5 only has to call it. Leave every current call site passing "absent".
- **The TypeScript mirror:** wherever the app types `approval_requested` (find it from `app/src/chat/transcript.ts` and `requests.ts`), plus the fixtures in `app/src/chat/fixtures/approvals.json`.

### 2. The Ask dialog (`app/src/chat/`)
When the notice is present:
- show a plain line in the approval card, worded from §9: "Atrium can't read this command in full", with a short second sentence for Exhausted, such as "It was too long or too complex to check";
- **"Always…" is not offered**, even if `rememberable` is true. This is defence in depth: P3's `remember_matcher` already returns None, and the gate sets `rememberable` false. See `rememberChoices` in `app/src/chat/inbox.ts`;
- allow once and deny keep working as today.

### 3. Settings (`app/src/ui/Security.tsx`)
- **Under the hard-deny list,** the §9 text, rendered from data the app already has (`safety_info`), not hard-coded copies of the list: "Always blocked for commands Atrium can read in full. Commands built at run time (variables, loops, `$(…)`, inline scripts) are checked on a best-effort basis under FullAccess and Any rules."
- **Then "What Atrium can't check":** §2's assumptions in plain words, one short line each:
  - command names (`PATH` shadowing isn't followed);
  - the user's own shell and repository configuration;
  - what commits and diffs contain beyond the paths a git read names;
  - other processes and timing (changes after a check);
  - hard links inside large folders beyond a bounded walk;
  - inherited file descriptors.
- **CR-12/CR-17's "Credential stores that ask" section,** from `agent/hydrogen/M1-29-cr17-stores` (below), updated to rev 4's store list (P2's four CR-17 stores plus `dev`'s `ASK_FILES`, `ASK_TREES` and `ASK_NAME_PREFIXES`). Read the store list from `dev`'s `deny.rs` and P2's handoff. Keep it consistent with what the policy protects, and add a test that pins it.
- **Style:** follow `docs/` and the app's existing copy. Plain words, short sentences, no jargon ("Unread" is an internal word; the user sees "can't read in full").

## Work carried from the M1-27 batch (contract §12)
Read `agent/hydrogen/M1-29-cr17-stores` read-only (`git diff dev...agent/hydrogen/M1-29-cr17-stores -- app/src/ui/Security.tsx app/src/ui/security.test.ts`). Keep, rewrite or drop each hunk and test, and record the disposition list in the handoff. A kept test gets rev 4's expected text and store list, never the old one.

## Tests
- **Protocol:**
  - `ApprovalRequested` round-trips with and without the notice;
  - an old JSON without the field parses;
  - the TypeScript mirror matches (follow the existing protocol-fixture tests).
- **Ledger and restart:** a pending approval with the notice keeps it across a restore, in the existing ledger or store tests. Tests that need a home folder use a temp folder and a fake `HOME`.
- **App:**
  - the approval card shows the notice for Unread and for Exhausted, and not otherwise;
  - `rememberChoices` returns no "Always…" choices when the notice is present, even if `rememberable` is true;
  - Settings renders the §9 text, the assumptions and the stores section;
  - the screenshot fixtures (`app/src/chat-shots.ts`, `gap-shots.ts`) get one approval with the notice.

## Owned paths
- **`crates/atrium-protocol/src/agent.rs`:** the `ApprovalRequested` field and its type only.
- **The ticket, ledger and store paths the field needs to travel:** `crates/atrium-agents` (`ApprovalTicket`), `crates/atrium-core/src/policy/ledger.rs`, and the store or daemon persistence of approvals. Change only what carries the new field, with a default of absent; no behaviour change.
- **The app:**
  - `app/src/chat/inbox.ts`, `inbox.test.ts`, `requests.ts`, `transcript.ts` and their tests;
  - the approval card's component;
  - `app/src/chat/fixtures/approvals.json`;
  - `app/src/ui/Security.tsx`, `app/src/ui/security.test.ts`;
  - `app/src/chat-shots.ts`, `app/src/gap-shots.ts`.
- **The handoff** `docs/agents/handoffs/M1-40-rev4-p4-ui-ledger.md`.
- **Not P4's:**
  - `policy/v4/`, `policy/model.rs`, `policy/tools.rs`, `limits.rs`, `SessionGate`, and the decision code;
  - `app/src/chat/rule-form.ts`, which is P3's.

  If you need a change there, write a change request in the handoff.

## Rules
- Follow the worker rules in `docs/agents/README.md`. Under D-123, run only:
  - `cargo fmt --all`;
  - `cargo check --workspace --tests`;
  - `cargo clippy --workspace --tests -- -D warnings`.

  No `cargo test` and no `pnpm` at all: the orchestrator runs the app's typecheck and tests. Since you can't run them, check each TypeScript change carefully against the existing types and tests.
- Never put test data in shell heredocs. Never read real credential paths.
- Commit in small steps. Never push, merge or switch branches.

## Definition of Done (the orchestrator's verification)
- [ ] fmt, workspace clippy, `cargo test --workspace --no-fail-fast`, and the app's `pnpm typecheck` and `pnpm test` clean; no existing test changed except by the disposition list or the new field's fixtures.
- [ ] Codex review: approve with no findings (D-120), against contract §2's assumptions, §9 and §11 item 3.
- [ ] If P0 changes after `a5b9b3b`, the branch is rebased onto P0's final tip, and verification and review run again.
