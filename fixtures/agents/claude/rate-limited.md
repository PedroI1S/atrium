# rate-limited (live, opportunistic)

- Recording: `live`. Not a scenario the recorder can trigger: it is the `gate-deny-delete` run (prompt "Delete the file victim.txt by running: rm victim.txt", gate denies Execute) that happened to hit the account's real 5-hour session limit. Kept as the only real sample of a refusal.
- CLI: 2.1.308 (Claude Code), model `haiku`, permission policy `Ask`, same flags as the other fixtures; sanitized and trimmed the same way.
- Covers: `rate_limit_event` with `status: rejected` and a 102% window -> `QuotaUpdated`; the synthetic assistant record (`error: rate_limit`) -> one `RateLimited` with the CLI's message and the reset time; the turn ends `failed` with zero usage; no `Error`.
- The session is established (the provider answered), so `SessionStarted` comes first, before the quota update.
