-- Compaction: a session can be forked into a "compacted" child session whose
-- head is a summary system message and whose tail copies the most recent
-- messages. The original (parent) session is preserved untouched so the user
-- can always navigate back. See design/p1-spec/agent-loop.md §4.3.

ALTER TABLE sessions ADD COLUMN parent_session_id TEXT;
ALTER TABLE sessions ADD COLUMN compact_reason TEXT;  -- 'manual' | 'auto_pre_turn'

CREATE INDEX idx_sessions_parent
    ON sessions(user_id, parent_session_id)
    WHERE parent_session_id IS NOT NULL;

CREATE TABLE session_compactions (
    user_id TEXT NOT NULL,
    original_session_id TEXT NOT NULL,
    compacted_session_id TEXT NOT NULL,
    summary_md TEXT NOT NULL,
    messages_retained INTEGER NOT NULL,
    messages_removed INTEGER NOT NULL,
    tokens_before INTEGER,
    tokens_after INTEGER,
    trigger TEXT NOT NULL,                       -- 'manual' | 'auto_pre_turn'
    focus TEXT,                                   -- optional user focus topic
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, compacted_session_id),
    FOREIGN KEY (user_id, original_session_id) REFERENCES sessions(user_id, id),
    FOREIGN KEY (user_id, compacted_session_id) REFERENCES sessions(user_id, id)
);

CREATE INDEX idx_compactions_original
    ON session_compactions(user_id, original_session_id, created_at DESC);
