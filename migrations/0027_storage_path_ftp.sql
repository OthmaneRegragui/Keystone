-- FTP/FTPS is a property of a *storage path*, not of a bucket.
--
-- A storage path is "local" by default: `path` is a physical root and buckets
-- created under it live on disk. A "ftp" storage path keeps `path` as a *local
-- staging* root and mirrors every object of every bucket created under it to a
-- remote FTP/FTPS server. Files are always written locally first, then
-- uploaded; `delete_local_after_upload` controls whether the staged copy is
-- removed once the upload succeeds.
--
-- All defaults preserve the pre-existing behaviour: existing paths become
-- `local`, no FTP connection is ever attempted for them, and every boolean
-- mirrors the conservative default (passive transfers on, TLS certificates
-- verified, local copies kept).
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS storage_type TEXT NOT NULL DEFAULT 'local';
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS ftp_host TEXT;
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS ftp_port INTEGER NOT NULL DEFAULT 21;
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS ftp_username TEXT;
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS ftp_password TEXT;
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS ftp_base_path TEXT NOT NULL DEFAULT '';
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS ftp_secure BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS ftp_passive BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS ftp_verify_cert BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE storage_paths ADD COLUMN IF NOT EXISTS delete_local_after_upload BOOLEAN NOT NULL DEFAULT FALSE;

-- Migration 0026 briefly modelled this on buckets; FTP now lives on the owning
-- storage path, so drop the (never-populated) bucket-level columns.
ALTER TABLE buckets DROP COLUMN IF EXISTS storage_type;
ALTER TABLE buckets DROP COLUMN IF EXISTS ftp_host;
ALTER TABLE buckets DROP COLUMN IF EXISTS ftp_port;
ALTER TABLE buckets DROP COLUMN IF EXISTS ftp_username;
ALTER TABLE buckets DROP COLUMN IF EXISTS ftp_password;
ALTER TABLE buckets DROP COLUMN IF EXISTS ftp_base_path;
ALTER TABLE buckets DROP COLUMN IF EXISTS ftp_secure;
ALTER TABLE buckets DROP COLUMN IF EXISTS ftp_passive;
ALTER TABLE buckets DROP COLUMN IF EXISTS ftp_verify_cert;
ALTER TABLE buckets DROP COLUMN IF EXISTS delete_local_after_upload;
