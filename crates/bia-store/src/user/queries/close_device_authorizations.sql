-- Closes device authorizations that are still open, so none outlives the reset.
-- $1: now
UPDATE oauth_device_authorizations
SET decided_at = coalesce(decided_at, $1), outcome = coalesce(outcome, 'denied'), consumed_at = coalesce(consumed_at, $1)
WHERE consumed_at IS NULL
