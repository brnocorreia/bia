SELECT NOT EXISTS (SELECT 1 FROM users WHERE password_hash IS NOT NULL) AS "needs_setup!"
