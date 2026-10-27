-- $1: workflow id
DELETE FROM queue_triggers
WHERE workflow_id = $1
