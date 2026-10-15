-- Saved connections: reusable remote-server configurations.
--
-- A connection stores everything needed to reach a remote server (an FTP/FTPS
-- server today; other types can reuse the same table later). The per-type
-- settings live in `config` as JSON, interpreted according to
-- `connection_type`, so a new connection type adds no columns and no migration.
--
-- A storage path optionally *assigns* one saved connection. Paths with no
-- connection stay on local disk; a path with an FTP connection keeps `path` as
-- a *local staging* root and mirrors each of its buckets to the remote server
-- (in a per-bucket remote directory). Files are always written locally first,
-- then uploaded; `delete_local_after_upload` controls whether the staged copy
-- is removed once the upload succeeds.
CREATE TABLE IF NOT EXISTS connections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    connection_type TEXT NOT NULL,
    config TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);

ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS connection_id TEXT REFERENCES connections(id) ON DELETE SET NULL;

-- Migration 0027 stored the FTP settings inline on each path; they now live in
-- a reusable connection, so drop the superseded columns. Only
-- `delete_local_after_upload` stays: it is a property of the path's staging
-- behaviour, not of the server.
ALTER TABLE storage_paths DROP COLUMN IF EXISTS storage_type;
ALTER TABLE storage_paths DROP COLUMN IF EXISTS ftp_host;
ALTER TABLE storage_paths DROP COLUMN IF EXISTS ftp_port;
ALTER TABLE storage_paths DROP COLUMN IF EXISTS ftp_username;
ALTER TABLE storage_paths DROP COLUMN IF EXISTS ftp_password;
ALTER TABLE storage_paths DROP COLUMN IF EXISTS ftp_base_path;
ALTER TABLE storage_paths DROP COLUMN IF EXISTS ftp_secure;
ALTER TABLE storage_paths DROP COLUMN IF EXISTS ftp_passive;
ALTER TABLE storage_paths DROP COLUMN IF EXISTS ftp_verify_cert;
