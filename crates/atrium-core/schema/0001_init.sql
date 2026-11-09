-- Atrium store, migration 0001 (contract §7). Contract: changes go through a
-- change request. Times are Unix milliseconds. JSON columns hold
-- atrium-protocol types, serialized with serde_json.

PRAGMA foreign_keys = ON;

CREATE TABLE schema_migrations (
    version     INTEGER PRIMARY KEY,
    applied_ms  INTEGER NOT NULL
);

-- One row per thread: the projection the inbox and lists read.
CREATE TABLE threads (
    thread_id          TEXT PRIMARY KEY,
    title              TEXT NOT NULL,
    provider           TEXT NOT NULL,           -- Provider
    model              TEXT,
    effort             TEXT,
    preset             TEXT NOT NULL,           -- Preset
    location           TEXT NOT NULL,           -- Location (JSON)
    state              TEXT NOT NULL,           -- ThreadState (JSON)
    incarnation        INTEGER NOT NULL DEFAULT 0,
    last_event_id      INTEGER,
    unread             INTEGER NOT NULL DEFAULT 0,
    pending_approvals  INTEGER NOT NULL DEFAULT 0,
    pending_questions  INTEGER NOT NULL DEFAULT 0,
    attention_since_ms INTEGER,
    attention_detail   TEXT,
    archived           INTEGER NOT NULL DEFAULT 0,
    created_ms         INTEGER NOT NULL,
    updated_ms         INTEGER NOT NULL
);

-- One row per incarnation (each start or resume of a provider process).
CREATE TABLE sessions (
    thread_id          TEXT NOT NULL REFERENCES threads(thread_id),
    incarnation        INTEGER NOT NULL,
    native_session_id  TEXT,                    -- from SessionStarted
    enforcement        TEXT,                    -- EnforcementDescriptor (JSON), latest
    process            TEXT,                    -- ProcessIdentity (JSON) while running
    started_ms         INTEGER NOT NULL,
    ended_ms           INTEGER,
    exit_code          INTEGER,
    cleanup            TEXT,                    -- Cleanup, after recovery
    PRIMARY KEY (thread_id, incarnation)
);

-- Append-only. `event_id` is the global EventId.
CREATE TABLE events (
    event_id     INTEGER PRIMARY KEY,
    thread_id    TEXT NOT NULL REFERENCES threads(thread_id),
    incarnation  INTEGER NOT NULL,
    seq          INTEGER NOT NULL,
    at_ms        INTEGER NOT NULL,
    kind         TEXT NOT NULL,                 -- AgentEvent "type"
    payload      TEXT NOT NULL,                 -- AgentEventEnvelope (JSON), ≤ MAX_EVENT_BYTES
    UNIQUE (thread_id, incarnation, seq)
);
CREATE INDEX events_by_thread ON events (thread_id, event_id);

CREATE TABLE turns (
    thread_id     TEXT NOT NULL,
    incarnation   INTEGER NOT NULL,
    turn          INTEGER NOT NULL,
    started_ms    INTEGER NOT NULL,
    completed_ms  INTEGER,
    outcome       TEXT,                         -- TurnOutcome
    usage         TEXT,                         -- Usage (JSON), final
    PRIMARY KEY (thread_id, incarnation, turn)
);

-- Open messages and tool calls: what closing needs, so it never folds the
-- whole history (gate review, L8). Rows are deleted when they complete.
CREATE TABLE open_messages (
    thread_id    TEXT NOT NULL,
    incarnation  INTEGER NOT NULL,
    message_id   TEXT NOT NULL,
    kind         TEXT NOT NULL,                 -- MessageKind
    chars        INTEGER NOT NULL DEFAULT 0,
    deltas       INTEGER NOT NULL DEFAULT 0,
    -- The running SHA-256 lives in the writer task's memory; after a crash
    -- it is recomputed from this message's stored deltas.
    PRIMARY KEY (thread_id, incarnation, message_id)
);

CREATE TABLE open_tool_calls (
    thread_id    TEXT NOT NULL,
    incarnation  INTEGER NOT NULL,
    call_id      TEXT NOT NULL,
    name         TEXT NOT NULL,
    PRIMARY KEY (thread_id, incarnation, call_id)
);

-- The approval ledger's persisted state (contract §5). `status` is written
-- only by the ledger (through RequestStore), never by event appends.
CREATE TABLE requests (
    thread_id     TEXT NOT NULL,
    incarnation   INTEGER NOT NULL,
    request_id    TEXT NOT NULL,
    kind          TEXT NOT NULL,                -- 'approval' | 'question'
    turn          INTEGER,
    tool_name     TEXT,
    tool_kind     TEXT,
    input_sha256  TEXT,
    context       TEXT,                         -- PolicyContext (JSON)
    title         TEXT,
    remember_matcher TEXT,                      -- RuleMatcher (JSON); NULL: not rememberable
    status        TEXT NOT NULL,                -- 'pending' | 'approved' | 'redeemed' | 'denied' | 'answered' | 'cancelled' | 'expired'
    opened_ms     INTEGER NOT NULL,
    resolved_ms   INTEGER,
    -- Set by the store when the ApprovalRequested/QuestionAsked and the
    -- RequestResolved events are stored; the ledger never touches them.
    -- Open for closing (OpenItems): event_opened = 1 AND event_closed = 0.
    event_opened  INTEGER NOT NULL DEFAULT 0,
    event_closed  INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (thread_id, incarnation, request_id)
);
CREATE INDEX requests_pending ON requests (status) WHERE status = 'pending';

CREATE TABLE rules (
    rule_id     TEXT PRIMARY KEY,
    rule        TEXT NOT NULL,                  -- Rule (JSON)
    created_ms  INTEGER NOT NULL
);

-- Single row. Bumped on every rule change; part of PolicyContext.
CREATE TABLE rules_revision (
    id        INTEGER PRIMARY KEY CHECK (id = 1),
    revision  INTEGER NOT NULL
);
INSERT INTO rules_revision (id, revision) VALUES (1, 0);

CREATE TABLE settings (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL                        -- JSON
);

CREATE TABLE snapshots (
    path        TEXT PRIMARY KEY,
    taken_ms    INTEGER NOT NULL,
    bytes       INTEGER NOT NULL
);
