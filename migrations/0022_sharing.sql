-- Sharing: allow users to share files/folders with other users by email.
-- A share is a directed link from a sharer to a recipient for a specific file or folder.
-- Recipients can view (and download if bucket permits) the shared item but cannot reshare it.

CREATE TABLE shared_items (
    id TEXT PRIMARY KEY,
    shared_by_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    shared_with_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    item_type TEXT NOT NULL CHECK (item_type IN ('file', 'folder')),
    item_id TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE(shared_by_user_id, shared_with_user_id, item_type, item_id)
);

CREATE INDEX idx_shared_items_recipient ON shared_items(shared_with_user_id);
CREATE INDEX idx_shared_items_sharer ON shared_items(shared_by_user_id);
CREATE INDEX idx_shared_items_item ON shared_items(item_type, item_id);

-- Per-group sharing permission (like allow_api_keys, allow_bots, etc.)
ALTER TABLE user_groups ADD COLUMN allow_sharing BOOLEAN NOT NULL DEFAULT FALSE;

-- Global sharing toggle (admin setting, defaults to true)
INSERT INTO admin_settings (key, value, updated_at)
VALUES ('allow_user_sharing', 'true', NOW())
ON CONFLICT (key) DO NOTHING;
