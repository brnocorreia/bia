-- $1: id
SELECT id, user_id, client_id, client_name, client_host, provenance, scope, resource, created_at, last_used_at, revoked_at FROM grants WHERE id = $1
