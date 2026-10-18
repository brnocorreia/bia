-- $1: job id, $2: the Lease's attempt
DELETE FROM jobs
WHERE id = $1 AND attempts = $2 AND locked_until IS NOT NULL
