CREATE TABLE agent_plan_items (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    item_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    step TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'in_progress', 'completed')),
    evidence TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, session_id, task_id, item_id)
);

CREATE INDEX idx_agent_plan_scope
    ON agent_plan_items(user_id, session_id, task_id, seq);

CREATE INDEX idx_agent_plan_status
    ON agent_plan_items(user_id, session_id, task_id, status, seq);
