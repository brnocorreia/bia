-- $1: workflow id, $2: the Version the Schedules were built from
UPDATE workflows
SET scheduled_version = $2
WHERE id = $1
