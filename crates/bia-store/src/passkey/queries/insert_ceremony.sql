-- $1: user id, $2: kind, $3: state, $4: expires at
INSERT INTO passkey_ceremonies (user_id, kind, state, expires_at)
VALUES ($1, $2, $3, $4)
RETURNING id
