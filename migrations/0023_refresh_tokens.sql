-- Persisted refresh tokens. Replaces the in-memory session store so sessions
-- survive restarts and are shared across instances behind a load balancer.
-- Tokens are stored as SHA-256 hashes (the raw value is only ever shown to the
-- client once), and rotation/revocation is shared, not per-process.

CREATE TABLE refresh_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX idx_refresh_tokens_user ON refresh_tokens(user_id);
CREATE INDEX idx_refresh_tokens_expires ON refresh_tokens(expires_at);