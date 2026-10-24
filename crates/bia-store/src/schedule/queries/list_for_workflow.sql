-- $1: workflow id
SELECT workflow_id, trigger_id, version, expr, tz, jitter, overlap, catch_up, next_at, due_at,
       queued, last_run_id
FROM schedules
WHERE workflow_id = $1
ORDER BY trigger_id
