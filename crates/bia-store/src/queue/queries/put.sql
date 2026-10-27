-- $1: name, $2: max retries, $3: visibility timeout in ms, $4: batch size, $5: dead letter,
-- $6: now
INSERT INTO queues (name, max_retries, visibility_timeout_ms, batch_size, dead_letter,
                    created_at, updated_at)
VALUES ($1, $2, $3, $4, $5, $6, $6)
ON CONFLICT (name) DO UPDATE
SET max_retries = EXCLUDED.max_retries,
    visibility_timeout_ms = EXCLUDED.visibility_timeout_ms,
    batch_size = EXCLUDED.batch_size,
    dead_letter = EXCLUDED.dead_letter,
    updated_at = EXCLUDED.updated_at
RETURNING name, max_retries, visibility_timeout_ms, batch_size, dead_letter, created_at,
          updated_at
