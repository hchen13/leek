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
//!   - `fatal_error`         — provider / internal failure; see `fatal_error`
//!
//! Auto-compaction is deliberately absent from this list: reaching the
//! context-window threshold folds the context and *continues* the turn
//! (REQUIREMENTS §7.1), so it is a `compaction_count`, not a `stop_reason`.

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
    /// Times auto-compaction folded the context during the turn.
    pub compaction_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
    pub stop_reason: &'a str,
    pub first_triggered_guard: Option<&'a str>,
    pub fatal_error: Option<&'a str>,
    /// M3.3: typed kind of the failure that ended the turn (`codex_http_5xx`,
    /// `codex_stream_silent`, …). `None` whenever the loop did not classify
    /// a reason — most stop_reasons are not fatal. The vocabulary lives in
    /// `agent::fatal::FatalReason`.
    pub fatal_reason_kind: Option<&'a str>,
    /// M3.3: human-readable detail for the typed reason — what the chat
    /// hint card renders and what a future failure-mode dashboard groups
    /// alongside `fatal_reason_kind`. `None` whenever `fatal_reason_kind`
    /// is `None`.
    pub fatal_reason_detail: Option<&'a str>,
    /// M2.7: parent turn id for a subagent row. `None` for main-agent
    /// turns. The parent / child rows are in the same table — observability
    /// walks the tree by following this column.
    pub parent_turn_id: Option<&'a str>,
    /// M2.7: 0 for main-agent turns, 1 for first-level subagents,
    /// 2 for grand-subagents (the depth cap).
    pub depth: i64,
}

