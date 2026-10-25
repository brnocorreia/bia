-- Creates an admin Token unless an unrevoked one exists; returns no row then.
-- Run under lock_admins.sql, or two callers could both see none.
-- $1: name, $2: hash, $3: now
INSERT INTO tokens (name, hash, scope, created_at)
SELECT $1, $2, 'admin', $3
WHERE NOT EXISTS (SELECT 1 FROM tokens WHERE scope = 'admin' AND revoked_at IS NULL)
RETURNING id, name, scope, workflow_id, trigger_id, created_at, last_used_at, revoked_at
