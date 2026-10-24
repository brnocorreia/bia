-- Deletes up to $3 finished Runs, oldest first; their Timeline entries and Jobs go with them.
-- $1: statuses, $2: delete only Runs finished before this, $3: limit
DELETE FROM runs
WHERE id IN (
    SELECT id
    FROM runs
    WHERE status = ANY($1)
      AND finished_at IS NOT NULL
      AND finished_at < $2
    ORDER BY finished_at
    LIMIT $3
    FOR UPDATE SKIP LOCKED
)
