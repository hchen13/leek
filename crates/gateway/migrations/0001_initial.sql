-- L.E.E.K vault — M0 clean-room schema.
--
-- M0 proves the HTTP + SQLite + SSE plumbing. Four tables, nothing about
-- agents / LLM / corpus / tools. Later milestones add tables one at a time,
-- each justified in its own migration header (see docs/ARCHITECTURE.md §6).

CREATE TABLE users (
    id          TEXT PRIMARY KEY,
    created_at  TEXT NOT NULL
);

CREATE TABLE sessions (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id),
    title           TEXT,
    created_at      TEXT NOT NULL,
    last_active_at  TEXT NOT NULL
);

CREATE INDEX idx_sessions_user ON sessions (user_id, last_active_at DESC);

-- Per-session conversation log. `seq` is per-session monotonic (1, 2, 3 …).
CREATE TABLE messages (
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    role        TEXT NOT NULL,            -- 'user' | 'assistant'
    content     TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
);

-- Per-session event log — the durable record behind the SSE stream. `seq` is
-- per-session monotonic and independent from `messages.seq`.
CREATE TABLE events (
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL,
    kind        TEXT NOT NULL,            -- 'message_created' | 'assistant_delta' | 'assistant_done'
    payload     TEXT NOT NULL,            -- JSON object
    created_at  TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
);
