-- 0007_user_settings.sql
-- Per-user runtime preferences. P1 holds only LLM tuning (per-surface
-- reasoning_effort + verbosity); future fields (locale, decision-card
-- defaults, etc.) extend the JSON blob without requiring a new migration.

CREATE TABLE user_settings (
    user_id TEXT NOT NULL PRIMARY KEY,
    tuning_json TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
