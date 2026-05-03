CREATE TABLE IF NOT EXISTS team_charters (
    user_id     TEXT    NOT NULL,
    id          TEXT    NOT NULL PRIMARY KEY,
    name        TEXT    NOT NULL DEFAULT 'default',
    charter_json TEXT   NOT NULL DEFAULT '{}',
    active      INTEGER NOT NULL DEFAULT 1,
    version     INTEGER NOT NULL DEFAULT 1,
    created_at  TEXT    NOT NULL,
    updated_at  TEXT    NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(user_id)
);

CREATE INDEX IF NOT EXISTS idx_team_charters_user_active
    ON team_charters (user_id, active);
