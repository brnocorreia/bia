-- $1: job id, $2: the Lease's attempt, $3: when the Lease now expires
UPDATE jobs
SET locked_until = $3
WHERE id = $1 AND attempts = $2 AND locked_until IS NOT NULL
