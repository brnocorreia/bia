-- Moves Messages that exhausted their retries to their Queue's Dead-Letter Queue as new rows,
-- so a Lease on the old row can never ack the dead letter; without a Dead-Letter Queue they
-- stay where they are and are never delivered again. The caller holds the rows' locks.
-- $1: message ids, $2: now, $3: error to record, or NULL to keep the last one
WITH dead AS (
    SELECT messages.id, queues.dead_letter
    FROM messages
    JOIN queues ON queues.name = messages.queue
    WHERE messages.id = ANY($1)
),
kept AS (
    UPDATE messages
    SET visible_at = NULL,
        claimed_by = NULL,
        run_id = NULL,
        last_error = coalesce($3, messages.last_error),
        dead_lettered_at = $2,
        dead_lettered_from = messages.queue
    FROM dead
    WHERE messages.id = dead.id AND dead.dead_letter IS NULL
    RETURNING messages.id
),
moved AS (
    DELETE FROM messages
    USING dead
    WHERE messages.id = dead.id AND dead.dead_letter IS NOT NULL
    RETURNING messages.id, messages.queue, messages.body, messages.published_at,
              messages.last_error, dead.dead_letter
)
INSERT INTO messages (queue, body, published_at, visible_at, last_error, dead_lettered_at,
                      dead_lettered_from)
SELECT dead_letter, body, published_at, $2, coalesce($3, last_error), $2, queue
FROM moved
ORDER BY id
