-- Backfill bucket_id from buckets table
ALTER TABLE group_buckets ADD COLUMN bucket_id TEXT DEFAULT '';
UPDATE group_buckets SET bucket_id = (SELECT id FROM buckets WHERE name = group_buckets.bucket_name);

-- Recreate group_buckets with bucket_id as primary key (replacing bucket_name)
CREATE TABLE group_buckets_new (
    group_id TEXT NOT NULL REFERENCES user_groups(id) ON DELETE CASCADE,
    bucket_id TEXT NOT NULL,
    can_upload BOOLEAN NOT NULL DEFAULT TRUE,
    can_download BOOLEAN NOT NULL DEFAULT TRUE,
    user_storage_limit INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (group_id, bucket_id)
);
INSERT INTO group_buckets_new (group_id, bucket_id, can_upload, can_download, user_storage_limit)
    SELECT group_id, bucket_id, can_upload, can_download, user_storage_limit FROM group_buckets;
DROP TABLE group_buckets;
ALTER TABLE group_buckets_new RENAME TO group_buckets;
CREATE INDEX IF NOT EXISTS idx_group_buckets_group ON group_buckets(group_id);
