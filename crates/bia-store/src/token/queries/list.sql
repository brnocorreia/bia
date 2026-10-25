SELECT id, name, scope, workflow_id, trigger_id, created_at, last_used_at, revoked_at
FROM tokens
ORDER BY created_at, id
