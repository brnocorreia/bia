-- $1: message id, $2: the Lease's delivery
DELETE FROM messages
WHERE id = $1 AND deliveries = $2 AND claimed_by IS NOT NULL AND run_id IS NULL
