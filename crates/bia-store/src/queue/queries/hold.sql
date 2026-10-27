-- Hands claimed Messages to the Run a queue Trigger started with them.
-- $1: message ids, $2: run id
UPDATE messages
SET run_id = $2
WHERE id = ANY($1)
