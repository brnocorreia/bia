-- The Grant an access token or a refresh token belongs to.
-- $1: hash
SELECT grant_id AS "grant_id!"
FROM (
    SELECT hash, grant_id FROM tokens WHERE grant_id IS NOT NULL
    UNION ALL
    SELECT hash, grant_id FROM grant_refresh_tokens
) credentials
WHERE hash = $1
LIMIT 1
