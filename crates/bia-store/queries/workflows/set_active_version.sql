-- Points the Active Version at a Published Version; updates no row otherwise.
-- $1: workflow id, $2: version number
UPDATE workflows SET active_version = v.number
FROM versions v
WHERE workflows.id = $1
  AND v.workflow_id = workflows.id
  AND v.number = $2
  AND v.state = 'published'
