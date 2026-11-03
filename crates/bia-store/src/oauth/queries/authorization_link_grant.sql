-- $1: authorization id, $2: grant id
UPDATE oauth_authorizations SET grant_id = $2 WHERE id = $1
