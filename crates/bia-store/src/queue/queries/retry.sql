-- $1: message id, $2: when it is claimable again, $3: error
UPDATE messages
SET claimed_by = NULL, visible_at = $2, last_error = $3
WHERE id = $1
