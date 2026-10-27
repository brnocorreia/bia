-- $1: name
SELECT name, max_retries, visibility_timeout_ms, batch_size, dead_letter, created_at, updated_at
FROM queues
WHERE name = $1
