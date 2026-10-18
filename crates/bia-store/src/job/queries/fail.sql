-- $1: job id, $2: the Lease's attempt, $3: error, $4: when to retry, $5: now
UPDATE jobs
SET locked_by = NULL,
    locked_until = NULL,
    last_error = $3,
    run_at = CASE WHEN attempts < max_attempts THEN $4 ELSE run_at END,
    failed_at = CASE WHEN attempts < max_attempts THEN NULL ELSE $5::timestamptz END
WHERE id = $1 AND attempts = $2 AND locked_until IS NOT NULL
RETURNING run_at, failed_at
