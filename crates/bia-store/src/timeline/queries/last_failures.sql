-- The last failed entry of each of the Runs.
-- $1: run ids
SELECT DISTINCT ON (run_id) run_id, seq, step_id, iteration_path, attempt, action, status,
       input, output, error, started_at, finished_at
FROM timeline_entries
WHERE run_id = ANY($1) AND status = 'failed'
ORDER BY run_id, seq DESC
