-- $1: now
DELETE FROM passkey_ceremonies WHERE expires_at <= $1
