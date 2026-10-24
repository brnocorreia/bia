-- $1: workflow id, $2: trigger id
DELETE FROM schedules
WHERE workflow_id = $1 AND trigger_id = $2
