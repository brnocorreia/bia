-- Puts a failed Run back to work from the Checkpoint given; changes nothing for any other Run.
-- $1: run id, $2: Cursor, $3: state, $4: resume time
UPDATE runs
SET status = 'running', cursor = $2, state = $3, updated_at = $4, finished_at = NULL
WHERE id = $1 AND status = 'failed'
RETURNING id, workflow_id, version, status, input, trigger,
          cursor AS "cursor: Json<Vec<Value>>", state, created_at, updated_at, finished_at,
          cancel_requested_at
