-- 0010_task_metrics.sql
--
-- Per-task observability. Written once at task lifecycle endpoint
-- (delivered / awaiting_user / fatal_error / aborted). Captures
-- wall-clock, iteration / tool counts, latency, stop reason.
--
-- Some columns are reserved for later milestones:
--   - token / cost columns: M1.5 wires them when per-model price
--     tables and turn-level cost cap land. Through M1.4 they stay 0.
--   - first_triggered_guard: M1.6 wires it when the doom-loop /
--     cap detectors land.
--   - parent_task_id / depth: M2.7 wires them when the generic
--     subagent mechanism lands.
--
-- Stop reasons in this table are a superset of the agent-loop
-- `stop_reason` strings; the table can also record guard-driven
-- stops (idle_timeout / wall_clock_exceeded / max_iterations /
-- cost_cap_exceeded / doom_loop) once those land.

CREATE TABLE task_metrics (
    user_id              TEXT    NOT NULL,
    task_id              TEXT    NOT NULL,
    session_id           TEXT    NOT NULL,

    started_at           TEXT    NOT NULL,
    ended_at             TEXT    NOT NULL,
    wall_clock_ms        INTEGER NOT NULL,

    iteration_count      INTEGER NOT NULL,
    tool_call_count      INTEGER NOT NULL,
    tool_error_count     INTEGER NOT NULL,

    total_input_tokens   INTEGER NOT NULL DEFAULT 0,
    total_output_tokens  INTEGER NOT NULL DEFAULT 0,
    total_cost_usd       REAL    NOT NULL DEFAULT 0.0,

    max_iter_latency_ms  INTEGER,
    p50_iter_latency_ms  INTEGER,

    stop_reason            TEXT NOT NULL,
    first_triggered_guard  TEXT,
    fatal_error            TEXT,

    parent_task_id       TEXT,
    depth                INTEGER NOT NULL DEFAULT 0,

    model                TEXT NOT NULL,

    PRIMARY KEY (user_id, task_id),
    FOREIGN KEY (user_id, task_id) REFERENCES tasks(user_id, id)
);

CREATE INDEX idx_task_metrics_session ON task_metrics(user_id, session_id);
CREATE INDEX idx_task_metrics_stop_reason ON task_metrics(stop_reason);
CREATE INDEX idx_task_metrics_parent
    ON task_metrics(user_id, parent_task_id)
    WHERE parent_task_id IS NOT NULL;
