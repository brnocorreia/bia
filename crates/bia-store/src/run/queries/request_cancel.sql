-- Flags an unfinished Run for cancellation; a Run flagged before keeps its first request time.
-- $1: run id, $2: request time
UPDATE runs
SET cancel_requested_at = COALESCE(cancel_requested_at, $2)
WHERE id = $1 AND finished_at IS NULL
RETURNING id
