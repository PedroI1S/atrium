# Review: M1-33-nonpolicy-fixes
Reviewer: GPT-6.1 Sol (Codex, read-only) · 2026-11-22 · round 4

## Verdict

**rework**

Both round-3 findings are **closed**. One new regression remains. No dependency on omitted policy work was found.

## Findings

1. **Minor — `app/src/chat/markdown.ts:312`: short table rows lose trailing empty cells.**  
   For `a|b|c\n-|-|-\n1|2\n3|4|5`, the first body row now renders only two `<td>` elements. The unchanged renderer omits the third column’s cell border, and `blocksText` drops its trailing tab. Confirmed with an in-memory DOM probe. **Fix:** preserve trailing empty-cell semantics using a compact representation and corresponding rendering/copy handling, while retaining linear resource bounds.

## Test gaps

- Add a short-row regression covering rendered trailing cells and copied text alongside the wide-table scaling test.
- Supplied integrated verification was considered; suites were not rerun.

No files were modified.