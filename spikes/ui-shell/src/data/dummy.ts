// Dummy data for the mock window. Deterministic (seeded) so measurements are repeatable.
// Topics, chats and the inbox follow the "B · Floating spaces" prototype (ui-direction §2–§4).

export type ThreadState = "running" | "needs-you" | "failed" | "ready" | "idle";

export interface Thread {
  id: string;
  title: string;
  state: ThreadState;
  agent: string;
  worktree: string;
  age: string;
}
export interface Topic {
  id: string;
  name: string;
  monogram: string;
  threads: Thread[];
}

export const TOPICS: Topic[] = [
  {
    id: "auth", name: "Auth & sessions", monogram: "Au", threads: [
      { id: "t-auth", title: "Rework session token refresh", state: "needs-you", agent: "Claude · Sonnet 5.5", worktree: "atrium · token-refresh", age: "2m" },
      { id: "t-keychain", title: "Keychain fallback on Linux", state: "running", agent: "Codex · GPT-6.1", worktree: "atrium · keychain", age: "now" },
    ],
  },
  {
    id: "daemon", name: "Daemon", monogram: "Da", threads: [
      { id: "t-socket", title: "Socket reconnect backoff", state: "ready", agent: "Claude · Sonnet 5.5", worktree: "atrium · reconnect", age: "18m" },
      { id: "t-perf", title: "Stream perf: batch per frame", state: "needs-you", agent: "Codex · GPT-6.1", worktree: "atrium · stream-batch", age: "6m" },
      { id: "t-wake", title: "Interrupt resumes on wake", state: "idle", agent: "Claude · Sonnet 5.5", worktree: "atrium · wake", age: "2d" },
    ],
  },
  {
    id: "docs", name: "Docs & release", monogram: "Do", threads: [
      { id: "t-lint", title: "Link checker in CI", state: "failed", agent: "lychee", worktree: "atrium · links", age: "1h" },
      { id: "t-readme", title: "Tighten the README", state: "ready", agent: "Claude · Sonnet 5.5", worktree: "atrium · readme", age: "3h" },
    ],
  },
  {
    id: "ideas", name: "Ideas", monogram: "Id", threads: [
      { id: "t-schema", title: "Schema diagram auto-layout", state: "idle", agent: "Gemini · agy", worktree: "atrium · schema", age: "Mon" },
    ],
  },
];

export const ALL_THREADS: Thread[] = TOPICS.flatMap((t) => t.threads);
export const threadById = (id: string): Thread => ALL_THREADS.find((t) => t.id === id) ?? ALL_THREADS[0]!;
export const topicOf = (threadId: string): Topic => TOPICS.find((t) => t.threads.some((x) => x.id === threadId)) ?? TOPICS[0]!;
/** A topic gets the amber dot when one of its chats needs you (ui-direction §4). */
export const topicNeedsYou = (topic: Topic): boolean => topic.threads.some((t) => t.state === "needs-you" || t.state === "failed");
/** Everything in the inbox waits on the human. */
export const needsYouCount = (): number => INBOX.length;

export const STATE_LABEL: Record<ThreadState, string> = {
  running: "Running",
  "needs-you": "Needs you",
  failed: "Failed",
  ready: "Ready for review",
  idle: "Idle",
};

// ---------------------------------------------------------------------------
// Inbox: one ranked list across all topics (PLAN §3.4)

export type InboxGroup = "ask" | "failed" | "ready";
export interface InboxItem {
  id: string;
  group: InboxGroup;
  threadId: string;
  ask: string;
  /** Inline code inside the ask, rendered in mono. */
  code?: string;
  source: string;
  age: string;
  actions: { label: string; primary?: boolean; ghost?: boolean }[];
  summary: string;
}

export const INBOX: InboxItem[] = [
  { id: "i-run", group: "ask", threadId: "t-auth", ask: "Run", code: "cargo test -p atriumd --test socket_reconnect", source: "Rework session token refresh · Auth & sessions · Claude", age: "2m",
    actions: [{ label: "Allow once", primary: true }, { label: "Always in this worktree" }, { label: "Deny", ghost: true }],
    summary: "Claude fixed the cache miss in SessionStore::refresh and wants to run the reconnect integration test to prove the retry. Unit tests and clippy pass." },
  { id: "i-120", group: "ask", threadId: "t-perf", ask: "Measure on the 120 Hz display as well?", source: "Stream perf: batch per frame · Daemon · Codex", age: "6m",
    actions: [{ label: "Yes, both" }, { label: "60 Hz only" }, { label: "Answer in chat", ghost: true }],
    summary: "Codex measured 60 fps with 10 threads on the 60 Hz panel and asks whether to repeat the run on a 120 Hz display." },
  { id: "i-links", group: "failed", threadId: "t-lint", ask: "Link checker: 2 broken links in docs/spikes", source: "Link checker in CI · Docs & release · lychee", age: "1h",
    actions: [{ label: "Ask the agent to fix" }, { label: "Open log", ghost: true }],
    summary: "lychee found two broken links in docs/spikes after the M0-06 screenshots moved. The agent can fix them or add an allowlist." },
  { id: "i-backoff", group: "ready", threadId: "t-socket", ask: "Socket reconnect backoff", source: "3 of 3 checks · cross-review by Sol: approve · 2 files", age: "18m", actions: [],
    summary: "Exponential backoff with jitter for socket reconnects. Three checks passed and Sol approved the cross-review." },
  { id: "i-readme", group: "ready", threadId: "t-readme", ask: "Tighten the README", source: "2 of 2 checks · 1 file", age: "3h", actions: [],
    summary: "Shorter intro and install steps for fish and zsh. Two checks passed." },
];

