# Handoff: M1-11-export

Model: Claude Fable 5 (the orchestrator, inline; D-055) · Status: done, before review

## Done
- `history::reconstruct_message`, `Redactor`, `export_thread` and `create_export_file` are implemented. No public signature changed, and `atrium-core` needed no new dependency.
- The module is now four files: `mod.rs` (reconstruction), `redact.rs`, `export.rs`, `file.rs`.
- The three stub tests are un-ignored. `tests/history.rs` has 25 tests and the module has 9 unit tests.

## How it works

**Reconstruction.** A `Streamed` body must match the deltas in count, in length (Unicode scalar values) and in SHA-256. An `Inline` body must come with no deltas, which is the same rule the store logs as an integrity error. Anything else is `HistoryError::Integrity`.

**Export.** Both formats are written from one folded event stream, so redaction always sees whole values (A12):
- A message's deltas are joined, verified, and written as one `MessageCompleted` with an `Inline` body.
- Streamed tool output is collected and written once, in `ToolCallFinished::output`. A final `output` replaces what was streamed, as in the M0 reducer.
- Items still open when their session ends, when the next session starts, or when the events run out are written as far as they got.
- JSONL is one folded event per line in the `StoredEvent` wire shape, so `bindings/protocol.ts` types read it.
- Markdown is a transcript: sessions, messages, tool calls with input and result, approvals, questions, plans, errors and turn totals. It leaves out running usage, context, quota and unmapped records.

**Redaction.** `Redactor::text` only replaces spans, so the exported text is the original with holes.
- Scanners run on the text as written and again on up to three layers of decoded escapes (JSON escapes, `\'`, `\xNN`, `\u{…}`). A match in a decoded layer is mapped back to the bytes that produced it (A13).
- For JSONL and for tool input in Markdown, the JSON value is walked: every string and object key is redacted, and a string under a key that names a secret is replaced whole.
- In Markdown, session text can only enter through methods that redact it; the method for fixed text takes `&'static str` only.
- The full list of what is recognised is in the `Redactor` doc comment. It is wider than the contract's list: GitLab, npm, Stripe and Google OAuth tokens, JWTs, URL passwords, `--flag value`, quoted values, and YAML lines were added.

**Export files.** The folder is opened first, its owner is checked on that handle, and the file is created relative to it with `O_CREAT | O_EXCL | O_NOFOLLOW`, then set to exactly 0600.

## Found while testing
- Two scanners were quadratic on one long line: a megabyte of `token=` took 43 s and a megabyte of `authorization:` took 16 s in a release build. Tool output is agent-controlled, so that would have stalled the daemon during an export. Both now skip past a value once it is taken. The worst of 53 adversarial one-megabyte inputs is 43 ms, and `redaction_stays_linear_on_one_long_line` guards it.
- Three deliberate breakages were each caught by the tests: no escape decoding, one escape layer only, and no folder-owner check.

## Not caught by redaction
- A secret with no recognisable shape that isn't next to a telling key, such as a bare password in prose.
- `key: value` YAML lines whose value is short or has no digit (`password: hunter2`). Taking those would also blank type annotations in code (`token: String`).
- `token = value` with spaces and no quotes, for the same reason (`token = lexer.next()`).
- Encodings other than JSON-style escapes: base64, percent-encoding, more than three layers of escaping.
- Numbers under a secret key (`"pin": 1234`).

## For M1-05 (`ExportThread`)
- Pass the thread's events from its first event, oldest first. The fold needs every delta of a message.
- On any error from `export_thread`, delete the file: what was written before the failure stays in it.
- `Redactor::new(home, extra)` takes the home directory and any literal secrets the daemon knows.
- `create_export_file` returns `AlreadyExists` for an existing path, `PermissionDenied` for a folder owned by someone else and `InvalidInput` for a path without a file name. The UI needs wording for each.

## Question for the driver
A message that fails verification fails the whole export, as contract §6.1 says ("a hard error at reconstruction, never patched"). The store keeps such a message and only logs it, so one damaged message makes its thread impossible to export. An alternative is to export the thread with that message marked as damaged. That would be a contract change.

## Definition of Done results
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 183 passed, 0 failed, 1 ignored (`consistency_all_fixtures`, owned by M1-07 and M1-08).
- `cargo test -p atrium-core --test history`: 25 passed. `cargo test -p atrium-core --lib`: 9 passed.
- `scripts/check-protocol-ts`: up to date.
- No `unsafe`, and no `unwrap`, `expect` or `panic!` in the module outside tests. The scanners do index into the text; `redaction_never_panics_and_never_lets_a_literal_secret_through` runs 3,000 generated texts through them.
