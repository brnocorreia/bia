-- Runs newest first. Every filter is optional.
-- $1: workflow id, $2: status, $3: list only Runs after this run id, $4: limit,
-- $5: created at or after, $6: created before
SELECT id, workflow_id, version, status, input, trigger,
       cursor AS "cursor: Json<Vec<Value>>", state, created_at, updated_at, finished_at,
       cancel_requested_at
FROM runs
WHERE ($1::text IS NULL OR workflow_id = $1)
  AND ($2::text IS NULL OR status = $2)
  AND ($3::uuid IS NULL OR (created_at, id) < (SELECT created_at, id FROM runs WHERE id = $3))
  AND ($5::timestamptz IS NULL OR created_at >= $5)
  AND ($6::timestamptz IS NULL OR created_at < $6)
ORDER BY created_at DESC, id DESC
LIMIT $4
