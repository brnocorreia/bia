-- $1: run id
SELECT run_id, seq, step_id, iteration_path, attempt, action, status,
       input, output, error, started_at, finished_at
FROM timeline_entries
WHERE run_id = $1
ORDER BY seq
