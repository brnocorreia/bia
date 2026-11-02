-- $1: id, $2: user id
DELETE FROM passkeys WHERE id = $1 AND user_id = $2
