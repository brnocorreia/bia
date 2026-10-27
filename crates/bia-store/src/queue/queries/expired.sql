-- Messages whose last delivery's visibility timeout ran out and that have no retries left. A
-- Message held by a Run has no timeout.
-- $1: queue, $2: now, $3: max retries
SELECT id
FROM messages
WHERE queue = $1 AND claimed_by IS NOT NULL AND run_id IS NULL AND visible_at <= $2
  AND deliveries > $3
ORDER BY visible_at, id
LIMIT 500
FOR UPDATE SKIP LOCKED
