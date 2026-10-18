-- $1: run id
SELECT id, workflow_id, version, status, input, trigger,
       cursor AS "cursor: Json<Vec<Value>>", state, created_at, updated_at, finished_at
FROM runs
WHERE id = $1
