// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { createComponent, type JSX } from "solid-js";
import { render } from "solid-js/web";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../platform", () => ({
  safetyInfo: async () => ({
    hardDeny: [{ id: "credential-read", description: "Credential stores and keychains" }],
    home: "/Users/me",
  }),
}));

import SecurityPage, { ASK_STORES } from "./Security";

describe("Settings → Security credential stores that ask (CR-12, CR-17)", () => {
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
      "~/.local/share/fish/fish_history",
    ]) {
      expect(joined, store).toContain(store);
    }
  });

  it("matches every ASK_FILES, ASK_TREES, and ASK_NAME_PREFIXES entry in core deny.rs", () => {
    const denyRs = readFileSync(join(import.meta.dirname, "../../../crates/atrium-core/src/policy/deny.rs"), "utf8");
    const extractStrings = (constName: string): string[] => {
      const m = new RegExp(`const ${constName}:[^=]*=\\s*&\\[([\\s\\S]*?)\\];`).exec(denyRs);
      if (!m) throw new Error(`${constName} not found in deny.rs`);
      return Array.from(m[1]!.matchAll(/"([^"]+)"/g), (hit) => hit[1]!);
    };
    const joined = ASK_STORES.map((s) => s.paths).join(" ");
    for (const rel of extractStrings("ASK_FILES")) {
      expect(joined, `ASK_FILES: ~/${rel}`).toContain(`~/${rel}`);
    }
    for (const tree of extractStrings("ASK_TREES")) {
      expect(joined, `ASK_TREES: ~/${tree}/`).toContain(`~/${tree}/`);
    }
    const prefixBlock = /const ASK_NAME_PREFIXES:[^=]*=\s*&\[([\s\S]*?)\];/.exec(denyRs);
    if (!prefixBlock) throw new Error("ASK_NAME_PREFIXES not found in deny.rs");
    for (const pair of prefixBlock[1]!.matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)) {
      const entry = `~/${pair[1]}/${pair[2]}*`;
      expect(joined, `ASK_NAME_PREFIXES: ${entry}`).toContain(entry);
    }
  });

  it("renders the ask-stores list inside SecurityPage", async () => {
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
    await Promise.resolve();
    const askList = host.querySelector('[data-atrium-part="ask-stores"]');
    expect(askList).not.toBeNull();
    const text = askList?.textContent ?? "";
    expect(text).toContain("~/.gem/credentials");
    expect(text).toContain("~/.terraform.d/credentials.tfrc.json");
    expect(text).toContain("~/.config/op/config");
    expect(text).toContain("~/.yarnrc.yml");
  });
});
