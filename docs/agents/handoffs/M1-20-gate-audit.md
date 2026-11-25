# Handoff: M1-20-gate-audit

- **Task:** `M1-20-gate-audit` ([D-104](../decisions.md))
- **Worker branch:** `agent/hydrogen/M1-20-gate-audit`
- **Audited baseline commit:** `b8ebe26` (`dev`)
- **Date:** 2026-11-16

## Deliverables Written

- [`docs/agents/reviews/M1-20-gate-audit.md`](../reviews/M1-20-gate-audit.md) — Full M1 gate audit covering `PLAN.md` (M1 bullets + referenced §3.x/§4/§8), `docs/contracts/m1.md` (§§1–15 rev 3 + rev 3.1), `docs/agents/m1-must-fix.md` (M0 gate promotions + open trackers), gate process items (7 `#[ignore]` Rust tests, 0 skipped Vitest tests, pinned §14 macOS/Linux evidence, `scripts/doctor` D-030 preflight coverage), ranked gaps (`blocker` / `major` / `minor`), and "For the gate reviewers" priorities.
- [`docs/agents/handoffs/M1-20-gate-audit.md`](M1-20-gate-audit.md) — This handoff note.

## Key Audit Findings (from `b8ebe26`)

- **Top Security Note:** No *new* Allow-on-credential or delete-outside-workspace bugs were found in code or active tests on `b8ebe26`. Four documented `M1-13` security findings remain open and `#[ignore]`d (`S5`, `S7`, `S8`, `S11` — 5 tests in `atrium-core`), plus `finding_6` (`S1` / CR-12) which is `in flight` in `M1-18`.
- **Blockers (2):**
  1. `crates/atrium-daemon/src/main.rs:68-86` leaves `options.adapters` empty (`Vec::new()` from `Options::new`); the real `atriumd` executable never registers `ClaudeAdapter` or `CodexAdapter`.
  2. Linux desktop idle memory in `docs/spikes/M1-linux-checks.md:183-199` misses the `<150 MB` §14 gate (`151.4–152.5 MiB` / `158.7–160.0 MB` private PSS), and raw M1-14b artifacts were lost (`docs/agents/status.md:35`).
- **Majors (5):** UI lacks `ExportThread` action (`rg` 0 hits in `app/src/`), Rules editor / `CreateRule` (`CommandPrefix` / `Always blocked` hard-deny list / `git config` & same-UID disclosures), `ArchiveThread` / `RenameThread` UI controls, Enforcement Matrix `"details"` drawer (`coverage` per `ToolClass` & `config_sources`), and Inbox batch approve/deny.
- **Minors (5):** `⌘⇧J` missing (`app/src/App.tsx:18` sends all `j` keys to `jumpToNext`), open `S5/S7/S8/S11` + `K5/P2`, live CLI verification gaps (`ClaudeAdapter::DeclineQuestion`, Linux GNOME tray unverified, quiet §14 re-runs, release `AppIdentitySource::None` until M8), `scripts/doctor:103-105` exiting `1` inside worker worktrees due to `.agents/hooks.json`, and `engine_never_panics_on_garbage` taking `119.54s` on `b8ebe26` (fix `in flight` in `M1-18`).
- **Marked `in flight` (excluded from gap counts):** `M1-18` (CR-12, `P1`, `K1–K4`, slow fuzz), `M1-19` (`N3`, `N6`, `N10`), `M1-08L` (live Codex checks `X1–X3`).

## Checks Run on `b8ebe26`

No GUI check scripts (`live_check.py`, `chat_check.py`, `screenshots.py`) or performance benches were run (per brief).

- `./scripts/check-protocol-ts` -> exited `0` (`crates/atrium-protocol/bindings/protocol.ts is up to date.`, `specta v2.0.0-rc.26`).
- `pnpm --dir app test && pnpm --dir app typecheck` -> exited `0` (Vitest: `14` test files, `264` tests passed, `0` skipped, `0` failed; `tsc --noEmit` clean).
- `cargo test -p atrium-core --no-fail-fast` (`+ adversarial_policy / adversarial_regression -q`) -> exited `0` (`250` passed, `0` failed, `6` ignored across `lib` 9, `adversarial_fuzz` 2/1i, `adversarial_gate` 9/2i, `adversarial_ledger` 14, `adversarial_ordinary` 16, `adversarial_policy` 15/3i, `adversarial_regression` 57, `attention` 9, `history` 25, `ledger` 20, `policy` 44, `store` 30).
- `cargo test --workspace --exclude atrium-core --no-fail-fast -- -q` -> exited `0` (`451` passed, `0` failed, `1` ignored on macOS: `atrium-agents` 235, `atrium-app` 47, `atrium-client` 40, `atrium-daemon` 81, `atrium-host` 35/1i, `atrium-protocol` 13).
- `./scripts/doctor` -> exited `1` solely on `✗ .agents/hooks.json exists in the main checkout` (expected in worker worktree; all tools, `gh`, synced-folder, `git fsck`, conflict-copy, `agy 1.5.9`, and `agy-policy.py` checks passed).

## Left Unchanged

- Zero code, test, or unrelated doc files modified; `docs/agents/reviews/M1-20-gate-audit.md` and `docs/agents/handoffs/M1-20-gate-audit.md` are the only files created/committed on `agent/hydrogen/M1-20-gate-audit`.
