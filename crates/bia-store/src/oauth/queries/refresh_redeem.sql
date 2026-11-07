-- Spends a refresh token that was not used and has not run out.
-- $1: hash, $2: now
UPDATE grant_refresh_tokens SET used_at = $2
WHERE hash = $1 AND used_at IS NULL AND expires_at > $2
RETURNING grant_id
