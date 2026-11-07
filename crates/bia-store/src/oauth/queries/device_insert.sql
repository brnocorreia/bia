-- $1: client id, $2: client name, $3: client host, $4: provenance, $5: requested scope, $6: resource, $7: device code hash, $8: user code hash, $9: now, $10: expires at, $11: interval seconds
INSERT INTO oauth_device_authorizations (client_id, client_name, client_host, provenance, requested_scope, resource, device_code_hash, user_code_hash, created_at, expires_at, interval_secs)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
RETURNING id, client_id, client_name, client_host, provenance, requested_scope, resource, created_at, expires_at, interval_secs, user_id, granted_scope, grant_id
