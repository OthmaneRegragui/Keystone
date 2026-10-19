-- group_buckets.bucket_id had no foreign key to buckets(id), so deleting a
-- bucket left dangling grant rows behind. Drop any existing orphans and add
-- the constraint with ON DELETE CASCADE.
DELETE FROM group_buckets gb
WHERE NOT EXISTS (SELECT 1 FROM buckets b WHERE b.id = gb.bucket_id);

ALTER TABLE group_buckets
    ADD CONSTRAINT group_buckets_bucket_id_fkey
    FOREIGN KEY (bucket_id) REFERENCES buckets(id) ON DELETE CASCADE;
