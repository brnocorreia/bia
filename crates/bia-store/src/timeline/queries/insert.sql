-- $1: run id, $2: seq, $3: step id, $4: iteration path, $5: attempt, $6: action,
-- $7: status, $8: input, $9: output, $10: error, $11: start time, $12: finish time
INSERT INTO timeline_entries (
    run_id, seq, step_id, iteration_path, attempt, action, status,
    input, output, error, started_at, finished_at
)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
