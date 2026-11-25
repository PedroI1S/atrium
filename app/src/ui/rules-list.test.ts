// @vitest-environment happy-dom

// Rendered Settings rules-list test (Finding 9, §11 item 5, D-153): verifies that valid rules
// render normally and stored invalid rules (e.g. Allow prefix `["awk"]`) render as invalid with
// their reason (covering nothing), warning indicator, and a working Delete button.

import { createComponent } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ListedRule } from "../daemon/types";
import { RulesListView } from "./RulesList";

const listedRule = (over: Partial<ListedRule>): ListedRule => ({
  id: "r1",
  scope: { type: "global" },
  tool_kind: "execute",
  tool_name: null,
  matcher: { type: "command_prefix", argv: ["cargo", "test"] },
  path_scope: null,
  decision: "allow",
  origin: { type: "settings" },
  created_at_ms: 1700000000000,
  invalid_reason: null,
  ...over,
});

describe("Settings rules list rendering (§11 item 5, D-153)", () => {
  let host: HTMLElement;
  let dispose: (() => void) | undefined;

  beforeEach(() => {
    host = document.createElement("div");
    document.body.appendChild(host);
  });

  afterEach(() => {
    dispose?.();
    dispose = undefined;
    host.remove();
  });

  it("renders stored invalid rules as invalid with their reason and non-ready indicator, while keeping valid rules normal", () => {
    const onDelete = vi.fn<(rule: ListedRule) => void>();
    const validAllow = listedRule({
      id: "r_valid_cargo",
      matcher: { type: "command_prefix", argv: ["cargo", "test"] },
    });
    const validDeno = listedRule({
      id: "r_valid_deno",
      matcher: { type: "command_prefix", argv: ["deno"] },
    });
    const validSedZ = listedRule({
      id: "r_valid_sed_z",
      matcher: { type: "command_prefix", argv: ["sed", "z"] },
    });
    const validDeny = listedRule({
      id: "r_valid_deny",
      decision: "deny",
      matcher: { type: "command_prefix", argv: ["awk"] },
    });
    const invalidFromDaemon = listedRule({
      id: "r_invalid_awk",
      matcher: { type: "command_prefix", argv: ["awk"] },
      invalid_reason: "An Allow prefix rule can't end at `awk` without pinning a program file.",
    });
    const invalidLocalFallback = listedRule({
      id: "r_invalid_git",
      matcher: { type: "command_prefix", argv: ["git"] },
      invalid_reason: null,
    });

    dispose = render(
      () =>
        createComponent(RulesListView, {
          rules: [validAllow, validDeno, validSedZ, validDeny, invalidFromDaemon, invalidLocalFallback],
          loading: false,
          deleting: false,
          titleOf: () => undefined,
          onDelete,
        }),
      host,
    );

    const items = Array.from(host.querySelectorAll('[data-atrium-part="rules"] .s-item')) as HTMLElement[];
    expect(items).toHaveLength(6);

    // 1–3. Valid Allow rules (including D-141 unknown tool `deno` and safe `sed z`): ready dot, ordinary label, no invalid reason.
    for (const idx of [0, 1, 2]) {
      const el = items[idx]!;
      expect(el.getAttribute("data-invalid")).toBeNull();
      expect(el.querySelector(".dot")?.getAttribute("data-state")).toBe("ready");
      expect(el.querySelector("b")?.textContent).toBe("Allow commands that start with");
      expect(el.querySelector('[data-atrium-part="rule-invalid-reason"]')).toBeNull();
    }

    // 4. Valid Deny rule: failed dot, ordinary Deny label, no invalid reason.
    const denyEl = items[3]!;
    expect(denyEl.getAttribute("data-invalid")).toBeNull();
    expect(denyEl.querySelector(".dot")?.getAttribute("data-state")).toBe("failed");
    expect(denyEl.querySelector("b")?.textContent).toBe("Deny commands that start with");
    expect(denyEl.querySelector('[data-atrium-part="rule-invalid-reason"]')).toBeNull();

    // 5. Stored invalid Allow prefix `["awk"]` with daemon `invalid_reason`:
    //    marked invalid, non-ready ("needs-you") indicator, shows reason and "covers nothing".
    const awkEl = items[4]!;
    expect(awkEl.getAttribute("data-invalid")).toBe("true");
    expect(awkEl.querySelector(".dot")?.getAttribute("data-state")).toBe("needs-you");
    expect(awkEl.querySelector("b")?.textContent).toContain("Invalid");
    const awkReason = awkEl.querySelector('[data-atrium-part="rule-invalid-reason"]')?.textContent ?? "";
    expect(awkReason).toContain("covers nothing");
    expect(awkReason).toContain("can't end at `awk`");

    // 6. Stored invalid Allow prefix `["git"]` caught even without daemon `invalid_reason`.
    const gitEl = items[5]!;
    expect(gitEl.getAttribute("data-invalid")).toBe("true");
    expect(gitEl.querySelector(".dot")?.getAttribute("data-state")).toBe("needs-you");
    const gitReason = gitEl.querySelector('[data-atrium-part="rule-invalid-reason"]')?.textContent ?? "";
    expect(gitReason).toContain("covers nothing");
    expect(gitReason).toContain("git");

    // Stored invalid rule stays deletable.
    const deleteBtn = awkEl.querySelector("button") as HTMLButtonElement;
    expect(deleteBtn.disabled).toBe(false);
    deleteBtn.click();
    expect(onDelete).toHaveBeenCalledTimes(1);
    expect(onDelete).toHaveBeenCalledWith(invalidFromDaemon);
  });
});
