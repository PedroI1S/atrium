// "Add a rule" in Settings → Approvals (PLAN §3.12, contract §9, CR-06): a command prefix, an
// exact command, a path glob, or every call of a kind; allow or deny; in every chat, Home, or
// one chat. Checked as typed (chat/rule-form.ts), then sent as one `CreateRule`.

import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import { ANY_KINDS, type Field, type MatcherKind, PATH_KINDS, type RuleForm, emptyRuleForm, pathScopeOffered, validateRule } from "../chat/rule-form";
import { ruleText } from "../chat/rules";
import { command, daemon } from "../daemon/client";
import { type Rule, type ToolKind, isCommandFailure } from "../daemon/types";
import { setRuleFormOpen, showToast } from "../state";
import { Seg, Select } from "./controls";
import { homeDir, loadSafety } from "./Security";

const MATCHERS: { value: MatcherKind; label: string }[] = [
  { value: "command_prefix", label: "Commands that start with" },
  { value: "command_exact", label: "This exact command" },
  { value: "path_glob", label: "Paths matching a glob" },
  { value: "any", label: "Every call of one kind" },
];

const KIND_LABEL: Record<ToolKind, string> = {
  read: "File reads", edit: "File edits", delete: "Deletions", move: "Moves and renames", search: "Searches",
  execute: "Commands", fetch: "Web requests", mcp: "MCP tool calls", think: "Thinking steps", other: "Other tool calls",
};
const kinds = (list: readonly ToolKind[]) => list.map((k) => ({ value: k, label: KIND_LABEL[k] }));

const titleOf = (id: string) => daemon().threads[id]?.title;

function TextField(props: { label: string; hint?: string; value: string; placeholder: string; error?: string; part: string; onInput: (v: string) => void }) {
  const id = `rule-${props.part}`;
  return (
    <label class="fld" for={id}>
      <b>{props.label}</b>
      <input type="text" id={id} data-atrium-part={`rule-${props.part}`} spellcheck={false} autocomplete="off" value={props.value} placeholder={props.placeholder}
        aria-invalid={props.error ? "true" : undefined} aria-describedby={props.error ? `${id}-err` : undefined} onInput={(e) => props.onInput(e.currentTarget.value)} />
      <Show when={props.error} fallback={<Show when={props.hint}><span class="hint">{props.hint}</span></Show>}>
        <span class="err" id={`${id}-err`}>{props.error}</span>
      </Show>
    </label>
  );
}

