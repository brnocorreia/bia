-- One Schedule with something to fire at $1: an occurrence that fell due, or a queued one
-- whose previous Run finished. Only Schedules of the current Active Version qualify; sharing
-- the Workflow row lock means a Version that stops being active cannot fire afterwards.
-- Schedules another scheduler is firing are skipped.
-- $1: now
SELECT s.workflow_id, s.trigger_id, s.version, s.expr, s.tz, s.jitter, s.overlap, s.catch_up,
       s.next_at, s.due_at, s.queued, s.last_run_id,
       (r.id IS NOT NULL AND r.finished_at IS NULL) AS "previous_active!"
FROM schedules s
JOIN workflows w ON w.id = s.workflow_id AND w.active_version = s.version
LEFT JOIN runs r ON r.id = s.last_run_id
WHERE s.due_at <= $1
   OR (cardinality(s.queued) > 0 AND (r.id IS NULL OR r.finished_at IS NOT NULL))
ORDER BY s.due_at, s.workflow_id, s.trigger_id
LIMIT 1
FOR UPDATE OF s SKIP LOCKED
FOR SHARE OF w
