use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct PlanItemInput {
    pub id: Option<String>,
    pub step: String,
    pub status: String,
    pub resolution: Option<String>,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PlanItemRow {
    pub item_id: String,
    pub seq: i64,
    pub step: String,
    pub status: String,
    pub resolution: Option<String>,
    pub evidence: Option<String>,
}

/// Resolution values an agent may attach to a `completed` plan item.
/// Mirrors `design/decisions/0012-plan-resolution-and-budget-recovery.md`.
pub const VALID_RESOLUTIONS: &[&str] = &[
    "done",
    "blocked",
    "deferred",
    "superseded",
    "satisfied_by_proxy",
    "insufficient_evidence",
];

/// Resolution values whose **net effect on the deliverable** is "the agent
/// produced the evidence it intended to produce." Used by the guard so it can
/// distinguish a plan that closed cleanly from a plan that closed with caveats.
pub const SUCCESSFUL_RESOLUTIONS: &[&str] = &["done", "satisfied_by_proxy"];

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
               resolution, evidence, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            item.resolution
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty()),
        )
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
        SELECT item_id, seq, step, status, resolution, evidence
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

/// Force every open (pending / in_progress) item into `completed` with the
/// supplied resolution. Used by the budget-finalization recovery turn so the
/// plan reflects honest closure even when the agent ran out of substantive
/// turns. Returns the post-image rows.
pub async fn close_open_items(
    pool: &SqlitePool,
    user_id: &str,
    session_id: &str,
    task_id: Option<&str>,
    resolution: &str,
    evidence: &str,
) -> Result<Vec<PlanItemRow>> {
    if !VALID_RESOLUTIONS.contains(&resolution) {
        return Err(anyhow!("invalid resolution for close_open_items: {resolution}"));
    }
    let task_id_str = scope_task_id(task_id);
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        r#"
        UPDATE agent_plan_items
        SET status = 'completed',
            resolution = ?,
            evidence = COALESCE(NULLIF(TRIM(evidence), ''), ?),
            updated_at = ?
        WHERE user_id = ? AND session_id = ? AND task_id = ?
          AND status IN ('pending', 'in_progress')
        "#,
    )
    .bind(resolution)
    .bind(evidence.trim())
    .bind(&now)
    .bind(user_id)
    .bind(session_id)
    .bind(&task_id_str)
    .execute(pool)
    .await
    .context("auto-closing open plan items")?;

    list_current(pool, user_id, session_id, Some(&task_id_str)).await
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
            "pending" => {
                if item.resolution.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some() {
                    return Err(anyhow!(
                        "pending plan item must not carry a resolution"
                    ));
                }
            }
            "in_progress" => {
                in_progress += 1;
                if item.resolution.as_deref().map(str::trim).filter(|s| !s.is_empty()).is_some() {
                    return Err(anyhow!(
                        "in_progress plan item must not carry a resolution"
                    ));
                }
            }
            "completed" => {
                let res = item
                    .resolution
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        anyhow!(
                            "completed plan item requires a 'resolution' \
                             (one of: {})",
                            VALID_RESOLUTIONS.join(", ")
                        )
                    })?;
                if !VALID_RESOLUTIONS.contains(&res) {
                    return Err(anyhow!(
                        "invalid plan item resolution: {res} \
                         (valid: {})",
                        VALID_RESOLUTIONS.join(", ")
                    ));
                }
                let needs_evidence = res != "superseded";
                let evidence_ok = item
                    .evidence
                    .as_deref()
                    .map(str::trim)
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
                if needs_evidence && !evidence_ok {
                    return Err(anyhow!(
                        "completed plan item with resolution `{res}` requires non-empty evidence"
                    ));
                }
            }
            other => return Err(anyhow!("invalid plan item status: {other}")),
        }
    }
    if in_progress > 1 {
        return Err(anyhow!("plan can have at most one in_progress item"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"
            CREATE TABLE agent_plan_items (
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                task_id TEXT NOT NULL,
                item_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                step TEXT NOT NULL,
                status TEXT NOT NULL CHECK (status IN ('pending', 'in_progress', 'completed')),
                resolution TEXT,
                evidence TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (user_id, session_id, task_id, item_id)
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn item(id: &str, step: &str, status: &str, res: Option<&str>, evidence: Option<&str>) -> PlanItemInput {
        PlanItemInput {
            id: Some(id.to_string()),
            step: step.to_string(),
            status: status.to_string(),
            resolution: res.map(str::to_string),
            evidence: evidence.map(str::to_string),
        }
    }

    #[test]
    fn rejects_completed_without_resolution() {
        let err = validate_items(&[item("p1", "do x", "completed", None, Some("ok"))]).unwrap_err();
        assert!(err.to_string().contains("requires a 'resolution'"));
    }

    #[test]
    fn rejects_invalid_resolution() {
        let err = validate_items(&[item("p1", "x", "completed", Some("nope"), Some("ok"))])
            .unwrap_err();
        assert!(err.to_string().contains("invalid plan item resolution"));
    }

    #[test]
    fn rejects_pending_with_resolution() {
        let err =
            validate_items(&[item("p1", "x", "pending", Some("done"), None)]).unwrap_err();
        assert!(err.to_string().contains("must not carry a resolution"));
    }

    #[test]
    fn requires_evidence_for_blocked() {
        let err =
            validate_items(&[item("p1", "x", "completed", Some("blocked"), None)]).unwrap_err();
        assert!(err.to_string().contains("non-empty evidence"));
    }

    #[test]
    fn allows_superseded_without_evidence() {
        validate_items(&[item("p1", "x", "completed", Some("superseded"), None)]).unwrap();
    }

    #[test]
    fn accepts_valid_completed_with_done() {
        validate_items(&[item("p1", "x", "completed", Some("done"), Some("evidence"))])
            .unwrap();
    }

    #[tokio::test]
    async fn close_open_items_marks_pending_blocked() {
        let pool = pool().await;
        let task = Some("t1");
        replace_current(
            &pool,
            "u",
            "s",
            task,
            &[
                item("p1", "x", "completed", Some("done"), Some("ok")),
                item("p2", "y", "pending", None, None),
            ],
        )
        .await
        .unwrap();
        let rows = close_open_items(&pool, "u", "s", task, "blocked", "budget exhausted")
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let p2 = rows.iter().find(|r| r.item_id == "p2").unwrap();
        assert_eq!(p2.status, "completed");
        assert_eq!(p2.resolution.as_deref(), Some("blocked"));
        assert_eq!(p2.evidence.as_deref(), Some("budget exhausted"));
        let p1 = rows.iter().find(|r| r.item_id == "p1").unwrap();
        // already completed/done — not overwritten.
        assert_eq!(p1.resolution.as_deref(), Some("done"));
    }
}
