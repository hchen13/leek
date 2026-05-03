-- Allow deliverables without a task (standalone decisions via tool)
CREATE TABLE deliverables_new (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    task_id TEXT,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    rejection_reason TEXT,
    created_at TEXT NOT NULL,
    ready_at TEXT,
    confirmed_at TEXT,
    rejected_at TEXT,
    PRIMARY KEY (user_id, id)
);

INSERT INTO deliverables_new SELECT * FROM deliverables;
DROP TABLE deliverables;
ALTER TABLE deliverables_new RENAME TO deliverables;

CREATE INDEX IF NOT EXISTS idx_deliverables_task
    ON deliverables(user_id, task_id) WHERE task_id IS NOT NULL;

CREATE INDEX idx_deliverables_status
    ON deliverables(user_id, status, created_at DESC);

-- Allow decisions without a task
CREATE TABLE decisions_new (
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
    mandate_check_json TEXT,
    review_schedule_json TEXT,
    review_done_dates_json TEXT,
    status TEXT NOT NULL DEFAULT 'open',
    superseded_by TEXT,
    confirmed_at TEXT NOT NULL,
    closed_at TEXT,
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, deliverable_id) REFERENCES deliverables(user_id, id)
);

INSERT INTO decisions_new SELECT * FROM decisions;
DROP TABLE decisions;
ALTER TABLE decisions_new RENAME TO decisions;

CREATE INDEX idx_decisions_ticker ON decisions(user_id, ticker, confirmed_at DESC);
CREATE INDEX idx_decisions_status ON decisions(user_id, status, confirmed_at DESC);
CREATE INDEX idx_decisions_review ON decisions(user_id, status) WHERE status = 'open';
