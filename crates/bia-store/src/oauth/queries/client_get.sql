-- $1: client id
SELECT client_id, client_name, redirect_uris, created_at FROM oauth_clients WHERE client_id = $1
