-- $1: grant id, $2: hash, $3: now, $4: expires at
INSERT INTO grant_refresh_tokens (grant_id, hash, created_at, expires_at) VALUES ($1, $2, $3, $4)
