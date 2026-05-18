-- L.E.E.K vault — M1 schema (agent loop MVP).
--
-- M1 turns the M0 echo skeleton into a real agent loop on the codex
-- backend. Two tables land here, each justified per docs/ARCHITECTURE.md §6
-- ("每张新加入的表都必须在 migration 文件头部说明为什么现在加"):
--
-- 1. turn_metrics — per-turn observability. The M1 loop is non-deterministic
--    (the model decides tool calls and iteration count), and ARCHITECTURE §5
--    lists per-turn metrics as a default-on, durable harness primitive. It is
--    added now because M1 is the first milestone with a loop to measure: a
--    loop you cannot inspect is one you cannot debug. One row per turn
--    (turn = one user prompt → one final assistant message; see MILESTONES
--    "命名约定").
--
-- 2. codex_auth — codex OAuth token storage. M1 is the first milestone that
--    authenticates against a model provider; the access / refresh tokens
--    must survive a gateway restart, so they need a durable home. One row
--    per user (M0 is single-user → one row, user_id='local'). Deliberately
--    a small, codex-specific table — not a speculative per-provider config
--    table (there is one concrete provider; see ARCHITECTURE §2,
--    "不抽象 LLM provider").

-- One row per turn. `turn_id` is a gateway-generated id; the matching
-- user / assistant messages live in `messages` (correlated via SSE
-- payloads, not a FK — messages have no turn column in M1).
CREATE TABLE turn_metrics (
    turn_id                TEXT    NOT NULL PRIMARY KEY,
    session_id             TEXT    NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    model                  TEXT    NOT NULL,

    started_at             TEXT    NOT NULL,
    ended_at               TEXT    NOT NULL,
    wall_clock_ms          INTEGER NOT NULL,

    iteration_count        INTEGER NOT NULL,   -- LLM calls inside the turn
    tool_call_count        INTEGER NOT NULL,
    tool_error_count       INTEGER NOT NULL,

    input_tokens           INTEGER NOT NULL DEFAULT 0,
    output_tokens          INTEGER NOT NULL DEFAULT 0,
    cost_usd               REAL    NOT NULL DEFAULT 0.0,

    -- Free-form, but write only the documented values (see vault/turn_metrics.rs):
    -- end_turn | max_tokens | idle_timeout | wall_clock_exceeded |
    -- max_iterations | cost_cap_exceeded | doom_loop | context_limit |
    -- fatal_error
    stop_reason            TEXT    NOT NULL,
    -- The guard that ended the turn, when a guard did (else NULL).
    first_triggered_guard  TEXT,
    -- Populated only when stop_reason = 'fatal_error'.
    fatal_error            TEXT
);

CREATE INDEX idx_turn_metrics_session ON turn_metrics (session_id);

-- One row per user. Codex tokens (chatgpt OAuth). `expires_at` is the
-- access_token JWT `exp`; the gateway refreshes when within a small skew
-- of it. `account_id` is the codex account the tokens belong to.
CREATE TABLE codex_auth (
    user_id        TEXT NOT NULL PRIMARY KEY REFERENCES users(id),
    access_token   TEXT NOT NULL,
    refresh_token  TEXT NOT NULL,
    account_id     TEXT,
    expires_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
