-- $1: worker, $2: when the Lease expires, $3: now, $4: how many Jobs to claim
-- SKIP LOCKED leaves Jobs another claimer is taking right now to that claimer.
WITH due AS MATERIALIZED (
    SELECT id
    FROM jobs
    WHERE failed_at IS NULL
      AND run_at <= $3
      AND (locked_until IS NULL OR locked_until <= $3)
    ORDER BY run_at, id
    LIMIT $4
    FOR UPDATE SKIP LOCKED
)
UPDATE jobs
SET attempts = attempts + 1, locked_by = $1, locked_until = $2
FROM due
WHERE jobs.id = due.id
RETURNING jobs.id, kind, run_id, payload, run_at, attempts, max_attempts, locked_by,
          locked_until, last_error, failed_at
