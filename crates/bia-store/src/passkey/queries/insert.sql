-- $1: user id, $2: name, $3: credential, $4: now
INSERT INTO passkeys (user_id, name, credential, created_at)
VALUES ($1, $2, $3, $4)
RETURNING id, user_id, name, credential, created_at, last_used_at
