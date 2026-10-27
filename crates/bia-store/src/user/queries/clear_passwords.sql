UPDATE users SET password_hash = NULL
RETURNING id
