-- $1: grant id, $2: now
UPDATE grants SET last_used_at = $2 WHERE id = $1
