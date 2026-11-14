// `ATRIUM_BENCH=shots-chat` with `ATRIUM_SHOT_DIR`: the pictures of the chat for reviews
// (M1-09b Definition of Done), run by app/scripts/screenshots.py in a throwaway ATRIUM_HOME.
//
// The synthetic agent streams plain text and can't raise an approval or a question (CR-13), so
// the scenes are SEEDED IN THIS WINDOW: thread summaries, inbox items and thread events are
// applied through the same reducers the bridge feeds (`daemon/client.ts` `apply`), under thread
// ids the daemon doesn't know. Nothing here is the daemon's data, except:
//   - Settings → Agents: the real `ProbeProviders` of this machine;
//   - Settings → Approvals: rules really created with `CreateRule` and read with `ListRules`;
//   - the last picture: a real refusal, the daemon's answer to "Allow" on a request it doesn't have.

import { holdThread } from "./chat/threads";
import { apply, command, daemon } from "./daemon/client";
import type { AgentEvent, EnforcementDescriptor, InboxItem, StoredEvent, ThreadState, ThreadSummary } from "./daemon/types";
import { writeReport } from "./platform";
import { shot, sleep, until } from "./roundtrip";
import { setInboxPick, setPanel, setScreen, setSettingsSection } from "./state";
import { openThread } from "./ui/Sidebar";
import { patchSettings, patchView } from "./windows";

const MINUTE_MS = 60_000;
/** Seeded event ids start far above anything a fresh daemon assigns. */
const FIRST_SEED_ID = 1_000_000;
/** How long the client may take to say it doesn't know a seeded thread. */
const UNKNOWN_THREAD_WAIT_MS = 700;
const STREAM_TICK_MS = 40;
const FENCE = "`".repeat(3);
const lines = (...l: string[]) => l.join(String.fromCharCode(10));

const ENFORCEMENT: EnforcementDescriptor = { level: "full", reasons: [], provider: "claude", cli_version: "2.1.0", version_tested: true, coverage: [], config_sources: [] };

let nextId = FIRST_SEED_ID;

class Seed {
  readonly events: StoredEvent[] = [];
  private seq = 0;
  constructor(readonly id: string, readonly title: string, private at: number) {}

  /** Adds an event `gapMs` after the one before. */
  add(event: AgentEvent, gapMs = 400): StoredEvent {
    this.at += gapMs;
    const stored: StoredEvent = { id: nextId++, thread_id: this.id, envelope: { incarnation: 1, seq: this.seq++, at_ms: this.at, event } };
    this.events.push(stored);
    return stored;
  }

  start(): this {
    this.add({ type: "session_started", provider: "claude", native_session_id: `seeded-${this.id}`, model: "sonnet-5.5", tools: ["Bash", "Read", "Edit"], enforcement: ENFORCEMENT });
    return this;
  }

  user(turn: number, text: string): this {
    this.add({ type: "turn_started", turn });
    this.add({ type: "message_completed", message_id: `u${turn}`, kind: "user", body: { type: "inline", text } });
    return this;
  }

  /** A whole assistant message. */
  says(id: string, text: string): this {
    this.add({ type: "message_delta", message_id: id, kind: "assistant", text });
    this.add({ type: "message_completed", message_id: id, kind: "assistant", body: { type: "inline", text } });
    return this;
  }

  tool(id: string, name: string, kind: "execute" | "read" | "edit", title: string, output: string | null): this {
    this.add({ type: "tool_call_started", call_id: id, name, kind, title, input: kind === "execute" ? { command: title } : { file_path: title } });
    if (output !== null) this.add({ type: "tool_call_finished", call_id: id, status: "succeeded", output, error: null });
    return this;
  }

  summary(state: ThreadState, over: Partial<ThreadSummary> = {}): ThreadSummary {
    const first = this.events[0]!.envelope.at_ms;
    return {
      thread_id: this.id, title: this.title, provider: "claude", model: "sonnet-5.5", location: { type: "home" }, state, enforcement: ENFORCEMENT,
      incarnation: 1, native_session_id: `seeded-${this.id}`, last_event: this.events[this.events.length - 1]!.id, unread: false,
      pending_approvals: 0, pending_questions: 0, attention_since_ms: null, attention_detail: null, created_at_ms: first, updated_at_ms: this.at, ...over,
    };
  }
}

