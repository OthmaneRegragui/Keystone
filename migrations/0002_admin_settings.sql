CREATE TABLE IF NOT EXISTS admin_settings (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO admin_settings (key, value, updated_at) VALUES
    ('block_registrations', 'true', NOW()),
    ('default_bucket', 'default', NOW()),
    ('allow_multi_bucket', 'false', NOW())
ON CONFLICT (key) DO NOTHING;

CREATE TABLE IF NOT EXISTS buckets (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_buckets_name ON buckets(name);
