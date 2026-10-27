-- $1: message id, $2: the Lease's delivery, $3: when the visibility timeout now ends
UPDATE messages
SET visible_at = $3
WHERE id = $1 AND deliveries = $2 AND claimed_by IS NOT NULL