const ANSWER_1 = lines(
  "The nightly build fails in **one test**, `store::recovery::replays_open_items`, and only on the release branch. I'll run it there to see the failure.",
);

const ANSWER_2 = lines(
  "## What fails",
  "",
  "The test opens a store written by the *previous* schema and expects the migration to run first. On the release branch the migration list is one entry short:",
  "",
  `${FENCE}rust`,
  "const MIGRATIONS: &[Migration] = &[",
  "    Migration { version: 7, up: add_sessions },",
  "    // version 8 (requests.event_opened) was never cherry-picked",
  "];",
  FENCE,
  "",
  "## Options",
  "",
  "1. Cherry-pick the migration commit (`4c1e9aa`) onto the release branch.",
  "2. Skip the test there. This hides a real upgrade bug, so I wouldn't.",
  "",
  "| Branch | Migrations | Test |",
  "|---|---:|:---:|",
  "| `dev` | 8 | passes |",
  "| `release/0.4` | 7 | **fails** |",
  "",
  "> The upgrade path is covered in [the store notes](https://example.com/atrium/store#migrations), and the schema file is [here](file:///home/you/atrium/crates/atrium-core/src/store/schema.sql).",
);

const ANSWER_3_WORDS = "I'm applying the first option now: the commit touches only the migration list and its test, so the cherry-pick should be clean. Once it lands I'll run the whole store suite on the release branch and report what changed, including the time the migration takes on a store with a million events.".split(" ");

const QUESTION = "The refresh-rate setting can apply to every display or only to 60 Hz ones. Which do you want?";
const FAILURE = "The agent stopped: its sign-in expired. Sign in again with the agent's own command, then resume.";

