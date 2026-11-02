-- Deletes the ceremony and returns it when it is the User's, of this kind, and not expired.
-- $1: id, $2: user id, $3: kind, $4: now
DELETE FROM passkey_ceremonies
WHERE id = $1 AND user_id = $2 AND kind = $3 AND expires_at > $4
RETURNING state
