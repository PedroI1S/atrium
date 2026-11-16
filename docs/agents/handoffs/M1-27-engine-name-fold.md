# M1-27-engine-name-fold: partial handoff (orchestrator)

**State: partial.** The Gemini 4 Hydrogen worker stopped on 2026-11-16 at about 14:45 when agy's quota ran out ("Individual quota reached", reset in about 35 minutes). Brief: `docs/agents/briefs/M1-27-engine-name-fold.md` on `dev` (it gained an "Added scope" section after this branch was cut; read the `dev` copy).

## Done
`818f2729` folds command basenames in `shell.rs` and `deny.rs` (CR-19); `c9fbc25a` adds `-c` payload assertions that fail on `dev`. New test file `crates/atrium-core/tests/M1-27-engine-name-fold.rs` (441 lines). Diff vs `dev`: `shell.rs` +304/−144 lines (more than the brief's "one change in `base()`"; the verifier and reviewer should check it stays inside the owned ranges), `deny.rs` 18 lines.

## Not done
- The Definition of Done was in progress: the worker had run fmt, clippy and the workspace tests once. In that workspace run, `atrium-daemon`'s `requests_notifications` failed once; re-run alone it passed 2/2 (likely a timing flake under load ~100; the verifier should re-run it on a quiet machine). The 3-run `atrium-core` timing loop didn't finish.
- No worker handoff was written; this note replaces it.
- **Added scope from D-121 not started:** K7 (`sh -c '$0'` positional scripts) and K9 (long `env` lines). See the brief's "Added scope" section.

## Next step
Under D-123 (workers write, one verification task tests) and D-120 (100% gate):
1. A worker finishes the remaining scope on this branch (writing code and tests only).
2. One verification task runs the brief's Definition of Done on this branch (fmt, clippy, workspace tests, the 3-run timing loop, and the new tests failing on `dev`'s code) and records the results here.
3. Review (Opus 5.5, cross-provider), then rework until it finds nothing.
