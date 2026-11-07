-- Closes authorization requests and codes that are still open, so none outlives the reset.
-- $1: now
UPDATE oauth_authorizations
SET decided_at = coalesce(decided_at, $1), code_used_at = coalesce(code_used_at, $1)
WHERE code_used_at IS NULL
