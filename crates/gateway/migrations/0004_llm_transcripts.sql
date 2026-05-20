-- L.E.E.K vault — F2 schema delta (raw LLM transcript archive).
--
-- One table is added, justified per ARCHITECTURE §6 ("每张新加入的表都必须
-- 在 migration 文件头部说明为什么现在加"):
--
-- llm_transcripts — per-iteration raw LLM-layer record: the verbatim
--   request body leek sent to the codex backend plus the raw SSE response
--   bytes it streamed back. Until now the only durable record of a turn
--   was the `events` table — already-processed gateway events. That is
--   enough for normal use, but every debug task that asks "what did the
--   provider actually send / receive?" (e.g. discovering the shape of
--   `web_search_call.action`) forces a re-hit of the codex backend with
--   fresh tokens, which is slow, expensive, and may not reproduce. F2
--   archives the raw bytes per (turn_id, iteration) so debug walks the
--   archive instead.
--
-- Processed events and raw transcripts are two parallel layers on purpose
-- (events stays the user-visible record; transcripts is the engineering
-- log). No other table is touched.
--
-- One row per LLM call inside a turn — that includes both the main agent
-- iterations and the auto-compaction summary call (every path through
-- `responses::stream` is hooked). `iteration` is a per-turn monotonic
-- counter assigned by the agent loop (see `vault/llm_transcripts.rs`);
-- it is NOT the same series as `turn_metrics.iteration_count`, which by
-- convention counts only main-loop iterations.
--
-- response_stream / http_status / finished_at are nullable: the row is
-- inserted before the request is sent (so a mid-stream crash still leaves
-- evidence), and updated once after the stream ends. MVP does no
-- chunk-level eager update — a single finalize is enough for debugging
-- and cheap enough to keep simple.

CREATE TABLE llm_transcripts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id      TEXT    NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id         TEXT    NOT NULL,
    iteration       INTEGER NOT NULL,
    model           TEXT    NOT NULL,
    -- The request body leek constructed and POSTed to the codex backend,
    -- raw bytes verbatim (`include` / tools / input / instructions all
    -- intact). The Authorization header is NOT in the body — it goes on
    -- the HTTP header, so no credentials land here.
    request_body    BLOB    NOT NULL,
    -- The codex backend's raw response body: SSE bytes (`event:` /
    -- `data:` framing intact) on a 2xx, or the error body on a non-2xx.
    -- NULL means the row was inserted but never finalized (gateway died
    -- mid-stream); the request_body alone is still useful.
    response_stream BLOB,
    -- HTTP status code of the response. 200 on a clean SSE stream;
    -- 4xx/5xx on a backend error (response_stream then carries the error
    -- body); 0 on a transport-level mid-stream failure (response_stream
    -- carries whatever bytes had arrived).
    http_status     INTEGER,
    started_at      TEXT    NOT NULL,
    finished_at     TEXT,
    -- One transcript per (turn_id, iteration). The loop assigns iteration
    -- monotonically per turn so the constraint also catches
    -- double-finalize bugs.
    UNIQUE (turn_id, iteration)
);

CREATE INDEX idx_llm_transcripts_session ON llm_transcripts(session_id);
CREATE INDEX idx_llm_transcripts_turn    ON llm_transcripts(turn_id, iteration);
