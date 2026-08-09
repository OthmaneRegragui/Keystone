-- Add user_files junction table for per-user file ownership.
-- Each user has their own logical view of files (with their own original_name and mime_type),
-- while the physical blob is shared via the files table (content-addressed deduplication).

CREATE TABLE user_files (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    file_id TEXT NOT NULL REFERENCES files(id),
    original_name TEXT NOT NULL,
    mime_type TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX idx_user_files_user_id ON user_files(user_id);
CREATE INDEX idx_user_files_file_id ON user_files(file_id);
CREATE UNIQUE INDEX idx_user_files_user_file ON user_files(user_id, file_id, original_name);
