-- Soft-delete support for folders: keep folder hierarchy in trash for restore.
ALTER TABLE user_folders ADD COLUMN deleted_at TEXT;
CREATE INDEX idx_user_folders_deleted ON user_folders(deleted_at);
