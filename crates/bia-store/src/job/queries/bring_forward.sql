-- Makes a Run's waiting Jobs claimable from $2, e.g. a sleeping Run's wake-up once it is
-- cancelled. Jobs under a Lease and Jobs that gave up are left alone.
-- $1: run id, $2: when the Jobs become claimable
UPDATE jobs
SET run_at = $2
WHERE run_id = $1 AND failed_at IS NULL AND locked_until IS NULL AND run_at > $2
