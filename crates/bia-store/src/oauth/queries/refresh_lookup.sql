-- $1: hash
SELECT grant_id, used_at FROM grant_refresh_tokens WHERE hash = $1
