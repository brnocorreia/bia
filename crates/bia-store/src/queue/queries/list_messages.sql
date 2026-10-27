-- A Queue's Messages, oldest first. Every filter but the Queue is optional.
-- $1: queue, $2: only dead letters (true) or only others (false), $3: after this id, $4: limit
SELECT id, queue, body, published_at, visible_at, deliveries, claimed_by, last_error,
       dead_lettered_at, dead_lettered_from
FROM messages
WHERE queue = $1
  AND ($2::boolean IS NULL OR (dead_lettered_at IS NOT NULL) = $2)
  AND ($3::bigint IS NULL OR id > $3)
ORDER BY id
LIMIT $4