export const INBOX_GROUPS: { id: InboxGroup; label: string; state: ThreadState }[] = [
  { id: "ask", label: "Approvals and questions", state: "needs-you" },
  { id: "failed", label: "Failed", state: "failed" },
  { id: "ready", label: "Ready for review", state: "ready" },
];

// ---------------------------------------------------------------------------
// Transcript

export type ToolVerb = "Read" | "Search" | "Edit" | "Run";
export type Message =
  | { kind: "user"; id: number; text: string }
  | { kind: "assistant"; id: number; paragraphs: string[] }
  | { kind: "tool"; id: number; verb: ToolVerb; target: string; result: { tone: "ok" | "diff" | "fail" | "plain"; text: string; add?: number; del?: number } }
  | { kind: "approval"; id: number; command: string; reason: string };

/** Mulberry32: tiny seeded PRNG. */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const USER_LINES = [
  "Can you make the refresh path retry once before surfacing the error?",
  "Looks right. Run the session tests again.",
  "Why does the second call skip the cache?",
  "Please keep the public API as it is.",
  "Add a test for the expired-token case.",
  "What changed in the last commit on this branch?",
  "Split that into two functions, the first one is doing too much.",
];
const SENTENCES = [
  "The refresh path currently clones the token before checking its expiry, which is why the second call misses the cache.",
  "I moved the expiry check ahead of the clone and kept the public signature unchanged.",
  "`SessionStore::refresh` now returns early when the token is still valid for more than the skew window.",
  "The retry happens once, with the same backoff the daemon uses for its socket reconnects.",
  "Nothing outside `session.rs` needed to change, but the doc comment on `Skew` was out of date, so I fixed that too.",
  "Two tests cover this: one for an expired token and one for a token that expires during the call.",
  "If you would rather surface the first error immediately, that is a one-line change in `retry_once`.",
  "I did not touch the lockfile; the dependency set is the same as before.",
];
const TOOLS: [ToolVerb, string][] = [
  ["Read", "crates/atrium-core/src/session.rs"],
  ["Search", "refresh_token"],
  ["Edit", "session.rs"],
  ["Run", "cargo test -p atrium-core session"],
  ["Read", "crates/atrium-core/src/skew.rs"],
];

/** The last messages of the long transcript: what the chat card shows on open. */
type Draft = Message extends infer M ? (M extends Message ? Omit<M, "id"> : never) : never;
const TAIL: Draft[] = [
  { kind: "user", text: "The second refresh call misses the cache. Check expiry before cloning, and retry once on a transient error." },
  { kind: "assistant", paragraphs: ["Found it. `SessionStore::refresh` clones the token before checking `expires_at`, so the cached value is never reused. I'll check expiry first and add a single retry with the daemon's backoff."] },
  { kind: "tool", verb: "Read", target: "crates/atrium-core/src/session.rs", result: { tone: "ok", text: "" } },
  { kind: "tool", verb: "Edit", target: "session.rs", result: { tone: "diff", text: "", add: 5, del: 3 } },
  { kind: "tool", verb: "Run", target: "cargo test -p atrium-core session", result: { tone: "ok", text: "14 passed" } },
  { kind: "assistant", paragraphs: ["Unit tests pass. To prove the retry against a real socket I'd like to run the reconnect integration test."] },
  { kind: "approval", command: "cargo test -p atriumd --test socket_reconnect", reason: "Starts a local daemon on a temp socket. No network." },
]; 

function toolResult(verb: ToolVerb, r: () => number): Extract<Message, { kind: "tool" }>["result"] {
  if (r() < 0.06) return { tone: "fail", text: "failed" };
  if (verb === "Edit") return { tone: "diff", text: "", add: 1 + Math.floor(r() * 12), del: Math.floor(r() * 6) };
  if (verb === "Run") return { tone: "ok", text: `${8 + Math.floor(r() * 10)} passed` };
  if (verb === "Search") return { tone: "plain", text: `${1 + Math.floor(r() * 9)} matches` };
  return { tone: "ok", text: "" };
}

