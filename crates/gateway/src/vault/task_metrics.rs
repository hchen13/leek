//! Per-task observability — written once at the task lifecycle endpoint.
//!
//! M1.1 establishes the table + writer; later milestones populate the
//! columns they own:
//!   - M1.2 `idle_timeout` lands in `stop_reason` ("idle_timeout")
//!   - M1.3 `wall_clock_exceeded` lands in `stop_reason`; soft-prompt
//!     hints are not recorded here (they're per-block, not per-task)
//!   - M1.4 `max_iterations` lands in `stop_reason` (and the column
//!     `iteration_count` is renamed from the existing `turn` variable)
//!   - M1.5 fills `total_input_tokens / total_output_tokens /
//!     total_cost_usd` from the LLM `usage` blocks
//!   - M1.6 fills `first_triggered_guard` (the *earliest* guard that
//!     fired even if multiple chained — useful for postmortems)
//!   - M2.7 fills `parent_task_id` and `depth` for subagent runs
//!
//! Stop-reason taxonomy (free-form string, but write only these
//! values to keep the index useful):
//!   - end_turn               — natural completion
//!   - awaiting_user          — handed off via ask_user_question
//!   - user_aborted           — explicit Esc / abort
//!   - max_iterations         — iteration cap (M1.4)
//!   - idle_timeout           — no activity (M1.2)
//!   - wall_clock_exceeded    — turn deadline (M1.3)
//!   - cost_cap_exceeded      — USD/turn cap (M1.5)
//!   - doom_loop              — repeated identical (tool, args) (M1.6)
//!   - plan_guard_continue    — soft replan (current loop)
//!   - plan_guard_exhausted   — hard plan-guard fail (current loop)
//!   - fatal_error            — anything else; check `fatal_error` col

use anyhow::{Context, Result};
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct NewTaskMetrics<'a> {
    pub user_id: &'a str,
    pub task_id: &'a str,
    pub session_id: &'a str,
    pub started_at: &'a str,
    pub ended_at: &'a str,
    pub wall_clock_ms: i64,

    pub iteration_count: i64,
    pub tool_call_count: i64,
    pub tool_error_count: i64,

    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cost_usd: f64,

    pub max_iter_latency_ms: Option<i64>,
    pub p50_iter_latency_ms: Option<i64>,

    pub stop_reason: &'a str,
    pub first_triggered_guard: Option<&'a str>,
    pub fatal_error: Option<&'a str>,

    pub parent_task_id: Option<&'a str>,
    pub depth: i64,

    pub model: &'a str,
}

pub async fn insert(pool: &SqlitePool, m: NewTaskMetrics<'_>) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO task_metrics
          (user_id, task_id, session_id,
           started_at, ended_at, wall_clock_ms,
           iteration_count, tool_call_count, tool_error_count,
           total_input_tokens, total_output_tokens, total_cost_usd,
           max_iter_latency_ms, p50_iter_latency_ms,
           stop_reason, first_triggered_guard, fatal_error,
           parent_task_id, depth, model)
        VALUES (?, ?, ?,
                ?, ?, ?,
                ?, ?, ?,
                ?, ?, ?,
                ?, ?,
                ?, ?, ?,
                ?, ?, ?)
        "#,
    )
    .bind(m.user_id)
    .bind(m.task_id)
    .bind(m.session_id)
    .bind(m.started_at)
    .bind(m.ended_at)
    .bind(m.wall_clock_ms)
    .bind(m.iteration_count)
    .bind(m.tool_call_count)
    .bind(m.tool_error_count)
    .bind(m.total_input_tokens)
    .bind(m.total_output_tokens)
    .bind(m.total_cost_usd)
    .bind(m.max_iter_latency_ms)
    .bind(m.p50_iter_latency_ms)
    .bind(m.stop_reason)
    .bind(m.first_triggered_guard)
    .bind(m.fatal_error)
    .bind(m.parent_task_id)
    .bind(m.depth)
    .bind(m.model)
    .execute(pool)
    .await
    .context("inserting task_metrics row")?;
    Ok(())
}

