-- Decided at most once, and only while pending.
-- $1: id, $2: now, $3: user id, $4: granted scope
UPDATE oauth_device_authorizations
SET decided_at = $2, outcome = 'approved', user_id = $3, granted_scope = $4
WHERE id = $1 AND decided_at IS NULL AND expires_at > $2
RETURNING id, client_id, client_name, client_host, provenance, requested_scope, resource, created_at, expires_at, interval_secs, user_id, granted_scope, grant_id
