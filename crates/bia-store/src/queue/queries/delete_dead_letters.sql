-- Deletes up to $2 dead letters, oldest first.
-- $1: delete only those dead-lettered before this, $2: limit
DELETE FROM messages
WHERE id IN (
    SELECT id
    FROM messages
    WHERE dead_lettered_at < $1
    ORDER BY dead_lettered_at
    LIMIT $2
    FOR UPDATE SKIP LOCKED
)
