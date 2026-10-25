// Dummy data for the mock window. Deterministic (seeded) so measurements are repeatable.

export type ThreadState = "running" | "needs-you" | "failed" | "ready" | "idle";

export interface Thread {
  id: string;
  title: string;
  state: ThreadState;
  agent: "claude" | "codex" | "agy" | "acp";
  meta: string;
}
export interface Workspace {
  id: string;
  name: string;
  branch: string;
  threads: Thread[];
}

export const WORKSPACES: Workspace[] = [
  {
    id: "atrium",
    name: "atrium",
    branch: "dev",
    threads: [
      { id: "t-auth", title: "Rework session token refresh", state: "running", agent: "claude", meta: "3m" },
      { id: "t-flaky", title: "Fix flaky replay test", state: "needs-you", agent: "codex", meta: "approve" },
      { id: "t-socket", title: "Daemon socket handshake", state: "ready", agent: "claude", meta: "review" },
      { id: "t-perf", title: "Stream perf: batch per frame", state: "idle", agent: "agy", meta: "2h" },
    ],
  },
  {
    id: "harness",
    name: "harness-docs",
    branch: "main",
    threads: [
      { id: "t-readme", title: "Tighten the README quickstart", state: "ready", agent: "claude", meta: "review" },
      { id: "t-lint", title: "Link checker in CI", state: "failed", agent: "codex", meta: "1 check" },
    ],
  },
  {
    id: "sandbox",
    name: "sandbox-lab",
    branch: "spike/vm",
    threads: [{ id: "t-vm", title: "Boot time under 1 s?", state: "running", agent: "acp", meta: "12m" }],
  },
];

export const ALL_THREADS: Thread[] = WORKSPACES.flatMap((w) => w.threads);
export const threadById = (id: string): Thread => ALL_THREADS.find((t) => t.id === id) ?? ALL_THREADS[0]!;

export interface TabSpec {
  id: string;
  title: string;
  kind: "thread" | "split" | "home";
  /** Threads shown in the main area (`split` shows two side by side). */
  threads: string[];
  file?: string;
}

export const INITIAL_TABS: TabSpec[] = [
  { id: "tab-auth", title: "Rework session token refresh", kind: "thread", threads: ["t-auth"], file: "session.rs" },
  { id: "tab-split", title: "Flaky test + socket", kind: "split", threads: ["t-flaky", "t-socket"] },
  { id: "tab-perf", title: "Stream perf", kind: "thread", threads: ["t-perf"], file: "stream.ts" },
  { id: "tab-home", title: "Home", kind: "home", threads: ["t-auth"], file: "session.rs" },
];

// ---------------------------------------------------------------------------
// Transcript

export type Message =
  | { kind: "user"; id: number; text: string }
  | { kind: "assistant"; id: number; paragraphs: string[] }
  | { kind: "tool"; id: number; tool: string; target: string; lines: string[]; status: "ok" | "error" }
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
const TOOLS: [string, string][] = [
  ["Read", "crates/atrium-core/src/session.rs"],
  ["Grep", "refresh_token"],
  ["Edit", "crates/atrium-core/src/session.rs"],
  ["Bash", "cargo test -p atrium-core session"],
  ["Read", "crates/atrium-core/src/skew.rs"],
];
const TOOL_OUT = [
  "   Compiling atrium-core v0.0.0",
  "    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.82s",
  "     Running unittests src/lib.rs",
  "test session::refresh_returns_early_when_valid ... ok",
  "test session::refresh_retries_once ... ok",
  "test result: ok. 14 passed; 0 failed; 0 ignored",
];

