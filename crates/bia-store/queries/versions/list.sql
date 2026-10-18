-- $1: workflow id
SELECT workflow_id, number, definition, state, published_at, created_at
FROM versions
WHERE workflow_id = $1
ORDER BY number
