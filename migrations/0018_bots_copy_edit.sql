-- Extend bot capabilities with copy and edit (rename/move) permissions.

ALTER TABLE bots ADD COLUMN can_copy BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE bots ADD COLUMN can_edit BOOLEAN NOT NULL DEFAULT FALSE;
