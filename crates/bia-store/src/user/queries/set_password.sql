-- $1: user id, $2: password hash
UPDATE users SET password_hash = $2
WHERE id = $1 AND password_hash IS NOT NULL
