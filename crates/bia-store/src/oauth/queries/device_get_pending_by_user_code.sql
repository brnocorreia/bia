-- The device authorization a user code names, if nobody decided it yet and it has not run out.
-- $1: user code hash, $2: now
SELECT id, client_id, client_name, client_host, provenance, requested_scope, resource, created_at, expires_at, interval_secs, user_id, granted_scope, grant_id
FROM oauth_device_authorizations
WHERE user_code_hash = $1 AND decided_at IS NULL AND expires_at > $2
