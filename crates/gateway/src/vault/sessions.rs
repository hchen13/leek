//! Session rows.

use anyhow::Result;
use serde::Serialize;
use sqlx::SqlitePool;

use super::LOCAL_USER;

#[derive(Serialize, sqlx::FromRow)]
pub struct Session {
    pub id: String,
    pub title: Option<String>,
    pub created_at: String,
    pub last_active_at: String,
}

/// All sessions, most-recently-active first.
pub async fn list(pool: &SqlitePool) -> Result<Vec<Session>> {
    let rows = sqlx::query_as::<_, Session>(
        "SELECT id, title, created_at, last_active_at FROM sessions \
         WHERE user_id = ? ORDER BY last_active_at DESC",
    )
    .bind(LOCAL_USER)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Session> {
    let row = sqlx::query_as::<_, Session>(
        "SELECT id, title, created_at, last_active_at FROM sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Insert a session, ignoring the insert if `id` already exists. Returns the
/// resulting row either way.
pub async fn create(pool: &SqlitePool, id: &str, title: Option<&str>) -> Result<Session> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, title, created_at, last_active_at) \
         VALUES (?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(LOCAL_USER)
    .bind(title)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    get(pool, id).await
}

/// Insert an untitled session if `id` is not already present.
pub async fn ensure(pool: &SqlitePool, id: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, title, created_at, last_active_at) \
         VALUES (?, ?, NULL, ?, ?) ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(LOCAL_USER)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update a session title. Returns `false` if the session does not exist.
pub async fn rename(pool: &SqlitePool, id: &str, title: &str) -> Result<bool> {
    let res = sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
        .bind(title)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Delete a session; its messages and events cascade. Returns `false` if the
/// session did not exist.
pub async fn delete(pool: &SqlitePool, id: &str) -> Result<bool> {
    let res = sqlx::query("DELETE FROM sessions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Bump `last_active_at` to now.
pub async fn touch(pool: &SqlitePool, id: &str) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("UPDATE sessions SET last_active_at = ? WHERE id = ?")
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
