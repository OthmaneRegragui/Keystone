-- Bots: user-scoped API keys with granular access restrictions.

-- Group-level permission: allow members to create bots (mirrors allow_api_keys).
ALTER TABLE user_groups ADD COLUMN allow_bots BOOLEAN NOT NULL DEFAULT FALSE;

CREATE TABLE IF NOT EXISTS bots (
    id TEXT PRIMARY KEY NOT NULL,
    -- Owner of the bot. The bot authenticates as this user and uses the user's
    -- storage/quota — it never gets its own bucket storage.
    user_id TEXT NOT NULL REFERENCES users(id),
    -- The underlying API key. user_id on that key is the same owner, so the
    -- key resolves to the owner's identity; this row carries the restrictions.
    key_id TEXT NOT NULL UNIQUE REFERENCES api_keys(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    can_upload BOOLEAN NOT NULL DEFAULT FALSE,
    can_download BOOLEAN NOT NULL DEFAULT FALSE,
    can_delete BOOLEAN NOT NULL DEFAULT FALSE,
    can_list BOOLEAN NOT NULL DEFAULT TRUE,
    -- JSON array of bucket names the bot may access; NULL = all buckets the
    -- owner has access to. An empty array = access to nothing (fail closed).
    allowed_buckets TEXT,
    -- JSON array of folder ids (user_folders) the bot may access; NULL = all.
    allowed_folder_ids TEXT,
    -- JSON array of user_file ids the bot may access; NULL = all.
    allowed_file_ids TEXT,
    -- 0 = unlimited. Lifetime cap on the total bytes the bot has uploaded
    -- (uploaded_bytes is never decreased, so deletes do not free it).
    upload_limit_bytes BIGINT NOT NULL DEFAULT 0,
    uploaded_bytes BIGINT NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_bots_user_id ON bots(user_id);
CREATE INDEX IF NOT EXISTS idx_bots_key_id ON bots(key_id);
