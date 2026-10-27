-- The delivery a Lease holds, locked; none once the Message was claimed again, acked or
-- dead-lettered.
-- $1: message id, $2: the Lease's delivery
SELECT messages.deliveries, queues.max_retries
FROM messages
JOIN queues ON queues.name = messages.queue
WHERE messages.id = $1 AND messages.deliveries = $2 AND messages.claimed_by IS NOT NULL
FOR UPDATE OF messages