export function makeTranscript(count: number, seed = 7): Message[] {
  const r = rng(seed);
  const pick = <T,>(xs: readonly T[]): T => xs[Math.floor(r() * xs.length)] as T;
  const out: Message[] = [];
  for (let i = 0; i < count; i++) {
    const roll = r();
    if (i % 487 === 486) {
      out.push({ kind: "approval", id: i, command: "cargo test -p atrium-core --no-fail-fast", reason: "Runs the test suite in this worktree" });
    } else if (roll < 0.16) {
      out.push({ kind: "user", id: i, text: pick(USER_LINES) });
    } else if (roll < 0.5) {
      const [tool, target] = pick(TOOLS);
      const n = 1 + Math.floor(r() * 5);
      out.push({ kind: "tool", id: i, tool, target, lines: Array.from({ length: n }, () => pick(TOOL_OUT)), status: r() < 0.06 ? "error" : "ok" });
    } else {
      const n = 1 + Math.floor(r() * 3);
      out.push({ kind: "assistant", id: i, paragraphs: Array.from({ length: n }, () => `${pick(SENTENCES)} ${r() < 0.5 ? pick(SENTENCES) : ""}`.trim()) });
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// File preview, diff, terminal, database

export const FILES: Record<string, { path: string; lines: string[] }> = {
  "session.rs": {
    path: "crates/atrium-core/src/session.rs",
    lines: [
      "use std::time::{Duration, Instant};",
      "",
      "/// How early we refresh before a token actually expires.",
      "const SKEW: Duration = Duration::from_secs(30);",
      "",
      "pub struct Token {",
      "    value: String,",
      "    expires_at: Instant,",
      "}",
      "",
      "impl SessionStore {",
      "    pub fn refresh(&mut self, now: Instant) -> Result<&Token, RefreshError> {",
      "        // Check expiry first: cloning an already-valid token was the bug.",
      "        if let Some(tok) = self.current.as_ref() {",
      "            if tok.expires_at > now + SKEW {",
      "                return Ok(tok);",
      "            }",
      "        }",
      "        let fresh = self.retry_once(|| self.client.exchange(&self.refresh_token))?;",
      "        self.current = Some(fresh);",
      "        Ok(self.current.as_ref().unwrap())",
      "    }",
      "}",
    ],
  },
  "stream.ts": {
    path: "src/stream/batch.ts",
    lines: [
      "// Batch token deltas so the DOM is touched at most once per frame.",
      "export function createFrameBatcher<T>(flush: (items: T[]) => void) {",
      "  let queue: T[] = [];",
      "  let scheduled = 0;",
      "  const run = () => {",
      "    scheduled = 0;",
      "    const items = queue;",
      "    queue = [];",
      "    flush(items);",
      "  };",
      "  return {",
      "    push(item: T) {",
      "      queue.push(item);",
      "      if (!scheduled) scheduled = requestAnimationFrame(run);",
      "    },",
      "  };",
      "}",
    ],
  },
};

export const DIFF_LINES: { kind: "ctx" | "add" | "del" | "hunk"; text: string }[] = [
  { kind: "hunk", text: "@@ crates/atrium-core/src/session.rs  -11,9 +11,13 @@ impl SessionStore" },
  { kind: "ctx", text: "     pub fn refresh(&mut self, now: Instant) -> Result<&Token, RefreshError> {" },
  { kind: "del", text: "-        let tok = self.current.clone();" },
  { kind: "del", text: "-        if tok.map_or(true, |t| t.expires_at <= now) {" },
  { kind: "add", text: "+        // Check expiry first: cloning an already-valid token was the bug." },
  { kind: "add", text: "+        if let Some(tok) = self.current.as_ref() {" },
  { kind: "add", text: "+            if tok.expires_at > now + SKEW {" },
  { kind: "add", text: "+                return Ok(tok);" },
  { kind: "add", text: "+            }" },
  { kind: "add", text: "+        }" },
  { kind: "ctx", text: "         let fresh = self.client.exchange(&self.refresh_token)?;" },
  { kind: "hunk", text: "@@ crates/atrium-core/src/session.rs  -31,6 +35,18 @@ mod tests" },
  { kind: "add", text: "+    #[test]" },
  { kind: "add", text: "+    fn refresh_returns_early_when_valid() {" },
  { kind: "add", text: "+        let mut store = SessionStore::with_token(valid_for(Duration::from_secs(300)));" },
  { kind: "add", text: "+        assert!(store.refresh(Instant::now()).is_ok());" },
  { kind: "add", text: "+        assert_eq!(store.client.calls(), 0);" },
  { kind: "add", text: "+    }" },
];

export const TERMINAL_LINES = [
  "$ cargo test -p atrium-core session",
  "   Compiling atrium-core v0.0.0 (/work/atrium/crates/atrium-core)",
  "    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.82s",
  "     Running unittests src/lib.rs",
  "",
  "running 14 tests",
  "test session::refresh_returns_early_when_valid ... ok",
  "test session::refresh_retries_once ... ok",
  "test session::refresh_surfaces_second_error ... ok",
  "",
  "test result: ok. 14 passed; 0 failed; 0 ignored",
  "$ ",
];

export const DB_ROWS = [
  { id: 1, thread: "t-auth", state: "running", agent: "claude", tokens: 18420 },
  { id: 2, thread: "t-flaky", state: "needs-you", agent: "codex", tokens: 9211 },
  { id: 3, thread: "t-socket", state: "ready", agent: "claude", tokens: 40233 },
  { id: 4, thread: "t-perf", state: "idle", agent: "agy", tokens: 1207 },
  { id: 5, thread: "t-vm", state: "running", agent: "acp", tokens: 6890 },
];
