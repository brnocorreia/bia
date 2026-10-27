-- Moves dead letters of a Queue back to the Queue they were dead-lettered from as new rows, so
-- they start their deliveries afresh and no Lease on the old row can touch them. Skips dead
-- letters a consumer holds right now and those whose source Queue no longer exists.
-- $1: queue, $2: message ids, $3: now
WITH chosen AS (
    SELECT messages.id
    FROM messages
    JOIN queues ON queues.name = messages.dead_lettered_from
    WHERE messages.queue = $1
      AND messages.id = ANY($2)
      AND messages.dead_lettered_at IS NOT NULL
      AND NOT (messages.claimed_by IS NOT NULL AND messages.visible_at > $3)
    -- Holding the source Queue's row keeps it from being deleted before the insert.
    FOR UPDATE OF messages FOR KEY SHARE OF queues
),
replayed AS (
    DELETE FROM messages
    USING chosen
    WHERE messages.id = chosen.id
    RETURNING messages.id, messages.dead_lettered_from, messages.body, messages.published_at,
              messages.last_error
)
INSERT INTO messages (queue, body, published_at, visible_at, last_error)
SELECT dead_lettered_from, body, published_at, $3, last_error
FROM replayed
ORDER BY id
RETURNING id
