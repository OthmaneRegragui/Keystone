-- Add group-level permissions for API keys and password changes
ALTER TABLE user_groups ADD COLUMN allow_api_keys BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE user_groups ADD COLUMN allow_password_change BOOLEAN NOT NULL DEFAULT FALSE;