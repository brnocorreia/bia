-- How many Messages each Queue holds, by state. `ready`, `in_flight` and `delayed` split the
-- Messages that are not dead letters, so the four counts add up to the Queue's depth. A
-- Message whose delivery's visibility timeout ran out is ready: the next claim takes it.
-- $1: now, $2: only this Queue, or NULL for every Queue
SELECT queue AS "queue!",
       count(*) FILTER (WHERE dead_lettered_at IS NULL AND visible_at <= $1) AS "ready!",
       count(*) FILTER (WHERE dead_lettered_at IS NULL AND visible_at > $1
                          AND claimed_by IS NOT NULL) AS "in_flight!",
       count(*) FILTER (WHERE dead_lettered_at IS NULL AND visible_at > $1
                          AND claimed_by IS NULL) AS "delayed!",
       count(*) FILTER (WHERE dead_lettered_at IS NOT NULL) AS "dead_lettered!"
FROM messages
WHERE $2::text IS NULL OR queue = $2
GROUP BY queue
