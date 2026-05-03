//! SQLite vault — single DB with multiple `user_id` columns.
//!
//! See `design/p1-spec/data-schema.md` for the full schema reference.

pub mod compactions;
pub mod decisions;
pub mod events;
pub mod holdings;
pub mod messages;
pub mod provider_configs;
pub mod sessions;
pub mod tasks;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

pub const LOCAL_USER_ID: &str = "local";

#[derive(Clone)]
pub struct Vault {
    pub pool: SqlitePool,
}

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

impl Vault {
    pub async fn open(db_path: impl AsRef<Path>) -> Result<Self> {
        let path = db_path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating vault parent dir {}", parent.display()))?;
            }
        }

        let url = format!("sqlite://{}", path.display());
        let opts = SqliteConnectOptions::from_str(&url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await
            .with_context(|| format!("opening SQLite vault at {}", path.display()))?;

        MIGRATOR
            .run(&pool)
            .await
            .context("running vault migrations")?;

        let vault = Self { pool };
        vault.ensure_local_user().await?;
        Ok(vault)
    }

    async fn ensure_local_user(&self) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT OR IGNORE INTO users (user_id, display_name, created_at) \
             VALUES (?, 'Local User', ?)",
        )
        .bind(LOCAL_USER_ID)
        .bind(&now)
        .execute(&self.pool)
        .await
        .context("ensuring local user row")?;
        Ok(())
    }
}
