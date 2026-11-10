# Review: M1-11-export, round 2
Reviewer: Gemini 3.8 Flash (read-only) · 2026-11-10
## Verdict
**rework**: while the recursive JSON key redaction and INI/YAML random-value rules are sound, the new `Cookie` header scanner blanks ordinary code and type annotations across session transcripts, and `-u`/`--user` leaks credentials whenever the flag argument is quoted.

## Findings

1. [major] **`Cookie` header scanner blanks ordinary code and type annotations in transcripts**
   - Where: `crates/atrium-core/src/history/redact.rs:644-668` (`scan_authorization`).
   - Problem: `scan_authorization` treats `b"cookie"` identically to `b"authorization"`, searching for case-insensitive substrings without word-boundary checks (`!is_word`), without line-start constraints, and accepting both `:` and `=`. In coding session transcripts, any variable name, function parameter, struct field, or assignment involving `cookie` is treated as an HTTP header, and `header_value_end` blanks the rest of the line up to a newline or quote.
     - Function parameter: `Redactor::text("fn parse(cookie: &str) -> bool")` returns `"fn parse(cookie: [redacted]"`.
     - Struct field: `Redactor::text("struct Req { cookie: String, path: String }")` returns `"struct Req { cookie: [redacted]"`.
     - Condition / comparison: `Redactor::text("if (cookie == null) { cookie = getCookie(); }")` returns `"if (cookie =[redacted]"`.
     - Identifier substring: `Redactor::text("my_cookie: String")` returns `"my_cookie: [redacted]"`.
     - Type annotation: `Redactor::text("cookie: str = request.cookies.get('session')")` returns `"cookie: [redacted]'session')"`.
     - Variable assignment: `Redactor::text("cookie = jar.get()")` returns `"cookie = [redacted]"`.
   - Fix: Cookie headers must not be matched via loose substring search. Require word boundaries before `cookie` (rejecting identifiers like `my_cookie` and allowing only `-` for `Set-Cookie:`), require `:` (never `=`), and restrict bare `Cookie:` / `Set-Cookie:` to line starts or explicit HTTP header flag contexts (`-H "Cookie: ..."`).

2. [major] **Password in quoted `-u` and `--user` curl flags is leaked unredacted**
   - Where: `crates/atrium-core/src/history/redact.rs:892-898` (`scan_flags`).
   - Problem: `scan_flags` handles `-u` and `--user` by measuring the value with `bare_value_len(&bytes[value..])`. When an argument is quoted (standard practice in shell scripts and commands for credentials with special characters), `bytes[value]` is a quote (`"` or `'`). `bare_value_len` stops at the quote, returning length 0. `end` equals `value`, no colon is searched, and the password is left completely unredacted.
     - Double quotes: `Redactor::text(r#"curl -u "pedro:hunter2" https://x"#)` returns `r#"curl -u "pedro:hunter2" https://x"#`.
     - Single quotes: `Redactor::text("curl -u 'pedro:hunter2' https://x")` returns `"curl -u 'pedro:hunter2' https://x"`.
     - Long flag: `Redactor::text(r#"curl --user "pedro:hunter2" https://x"#)` returns `r#"curl --user "pedro:hunter2" https://x"#`.
   - Fix: In `scan_flags`, check if `bytes.get(value)` is a quote. If quoted, find the closing quote via `closing_quote(bytes, value + 1, quote)`, and search for the colon separator within `value + 1..end` to emit the password span.

3. [minor] **New test additions mirror the implementation rather than testing boundary conditions**
   - Where: `crates/atrium-core/tests/history.rs:417-443, 467-474`.
   - Problem: The round-2 tests mirror the author's code paths without testing common real-world variations:
     - `-u` and `--user` tests only check unquoted values (`curl -u pedro:hunter2`), missing quoted shell arguments and concealing Finding 2.
     - The negative test for `cookie` in `ordinary_text_and_code_are_left_alone` only tests `"the cookies are in the jar, and a cookie-cutter"` (prose without colons or equals signs), omitting code patterns like `cookie: &str` or `cookie = get()`, which would have surfaced Finding 1.
   - Fix: Add test cases for quoted `-u`/`--user` arguments (`"user:pass"`, `'user:pass'`) and code containing `cookie: <type>`, `cookie = <expr>`, and `my_cookie: <type>`.
