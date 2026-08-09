-- Align Postgres column types with the Rust data model (i64).
--
-- On the sqlite -> postgres port, INTEGER columns stayed INTEGER (INT4), but
-- sqlx decodes them into i64 and strictly rejects INT4 -> i64, which made
-- every query reading these columns fail at runtime (e.g.
-- BucketRepository::list). Convert the columns the app models as i64 to
-- BIGINT. files.ref_count is intentionally left INTEGER: the app decodes it
-- as i32.

ALTER TABLE files ALTER COLUMN size TYPE BIGINT;
ALTER TABLE users ALTER COLUMN storage_quota TYPE BIGINT;
ALTER TABLE users ALTER COLUMN storage_used TYPE BIGINT;
ALTER TABLE buckets ALTER COLUMN storage_limit TYPE BIGINT;
ALTER TABLE group_buckets ALTER COLUMN user_storage_limit TYPE BIGINT;
