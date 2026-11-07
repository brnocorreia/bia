-- $1: id, $2: now, $3: interval seconds
UPDATE oauth_device_authorizations SET last_polled_at = $2, interval_secs = $3 WHERE id = $1