function seeds(now: number) {
  const stream = new Seed("seed-stream", "Why does the nightly build fail?", now - 6 * MINUTE_MS).start()
    .user(1, "Why does the nightly build fail on the release branch?")
    .says("a1", ANSWER_1)
    .tool("c1", "Bash", "execute", "cargo test -p atrium-core store::recovery", lines("running 1 test", "test store::recovery::replays_open_items ... FAILED", "", "failures:", "    store::recovery::replays_open_items: no such column: event_opened", "", "test result: FAILED. 0 passed; 1 failed"))
    .tool("c2", "Read", "read", "crates/atrium-core/src/store/migrations.rs", "38 lines")
    .says("a2", ANSWER_2)
    .tool("c3", "Bash", "execute", "git cherry-pick 4c1e9aa", "[release/0.4 9be02d1] store: migration 8 (requests.event_opened)");
  stream.add({ type: "context_updated", used_tokens: 48_200, window_tokens: 200_000 });
  stream.add({ type: "usage", usage: { input_tokens: 46_900, output_tokens: 1_310, cache_read_tokens: 39_000, cache_write_tokens: null, reasoning_tokens: null, cost_usd: null } });

  const approval = new Seed("seed-approval", "Clean the build folder and rebuild", now - 4 * MINUTE_MS).start()
    .user(1, "Clean the build folder and rebuild")
    .says("a1", "I'll remove the old build output first, then rebuild from scratch.")
    .tool("c1", "Bash", "execute", "rm -rf build", null);
  approval.add({ type: "approval_requested", request_id: "q1", call_id: "c1", name: "Bash", kind: "execute", title: "rm -rf build", input: { command: "rm -rf build" }, reason: "Home asks before anything is deleted or changed.", rememberable: true });

  const question = new Seed("seed-question", "Add a refresh-rate setting", now - 9 * MINUTE_MS).start()
    .user(1, "Add a refresh-rate setting to the display preferences")
    .says("a1", "Before I write it, one thing decides the shape of the setting.");
  question.add({ type: "question_asked", request_id: "q1", questions: [{ id: "scope", prompt: QUESTION, options: [{ id: "both", label: "Yes, both", description: "Every connected display" }, { id: "sixty", label: "60 Hz only", description: null }], multi_select: false, allow_text: true }] });

  const failed = new Seed("seed-failed", "Summarise this week's notes", now - 26 * MINUTE_MS).start()
    .user(1, "Summarise this week's notes in ~/Documents/notes");
  failed.add({ type: "error", message: FAILURE, fatal: true, details: ["stderr: error: authentication required", "stderr: run the agent's login command and try again"] });
  failed.add({ type: "session_ended", exit_code: 1, reason: null });

  const reply = new Seed("seed-reply", "Which folders take the most space?", now - 48 * MINUTE_MS).start()
    .user(1, "Which folders take the most space?")
    .tool("c1", "Bash", "execute", "du -sh ~/* | sort -h | tail -5", lines("4.1G  Downloads", "9.8G  Pictures", "21G   Videos"))
    .says("a1", lines("Three folders hold most of it:", "", "- **Videos**: 21 GB", "- **Pictures**: 9.8 GB", "- **Downloads**: 4.1 GB"));
  reply.add({ type: "turn_completed", turn: 1, outcome: "completed", usage: { input_tokens: 2_100, output_tokens: 96, cache_read_tokens: null, cache_write_tokens: null, reasoning_tokens: null, cost_usd: null } });

  const summaries: ThreadSummary[] = [
    stream.summary({ type: "running" }),
    approval.summary({ type: "needs_approval" }, { pending_approvals: 1, attention_since_ms: now - 3 * MINUTE_MS, attention_detail: "rm -rf build" }),
    question.summary({ type: "needs_input" }, { pending_questions: 1, attention_since_ms: now - 8 * MINUTE_MS, attention_detail: QUESTION }),
    failed.summary({ type: "failed", message: FAILURE }, { attention_since_ms: now - 25 * MINUTE_MS, attention_detail: FAILURE }),
    reply.summary({ type: "idle" }, { unread: true, attention_since_ms: now - 47 * MINUTE_MS }),
  ];
  const inbox: InboxItem[] = [
    { thread_id: approval.id, reason: "approval", since_ms: now - 3 * MINUTE_MS, title: approval.title, detail: "rm -rf build" },
    { thread_id: question.id, reason: "question", since_ms: now - 8 * MINUTE_MS, title: question.title, detail: QUESTION },
    { thread_id: failed.id, reason: "failed", since_ms: now - 25 * MINUTE_MS, title: failed.title, detail: FAILURE },
    { thread_id: reply.id, reason: "reply", since_ms: now - 47 * MINUTE_MS, title: reply.title, detail: null },
  ];
  return { all: [stream, approval, question, failed, reply], stream, approval, question, summaries, inbox };
}

const click = (selector: string): boolean => {
  const el = document.querySelector<HTMLElement>(selector);
  el?.click();
  return el !== null;
};

async function both(name: string): Promise<void> {
  await patchSettings({ theme: "light" });
  await shot(`${name}-light`);
  await patchSettings({ theme: "dark" });
  await shot(`${name}-dark`);
}

