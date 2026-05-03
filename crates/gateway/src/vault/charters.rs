use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;
use uuid::Uuid;

pub struct CharterRow {
    pub id: String,
    pub name: String,
    pub text: String,
    pub version: i64,
    pub updated_at: String,
}

pub async fn get_active_text(pool: &SqlitePool, user_id: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT charter_json FROM team_charters \
         WHERE user_id = ? AND active = 1 \
         ORDER BY version DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("fetching active charter")?;

    let Some((charter_json,)) = row else {
        return Ok(None);
    };

    let v: serde_json::Value = serde_json::from_str(&charter_json).context("parsing charter_json")?;
    let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
    if text.trim().is_empty() {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}

pub async fn get_active(pool: &SqlitePool, user_id: &str) -> Result<Option<CharterRow>> {
    let row: Option<(String, String, String, i64, String)> = sqlx::query_as(
        "SELECT id, name, charter_json, version, updated_at FROM team_charters \
         WHERE user_id = ? AND active = 1 \
         ORDER BY version DESC LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("fetching active charter row")?;

    let Some((id, name, charter_json, version, updated_at)) = row else {
        return Ok(None);
    };

    let v: serde_json::Value = serde_json::from_str(&charter_json).context("parsing charter_json")?;
    let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();

    Ok(Some(CharterRow { id, name, text, version, updated_at }))
}

pub async fn upsert(pool: &SqlitePool, user_id: &str, text: &str) -> Result<CharterRow> {
    let charter_json = serde_json::json!({ "text": text }).to_string();
    let now = Utc::now().to_rfc3339();

    let existing_id: Option<(String,)> = sqlx::query_as(
        "SELECT id FROM team_charters WHERE user_id = ? AND active = 1 LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("checking existing charter")?;

    if let Some((id,)) = existing_id {
        sqlx::query(
            "UPDATE team_charters \
             SET charter_json = ?, version = version + 1, updated_at = ? \
             WHERE user_id = ? AND id = ?",
        )
        .bind(&charter_json)
        .bind(&now)
        .bind(user_id)
        .bind(&id)
        .execute(pool)
        .await
        .context("updating charter")?;

        let row = get_active(pool, user_id)
            .await?
            .context("charter missing after update")?;
        Ok(row)
    } else {
        let id = Uuid::now_v7().to_string();
        sqlx::query(
            "INSERT INTO team_charters \
             (user_id, id, name, charter_json, active, version, created_at, updated_at) \
             VALUES (?, ?, 'default', ?, 1, 1, ?, ?)",
        )
        .bind(user_id)
        .bind(&id)
        .bind(&charter_json)
        .bind(&now)
        .bind(&now)
        .execute(pool)
        .await
        .context("inserting charter")?;

        Ok(CharterRow {
            id,
            name: "default".to_string(),
            text: text.to_string(),
            version: 1,
            updated_at: now,
        })
    }
}
