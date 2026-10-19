-- Repair and prevent duplicate physical rows for the same content.
--
-- Uploads used a check-then-insert (`find_by_hash` -> `INSERT`) guarded only by
-- the non-unique index `idx_files_blake3_hash`. Two concurrent uploads of
-- identical content could therefore both insert a `files` row (each with
-- ref_count = 1) pointing at the same content-addressed blob. Deleting one of
-- them then let the GC worker remove the shared blob and destroy the other
-- user's file.
--
-- This migration (1) collapses any existing duplicates onto the oldest row per
-- hash and (2) makes `blake3_hash` and `(file_id, backend)` unique so it can
-- never happen again.

-- 1a. Point every user_files link at the canonical (oldest) physical row.
WITH canonical AS (
    SELECT DISTINCT ON (blake3_hash) blake3_hash, id AS keep_id
    FROM files
    ORDER BY blake3_hash, created_at ASC, id ASC
)
UPDATE user_files uf
SET file_id = c.keep_id
FROM files f
JOIN canonical c ON c.blake3_hash = f.blake3_hash
WHERE uf.file_id = f.id AND f.id <> c.keep_id;

-- 1b. Repoint storage objects to the canonical row where the backend slot is
--     still free, then drop the leftovers.
WITH canonical AS (
    SELECT DISTINCT ON (blake3_hash) blake3_hash, id AS keep_id
    FROM files
    ORDER BY blake3_hash, created_at ASC, id ASC
)
UPDATE storage_objects so
SET file_id = c.keep_id
FROM files f
JOIN canonical c ON c.blake3_hash = f.blake3_hash
WHERE so.file_id = f.id
  AND f.id <> c.keep_id
  AND NOT EXISTS (
      SELECT 1 FROM storage_objects other
      WHERE other.file_id = c.keep_id AND other.backend = so.backend
  );

WITH canonical AS (
    SELECT DISTINCT ON (blake3_hash) blake3_hash, id AS keep_id
    FROM files
    ORDER BY blake3_hash, created_at ASC, id ASC
)
DELETE FROM storage_objects so
USING files f
JOIN canonical c ON c.blake3_hash = f.blake3_hash
WHERE so.file_id = f.id AND f.id <> c.keep_id;

-- 1c. Delete the duplicate physical rows.
WITH canonical AS (
    SELECT DISTINCT ON (blake3_hash) blake3_hash, id AS keep_id
    FROM files
    ORDER BY blake3_hash, created_at ASC, id ASC
)
DELETE FROM files f
USING canonical c
WHERE f.blake3_hash = c.blake3_hash AND f.id <> c.keep_id;

-- 1d. Recompute ref_count as the number of live links, fixing both the merged
--     rows and any pre-existing drift.
UPDATE files f
SET ref_count = COALESCE(sub.n, 0)
FROM (
    SELECT f2.id AS id, COUNT(uf.id) AS n
    FROM files f2
    LEFT JOIN user_files uf
      ON uf.file_id = f2.id AND uf.deleted_at IS NULL AND uf.purged_at IS NULL
    GROUP BY f2.id
) sub
WHERE sub.id = f.id;

-- 1e. Collapse duplicate storage_objects rows for the same (file_id, backend),
--     keeping the earliest.
DELETE FROM storage_objects
WHERE id IN (
    SELECT id FROM (
        SELECT id,
               ROW_NUMBER() OVER (
                   PARTITION BY file_id, backend ORDER BY created_at ASC, id ASC
               ) AS rn
        FROM storage_objects
    ) ranked
    WHERE rn > 1
);

-- 2. Enforce uniqueness going forward. The unique index supersedes the old
--    non-unique lookup index on blake3_hash.
DROP INDEX IF EXISTS idx_files_blake3_hash;
CREATE UNIQUE INDEX IF NOT EXISTS idx_files_blake3_hash_unique ON files(blake3_hash);
CREATE UNIQUE INDEX IF NOT EXISTS idx_storage_objects_file_backend
    ON storage_objects(file_id, backend);
