// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createComponent, type JSX } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../platform", () => ({
  safetyInfo: async () => ({
    hardDeny: [
      { id: "recursive-delete-outside-cwd", description: "Recursive delete outside the chat's folder" },
      { id: "credential-read", description: "Credential stores and keychains" },
    ],
    home: "/Users/me",
  }),
}));

import SecurityPage, { ASK_STORES, UNCHECKED_ASSUMPTIONS } from "./Security";

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
  await new Promise((r) => setTimeout(r, 0));
}

describe("Settings → Security (contract rev 4 §2, §9, §11 item 3; CR-12, CR-17)", () => {
  let dispose: (() => void) | null = null;

  afterEach(() => {
    dispose?.();
    dispose = null;
    document.body.innerHTML = "";
  });

  it("lists every CR-12 and CR-17 store in ASK_STORES", () => {
    const joined = ASK_STORES.map((s) => s.paths).join(" ");
    for (const store of [
      "~/.cargo/credentials.toml",
      "~/.cargo/credentials",
      "~/.pypirc",
      "~/.pgpass",
      "~/.vault-token",
      "~/.azure/accessTokens.json",
      "~/.azure/msal_token_cache.*",
      "~/.aws/sso/cache/",
      "~/.config/git/credentials",
      "~/.env",
      "~/.gem/credentials",
      "~/.terraform.d/credentials.tfrc.json",
      "~/.config/op/config",
      "~/.yarnrc.yml",
      "~/.bash_history",
      "~/.zsh_history",
      "~/.zhistory",
      "~/.sh_history",
      "~/.ksh_history",
      "~/.history",
      "~/.local/share/fish/fish_history",
      "~/.config/fish/fish_history",
      "~/.zsh_sessions/",
      "~/.bash_sessions/",
    ]) {
      expect(joined, store).toContain(store);
    }
  });

  it("matches every ASK_FILES, ASK_TREES, and ASK_NAME_PREFIXES entry in core v4/paths/sensitive.rs", () => {
    const sensitiveRs = readFileSync(
      join(import.meta.dirname, "../../../crates/atrium-core/src/policy/v4/paths/sensitive.rs"),
      "utf8",
    );
    const extractFirstTupleStrings = (constName: string): string[] => {
      const m = new RegExp(`const ${constName}:[^=]*=\\s*&\\[([\\s\\S]*?)\\];`).exec(sensitiveRs);
      if (!m) throw new Error(`${constName} not found in v4/paths/sensitive.rs`);
      return Array.from(m[1]!.matchAll(/\(\s*"([^"]+)",\s*"[^"]+",?\s*\)/g), (hit) => hit[1]!);
    };
    const joinedLower = ASK_STORES.map((s) => s.paths).join(" ").toLowerCase();
    const askFiles = extractFirstTupleStrings("ASK_FILES");
    expect(askFiles.length).toBeGreaterThanOrEqual(20);
    for (const rel of askFiles) {
      expect(joinedLower, `ASK_FILES: ~/${rel}`).toContain(`~/${rel.toLowerCase()}`);
    }
    const askTrees = extractFirstTupleStrings("ASK_TREES");
    expect(askTrees.length).toBeGreaterThanOrEqual(3);
    for (const tree of askTrees) {
      expect(joinedLower, `ASK_TREES: ~/${tree}/`).toContain(`~/${tree.toLowerCase()}/`);
    }
    const prefixBlock = /const ASK_NAME_PREFIXES:[^=]*=\s*&\[([\s\S]*?)\];/.exec(sensitiveRs);
    if (!prefixBlock) throw new Error("ASK_NAME_PREFIXES not found in v4/paths/sensitive.rs");
    const prefixes = Array.from(prefixBlock[1]!.matchAll(/\(\s*"([^"]+)",\s*"([^"]+)",\s*"[^"]+",?\s*\)/g));
    expect(prefixes.length).toBeGreaterThanOrEqual(1);
    for (const triple of prefixes) {
      const entry = `~/${triple[1]}/${triple[2]}*`;
      expect(joinedLower, `ASK_NAME_PREFIXES: ${entry}`).toContain(entry.toLowerCase());
    }
  });

  it("renders the hard-deny list from safety_info, §9 scope text, §2 assumptions, stores, and known limits", async () => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    type SecurityRow = { id: string; label: string };
    const rows: SecurityRow[] = [
      { id: "always-blocked", label: "Always blocked (for calls Atrium sees)" },
      { id: "known-limits", label: "Known limits" },
    ];
    dispose = render(
      () =>
        createComponent<{ rows: readonly SecurityRow[]; row: (row: SecurityRow) => JSX.Element }>(
          SecurityPage,
          {
            rows,
            row: (r: SecurityRow) => {
              const el = document.createElement("div");
              el.textContent = r.label;
              return el;
            },
          },
        ),
      host,
    );
    await flush();

    // Hard-deny items rendered from safety_info() data.
    const blockedItems = Array.from(host.querySelectorAll<HTMLElement>('[data-atrium-part="always-blocked"] .s-item'));
    expect(blockedItems.map((el) => el.dataset.id)).toEqual(["recursive-delete-outside-cwd", "credential-read"]);

    // §9 hard-deny scope text under the hard-deny list.
    const scopeNote = host.querySelector('[data-atrium-part="hard-deny-scope"]');
    expect(scopeNote?.textContent).toContain("Always blocked for commands Atrium can read in full.");
    expect(scopeNote?.textContent).toContain(
      "Commands built at run time (variables, loops, $(…), inline scripts) are checked on a best-effort basis under FullAccess and Any rules.",
    );

    // §2 assumptions under "What Atrium can't check".
    const cantCheckSection = host.querySelector('[data-atrium-part="cant-check-section"]');
    expect(cantCheckSection?.textContent).toContain("What Atrium can't check");
    const assumptionItems = Array.from(host.querySelectorAll<HTMLElement>('[data-atrium-part="cant-check"] .s-item'));
    expect(assumptionItems).toHaveLength(UNCHECKED_ASSUMPTIONS.length);
    expect(assumptionItems.map((el) => el.dataset.id)).toEqual([
      "command-names",
      "user-config",
      "repo-contents",
      "other-processes",
      "hard-links",
      "inherited-fds",
    ]);
    const cantCheckText = cantCheckSection?.textContent ?? "";
    expect(cantCheckText).toContain("usual system tool");
    expect(cantCheckText).toContain("another program with that name");
    expect(cantCheckText).toContain("shell configuration");
    expect(cantCheckText).toContain("repository configuration");
    expect(cantCheckText).toContain("commits and diffs");
    expect(cantCheckText).toContain("another process");
    expect(cantCheckText).toContain("limited number of files and folder levels");
    expect(cantCheckText).toContain("parent program");

    // Plain words only: internal jargon must not appear in user-facing copy.
    const pageText = host.textContent ?? "";
    for (const jargon of [/\bunread\b/i, /\bexhausted\b/i, /\bwalk\b/i, /\bcanonical\b/i, /\bPATH\b/, /\bfd\b/i, /\bidentity\b/i]) {
      expect(pageText, `jargon ${jargon}`).not.toMatch(jargon);
    }

    // CR-12 and CR-17 "Credential stores that ask" section.
    const askList = host.querySelector('[data-atrium-part="ask-stores"]');
    expect(askList).not.toBeNull();
    const askText = askList?.textContent ?? "";
    expect(askText).toContain("~/.gem/credentials");
    expect(askText).toContain("~/.terraform.d/credentials.tfrc.json");
    expect(askText).toContain("~/.config/op/config");
    expect(askText).toContain("~/.yarnrc.yml");
    expect(askText).toContain("~/.aws/sso/cache/");
    expect(askText).toContain("~/.azure/msal_token_cache.*");

    // Existing Known limits section stays mounted.
    const limitIds = Array.from(host.querySelectorAll<HTMLElement>('[data-atrium-part="known-limits"] .s-item')).map(
      (el) => el.dataset.id,
    );
    expect(limitIds).toEqual(["git-config", "same-user", "no-isolation"]);
  });
});
