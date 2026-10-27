-- Queue Triggers of Active Versions whose Queue has a Message to claim at $1, with the batch
-- size each dispatches: its own, or else its Queue's.
-- $1: now
SELECT t.workflow_id, t.trigger_id, t.version, t.queue, t.batch_size,
       coalesce(t.batch_size, q.batch_size) AS "effective_batch_size!"
FROM queue_triggers t
JOIN workflows w ON w.id = t.workflow_id AND w.active_version = t.version
JOIN queues q ON q.name = t.queue
WHERE EXISTS (
    SELECT 1
    FROM messages m
    WHERE m.queue = t.queue AND m.visible_at <= $1 AND m.run_id IS NULL
)
ORDER BY t.workflow_id, t.trigger_id