export function makeTranscript(count: number, seed = 7, withTail = false): Message[] {
  const r = rng(seed);
  const pick = <T,>(xs: readonly T[]): T => xs[Math.floor(r() * xs.length)] as T;
  const out: Message[] = [];
  const body = withTail ? count - TAIL.length : count;
  for (let i = 0; i < body; i++) {
    const roll = r();
    if (i % 487 === 486) {
      out.push({ kind: "approval", id: i, command: "cargo test -p atrium-core --no-fail-fast", reason: "Runs the test suite in this worktree. No network." });
    } else if (roll < 0.16) {
      out.push({ kind: "user", id: i, text: pick(USER_LINES) });
    } else if (roll < 0.5) {
      const [verb, target] = pick(TOOLS);
      out.push({ kind: "tool", id: i, verb, target, result: toolResult(verb, r) });
    } else {
      const n = 1 + Math.floor(r() * 2);
      out.push({ kind: "assistant", id: i, paragraphs: Array.from({ length: n }, () => `${pick(SENTENCES)} ${r() < 0.5 ? pick(SENTENCES) : ""}`.trim()) });
    }
  }
  if (withTail) TAIL.forEach((m, k) => out.push({ ...m, id: body + k } as Message));
  return out;
}

// ---------------------------------------------------------------------------
// Panels: diff, checks, files, terminal, database

export interface DiffLine { kind: "ctx" | "add" | "del"; no: number; indent: number; code: string }
export const DIFF: { path: string; lines: DiffLine[] } = {
  path: "crates/atrium-core/src/session.rs",
  lines: [
    { kind: "ctx", no: 41, indent: 0, code: "impl SessionStore {" },
    { kind: "ctx", no: 42, indent: 1, code: "pub fn refresh(&mut self, now: Instant) {" },
    { kind: "del", no: 43, indent: 2, code: "let tok = self.current.clone();" },
    { kind: "del", no: 44, indent: 2, code: "if let Some(t) = tok {" },
    { kind: "add", no: 43, indent: 2, code: "if let Some(tok) = &self.current {" },
    { kind: "add", no: 44, indent: 3, code: "if tok.expires_at > now + SKEW {" },
    { kind: "add", no: 45, indent: 4, code: "return Ok(tok);" },
    { kind: "add", no: 46, indent: 3, code: "}" },
    { kind: "ctx", no: 47, indent: 2, code: "}" },
    { kind: "del", no: 48, indent: 2, code: "let fresh = self.fetch()?;" },
    { kind: "add", no: 48, indent: 2, code: "let fresh = retry_once(|| self.fetch(), BACKOFF)?;" },
    { kind: "ctx", no: 49, indent: 2, code: "self.current = Some(fresh);" },
    { kind: "ctx", no: 50, indent: 1, code: "}" },
    { kind: "ctx", no: 51, indent: 0, code: "}" },
  ],
};

export const CHECKS: { state: ThreadState; label: string; mono?: boolean; result: string }[] = [
  { state: "ready", label: "cargo test -p atrium-core", mono: true, result: "14 passed" },
  { state: "ready", label: "cargo clippy -D warnings", mono: true, result: "clean" },
  { state: "needs-you", label: "Evidence for “retry once”", result: "needs your approval" },
];

export interface FileNode { name: string; depth: number; dir?: boolean; open?: boolean; mark?: "M" | "A"; selected?: boolean }
export const FILE_TREE: FileNode[] = [
  { name: "crates", depth: 0, dir: true, open: true },
  { name: "atrium-core", depth: 1, dir: true, open: true },
  { name: "lib.rs", depth: 2 },
  { name: "session.rs", depth: 2, mark: "M", selected: true },
  { name: "retry.rs", depth: 2, mark: "A" },
  { name: "atriumd", depth: 1, dir: true },
  { name: "docs", depth: 0, dir: true },
  { name: "Cargo.toml", depth: 0 },
];

export const TERMINAL_LINES: { text: string; strong?: boolean }[] = [
  { text: "~/atrium/.worktrees/token-refresh $ cargo test -p atrium-core session", strong: true },
  { text: "   Compiling atrium-core v0.1.0" },
  { text: "    Finished test profile in 3.42s" },
  { text: "running 14 tests" },
  { text: "test session::refresh_reuses_valid_token ... ok" },
  { text: "test session::refresh_retries_once ... ok" },
  { text: "test session::expired_token_fetches ... ok" },
  { text: "test result: ok. 14 passed; 0 failed", strong: true },
];

export const DB_ROWS = [
  { id: 1, thread: "t-auth", state: "needs-you", agent: "claude", tokens: 18420 },
  { id: 2, thread: "t-keychain", state: "running", agent: "codex", tokens: 9211 },
  { id: 3, thread: "t-socket", state: "ready", agent: "claude", tokens: 40233 },
  { id: 4, thread: "t-perf", state: "needs-you", agent: "codex", tokens: 1207 },
  { id: 5, thread: "t-lint", state: "failed", agent: "lychee", tokens: 690 },
];
