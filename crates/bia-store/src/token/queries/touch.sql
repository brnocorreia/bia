-- $1: token id, $2: now
UPDATE tokens SET last_used_at = $2 WHERE id = $1
