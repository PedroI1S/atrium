# Review: M1-11-export, round 4
Reviewer: Gemini 3.8 Flash (read-only) · 2026-11-10

## Verdict
**approve**, the round-3 major findings are cleanly resolved, header string escaping is robust across edge cases, and all history tests pass.

## Findings
None.

### Verification notes

1. **Round-3 majors**:
   - Escaped inner quotes in header values (`header_value_end`):
     - `{"Authorization": "Digest username=\"u\", response=\"0a1b\""}` -> `{"Authorization": "[redacted]"}`
     - `curl -H "Authorization: Digest username=\"u\", response=\"0a1b\"" x` -> `curl -H "Authorization: [redacted]" x`
     - `-H "X-Api-Key: a\"b" -v` -> `-H "X-Api-Key: [redacted]" -v`
   - RFC 6265 cookie names (`is_cookie_name`):
     - `Cookie: $Version=1; session=secret123` -> `Cookie: [redacted]`
     - `cookie: auth+session=secret123` -> `cookie: [redacted]`

2. **Edge cases in `header_value_end`**:
   - **Backslash as last byte** (`"Authorization: Bearer abc\`): `end.min(bytes.len())` bounds `end` to text length without slicing past buffer ends; no panic or leak.
   - **Backslash before multi-byte character** (`"Authorization: Basic abc\🦀"`): UTF-8 continuation bytes (`0x80..=0xBF`) never match delimiters (`\n`, `\r`, `\\`, or ASCII quote closers), so the loop advances to complete codepoints and character boundaries.
   - **Backslash before newline**: Inside a quoted string, `\` escapes `\n` across continuation lines; for unquoted headers (`closer == None`), line ending at `\n` is respected.
   - **No closing quote at all**: Safely runs to EOF / line boundary without panicking.
   - **Linear runtime**: Remains linear O(N) on 1 MB inputs; `BareRuns` prevents quadratic re-measurement.

3. **No false positives on ordinary code**:
   - `cookie: &str` -> `"let cookie: &str = \"foo\";"` (preserved; no `=`).
   - `cookie: *ptr = x` -> `"cookie: *ptr = x;"` (preserved; space before `=`).
   - `cookie: !done` -> `"cookie: !done"` (preserved; no `=`).

4. **Promised forms in doc comment**:
   - `--proxy-user pedro:hunter2` -> `curl --proxy-user pedro:[redacted] x`
   - `--proxy-user=pedro:hunter2` -> `curl --proxy-user=pedro:[redacted] x`
   - `--proxy-user 'pedro:hunter2'` -> `curl --proxy-user 'pedro:[redacted]' x`
   - `-u 'pedro:hunter2'` -> `curl -u 'pedro:[redacted]' x`
