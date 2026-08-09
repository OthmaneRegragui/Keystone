-- Add soft-delete support: files are hidden from users but not physically removed
ALTER TABLE user_files ADD COLUMN deleted_at TEXT;
CREATE INDEX idx_user_files_deleted ON user_files(deleted_at);
