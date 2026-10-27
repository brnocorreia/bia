-- $1: queue, $2: message ids
DELETE FROM messages
WHERE queue = $1 AND id = ANY($2) AND dead_lettered_at IS NOT NULL
