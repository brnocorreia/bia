-- $1: name, $2: hash, $3: scope, $4: workflow id, $5: trigger id, $6: user id, $7: now, $8: origin, $9: user agent, $10: grant id, $11: expires at, $12: audience
INSERT INTO tokens (name, hash, scope, workflow_id, trigger_id, user_id, created_at, origin, user_agent, grant_id, expires_at, audience)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
RETURNING id, name, scope, origin, workflow_id, trigger_id, user_id, created_at, last_used_at, revoked_at, user_agent, grant_id, expires_at, audience
