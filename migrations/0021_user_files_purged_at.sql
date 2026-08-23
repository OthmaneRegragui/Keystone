-- Tombstones for permanently-deleted files.
-- Permanent delete from the trash used to hard-delete the user_files row,
-- which erased name/bucket/owner metadata and made the physical file
-- unattributable in the admin orphans view. Instead we now keep the row as a
-- tombstone (deleted_at stays set, purged_at records when it was purged).
-- Tombstones are hidden from the trash listing/restore and excluded from the
-- unique index so the same file can be re-uploaded afterwards.
ALTER TABLE user_files ADD COLUMN IF NOT EXISTS purged_at TEXT;

DROP INDEX IF EXISTS idx_user_files_user_file;
CREATE UNIQUE INDEX idx_user_files_user_file
    ON user_files (user_id, file_id, original_name)
    WHERE purged_at IS NULL;
