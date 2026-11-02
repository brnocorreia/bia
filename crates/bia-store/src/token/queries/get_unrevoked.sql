-- $1: hash
SELECT id, name, scope, origin, workflow_id, trigger_id, user_id, created_at, last_used_at, revoked_at, user_agent, grant_id, expires_at, audience
FROM tokens
WHERE hash = $1 AND revoked_at IS NULL
