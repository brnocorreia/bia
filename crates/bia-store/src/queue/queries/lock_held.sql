-- The Messages a Run holds, locked, oldest first.
-- $1: run id
SELECT messages.id, messages.deliveries, queues.max_retries
FROM messages
JOIN queues ON queues.name = messages.queue
WHERE messages.run_id = $1
ORDER BY messages.id
FOR UPDATE OF messages
