-- Revoking twice keeps the first revocation time.
-- $1: id, $2: now
UPDATE grants SET revoked_at = coalesce(revoked_at, $2) WHERE id = $1
RETURNING id, user_id, client_id, client_name, client_host, provenance, scope, resource, created_at, last_used_at, revoked_at
