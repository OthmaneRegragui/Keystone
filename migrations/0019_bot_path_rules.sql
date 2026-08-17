-- Replace the bot's bucket/folder/file restriction lists with a single list
-- of path rules: JSON array of {bucket, path, status(allow|block)}.
--
-- Semantics (evaluated per bucket, most-restrictive wins):
--   - A bucket with no rules is fully accessible (like NULL before).
--   - "" (empty path) in a rule applies to the whole bucket.
--   - An allow rule opens a path (and everything beneath it); a block rule
--     denies a path and everything beneath it; block beats allow.
--   - A bucket that has rules is restricted: a path is allowed only when an
--     allow rule covers it and no block rule covers it.
--
-- NULL = unrestricted (default for existing bots). An empty array = every
-- bucket the owner can reach (no rules for any bucket).

ALTER TABLE bots DROP COLUMN allowed_buckets;
ALTER TABLE bots DROP COLUMN allowed_folder_ids;
ALTER TABLE bots DROP COLUMN allowed_file_ids;
ALTER TABLE bots ADD COLUMN path_rules TEXT;