export async function runChatShots(): Promise<void> {
  await until("the daemon", () => daemon().connection.type === "connected" && daemon().loaded);
  const notes: Record<string, unknown> = {};

  // Real rules, for Settings → Approvals.
  await command({ type: "create_rule", rule: { scope: { type: "home" }, tool_kind: "execute", tool_name: null, matcher: { type: "command_exact", argv: ["cargo", "test", "--workspace"] }, path_scope: null, decision: "allow" } });
  await command({ type: "create_rule", rule: { scope: { type: "global" }, tool_kind: "edit", tool_name: "Edit", matcher: { type: "path_glob", glob: "/home/you/notes/todo.md" }, path_scope: null, decision: "allow" } });
  await command({ type: "create_rule", rule: { scope: { type: "global" }, tool_kind: "execute", tool_name: null, matcher: { type: "command_exact", argv: ["git", "push", "--force"] }, path_scope: null, decision: "deny" } });

  const seeded = seeds(Date.now());
  for (const summary of seeded.summaries) apply({ type: "thread_updated", summary });
  // This window holds every seeded thread for the whole run, so each keeps its model.
  for (const seed of seeded.all) holdThread(seed.id);
  await sleep(UNKNOWN_THREAD_WAIT_MS);
  for (const seed of seeded.all) apply({ type: "snapshot", thread_id: seed.id, events: seed.events, has_older: false, truncated: false, last: seed.events[seed.events.length - 1]!.id });
  apply({ type: "inbox_updated", items: seeded.inbox });

  // 1. A streaming chat with markdown and tool calls: the last message grows while it is pictured.
  openThread(seeded.stream.id);
  let word = 0;
  const streaming = setInterval(() => {
    const text = `${ANSWER_3_WORDS[word % ANSWER_3_WORDS.length]} `;
    word++;
    apply({ type: "event", event: seeded.stream.add({ type: "message_delta", message_id: "a3", kind: "assistant", text }, STREAM_TICK_MS) });
  }, STREAM_TICK_MS);
  await sleep((STREAM_TICK_MS * ANSWER_3_WORDS.length) / 2);
  await both("chat-streaming");
  clearInterval(streaming);
  notes.streamingRows = document.querySelectorAll(".thread-row").length;
  notes.streamingOpenBlock = document.querySelector(".md.streaming") !== null;

  // 2. An approval card (above the composer), and its "Always…" choices.
  openThread(seeded.approval.id);
  await patchView(seeded.approval.id, { draft: "" });
  await both("chat-approval");
  notes.alwaysMenu = click('[data-atrium-part="approval"] button[aria-haspopup="menu"]');
  await shot("chat-approval-always-dark");
  await patchSettings({ theme: "light" });
  await shot("chat-approval-always-light");
  click('[data-atrium-part="approval"] button[aria-haspopup="menu"]');

  // 3. A question form, with one option chosen.
  openThread(seeded.question.id);
  await sleep(STREAM_TICK_MS);
  notes.optionPicked = click('[data-atrium-part="question"] .q-opt');
  await both("chat-question");

  // 4. The inbox with items, the first one picked (its preview is in the panel).
  setPanel({ open: true });
  setScreen("inbox");
  setInboxPick(seeded.approval.id);
  await both("inbox-items");
  notes.inboxItems = document.querySelectorAll('[data-atrium-part="inbox-item"]').length;
  notes.inboxCards = document.querySelectorAll('[data-atrium-part="inbox"] .ask').length;

  // 5. Settings: Agents (this machine's real probes), Approvals (real rules), Notifications.
  setScreen("settings");
  setSettingsSection("agents");
  await until("the probes", () => document.querySelectorAll('[data-atrium-part="agents"] .s-item').length > 0 || document.querySelector('[data-atrium-part="agents"] .s-empty')?.textContent?.startsWith("Atrium found") === true);
  await both("settings-agents");
  setSettingsSection("approvals");
  await until("the rules", () => document.querySelectorAll('[data-atrium-part="rules"] .s-item').length === 3);
  await both("settings-approvals");
  setSettingsSection("notifications");
  await both("settings-notifications");

  // 6. A real refusal: "Allow once" on a request the daemon doesn't have. The card says what the
  //    daemon answered and nothing is allowed.
  openThread(seeded.approval.id);
  await sleep(STREAM_TICK_MS);
  notes.allowClicked = click('[data-atrium-part="approval"] .btn.pri');
  await until("the daemon's answer", () => {
    const state = document.querySelector<HTMLElement>('[data-atrium-part="approval"]')?.dataset.state;
    return state !== undefined && state !== "fresh" && state !== "sending";
  });
  notes.refusedState = document.querySelector<HTMLElement>('[data-atrium-part="approval"]')?.dataset.state;
  notes.refusedText = document.querySelector('[data-atrium-part="approval"] .ask-state')?.textContent;
  await both("chat-approval-refused");

  await sleep(STREAM_TICK_MS);
  await writeReport({ kind: "shots", pass: true, notes });
}
