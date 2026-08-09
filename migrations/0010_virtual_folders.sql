-- Virtual folders for organizing files within buckets.
-- Folders are purely virtual (database-only); actual file storage remains content-addressed.
CREATE TABLE user_folders (
    id TEXT PRIMARY KEY NOT NULL,
    user_id TEXT NOT NULL REFERENCES users(id),
    bucket_name TEXT NOT NULL,
    parent_id TEXT REFERENCES user_folders(id) ON DELETE SET NULL,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(user_id, bucket_name, parent_id, name)
);

CREATE INDEX idx_user_folders_user_bucket ON user_folders(user_id, bucket_name);
CREATE INDEX idx_user_folders_parent ON user_folders(parent_id);

-- Link files to virtual folders. NULL = root of bucket.
ALTER TABLE user_files ADD COLUMN folder_id TEXT REFERENCES user_folders(id) ON DELETE SET NULL;
CREATE INDEX idx_user_files_folder ON user_files(folder_id);
