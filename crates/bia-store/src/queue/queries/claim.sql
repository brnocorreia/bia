-- $1: queue, $2: consumer, $3: now, $4: how many to claim, $5: when the deliveries' visibility
-- timeout ends, $6: max retries
-- SKIP LOCKED leaves Messages another consumer is taking right now to that consumer, so no
-- Message is delivered to two consumers at once.
WITH due AS MATERIALIZED (
    SELECT id
    FROM messages
    WHERE queue = $1
      AND visible_at <= $3
      AND (claimed_by IS NULL OR deliveries <= $6)
    ORDER BY visible_at, id
    LIMIT $4
    FOR UPDATE SKIP LOCKED
)
UPDATE messages
SET deliveries = deliveries + 1, claimed_by = $2, visible_at = $5
FROM due
WHERE messages.id = due.id
RETURNING messages.id, queue, body, published_at, visible_at, deliveries, claimed_by,
          last_error, dead_lettered_at, dead_lettered_from
