-- Revokes the User's OAuth Grants, whose refresh tokens would otherwise outlive the reset.
-- $1: user id, $2: now
UPDATE grants SET revoked_at = $2
WHERE user_id = $1 AND revoked_at IS NULL
