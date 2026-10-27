-- $1: run id
DELETE FROM messages
WHERE run_id = $1