/// Write the metrics row for a finished turn.
pub async fn insert(pool: &SqlitePool, m: &NewTurnMetrics<'_>) -> Result<()> {
    sqlx::query(
        "INSERT INTO turn_metrics \
           (turn_id, session_id, model, started_at, ended_at, wall_clock_ms, \
            iteration_count, tool_call_count, tool_error_count, compaction_count, \
            input_tokens, output_tokens, cost_usd, \
            stop_reason, first_triggered_guard, fatal_error, \
            fatal_reason_kind, fatal_reason_detail, \
            parent_turn_id, depth) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(m.compaction_count)
    .bind(m.input_tokens)
    .bind(m.output_tokens)
    .bind(m.cost_usd)
    .bind(m.stop_reason)
    .bind(m.first_triggered_guard)
    .bind(m.fatal_error)
    .bind(m.fatal_reason_kind)
    .bind(m.fatal_reason_detail)
    .bind(m.parent_turn_id)
    .bind(m.depth)
    .execute(pool)
    .await
    .context("inserting turn_metrics row")?;
    Ok(())
}

/// Fetch the subagent rows that descend from `parent_turn_id`. Used by
/// tests + the workbench debug surface to walk the subagent tree. One
/// level of children; for nested chains, the caller recurses.
///
/// `#[allow(dead_code)]`: this is exposed as observability infrastructure
/// for the M2.7 workbench (subagent_card → "open turn tree" debug view),
/// but the frontend currently reads the same data straight off the
/// `subagent_lifecycle` event payloads — so there is no production call
/// site yet. Kept here so a future debug surface doesn't need a fresh
/// migration to query subagent rows.
#[allow(dead_code)]
pub async fn list_children(
    pool: &SqlitePool,
    parent_turn_id: &str,
) -> Result<Vec<ChildRow>> {
    let rows = sqlx::query_as::<_, ChildRow>(
        "SELECT turn_id, parent_turn_id, depth, stop_reason, cost_usd, \
                wall_clock_ms, iteration_count \
         FROM turn_metrics WHERE parent_turn_id = ? ORDER BY started_at ASC",
    )
    .bind(parent_turn_id)
    .fetch_all(pool)
    .await
    .context("fetching subagent turn_metrics rows")?;
    Ok(rows)
}

/// A subagent row in a compact shape — enough to render the canvas
/// subagent_card without re-querying. Paired with `list_children` —
/// both live behind `#[allow(dead_code)]` until a workbench debug
/// surface starts calling them.
#[allow(dead_code)]
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChildRow {
    pub turn_id: String,
    pub parent_turn_id: Option<String>,
    pub depth: i64,
    pub stop_reason: String,
    pub cost_usd: f64,
    pub wall_clock_ms: i64,
    pub iteration_count: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::{sessions, Vault};

    async fn test_vault() -> Vault {
        let path = std::env::temp_dir().join(format!(
            "leek-turn-metrics-test-{}.db",
            uuid::Uuid::new_v4()
        ));
        Vault::open(&path).await.unwrap()
    }

    #[tokio::test]
    async fn parent_turn_id_and_depth_round_trip() {
        // M2.7: a subagent row gets its parent + depth persisted, and
        // can be fetched via `list_children`. This is the contract the
        // workbench uses to walk the subagent tree.
        let vault = test_vault().await;
        let sess_id = uuid::Uuid::new_v4().to_string();
        let session = sessions::create(&vault.pool, &sess_id, Some("t"))
            .await
            .unwrap();

        // Parent (main agent) row.
        insert(
            &vault.pool,
            &NewTurnMetrics {
                turn_id: "parent-turn",
                session_id: &session.id,
                model: "gpt-5.5",
                started_at: "2026-05-24T00:00:00Z",
                ended_at: "2026-05-24T00:00:05Z",
                wall_clock_ms: 5000,
                iteration_count: 3,
                tool_call_count: 1,
                tool_error_count: 0,
                compaction_count: 0,
                input_tokens: 1000,
                output_tokens: 200,
                cost_usd: 0.05,
                stop_reason: "end_turn",
                first_triggered_guard: None,
                fatal_error: None,
                fatal_reason_kind: None,
                fatal_reason_detail: None,
                parent_turn_id: None,
                depth: 0,
            },
        )
        .await
        .unwrap();

        // Subagent row pointing at the parent.
        insert(
            &vault.pool,
            &NewTurnMetrics {
                turn_id: "parent-turn.sub-abc12345",
                session_id: &session.id,
                model: "gpt-5.5",
                started_at: "2026-05-24T00:00:01Z",
                ended_at: "2026-05-24T00:00:04Z",
                wall_clock_ms: 3000,
                iteration_count: 2,
                tool_call_count: 4,
                tool_error_count: 0,
                compaction_count: 0,
                input_tokens: 800,
                output_tokens: 100,
                cost_usd: 0.02,
                stop_reason: "end_turn",
                first_triggered_guard: None,
                fatal_error: None,
                fatal_reason_kind: None,
                fatal_reason_detail: None,
                parent_turn_id: Some("parent-turn"),
                depth: 1,
            },
        )
        .await
        .unwrap();

        let kids = list_children(&vault.pool, "parent-turn").await.unwrap();
        assert_eq!(kids.len(), 1);
        let k = &kids[0];
        assert_eq!(k.turn_id, "parent-turn.sub-abc12345");
        assert_eq!(k.parent_turn_id.as_deref(), Some("parent-turn"));
        assert_eq!(k.depth, 1);
        assert_eq!(k.iteration_count, 2);
        assert!((k.cost_usd - 0.02).abs() < 1e-9);
    }

    #[tokio::test]
    async fn main_agent_row_has_null_parent_and_depth_zero() {
        let vault = test_vault().await;
        let sess_id = uuid::Uuid::new_v4().to_string();
        let session = sessions::create(&vault.pool, &sess_id, Some("t"))
            .await
            .unwrap();

        insert(
            &vault.pool,
            &NewTurnMetrics {
                turn_id: "solo-turn",
                session_id: &session.id,
                model: "gpt-5.5",
                started_at: "2026-05-24T00:00:00Z",
                ended_at: "2026-05-24T00:00:01Z",
                wall_clock_ms: 1000,
                iteration_count: 1,
                tool_call_count: 0,
                tool_error_count: 0,
                compaction_count: 0,
                input_tokens: 100,
                output_tokens: 50,
                cost_usd: 0.001,
                stop_reason: "end_turn",
                first_triggered_guard: None,
                fatal_error: None,
                fatal_reason_kind: None,
                fatal_reason_detail: None,
                parent_turn_id: None,
                depth: 0,
            },
        )
        .await
        .unwrap();

        // A turn with no parent has no children — make sure
        // `list_children` returns an empty vec instead of erroring.
        let kids = list_children(&vault.pool, "solo-turn").await.unwrap();
        assert!(kids.is_empty());
    }

    /// M3.3: fatal_reason_kind + fatal_reason_detail round-trip through
    /// the new columns. The chat hint card reads these via SSE
    /// (assistant_done payload), but the migration's value-add is making
    /// a workbench "group by failure kind" query trivial — verify the
    /// columns survive the insert.
    #[tokio::test]
    async fn fatal_reason_columns_round_trip() {
        let vault = test_vault().await;
        let sess_id = uuid::Uuid::new_v4().to_string();
        let session = sessions::create(&vault.pool, &sess_id, Some("t"))
            .await
            .unwrap();

        insert(
            &vault.pool,
            &NewTurnMetrics {
                turn_id: "fatal-turn",
                session_id: &session.id,
                model: "gpt-5.5",
                started_at: "2026-05-24T00:00:00Z",
                ended_at: "2026-05-24T00:00:01Z",
                wall_clock_ms: 1000,
                iteration_count: 1,
                tool_call_count: 0,
                tool_error_count: 0,
                compaction_count: 0,
                input_tokens: 100,
                output_tokens: 0,
                cost_usd: 0.001,
                stop_reason: "fatal_error",
                first_triggered_guard: None,
                fatal_error: Some("codex backend returned HTTP 503: service unavailable"),
                fatal_reason_kind: Some("codex_http_5xx"),
                fatal_reason_detail: Some("codex 返回 HTTP 503：service unavailable"),
                parent_turn_id: None,
                depth: 0,
            },
        )
        .await
        .unwrap();

        // Read the row back with a thin query — we don't have a list
        // helper, but we have enough plumbing in scope to issue a raw
        // sqlx query for the two new columns.
        let row: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT fatal_reason_kind, fatal_reason_detail FROM turn_metrics WHERE turn_id = ?",
        )
        .bind("fatal-turn")
        .fetch_one(&vault.pool)
        .await
        .unwrap();
        assert_eq!(row.0.as_deref(), Some("codex_http_5xx"));
        assert!(row.1.unwrap().contains("503"));
    }
}
