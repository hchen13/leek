//! Per-turn observability — one row written at the end of every agent turn.
//!
//! A *turn* is one user prompt → one final assistant message; an
//! *iteration* is one LLM call inside that turn (see MILESTONES "命名约定").
//! The row captures everything needed to debug a non-deterministic loop
//! after the fact: how it stopped, how much it spent, which guard (if any)
//! cut it short.
//!
//! `stop_reason` is a free-form column, but write only these values so the
//! string stays groupable:
//!   - `end_turn`            — model finished naturally
//!   - `max_tokens`          — provider hit its output-length limit
//!   - `idle_timeout`        — stream went silent past the idle budget
//!   - `wall_clock_exceeded` — turn passed its wall-clock deadline
//!   - `max_iterations`      — iteration cap reached (opt-in guard)
//!   - `cost_cap_exceeded`   — USD cost cap reached (opt-in guard)
//!   - `doom_loop`           — same (tool, args) called N times in a row
//!   - `context_limit`       — context window near full (auto-compaction)
//!   - `fatal_error`         — provider / internal failure; see `fatal_error`

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// Values for one `turn_metrics` row, ready to insert.
#[derive(Debug, Clone)]
pub struct NewTurnMetrics<'a> {
    pub turn_id: &'a str,
    pub session_id: &'a str,
    pub model: &'a str,
    pub started_at: &'a str,
    pub ended_at: &'a str,
    pub wall_clock_ms: i64,
    pub iteration_count: i64,
    pub tool_call_count: i64,
    pub tool_error_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub stop_reason: &'a str,
    pub first_triggered_guard: Option<&'a str>,
    pub fatal_error: Option<&'a str>,
}

/// Write the metrics row for a finished turn.
pub async fn insert(pool: &SqlitePool, m: &NewTurnMetrics<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO turn_metrics \
           (turn_id, session_id, model, started_at, ended_at, wall_clock_ms, \
            iteration_count, tool_call_count, tool_error_count, \
            input_tokens, output_tokens, cost_usd, \
            stop_reason, first_triggered_guard, fatal_error) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(m.turn_id)
    .bind(m.session_id)
    .bind(m.model)
    .bind(m.started_at)
    .bind(m.ended_at)
    .bind(m.wall_clock_ms)
    .bind(m.iteration_count)
    .bind(m.tool_call_count)
    .bind(m.tool_error_count)
    .bind(m.input_tokens)
    .bind(m.output_tokens)
    .bind(m.cost_usd)
    .bind(m.stop_reason)
    .bind(m.first_triggered_guard)
    .bind(m.fatal_error)
    .execute(pool)
    .await
    .context("inserting turn_metrics row")?;
    Ok(())
}
