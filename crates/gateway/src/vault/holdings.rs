use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, FromRow, Serialize, Clone)]
pub struct HoldingRow {
    pub snapshot_at: String,
    pub ticker: String,
    pub qty: f64,
    pub avg_cost: Option<f64>,
    pub account: String,
    pub notes: Option<String>,
    pub source: String,
}

#[derive(Debug, Deserialize)]
pub struct NewHolding {
    pub ticker: String,
    pub qty: f64,
    pub avg_cost: Option<f64>,
    pub account: Option<String>,
    pub notes: Option<String>,
}

pub async fn get_latest(pool: &SqlitePool, user_id: &str) -> Result<Vec<HoldingRow>> {
    let rows: Vec<HoldingRow> = sqlx::query_as(
        r#"
        SELECT snapshot_at, ticker, qty, avg_cost, account, notes, source
        FROM holdings
        WHERE user_id = ?
          AND snapshot_at = (
              SELECT MAX(snapshot_at) FROM holdings WHERE user_id = ?
          )
        ORDER BY ticker ASC
        "#,
    )
    .bind(user_id)
    .bind(user_id)
    .fetch_all(pool)
    .await
    .context("loading latest holdings snapshot")?;
    Ok(rows)
}

pub async fn insert_snapshot(
    pool: &SqlitePool,
    user_id: &str,
    holdings: &[NewHolding],
) -> Result<String> {
    let snapshot_at = chrono::Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    for h in holdings {
        let account = h.account.as_deref().unwrap_or("main");
        sqlx::query(
            r#"
            INSERT INTO holdings (user_id, snapshot_at, ticker, qty, avg_cost, account, notes, source)
            VALUES (?, ?, ?, ?, ?, ?, ?, 'manual')
            "#,
        )
        .bind(user_id)
        .bind(&snapshot_at)
        .bind(&h.ticker)
        .bind(h.qty)
        .bind(h.avg_cost)
        .bind(account)
        .bind(&h.notes)
        .execute(&mut *tx)
        .await
        .context("inserting holding row")?;
    }
    tx.commit().await?;
    Ok(snapshot_at)
}

pub async fn list_snapshots(pool: &SqlitePool, user_id: &str, limit: i64) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT DISTINCT snapshot_at
        FROM holdings
        WHERE user_id = ?
        ORDER BY snapshot_at DESC
        LIMIT ?
        "#,
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("listing holding snapshots")?;
    Ok(rows.into_iter().map(|(s,)| s).collect())
}
