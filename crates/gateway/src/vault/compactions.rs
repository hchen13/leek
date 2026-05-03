//! Session-compaction row helpers.
//!
//! Each row records one compaction event: the original session, the forked
//! child session that received the summary, and per-call metadata. See
//! `migrations/0002_compaction.sql` for the schema.

use anyhow::{Context, Result};
use serde::Serialize;
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, FromRow, Serialize, Clone)]
#[allow(dead_code)] // fields exposed via API once compaction history UI lands
pub struct CompactionRow {
    pub original_session_id: String,
    pub compacted_session_id: String,
    pub summary_md: String,
    pub messages_retained: i64,
    pub messages_removed: i64,
    pub tokens_before: Option<i64>,
    pub tokens_after: Option<i64>,
    pub trigger: String,
    pub focus: Option<String>,
    pub created_at: String,
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool: &SqlitePool,
    user_id: &str,
    original_session_id: &str,
    compacted_session_id: &str,
    summary_md: &str,
    messages_retained: i64,
    messages_removed: i64,
    tokens_before: Option<i64>,
    tokens_after: Option<i64>,
    trigger: &str,
    focus: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        INSERT INTO session_compactions (
            user_id, original_session_id, compacted_session_id,
            summary_md, messages_retained, messages_removed,
            tokens_before, tokens_after, trigger, focus, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(user_id)
    .bind(original_session_id)
    .bind(compacted_session_id)
    .bind(summary_md)
    .bind(messages_retained)
    .bind(messages_removed)
    .bind(tokens_before)
    .bind(tokens_after)
    .bind(trigger)
    .bind(focus)
    .bind(&now)
    .execute(pool)
    .await
    .context("inserting session_compactions row")?;
    Ok(())
}
