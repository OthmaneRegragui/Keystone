-- Emails were compared case-insensitively in the auth flow (login/register
-- lowercased the input) but stored/validated case-sensitively everywhere else,
-- so `Alice@Example.com` and `alice@example.com` could both exist and the
-- uppercase account could then never log in. Normalize the stored values and
-- enforce case-insensitive uniqueness.

-- Lowercase existing addresses that do not collide with another account after
-- normalization (colliding rows are left for an admin to resolve).
UPDATE users u
SET email = LOWER(u.email)
WHERE u.email <> LOWER(u.email)
  AND NOT EXISTS (
      SELECT 1 FROM users u2
      WHERE u2.id <> u.id AND LOWER(u2.email) = LOWER(u.email)
  );

-- Add the functional unique index only when no case-variant duplicates remain.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM (
            SELECT LOWER(email) AS e FROM users GROUP BY LOWER(email) HAVING COUNT(*) > 1
        ) dupes
    ) THEN
        CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email_lower ON users(LOWER(email));
    END IF;
END $$;
