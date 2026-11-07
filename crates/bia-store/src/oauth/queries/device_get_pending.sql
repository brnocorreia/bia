-- $1: id, $2: now
SELECT id, client_id, client_name, client_host, provenance, requested_scope, resource, created_at, expires_at, interval_secs, user_id, granted_scope, grant_id
FROM oauth_device_authorizations
WHERE id = $1 AND decided_at IS NULL AND expires_at > $2
