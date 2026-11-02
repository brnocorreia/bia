-- $1: id, $2: user id, $3: name
UPDATE passkeys SET name = $3 WHERE id = $1 AND user_id = $2
RETURNING id, user_id, name, credential, created_at, last_used_at
