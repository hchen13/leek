-- L.E.E.K vault initial schema.
-- See design/p1-spec/data-schema.md §2 for the full reference.

-- ============================================================
-- Identity & configuration
-- ============================================================

CREATE TABLE users (
    user_id TEXT PRIMARY KEY,
    display_name TEXT,
    timezone TEXT NOT NULL DEFAULT 'UTC',
    locale TEXT NOT NULL DEFAULT 'zh-CN',
    created_at TEXT NOT NULL,
    last_active_at TEXT
);

CREATE TABLE team_charters (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL DEFAULT 'default',
    charter_json TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    version INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_charters_active ON team_charters(user_id, active);

CREATE TABLE llm_provider_configs (
    user_id TEXT NOT NULL,
    provider_name TEXT NOT NULL,
    auth_kind TEXT NOT NULL,
    api_key_encrypted TEXT,
    oauth_access_token TEXT,
    oauth_refresh_token TEXT,
    oauth_expires_at TEXT,
    oauth_scope TEXT,
    default_model TEXT,
    model_aliases_json TEXT,
    priority INTEGER NOT NULL DEFAULT 0,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_used_at TEXT,
    last_error TEXT,
    last_error_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, provider_name)
);

-- ============================================================
-- Interaction core
-- ============================================================

CREATE TABLE sessions (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    title TEXT,
    status TEXT NOT NULL DEFAULT 'active',
    pinned INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    last_active_at TEXT NOT NULL,
    archived_at TEXT,
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_sessions_active ON sessions(user_id, status, last_active_at DESC);

CREATE TABLE tasks (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL,
    goal TEXT NOT NULL,
    constraints_json TEXT,
    expected_deliverable TEXT NOT NULL,
    priority TEXT NOT NULL DEFAULT 'normal',
    context_refs_json TEXT,
    source TEXT NOT NULL DEFAULT 'user',
    parent_task_id TEXT,
    status TEXT NOT NULL DEFAULT 'draft',
    status_reason TEXT,
    created_at TEXT NOT NULL,
    queued_at TEXT,
    started_at TEXT,
    delivered_at TEXT,
    closed_at TEXT,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    PRIMARY KEY (user_id, id),
    FOREIGN KEY (user_id, session_id) REFERENCES sessions(user_id, id)
);

CREATE INDEX idx_tasks_session ON tasks(user_id, session_id, created_at DESC);
CREATE INDEX idx_tasks_status ON tasks(user_id, status, created_at DESC);
CREATE INDEX idx_tasks_priority ON tasks(user_id, status, priority, created_at);

CREATE TABLE messages (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    task_id TEXT,
    role TEXT NOT NULL,
    content_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, session_id, seq)
);

CREATE INDEX idx_messages_task ON messages(user_id, task_id, seq) WHERE task_id IS NOT NULL;

CREATE TABLE events (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    task_id TEXT,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    source TEXT,
    ts TEXT NOT NULL,
    persisted_at TEXT NOT NULL,
    PRIMARY KEY (user_id, session_id, seq)
);

CREATE INDEX idx_events_task ON events(user_id, task_id, seq) WHERE task_id IS NOT NULL;
CREATE INDEX idx_events_kind ON events(user_id, session_id, kind, seq);

CREATE TABLE artifacts (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    parent_artifact_id TEXT,
    pinned INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_artifacts_session ON artifacts(user_id, session_id, created_at DESC);
CREATE INDEX idx_artifacts_task ON artifacts(user_id, task_id, created_at DESC) WHERE task_id IS NOT NULL;
CREATE INDEX idx_artifacts_kind ON artifacts(user_id, kind, updated_at DESC);

CREATE TABLE panels_layouts (
    user_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    layout_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, session_id)
);

-- ============================================================
-- Task execution
-- ============================================================

CREATE TABLE task_assignments (
    user_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    assignee TEXT NOT NULL,
    assigned_by TEXT NOT NULL,
    reason TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, task_id, seq)
);

CREATE TABLE subagent_runs (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    parent_run_id TEXT,
    spec_name TEXT NOT NULL,
    scope_json TEXT NOT NULL,
    input_json TEXT NOT NULL,
    output_json TEXT,
    success INTEGER,
    error TEXT,
    tokens_used INTEGER NOT NULL DEFAULT 0,
    turns INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER,
    spawned_at TEXT NOT NULL,
    completed_at TEXT,
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_subagent_task ON subagent_runs(user_id, task_id, spawned_at DESC);

CREATE TABLE tool_call_runs (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    task_id TEXT,
    invoker TEXT NOT NULL,
    tool_name TEXT NOT NULL,
    arguments_json TEXT NOT NULL,
    result_json TEXT,
    success INTEGER,
    error TEXT,
    duration_ms INTEGER,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_tool_runs_task ON tool_call_runs(user_id, task_id, started_at DESC);
CREATE INDEX idx_tool_runs_tool ON tool_call_runs(user_id, tool_name, started_at DESC);

CREATE TABLE reasoning_dag_traces (
    user_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    nodes_json TEXT NOT NULL,
    edges_json TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, task_id)
);

-- ============================================================
-- Deliverables & derived
-- ============================================================

CREATE TABLE deliverables (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    task_id TEXT NOT NULL,
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

CREATE INDEX idx_deliverables_task ON deliverables(user_id, task_id);

CREATE TABLE decisions (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    deliverable_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
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

CREATE INDEX idx_decisions_ticker ON decisions(user_id, ticker, confirmed_at DESC);
CREATE INDEX idx_decisions_status ON decisions(user_id, status, confirmed_at DESC);
CREATE INDEX idx_decisions_review ON decisions(user_id, status) WHERE status = 'open';

CREATE TABLE reviews (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    deliverable_id TEXT,
    decision_id TEXT,
    period_start TEXT,
    period_end TEXT,
    summary_md TEXT NOT NULL,
    self_score INTEGER,
    agent_score INTEGER,
    lessons_md TEXT,
    corpus_inbox_candidates_json TEXT,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_reviews_decision ON reviews(user_id, decision_id) WHERE decision_id IS NOT NULL;

-- ============================================================
-- Assets & monitoring
-- ============================================================

CREATE TABLE holdings (
    user_id TEXT NOT NULL,
    snapshot_at TEXT NOT NULL,
    ticker TEXT NOT NULL,
    qty REAL NOT NULL,
    avg_cost REAL,
    account TEXT NOT NULL DEFAULT 'main',
    notes TEXT,
    source TEXT NOT NULL,
    PRIMARY KEY (user_id, snapshot_at, ticker, account)
);

CREATE INDEX idx_holdings_user_recent ON holdings(user_id, snapshot_at DESC);

CREATE TABLE watchlists (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    name TEXT NOT NULL,
    tickers_json TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

-- ============================================================
-- Diagnostics
-- ============================================================

CREATE TABLE llm_usage_log (
    user_id TEXT NOT NULL,
    id TEXT NOT NULL,
    provider_name TEXT NOT NULL,
    model TEXT NOT NULL,
    invoker TEXT NOT NULL,
    session_id TEXT,
    task_id TEXT,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL,
    success INTEGER NOT NULL,
    error TEXT,
    started_at TEXT NOT NULL,
    PRIMARY KEY (user_id, id)
);

CREATE INDEX idx_usage_provider ON llm_usage_log(user_id, provider_name, started_at DESC);
CREATE INDEX idx_usage_task ON llm_usage_log(user_id, task_id, started_at DESC) WHERE task_id IS NOT NULL;
