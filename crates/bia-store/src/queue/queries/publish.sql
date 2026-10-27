-- $1: queue, $2: bodies, $3: now, $4: when the Messages become claimable
INSERT INTO messages (queue, body, published_at, visible_at)
SELECT $1, body, $3, $4
FROM unnest($2::jsonb[]) WITH ORDINALITY AS bodies (body, position)
ORDER BY position
RETURNING id
