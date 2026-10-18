-- Creates a pending Run pinned to a Published Version; inserts no row otherwise.
-- $1: workflow id, $2: version number, $3: input, $4: trigger, $5: Cursor, $6: state,
-- $7: creation time
INSERT INTO runs (
    workflow_id, version, status, input, trigger, cursor, state, created_at, updated_at
)
SELECT v.workflow_id, v.number, 'pending', $3, $4, $5, $6, $7, $7
FROM versions v
WHERE v.workflow_id = $1 AND v.number = $2 AND v.state = 'published'
RETURNING id, workflow_id, version, status, input, trigger,
          cursor AS "cursor: Json<Vec<Value>>", state, created_at, updated_at, finished_at
