-- $1: run id, $2: status, $3: Cursor, $4: state, $5: transition time,
-- $6: finish time (NULL unless the status is finished)
UPDATE runs
SET status = $2, cursor = $3, state = $4, updated_at = $5, finished_at = $6
WHERE id = $1
