//! SQLite vault — open + migrate.
//!
//! M0 schema: users / sessions / messages / events.
//! M1 adds: turn_metrics (per-turn observability) + codex_auth (OAuth tokens).

pub mod auth_tokens;
pub mod events;
pub mod messages;
pub mod sessions;
pub mod turn_metrics;

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

/// M0 is single-user; every session belongs to this fixed local user.
pub const LOCAL_USER: &str = "local";

pub struct Vault {
    pub pool: SqlitePool,
}

impl Vault {
    /// Open (creating if absent) the vault at `path`, run migrations, and make
    /// sure the local user row exists.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating vault dir {}", parent.display()))?;
            }
        }

        let url = format!("sqlite://{}", path.display());
        let opts = SqliteConnectOptions::from_str(&url)
            .with_context(|| format!("parsing sqlite url {url}"))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(opts)
            .await
            .with_context(|| format!("opening vault {}", path.display()))?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .context("running migrations")?;

        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT OR IGNORE INTO users (id, created_at) VALUES (?, ?)")
            .bind(LOCAL_USER)
            .bind(&now)
            .execute(&pool)
            .await
            .context("ensuring local user")?;

        Ok(Self { pool })
    }
}
