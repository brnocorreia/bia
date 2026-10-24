-- $1: workflow id, $2: trigger id, $3: version, $4: expr, $5: tz, $6: jitter, $7: overlap,
-- $8: catch-up, $9: next occurrence, $10: when it fires, $11: queued occurrences,
-- $12: last run id
INSERT INTO schedules (
    workflow_id, trigger_id, version, expr, tz, jitter, overlap, catch_up, next_at, due_at,
    queued, last_run_id
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
ON CONFLICT (workflow_id, trigger_id) DO UPDATE
SET version = EXCLUDED.version,
    expr = EXCLUDED.expr,
    tz = EXCLUDED.tz,
    jitter = EXCLUDED.jitter,
    overlap = EXCLUDED.overlap,
    catch_up = EXCLUDED.catch_up,
    next_at = EXCLUDED.next_at,
    due_at = EXCLUDED.due_at,
    queued = EXCLUDED.queued,
    last_run_id = EXCLUDED.last_run_id
