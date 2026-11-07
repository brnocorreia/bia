-- $1: client id, $2: name, $3: redirect URIs, $4: now
INSERT INTO oauth_clients (client_id, client_name, redirect_uris, created_at)
VALUES ($1, $2, $3, $4)
