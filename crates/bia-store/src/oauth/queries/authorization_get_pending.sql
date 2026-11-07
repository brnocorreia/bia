-- An authorization nobody decided yet and that has not run out.
-- $1: id, $2: now
SELECT id, client_id, client_name, client_host, provenance, redirect_uri, requested_scope, resource, state, code_challenge, created_at, expires_at, user_id, granted_scope, code_expires_at, grant_id
FROM oauth_authorizations
WHERE id = $1 AND decided_at IS NULL AND expires_at > $2
