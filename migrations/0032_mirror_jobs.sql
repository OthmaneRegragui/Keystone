-- Admin-managed scheduled mirror jobs ("robots").
--
-- A job copies every active file a user owns in `source_bucket` into
-- `target_bucket`, preserving the per-user folder hierarchy. Each target link
-- keeps its own physical blob copy in the target bucket's backend, so the
-- target behaves like a RAID-1 replica rather than a logical alias.
CREATE TABLE IF NOT EXISTS mirror_jobs (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    job_type TEXT NOT NULL DEFAULT 'mirror_bucket',
    source_bucket TEXT NOT NULL REFERENCES buckets(name) ON UPDATE CASCADE ON DELETE CASCADE,
    target_bucket TEXT NOT NULL REFERENCES buckets(name) ON UPDATE CASCADE ON DELETE CASCADE,
    -- Daily UTC start time in HH:MM (24-hour) form.
    schedule_time TEXT NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    -- When true, target links created by this job are removed once their
    -- source file no longer exists.
    prune BOOLEAN NOT NULL DEFAULT FALSE,
    running BOOLEAN NOT NULL DEFAULT FALSE,
    running_since TEXT,
    last_run_at TEXT,
    last_finished_at TEXT,
    next_run_at TEXT,
    last_status TEXT,
    last_error TEXT,
    last_files_copied INTEGER NOT NULL DEFAULT 0,
    last_files_pruned INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CONSTRAINT mirror_jobs_distinct_buckets CHECK (source_bucket <> target_bucket)
);

CREATE INDEX IF NOT EXISTS idx_mirror_jobs_due
    ON mirror_jobs (next_run_at)
    WHERE NOT running;

-- Provenance for links/folders created by a mirror job. Pruning only ever
-- touches rows tagged with the job that created them.
ALTER TABLE user_files
    ADD COLUMN IF NOT EXISTS mirror_job_id TEXT REFERENCES mirror_jobs(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_user_files_mirror_job_id
    ON user_files (mirror_job_id)
    WHERE mirror_job_id IS NOT NULL;

ALTER TABLE user_folders
    ADD COLUMN IF NOT EXISTS mirror_job_id TEXT REFERENCES mirror_jobs(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_user_folders_mirror_job_id
    ON user_folders (mirror_job_id)
    WHERE mirror_job_id IS NOT NULL;
