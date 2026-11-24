# Handoff: M1-40-rev4-p4-ui-ledger (P4)

- **Branch:** `agent/neon/M1-40-rev4-p4-ui-ledger` (branched from P0's approved tip `a5b9b3b`)
- **Model:** Gemini 4 Neon (High) (D-134)
- **Contract:** `docs/contracts/m1-policy-rev4.md` (draft 5) — §2 (stated assumptions), §7 & §9 (what the user sees: Ask dialogs and Settings), §11 item 3, §12 (carried M1-29 `Security.tsx` & `security.test.ts`)
- **Behaviour change:** None for live policy decisions. Nothing is wired into `SessionGate` (that is P5); every live call site passes `unread: None`, so runtime approval behaviour is unchanged until P5 sets the notice. Settings → Security now displays the rev 4 §9 hard-deny scope text, §2 assumptions ("What Atrium can't check"), and the CR-12/CR-17 "Credential stores that ask" section.

---

## 1. What was implemented, by contract section

### §9 & §7 — Typed `UnreadNotice` in `ApprovalRequested`, tickets, ledger, store, and adapters
- **Protocol (`crates/atrium-protocol/src/agent.rs`, `examples/export-ts.rs`, `bindings/protocol.ts`, `tests/serde_shape.rs`):**
  - Added `pub enum UnreadNotice { Unread, Exhausted }` (`#[serde(rename_all = "snake_case")]`, `#[cfg_attr(feature = "ts", derive(specta::Type))]`) with no dependency on `atrium-core`.
  - Added `#[serde(default, skip_serializing_if = "Option::is_none")] unread: Option<UnreadNotice>` to `AgentEvent::ApprovalRequested`, next to `reason` and `rememberable`.
  - Registered `UnreadNotice` in `examples/export-ts.rs` and updated `bindings/protocol.ts` and `app/src/daemon/types.ts`.
  - Added round-trip tests (absent, `Unread`, `Exhausted`, explicit `null`), legacy-JSON backwards-compatibility tests, and a protocol-fixture test verifying `app/src/chat/fixtures/approvals.json` and `bindings/protocol.ts` against Rust `ServerMessage` / `UnreadNotice`.
- **Tickets and constructors (`crates/atrium-agents/src/lib.rs`, `src/claude/normalizer.rs`, `src/codex/normalizer.rs`, `crates/atrium-daemon/src/fake.rs`):**
  - Added `#[serde(default, skip_serializing_if = "Option::is_none")] pub unread: Option<UnreadNotice>` to `ApprovalTicket`.
  - Added `ApprovalTicket::new(request_id, rememberable)`, `ApprovalTicket::with_unread(request_id, unread)` (forces `rememberable: false`), and `ApprovalTicket::with_notice(request_id, rememberable, unread)` (forces `rememberable: false` whenever `unread.is_some()`).
  - Propagated `ticket.unread` into `AgentEvent::ApprovalRequested` in both `ClaudeNormalizer`, `CodexNormalizer`, `atrium-daemon::fake`, and daemon test helpers. Left all existing call sites passing absent (`None`).
  - Added normalizer tests in `crates/atrium-agents/tests/claude_normalizer.rs` and `codex_normalizer.rs`.
- **Ledger and restart persistence (`crates/atrium-core/src/lib.rs`, `src/policy/ledger.rs`, `src/store/requests.rs`, `src/policy/gate.rs`, `src/history/export.rs`):**
  - Added `pub unread: Option<UnreadNotice>` to `Binding` (existing `SessionGate::do_check` literal in `policy/gate.rs` passes `unread: None` with zero behaviour change).
  - Updated `Ledger::open` to carry `binding.unread` into `ApprovalTicket::with_notice`, forcing `rememberable = false` when `unread.is_some()`.
  - Updated `Ledger::resolve` as defence in depth: refuses to create a remembered rule (`Ok(None)`) when `binding.unread.is_some()`, while still allowing once.
  - Updated `SqliteStore` (`store/requests.rs`) to persist `Binding::unread` in `requests.context` via a private `StoredContext` (`#[serde(flatten)]` + `#[serde(default, skip_serializing_if = "Option::is_none")]`), keeping schema version 1 untouched (exact legacy JSON when `unread: None`, backwards-compatible with existing rows). Event history in `events.payload` persists `ApprovalRequested.unread` automatically.
  - Added tests in `crates/atrium-core/tests/ledger.rs` (`unread_and_exhausted_approvals_carry_notice_and_never_remember`) and `crates/atrium-core/tests/store.rs` (`pending_approval_with_unread_notice_survives_restart`, using `tempfile::tempdir()`).

### §9 & §7 — Ask dialog (`app/src/chat/`, `app/src/ui/RequestCard.tsx`, fixtures & shots)
- **`app/src/chat/transcript.ts`:** added `unread: UnreadNotice | null` to `ApprovalItem`, defaulting omitted/null `ev.unread` to `null`.
- **`app/src/chat/requests.ts`:**
  - Added `unreadNoticeOf(request: ApprovalItem): string | null`:
    - `"unread"` → `"Atrium can't read this command in full."`
    - `"exhausted"` → `"Atrium can't read this command in full. It was too long or too complex to check."`
    - `null` → `null`
  - Updated `rememberChoices(request)` so it returns `[]` whenever `request.unread !== null`, even if `request.rememberable` is `true` (defence in depth).
- **`app/src/ui/RequestCard.tsx`:** renders the notice (`[data-atrium-part="approval-unread"]` with `data-unread`) in `ApprovalCard` when present, omits the "Always…" button/menu via `rememberChoices`, and leaves "Allow once" and "Deny" unchanged.
- **Fixtures and screenshot scripts:**
  - Appended two events (ids 21 and 22, preserving ids 1–20) to `app/src/chat/fixtures/approvals.json`: one with `"unread": "unread"` and one with `"unread": "exhausted"`.
  - Added an `unread: "unread"` approval scene (`chat-approval-unread`) to `app/src/chat-shots.ts` without altering `seeds().inbox` counts used by `gap-shots.ts`.
  - Updated `seed-approval-3` in `app/src/gap-shots.ts` to carry `unread: "unread"` (keeping the exact 3-approval batch count).
- **App tests:** added coverage in `app/src/chat/inbox.test.ts`, `app/src/chat/transcript.test.ts`, and `app/src/requests.test.ts` (DOM rendering of `ApprovalCard` for ordinary, `unread`, and `exhausted` approvals, verifying notice copy, absence of `Always…` even when `rememberable: true`, and working `Allow once` / `Deny`).

### §2, §9, §11 item 3 & §12 — Settings → Security (`app/src/ui/Security.tsx`, `app/src/ui/security.test.ts`)
- **Hard-deny list (`AlwaysBlocked`):** continues rendering `HARD_DENY_DEFAULTS` from `safetyInfo()` data (never hard-coded) and adds the §9 text under the list (`[data-atrium-part="hard-deny-scope"]`):
  > "Always blocked for commands Atrium can read in full. Commands built at run time (variables, loops, $(…), inline scripts) are checked on a best-effort basis under FullAccess and Any rules."
  followed by the coverage/isolation reminder.
- **"What Atrium can't check" (`WhatAtriumCantCheck`, `UNCHECKED_ASSUMPTIONS`):** renders §2's six stated assumptions in plain words, one short line each (no internal jargon such as "Unread"):
  1. Command names (`PATH` shadowing isn't followed);
  2. Your own shell and repository configuration;
  3. Commit and diff contents beyond the paths a git read names;
  4. Other processes and timing (changes after a check);
  5. Hard links inside large folders beyond a bounded walk;
  6. Inherited file descriptors beyond standard input, output, and error.
- **"Credential stores that ask" (`CredentialStoresThatAsk`, `ASK_STORES`):** carried from `agent/hydrogen/M1-29-cr17-stores` and updated for rev 4 (all 16 `dev` `ASK_FILES`, 3 `ASK_TREES`, 1 `ASK_NAME_PREFIXES`, P2's 4 CR-17 stores, and rev 4 §6 git reads `git show`, `git cat-file`, `git log -p`, `git diff`).
- **Known limits (`KnownLimits`):** kept intact (`git-config`, `same-user`, `no-isolation`) for `m1-demo.ts` compatibility.

---

## 2. Disposition list for carried hunks and tests (`agent/hydrogen/M1-29-cr17-stores`, contract §12)

Inspected via `git diff dev...agent/hydrogen/M1-29-cr17-stores -- app/src/ui/Security.tsx app/src/ui/security.test.ts`.

### Hunks (`app/src/ui/Security.tsx`)
| Source hunk | Disposition | Notes |
|---|---|---|
| `AskStore` interface + `ASK_STORES` constant (4 groups covering CR-12 + CR-17 stores) | **Kept (updated for rev 4)** | Kept all 24 store paths (16 `dev` `ASK_FILES`, 3 `ASK_TREES`, 1 `ASK_NAME_PREFIXES`, and P2's 4 CR-17 stores: `~/.gem/credentials`, `~/.terraform.d/credentials.tfrc.json`, `~/.config/op/config`, `~/.yarnrc.yml`); updated the git-reads wording in `git-env` to match rev 4 §6 (`git show, git cat-file, git log -p, git diff`) and aligned group titles with the app's plain copy style (`and` instead of `&`). |
| `CredentialStoresThatAsk()` replacing the `null` placeholder | **Kept** | Mounted in `BLOCKS["always-blocked"]` after the new `WhatAtriumCantCheck` section and before `KnownLimits`. |

### Tests (`app/src/ui/security.test.ts`)
| Source test | Disposition | Notes |
|---|---|---|
| `lists every CR-12 and CR-17 store in ASK_STORES` | **Kept (expanded for rev 4)** | Expanded from 17 to all 24 rev 4 store paths (adding `.zhistory`, `.sh_history`, `.ksh_history`, `.history`, `.config/fish/fish_history`, `.zsh_sessions/`, `.bash_sessions/`). |
| `matches every ASK_FILES, ASK_TREES, and ASK_NAME_PREFIXES entry in core deny.rs` | **Kept (updated for rev 4)** | Still parses `crates/atrium-core/src/policy/deny.rs` (`ASK_FILES`, `ASK_TREES`, `ASK_NAME_PREFIXES`) and additionally pins P2's 4 CR-17 stores explicitly (since `deny.rs` is untouched until P5 integrates P2). |
| `renders the ask-stores list inside SecurityPage` | **Kept (expanded for rev 4)** | Expanded to also assert rendering of `safetyInfo().hardDeny`, the §9 hard-deny scope text (`[data-atrium-part="hard-deny-scope"]`), the "What Atrium can't check" section with all 6 §2 assumptions, absence of internal jargon (`"Unread"`), CR-12/CR-17 ask stores, and the existing `KnownLimits` items. |

---

## 3. Change requests against P0's interface

None. No file under `crates/atrium-core/src/policy/{model.rs,limits.rs,tools.rs,v4/}` was modified, and no P0 interface changes are needed for P4.

---

## 4. Verification commands

Run by worker (allowed under D-123):
- `cargo fmt --all` — clean
- `cargo check --workspace --tests` — clean (0 errors, 0 warnings)
- `cargo clippy --workspace --tests -- -D warnings` — clean (0 errors, 0 warnings)

Commands for the orchestrator's verification task (fish-compatible):

```sh
cargo run -p atrium-protocol --example export-ts --features ts
bash scripts/check-protocol-ts
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --no-fail-fast
pnpm --dir app typecheck
pnpm --dir app test
```

Note: `crates/atrium-protocol/bindings/protocol.ts` was updated by hand to match Specta's output because `cargo run` is outside the worker's D-123 command allowlist; running `export-ts` and `bash scripts/check-protocol-ts` first verifies or regenerates the exact byte-for-byte binding file before `pnpm --dir app typecheck` and `pnpm --dir app test`.

---

## 5. Round 2 (rework)

Addressed the sole finding from `.orchestration/review-input/review-round1.md` (Low: Settings assumptions retained technical shorthand at `app/src/ui/Security.tsx:62,82`) and audited every user-facing string added in P4 (Ask-dialog notice in `app/src/chat/requests.ts`, hard-deny scope text, assumptions, and credential stores section in `app/src/ui/Security.tsx`) so none uses internal words (`Unread`, `Exhausted`, `walk`, `canonical`, `PATH`, `fd`, `identity`).

### Strings changed (`app/src/ui/Security.tsx`, `UNCHECKED_ASSUMPTIONS`)

| Item (`id`) | Old text | New text |
|---|---|---|
| `command-names` | `A command like cat or git is assumed to be that system tool; PATH shadowing isn't followed.` | `A command called cat or git is assumed to be the usual system tool, even if another program with that name comes first on the system.` |
| `repo-contents` | `What commits and diffs contain beyond the paths a git read names.` | `What commits and diffs contain beyond the file paths a git command names.` |
| `hard-links` | `Hard links inside a large folder beyond Atrium's bounded walk.` | `In large folders, Atrium checks only a limited number of files and folder levels for hard links to protected files.` |
| `inherited-fds` | `Open file descriptors inherited beyond standard input, output, and error.` | `Open files passed down from a parent program beyond standard input, output, and error.` |

### Other user-facing strings audited (unchanged, already plain words)
- Ask-dialog notice (`app/src/chat/requests.ts`): `"Atrium can't read this command in full."` and `"Atrium can't read this command in full. It was too long or too complex to check."`
- Settings hard-deny scope (`app/src/ui/Security.tsx`): contract §9 wording + existing `dev` sentences.
- Settings assumptions heading & unchanged items (`user-config`, `other-processes`) and "Credential stores that ask" section (`app/src/ui/Security.tsx`).

### Tests and verification
- Updated `app/src/ui/security.test.ts` to assert the rewritten plain-word assumptions and check rendered `SecurityPage` text against `/\bunread\b/i`, `/\bexhausted\b/i`, `/\bwalk\b/i`, `/\bcanonical\b/i`, `/\bPATH\b/`, `/\bfd\b/i`, and `/\bidentity\b/i`.
- No Rust files were modified in Round 2 (only `app/src/ui/Security.tsx`, `app/src/ui/security.test.ts`, and this handoff).
- Orchestrator verification commands (fish-compatible):
  ```sh
  pnpm --dir app typecheck
  pnpm --dir app test
  ```
