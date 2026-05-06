-- Align compaction persistence with the in-place UX: one session can be
-- compacted many times, and each compaction gets its own immutable row.

CREATE TABLE session_compactions_new (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    original_session_id TEXT NOT NULL,
    compacted_session_id TEXT NOT NULL,
    summary_md TEXT NOT NULL,
    messages_retained INTEGER NOT NULL,
    messages_removed INTEGER NOT NULL,
    tokens_before INTEGER,
    tokens_after INTEGER,
    trigger TEXT NOT NULL,
    focus TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, original_session_id) REFERENCES sessions(user_id, id),
    FOREIGN KEY (user_id, compacted_session_id) REFERENCES sessions(user_id, id)
);

INSERT INTO session_compactions_new (
    user_id, id, original_session_id, compacted_session_id,
    summary_md, messages_retained, messages_removed,
    tokens_before, tokens_after, trigger, focus, created_at
)
SELECT
    user_id,
    lower(hex(randomblob(16))),
    original_session_id,
    compacted_session_id,
    summary_md,
    messages_retained,
    messages_removed,
    tokens_before,
    tokens_after,
    trigger,
    focus,
    created_at
FROM session_compactions;

DROP TABLE session_compactions;

ALTER TABLE session_compactions_new RENAME TO session_compactions;

CREATE INDEX idx_compactions_original
    ON session_compactions(user_id, original_session_id, created_at DESC);

CREATE INDEX idx_compactions_compacted
    ON session_compactions(user_id, compacted_session_id, created_at DESC);
