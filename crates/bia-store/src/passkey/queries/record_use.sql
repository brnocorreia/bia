-- $1: id, $2: credential after the sign-in, $3: now
UPDATE passkeys SET credential = $2, last_used_at = $3 WHERE id = $1
