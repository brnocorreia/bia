-- Revokes the User's Sessions but the one in $2, if any; already revoked ones keep their time.
-- $1: user id, $2: the Token to keep, $3: now
UPDATE tokens SET revoked_at = $3
WHERE user_id = $1 AND origin = 'session' AND revoked_at IS NULL AND id IS DISTINCT FROM $2
