-- Locks the row so two polls of one device code are answered one after the other.
-- $1: device code hash
SELECT id, expires_at, interval_secs, last_polled_at, outcome, consumed_at, grant_id
FROM oauth_device_authorizations
WHERE device_code_hash = $1
FOR UPDATE
