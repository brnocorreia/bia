-- $1: run id
SELECT id, kind, run_id, payload, run_at, attempts, locked_by, locked_until, last_error
FROM jobs
WHERE run_id = $1
ORDER BY id
