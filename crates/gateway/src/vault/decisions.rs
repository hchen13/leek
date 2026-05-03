use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DeliverableRow {
    pub id: String,
    pub task_id: Option<String>,
    pub kind: String,
    pub payload_json: String,
    pub status: String,
    pub rejection_reason: Option<String>,
    pub created_at: String,
    pub ready_at: Option<String>,
    pub confirmed_at: Option<String>,
    pub rejected_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DecisionRow {
    pub id: String,
    pub deliverable_id: String,
    pub task_id: Option<String>,
    pub session_id: String,
    pub ticker: String,
    pub direction: String,
    pub size_shares: Option<f64>,
    pub size_pct: Option<f64>,
    pub stop_loss: Option<f64>,
    pub target: Option<f64>,
    pub horizon_days: Option<i64>,
    pub rationale_md: String,
    pub status: String,
    pub confirmed_at: String,
    pub closed_at: Option<String>,
}

pub async fn list_pending(pool: &SqlitePool, user_id: &str) -> Result<Vec<DeliverableRow>> {
    let rows = sqlx::query_as::<_, DeliverableRow>(
        r#"
        SELECT id, task_id, kind, payload_json, status, rejection_reason,
               created_at, ready_at, confirmed_at, rejected_at
        FROM deliverables
        WHERE user_id = ? AND status = 'pending_confirm'
        ORDER BY created_at DESC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .context("listing pending deliverables")?;
    Ok(rows)
}

pub async fn list_decisions(
    pool: &SqlitePool,
    user_id: &str,
    limit: i64,
) -> Result<Vec<DecisionRow>> {
    let rows = sqlx::query_as::<_, DecisionRow>(
        r#"
        SELECT id, deliverable_id, task_id, session_id, ticker, direction,
               size_shares, size_pct, stop_loss, target, horizon_days,
               rationale_md, status, confirmed_at, closed_at
        FROM decisions
        WHERE user_id = ?
        ORDER BY confirmed_at DESC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("listing decisions")?;
    Ok(rows)
}

struct DeliverableLookup {
    task_id: Option<String>,
    payload_json: String,
    status: String,
}

pub async fn confirm_deliverable(
    pool: &SqlitePool,
    user_id: &str,
    deliverable_id: &str,
    session_id: &str,
) -> Result<String> {
    let row = sqlx::query_as::<_, (Option<String>, String, String)>(
        r#"
        SELECT task_id, payload_json, status
        FROM deliverables
        WHERE user_id = ? AND id = ?
        "#,
    )
    .bind(user_id)
    .bind(deliverable_id)
    .fetch_optional(pool)
    .await
    .context("fetching deliverable for confirm")?
    .map(|(task_id, payload_json, status)| DeliverableLookup { task_id, payload_json, status })
    .ok_or_else(|| anyhow::anyhow!("deliverable not found: {deliverable_id}"))?;

    if row.status != "pending_confirm" {
        bail!("deliverable {deliverable_id} is not pending_confirm (status={})", row.status);
    }

    let payload: serde_json::Value = serde_json::from_str(&row.payload_json)
        .context("parsing deliverable payload_json")?;

    let ticker = payload["ticker"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("payload missing ticker"))?
        .to_string();
    let direction = payload["direction"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("payload missing direction"))?
        .to_string();
    let rationale_md = payload["rationale"].as_str().unwrap_or("").to_string();
    let size_pct = payload["size_pct"].as_f64();
    let stop_loss = payload["stop_loss"].as_f64();
    let target = payload["target"].as_f64();
    let horizon_days = payload["horizon_days"].as_i64();

    let now = Utc::now().to_rfc3339();
    let decision_id = Uuid::now_v7().to_string();

    let mut tx = pool.begin().await?;

    sqlx::query(
        r#"
        UPDATE deliverables
        SET status = 'confirmed', confirmed_at = ?
        WHERE user_id = ? AND id = ?
        "#,
    )
    .bind(&now)
    .bind(user_id)
    .bind(deliverable_id)
    .execute(&mut *tx)
    .await
    .context("confirming deliverable")?;

    sqlx::query(
        r#"
        INSERT INTO decisions
          (user_id, id, deliverable_id, task_id, session_id, ticker, direction,
           size_pct, stop_loss, target, horizon_days, rationale_md, status, confirmed_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'open', ?)
        "#,
    )
    .bind(user_id)
    .bind(&decision_id)
    .bind(deliverable_id)
    .bind(&row.task_id)
    .bind(session_id)
    .bind(&ticker)
    .bind(&direction)
    .bind(size_pct)
    .bind(stop_loss)
    .bind(target)
    .bind(horizon_days)
    .bind(&rationale_md)
    .bind(&now)
    .execute(&mut *tx)
    .await
    .context("inserting decision row")?;

    tx.commit().await?;
    Ok(decision_id)
}

pub async fn reject_deliverable(
    pool: &SqlitePool,
    user_id: &str,
    deliverable_id: &str,
    reason: Option<&str>,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let affected = sqlx::query(
        r#"
        UPDATE deliverables
        SET status = 'rejected', rejected_at = ?, rejection_reason = ?
        WHERE user_id = ? AND id = ? AND status = 'pending_confirm'
        "#,
    )
    .bind(&now)
    .bind(reason)
    .bind(user_id)
    .bind(deliverable_id)
    .execute(pool)
    .await
    .context("rejecting deliverable")?
    .rows_affected();

    if affected == 0 {
        bail!("deliverable {deliverable_id} not found or not pending_confirm");
    }
    Ok(())
}
