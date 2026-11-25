// The remembered rules list in Settings → Approvals (contract §5, §7, §11 item 5, D-153).
// Renders valid rules normally and marks stored invalid rules as invalid with their reason
// (covering nothing) while keeping them deletable.

import { For, Show } from "solid-js";
import { ruleText } from "../chat/rules";
import type { ListedRule } from "../daemon/types";
import { Dot } from "./controls";

export interface RulesListProps {
  rules: readonly ListedRule[];
  loading?: boolean;
  deleting?: boolean;
  titleOf: (threadId: string) => string | undefined;
  onDelete: (rule: ListedRule) => void;
}

export function RulesListView(props: RulesListProps) {
  return (
    <div class="s-list" data-atrium-part="rules">
      <For
        each={props.rules}
        fallback={
          <p class="s-empty">
            {props.loading
              ? ""
              : "Nothing is remembered. Atrium asks each time, unless you choose “Always…” on an approval."}
          </p>
        }
      >
        {(rule) => {
          const text = ruleText(rule, props.titleOf);
          const invalid = text.invalidReason !== null;
          return (
            <div class="s-item" data-invalid={invalid ? "true" : undefined}>
              <Dot
                tone={invalid ? "needs-you" : rule.decision === "allow" ? "ready" : "failed"}
                label={invalid ? "Invalid rule" : undefined}
              />
              <div class="s-main">
                <b>
                  {invalid
                    ? `Invalid — ${text.decision} ${text.what}`
                    : `${text.decision} ${text.what}`}
                </b>
                <Show when={text.code}><span class="code">{text.code}</span></Show>
                <Show when={text.invalidReason}>
                  {(reason) => (
                    <span class="err" data-atrium-part="rule-invalid-reason">
                      Invalid — covers nothing: {reason()}
                    </span>
                  )}
                </Show>
                <span>{text.where}</span>
                <span>{text.origin}</span>
              </div>
              <button
                type="button"
                class="btn sm"
                disabled={props.deleting}
                aria-label={`Delete the ${invalid ? "invalid " : ""}rule: ${text.decision} ${text.what} ${text.code ?? ""}`}
                onClick={() => props.onDelete(rule)}
              >
                Delete
              </button>
            </div>
          );
        }}
      </For>
    </div>
  );
}
