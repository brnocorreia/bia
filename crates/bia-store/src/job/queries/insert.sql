-- $1: kind, $2: run id, $3: payload, $4: when the Job becomes claimable
INSERT INTO jobs (kind, run_id, payload, run_at)
VALUES ($1, $2, $3, $4)
RETURNING id
