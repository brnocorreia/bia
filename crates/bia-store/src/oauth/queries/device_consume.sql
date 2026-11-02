-- $1: id, $2: now
UPDATE oauth_device_authorizations SET consumed_at = $2
WHERE id = $1
RETURNING id, client_id, client_name, client_host, provenance, requested_scope, resource, created_at, expires_at, interval_secs, user_id, granted_scope, grant_id
