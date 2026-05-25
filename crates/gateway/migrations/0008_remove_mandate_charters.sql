DROP INDEX IF EXISTS idx_team_charters_user_active;
DROP INDEX IF EXISTS idx_charters_active;
DROP TABLE IF EXISTS team_charters;

CREATE TABLE decisions_without_mandate (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    deliverable_id TEXT NOT NULL,
    task_id TEXT,
    session_id TEXT NOT NULL,
    ticker TEXT NOT NULL,
    direction TEXT NOT NULL,
    size_shares REAL,
    size_pct REAL,
    stop_loss REAL,
    target REAL,
    horizon_days INTEGER,
    rationale_md TEXT NOT NULL,
    corpus_refs_json TEXT,
    review_schedule_json TEXT,
    review_done_dates_json TEXT,
    status TEXT NOT NULL DEFAULT 'open',
    superseded_by TEXT,
    confirmed_at TEXT NOT NULL,
    closed_at TEXT,
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, deliverable_id) REFERENCES deliverables(user_id, id)
);

INSERT INTO decisions_without_mandate (
    user_id,
    id,
    deliverable_id,
    task_id,
    session_id,
    ticker,
    direction,
    size_shares,
    size_pct,
    stop_loss,
    target,
    horizon_days,
    rationale_md,
    corpus_refs_json,
    review_schedule_json,
    review_done_dates_json,
    status,
    superseded_by,
    confirmed_at,
    closed_at
)
SELECT
    user_id,
    id,
    deliverable_id,
    task_id,
    session_id,
    ticker,
    direction,
    size_shares,
    size_pct,
    stop_loss,
    target,
    horizon_days,
    rationale_md,
    corpus_refs_json,
    review_schedule_json,
    review_done_dates_json,
    status,
    superseded_by,
    confirmed_at,
    closed_at
FROM decisions;

DROP TABLE decisions;
ALTER TABLE decisions_without_mandate RENAME TO decisions;

CREATE INDEX idx_decisions_ticker ON decisions(user_id, ticker, confirmed_at DESC);
CREATE INDEX idx_decisions_status ON decisions(user_id, status, confirmed_at DESC);
CREATE INDEX idx_decisions_review ON decisions(user_id, status) WHERE status = 'open';
