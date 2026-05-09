//! Per-user runtime preferences. P1 only holds LLM tuning (per-surface
//! reasoning_effort + verbosity). The on-disk shape is a single JSON blob
//! per user so we can extend without further migrations.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::llm::{LlmTuning, ReasoningEffort, SurfaceTuning, Verbosity};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSurface {
    reasoning_effort: String,
    verbosity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTuning {
    main: StoredSurface,
    routing: StoredSurface,
    compaction: StoredSurface,
    subagent: StoredSurface,
}

impl From<SurfaceTuning> for StoredSurface {
    fn from(s: SurfaceTuning) -> Self {
        Self {
            reasoning_effort: s.reasoning_effort.as_str().to_string(),
            verbosity: s.verbosity.as_str().to_string(),
        }
    }
}

impl StoredSurface {
    fn into_runtime(self, fallback: SurfaceTuning) -> SurfaceTuning {
        SurfaceTuning {
            reasoning_effort: ReasoningEffort::from_str_ci(&self.reasoning_effort)
                .unwrap_or(fallback.reasoning_effort),
            verbosity: Verbosity::from_str_ci(&self.verbosity).unwrap_or(fallback.verbosity),
        }
    }
}

impl From<LlmTuning> for StoredTuning {
    fn from(t: LlmTuning) -> Self {
        Self {
            main: t.main.into(),
            routing: t.routing.into(),
            compaction: t.compaction.into(),
            subagent: t.subagent.into(),
        }
    }
}

impl StoredTuning {
    fn into_runtime(self) -> LlmTuning {
        let defaults = LlmTuning::defaults();
        LlmTuning {
            main: self.main.into_runtime(defaults.main),
            routing: self.routing.into_runtime(defaults.routing),
            compaction: self.compaction.into_runtime(defaults.compaction),
            subagent: self.subagent.into_runtime(defaults.subagent),
        }
    }
}

/// Load the user's tuning. Falls back to defaults when the row doesn't exist
/// (first run) or when the JSON is corrupt (with a warning so we don't
/// silently overwrite user choices on reboot).
pub async fn load_tuning(pool: &SqlitePool, user_id: &str) -> Result<LlmTuning> {
    let row: Option<(String,)> = sqlx::query_as("SELECT tuning_json FROM user_settings WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .context("loading user_settings")?;

    let Some((json,)) = row else {
        return Ok(LlmTuning::defaults());
    };

    match serde_json::from_str::<StoredTuning>(&json) {
        Ok(stored) => Ok(stored.into_runtime()),
        Err(err) => {
            tracing::warn!(?err, "user_settings.tuning_json is corrupt; using defaults");
            Ok(LlmTuning::defaults())
        }
    }
}

pub async fn save_tuning(pool: &SqlitePool, user_id: &str, tuning: LlmTuning) -> Result<()> {
    let stored: StoredTuning = tuning.into();
    let json = serde_json::to_string(&stored).context("serializing tuning")?;
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO user_settings (user_id, tuning_json, updated_at) \
         VALUES (?, ?, ?) \
         ON CONFLICT(user_id) DO UPDATE SET tuning_json = excluded.tuning_json, \
                                            updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(&json)
    .bind(&now)
    .execute(pool)
    .await
    .context("upserting user_settings")?;
    Ok(())
}
