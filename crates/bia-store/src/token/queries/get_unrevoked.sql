-- $1: hash
SELECT id, name, scope, workflow_id, trigger_id, created_at, last_used_at, revoked_at
FROM tokens
WHERE hash = $1 AND revoked_at IS NULL
