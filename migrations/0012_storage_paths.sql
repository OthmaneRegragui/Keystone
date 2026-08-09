-- Storage paths: named filesystem locations admins assign to buckets
CREATE TABLE storage_paths (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL,
    created_at TEXT NOT NULL
);
