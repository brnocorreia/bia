-- $1: job id
SELECT id, kind, run_id, payload, run_at, attempts, max_attempts, locked_by, locked_until,
       last_error, failed_at
FROM jobs
WHERE id = $1
