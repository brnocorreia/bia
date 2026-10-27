-- Revokes every Token the User still has; already revoked ones keep their time.
-- $1: user id, $2: now
UPDATE tokens SET revoked_at = $2
WHERE user_id = $1 AND revoked_at IS NULL
