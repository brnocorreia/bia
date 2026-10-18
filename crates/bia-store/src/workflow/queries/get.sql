-- $1: workflow id
SELECT id, active_version, created_at
FROM workflows
WHERE id = $1
