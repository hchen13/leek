use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct PlanItemInput {
    pub id: Option<String>,
    pub step: String,
    pub status: String,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlanItemRow {
    pub item_id: String,
    pub seq: i64,
    pub step: String,
    pub status: String,
    pub evidence: Option<String>,
}

pub async fn replace_current(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    items: &[PlanItemInput],
) -> Result<Vec<PlanItemRow>> {
    validate_items(items)?;
    let task_id = scope_task_id(task_id);
    let now = Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;

    sqlx::query(
        "DELETE FROM agent_plan_items \
         WHERE user_id = ? AND session_id = ? AND task_id = ?",
    )
    .bind(user_id)
    .bind(session_id)
    .bind(&task_id)
    .execute(&mut *tx)
    .await
    .context("clearing current agent plan")?;

    for (idx, item) in items.iter().enumerate() {
        let item_id = item
            .id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("p{}", idx + 1));
        let seq = i64::try_from(idx + 1).unwrap_or(i64::MAX);
        sqlx::query(
            r#"
            INSERT INTO agent_plan_items
              (user_id, session_id, task_id, item_id, seq, step, status,
               evidence, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(user_id)
        .bind(session_id)
        .bind(&task_id)
        .bind(item_id)
        .bind(seq)
        .bind(item.step.trim())
        .bind(item.status.as_str())
        .bind(
            item.evidence
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .context("inserting agent plan item")?;
    }

    tx.commit().await?;
    list_current(pool, user_id, session_id, Some(&task_id)).await
}

pub async fn list_current(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
) -> Result<Vec<PlanItemRow>> {
    let task_id = scope_task_id(task_id);
    sqlx::query_as::<_, PlanItemRow>(
        r#"
        SELECT item_id, seq, step, status, evidence
        FROM agent_plan_items
        WHERE user_id = ? AND session_id = ? AND task_id = ?
        ORDER BY seq ASC
        "#,
    )
    .bind(user_id)
    .bind(session_id)
    .bind(task_id)
    .fetch_all(pool)
    .await
    .context("listing current agent plan")
}

pub fn scope_task_id(task_id: Option<&str>) -> String {
    task_id.unwrap_or("").to_string()
}

fn validate_items(items: &[PlanItemInput]) -> Result<()> {
    if items.is_empty() {
        return Err(anyhow!("plan must contain at least one item"));
    }
    let mut in_progress = 0usize;
    for item in items {
        if item.step.trim().is_empty() {
            return Err(anyhow!("plan item step cannot be empty"));
        }
        match item.status.as_str() {
            "pending" | "completed" => {}
            "in_progress" => in_progress += 1,
            other => return Err(anyhow!("invalid plan item status: {other}")),
        }
    }
    if in_progress > 1 {
        return Err(anyhow!("plan can have at most one in_progress item"));
    }
    Ok(())
}
