-- $1: client id, $2: client name, $3: client host, $4: provenance, $5: redirect URI, $6: requested scope, $7: resource, $8: state, $9: code challenge, $10: now, $11: expires at
INSERT INTO oauth_authorizations (client_id, client_name, client_host, provenance, redirect_uri, requested_scope, resource, state, code_challenge, created_at, expires_at)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
RETURNING id, client_id, client_name, client_host, provenance, redirect_uri, requested_scope, resource, state, code_challenge, created_at, expires_at, user_id, granted_scope, code_expires_at, grant_id
