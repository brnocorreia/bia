-- Revoking twice keeps the first revocation time.
-- $1: token id, $2: now
UPDATE tokens SET revoked_at = coalesce(revoked_at, $2)
WHERE id = $1
RETURNING id, name, scope, workflow_id, trigger_id, created_at, last_used_at, revoked_at
