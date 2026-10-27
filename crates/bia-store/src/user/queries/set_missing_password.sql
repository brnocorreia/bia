-- Gives the User a password if `bia serve --reset-admin` cleared it. Run under lock.sql.
-- $1: password hash
UPDATE users SET password_hash = $1
WHERE password_hash IS NULL
RETURNING id, created_at
