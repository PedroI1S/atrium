# Review: M1-11-export, round 3
Reviewer: Gemini 3.8 Flash (read-only) · 2026-11-10
## Verdict
**rework**, credentials leak in `Digest` authorization headers inside JSON or double-quoted curl commands, and cookies with standard RFC 2109 `$Version=1` prefixes are missed.

## Findings
1. [major] **`header_value_end` truncates at inner escaped quotes, leaking `Digest` credentials inside JSON and double-quoted strings**
   - **Where:** `crates/atrium-core/src/history/redact.rs:905-915` (and called from `scan_authorization`, line 678)
   - **Problem:** When an `Authorization` header is enclosed in double quotes (e.g., inside a JSON document `{"Authorization": "Digest username=\"u\", response=\"0a1b\""}` or a double-quoted curl flag `curl -H "Authorization: Digest username=\"u\", response=\"0a1b\""`), `closer` is set to `Some(b'"')`. `header_value_end` uses `take_while(|c| !matches!(c, b'\n' | b'\r') && Some(**c) != closer)`. Because it does not recognize backslash escapes (`\"`), it treats the first inner quote following `username=` as the closing delimiter of the header value. This leaves the remainder of the header—including actual credentials like `response="..."`—unredacted.
     - Input:
       ```json
       {"Authorization": "Digest username=\"u\", response=\"0a1b\""}
       ```
     - Output:
       ```json
       {"Authorization": "[redacted]"u\", response=\"0a1b\""}
       ```
     - Input:
       ```sh
       curl -H "Authorization: Digest username=\"u\", response=\"0a1b\""
       ```
     - Output:
       ```sh
       curl -H "Authorization: [redacted]"u\", response=\"0a1b\""
       ```
     Similarly, a cookie with quoted values inside JSON leaks:
     - Input:
       ```json
       {"Cookie": "a=\"secret123\""}
       ```
     - Output:
       ```json
       {"Cookie": "[redacted]"secret123\""}
       ```
   - **Fix:** In `header_value_end`, when `closer` is present, handle backslash escaping (similar to `closing_quote`'s `b'\\' => i += 1`) so that escaped quotes `\"` do not terminate the header value prematurely.

2. [major] **`cookie_end` only recognizes `is_key` names, leaking cookies prefixed with RFC 2109 `$Version=1` or containing RFC-valid characters like `+`**
   - **Where:** `crates/atrium-core/src/history/redact.rs:707-720`
   - **Problem:** `cookie_end` extracts the first cookie name using `take_while(|c| is_key(**c))` where `is_key` only allows `[a-zA-Z0-9_.-]`. RFC 2109 / RFC 2965 cookies widely used across Java / Tomcat servlet containers start with `$Version=1` (e.g. `Cookie: $Version=1; Skin=new; session=secret123`). Because `$` is not in `is_key`, `name` is evaluated as 0, failing `(1..=MAX_NAME).contains(&name)` and causing `cookie_end` to return `None`. The entire cookie header and all its secret values leak without redaction. Likewise, RFC 6265 token characters like `+` in a cookie name cause the check to fail.
     - Input:
       ```http
       Cookie: $Version=1; session=secret123
       ```
     - Output:
       ```http
       Cookie: $Version=1; session=secret123
       ```
     - Input:
       ```http
       Cookie: auth+session=secret123
       ```
     - Output:
       ```http
       Cookie: auth+session=secret123
       ```
   - **Fix:** In `cookie_end`, permit `$` (and RFC 6265 token characters) in cookie name matching, or recognize the `$Version` attribute prefix specifically.
