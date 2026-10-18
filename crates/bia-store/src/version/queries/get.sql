-- $1: workflow id, $2: version number
SELECT workflow_id, number, definition, state, published_at, created_at
FROM versions
WHERE workflow_id = $1 AND number = $2
