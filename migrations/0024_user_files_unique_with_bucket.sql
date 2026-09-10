-- Allow the same (user, file, original_name) to exist in DIFFERENT buckets.
-- A backup restored into another bucket must show its files there even when
-- the user owns the same file+name elsewhere. Uniqueness is now per bucket:
-- a user still cannot have two copies of the same file+name inside ONE bucket.
DROP INDEX IF EXISTS idx_user_files_user_file;
CREATE UNIQUE INDEX idx_user_files_user_file
    ON user_files (user_id, file_id, original_name, COALESCE(bucket_name, ''));