/// Compute the median (p50) of a slice of latency samples in milliseconds.
/// Returns `None` for empty input. Used by the agent loop to summarize
/// per-iteration latency at task close.
pub fn p50_ms(samples: &[i64]) -> Option<i64> {
    if samples.is_empty() {
        return None;
    }
    let mut v: Vec<i64> = samples.to_vec();
    v.sort_unstable();
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        // Even count: average the two middle values, rounded toward zero.
        (v[n / 2 - 1] + v[n / 2]) / 2
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory pool with just the `task_metrics` schema — FK-free so
    /// tests don't need a full `tasks` / `sessions` row to exercise
    /// inserts. Mirrors `vault::plans::tests::pool()` in spirit.
    async fn pool() -> sqlx::SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
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
                PRIMARY KEY (user_id, task_id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[test]
    fn p50_empty_returns_none() {
        assert_eq!(p50_ms(&[]), None);
    }

    #[test]
    fn p50_single_returns_self() {
        assert_eq!(p50_ms(&[42]), Some(42));
    }

    #[test]
    fn p50_odd_count_takes_middle() {
        assert_eq!(p50_ms(&[5, 1, 3, 2, 4]), Some(3));
    }

    #[test]
    fn p50_even_count_averages_middle_pair() {
        // Sorted: [1, 2, 3, 4] → (2 + 3) / 2 = 2 (integer division toward zero)
        assert_eq!(p50_ms(&[4, 1, 3, 2]), Some(2));
    }

    #[tokio::test]
    async fn insert_round_trips_a_minimal_row() {
        let pool = pool().await;
        insert(
            &pool,
            NewTaskMetrics {
                user_id: "local",
                task_id: "t-1",
                session_id: "s-1",
                started_at: "2026-05-10T00:00:00Z",
                ended_at: "2026-05-10T00:01:00Z",
                wall_clock_ms: 60_000,
                iteration_count: 5,
                tool_call_count: 3,
                tool_error_count: 0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost_usd: 0.0,
                max_iter_latency_ms: Some(2500),
                p50_iter_latency_ms: Some(1100),
                stop_reason: "end_turn",
                first_triggered_guard: None,
                fatal_error: None,
                parent_task_id: None,
                depth: 0,
                model: "gpt-5.5",
            },
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_metrics WHERE user_id='local' AND task_id='t-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn insert_persists_optional_fields() {
        let pool = pool().await;
        insert(
            &pool,
            NewTaskMetrics {
                user_id: "local",
                task_id: "t-2",
                session_id: "s-1",
                started_at: "2026-05-10T00:00:00Z",
                ended_at: "2026-05-10T00:00:30Z",
                wall_clock_ms: 30_000,
                iteration_count: 2,
                tool_call_count: 1,
                tool_error_count: 1,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cost_usd: 0.0,
                max_iter_latency_ms: Some(15_000),
                p50_iter_latency_ms: Some(15_000),
                stop_reason: "fatal_error",
                first_triggered_guard: Some("doom_loop"),
                fatal_error: Some("identical (tool, args) ≥ 3"),
                parent_task_id: Some("t-1"),
                depth: 1,
                model: "gpt-5.5",
            },
        )
        .await
        .unwrap();

        let (guard, fatal, parent, depth): (Option<String>, Option<String>, Option<String>, i64) =
            sqlx::query_as(
                "SELECT first_triggered_guard, fatal_error, parent_task_id, depth
                 FROM task_metrics WHERE task_id='t-2'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(guard.as_deref(), Some("doom_loop"));
        assert_eq!(fatal.as_deref(), Some("identical (tool, args) ≥ 3"));
        assert_eq!(parent.as_deref(), Some("t-1"));
        assert_eq!(depth, 1);
    }
}
