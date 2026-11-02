SELECT id, created_at, password_hash AS "password_hash!"
FROM users
WHERE password_hash IS NOT NULL
ORDER BY created_at, id
LIMIT 1
