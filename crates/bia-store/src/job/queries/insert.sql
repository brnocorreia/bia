-- $1: kind, $2: run id, $3: payload, $4: when the Job becomes claimable, $5: max attempts
INSERT INTO jobs (kind, run_id, payload, run_at, max_attempts)
VALUES ($1, $2, $3, $4, $5)
RETURNING id
