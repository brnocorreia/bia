-- Deletes the Workflow's Schedules for Triggers not in $2.
-- $1: workflow id, $2: trigger ids to keep
DELETE FROM schedules
WHERE workflow_id = $1 AND NOT (trigger_id = ANY ($2))
