-- $1: name, $2: hash, $3: scope, $4: workflow id, $5: trigger id, $6: user id, $7: now
INSERT INTO tokens (name, hash, scope, workflow_id, trigger_id, user_id, created_at)
VALUES ($1, $2, $3, $4, $5, $6, $7)
RETURNING id, name, scope, workflow_id, trigger_id, user_id, created_at, last_used_at, revoked_at
