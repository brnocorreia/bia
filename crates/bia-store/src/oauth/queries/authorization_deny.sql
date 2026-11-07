-- $1: id, $2: now
UPDATE oauth_authorizations SET decided_at = $2
WHERE id = $1 AND decided_at IS NULL AND expires_at > $2
RETURNING id, client_id, client_name, client_host, provenance, redirect_uri, requested_scope, resource, state, code_challenge, created_at, expires_at, user_id, granted_scope, code_expires_at, grant_id
