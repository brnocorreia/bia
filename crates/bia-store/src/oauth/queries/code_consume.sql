-- Spends an approved code that has not been used and has not run out.
-- $1: code hash, $2: now
UPDATE oauth_authorizations SET code_used_at = $2
WHERE code_hash = $1 AND code_used_at IS NULL AND code_expires_at > $2
RETURNING id, client_id, client_name, client_host, provenance, redirect_uri, requested_scope, resource, state, code_challenge, created_at, expires_at, user_id, granted_scope, code_expires_at, grant_id
