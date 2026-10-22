-- Gives up a Lease so the Job is claimable at once, and wakes idle claimers. The Job keeps
-- its attempt count, which stays the fencing token for the released Lease.
-- $1: job id, $2: the Lease's attempt
WITH released AS (
    UPDATE jobs
    SET locked_by = NULL, locked_until = NULL
    WHERE id = $1 AND attempts = $2 AND locked_until IS NOT NULL
    RETURNING kind
)
SELECT released.kind
FROM released, pg_notify('bia_jobs', released.kind)
