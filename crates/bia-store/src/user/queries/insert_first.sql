-- Creates the first User; returns no row once one exists. Run under lock.sql, or two callers
-- could both see none.
-- $1: password hash, $2: now
INSERT INTO users (password_hash, created_at)
SELECT $1, $2
WHERE NOT EXISTS (SELECT 1 FROM users)
RETURNING id, created_at