export default function RuleEditor(props: { onDone: () => void }) {
  loadSafety();
  onCleanup(() => setRuleFormOpen(false));
  const [form, setForm] = createSignal<RuleForm>(emptyRuleForm());
  const [tried, setTried] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [failure, setFailure] = createSignal<string | null>(null);
  const set = (patch: Partial<RuleForm>) => { setForm({ ...form(), ...patch }); setFailure(null); };
  const result = createMemo(() => validateRule(form(), { home: homeDir(), threadExists: (id) => id in daemon().threads }));
  /** A field's error once the user has typed in it, or tried to add the rule. */
  const error = (f: Field): string | undefined => {
    const r = result();
    if (r.ok) return undefined;
    const typed = f === "command" ? form().command : f === "glob" ? form().glob : f === "toolName" ? form().toolName : f === "pathScope" ? form().pathScope : "";
    return tried() || typed.trim() !== "" ? r.errors[f] : undefined;
  };
  const preview = () => {
    const r = result();
    if (!r.ok) return null;
    const rule: Rule = { ...r.draft, id: "preview", origin: { type: "settings" }, created_at_ms: Date.now() };
    const t = ruleText(rule, titleOf);
    return `${t.decision} ${t.what}${t.code ? ` ${t.code}` : ""} · ${t.where}`;
  };
  const threads = () => daemon().order.map((id) => ({ value: id, label: titleOf(id) || "Untitled chat" }));
  const isCommand = () => form().matcher === "command_prefix" || form().matcher === "command_exact";

  const submit = async (e: Event) => {
    e.preventDefault();
    setTried(true);
    const r = result();
    if (!r.ok || saving()) return;
    setSaving(true);
    try {
      await command({ type: "create_rule", rule: r.draft });
      showToast("Rule added");
      props.onDone();
    } catch (err) {
      setFailure(isCommandFailure(err) ? (err.kind === "unknown_outcome" ? `${err.message}. It may have been added: check the list before trying again.` : err.message) : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <form class="rule-form" aria-label="Add a rule" data-atrium-part="rule-editor" onSubmit={submit} onKeyDown={(e) => { if (e.key === "Escape") { e.stopPropagation(); props.onDone(); } }}>
      <div class="fld">
        <b>Decision</b>
        <div class="fld-row"><Seg label="Decision" value={form().decision} options={[{ value: "allow", label: "Allow" }, { value: "deny", label: "Deny" }]} onChange={(v) => set({ decision: v })} /></div>
      </div>
      <div class="fld">
        <b>Matches</b>
        <div class="fld-row">
          <Select label="What the rule matches" value={form().matcher} options={MATCHERS} onChange={(v) => set({ matcher: v })} />
          <Show when={form().matcher === "path_glob"}>
            <Select label="Kind of call" value={(PATH_KINDS as readonly string[]).includes(form().toolKind) ? form().toolKind : "edit"} options={kinds(PATH_KINDS)} onChange={(v) => set({ toolKind: v })} />
          </Show>
          <Show when={form().matcher === "any"}>
            <Select label="Kind of call" value={(ANY_KINDS as readonly string[]).includes(form().toolKind) ? form().toolKind : "other"} options={kinds(ANY_KINDS)} onChange={(v) => set({ toolKind: v })} />
          </Show>
        </div>
      </div>
      <Show when={isCommand()}>
        <TextField label="Command" part="command" placeholder={form().matcher === "command_prefix" ? "cargo test" : "rm -rf build"} value={form().command} error={error("command")}
          hint={form().matcher === "command_prefix" ? "Matches a simple command whose words start with these, in order. Quote a word with spaces. Shells, wrappers like env or sudo, and inline code can't be allowed; for python3, node and the like, put the script right after the command." : "Matches only this command, word for word."}
          onInput={(v) => set({ command: v })} />
      </Show>
      <Show when={form().matcher === "path_glob"}>
        <TextField label="Path or glob" part="glob" placeholder="~/projects/notes/**" value={form().glob} error={error("glob")} hint="A full path; * matches within a folder, ** across folders." onInput={(v) => set({ glob: v })} />
      </Show>
      <TextField label="Only for the tool named (optional)" part="tool" placeholder="Bash" value={form().toolName} error={error("toolName")} hint="The agent's own name for the tool. Empty: any tool of this kind." onInput={(v) => set({ toolName: v })} />
      {/* Only where the engine can match it: allow rules about file calls (rule-form.ts). */}
      <Show when={pathScopeOffered(form())}>
        <TextField label="Only for paths inside (optional)" part="path-scope" placeholder="~/projects/**" value={form().pathScope} error={error("pathScope")} hint="Every path the call touches must match." onInput={(v) => set({ pathScope: v })} />
      </Show>
      <div class="fld">
        <b>Applies in</b>
        <div class="fld-row">
          <Seg label="Applies in" value={form().scope} options={[{ value: "global", label: "Every chat" }, { value: "home", label: "Home chats" }, { value: "thread", label: "One chat" }]}
            onChange={(v) => set({ scope: v, threadId: v === "thread" ? form().threadId ?? daemon().order[0] ?? null : form().threadId })} />
          <Show when={form().scope === "thread"}>
            <Show when={threads().length} fallback={<span class="err">There is no chat yet</span>}>
              <Select label="Chat" value={form().threadId ?? ""} options={threads()} onChange={(v) => set({ threadId: v })} />
            </Show>
          </Show>
        </div>
        <Show when={error("thread")}><span class="err">{error("thread")}</span></Show>
      </div>
      <Show when={preview()}>
        {(p) => <p class="preview" data-atrium-part="rule-preview">{p()}</p>}
      </Show>
      <Show when={result().ok && (result() as { warnings: string[] }).warnings.length}>
        <For each={(result() as { warnings: string[] }).warnings}>{(w) => <p class="note error" role="note">{w}</p>}</For>
      </Show>
      <Show when={failure()}><p class="note error" role="alert">Couldn't add the rule: {failure()}</p></Show>
      <div class="row-actions">
        <button type="submit" class="btn pri sm" data-atrium-part="rule-add" disabled={saving()}>{form().decision === "allow" ? "Add allow rule" : "Add deny rule"}</button>
        <button type="button" class="btn ghost sm" onClick={() => props.onDone()}>Cancel</button>
      </div>
    </form>
  );
}
