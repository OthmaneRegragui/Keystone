-- FTP/FTPS-backed buckets.
--
-- A bucket is "local" by default: `path` is its physical storage root. An
-- "ftp" bucket instead uses `path` as a *local staging* directory and mirrors
-- every object to a remote FTP/FTPS server. Files are always written locally
-- first, then uploaded; `delete_local_after_upload` controls whether the local
-- copy is removed once the upload succeeds.
--
-- All defaults preserve the pre-existing behaviour: existing rows become
-- `local`, no FTP connection is ever attempted for them, and every boolean
-- mirrors the conservative default (passive transfers on, TLS certificates
-- verified, local copies kept).
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS storage_type TEXT NOT NULL DEFAULT 'local';
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS ftp_host TEXT;
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS ftp_port INTEGER NOT NULL DEFAULT 21;
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS ftp_username TEXT;
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS ftp_password TEXT;
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS ftp_base_path TEXT NOT NULL DEFAULT '';
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS ftp_secure BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS ftp_passive BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS ftp_verify_cert BOOLEAN NOT NULL DEFAULT TRUE;
ALTER TABLE buckets ADD COLUMN IF NOT EXISTS delete_local_after_upload BOOLEAN NOT NULL DEFAULT FALSE;
