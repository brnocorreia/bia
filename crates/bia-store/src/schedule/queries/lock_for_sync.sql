-- Locks the Workflow row, so its Active Version cannot change while its Schedules are
-- rebuilt.
-- $1: workflow id
SELECT active_version, scheduled_version
FROM workflows
WHERE id = $1
FOR UPDATE
