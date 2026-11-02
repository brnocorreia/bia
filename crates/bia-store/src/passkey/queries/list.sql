-- $1: user id
SELECT id, user_id, name, credential, created_at, last_used_at
FROM passkeys WHERE user_id = $1 ORDER BY created_at, id
