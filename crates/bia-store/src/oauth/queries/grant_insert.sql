-- $1: user id, $2: client id, $3: client name, $4: client host, $5: provenance, $6: scope, $7: resource, $8: now
INSERT INTO grants (user_id, client_id, client_name, client_host, provenance, scope, resource, created_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
RETURNING id, user_id, client_id, client_name, client_host, provenance, scope, resource, created_at, last_used_at, revoked_at
