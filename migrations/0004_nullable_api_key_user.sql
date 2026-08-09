-- Make api_keys.user_id nullable for bot/programmatic keys
ALTER TABLE api_keys RENAME TO api_keys_old;

CREATE TABLE IF NOT EXISTS api_keys (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT REFERENCES users(id),
    name TEXT NOT NULL,
    key_prefix TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    scopes TEXT NOT NULL DEFAULT '[]',
    last_used_at TEXT,
    expires_at TEXT,
    created_at TEXT NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);

INSERT INTO api_keys (id, user_id, name, key_prefix, key_hash, scopes, last_used_at, expires_at, created_at, is_active)
SELECT id, user_id, name, key_prefix, key_hash, scopes, last_used_at, expires_at, created_at, is_active
FROM api_keys_old;

DROP TABLE api_keys_old;

CREATE INDEX IF NOT EXISTS idx_api_keys_user_id ON api_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_api_keys_key_hash ON api_keys(key_hash);
