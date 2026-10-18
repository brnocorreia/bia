-- $1: workflow id, $2: version number
SELECT state
FROM versions
WHERE workflow_id = $1 AND number = $2
