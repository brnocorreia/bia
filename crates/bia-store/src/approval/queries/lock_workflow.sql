-- Serialises requests per Workflow. NO KEY UPDATE, so Versions can still be inserted.
-- $1: workflow id
SELECT id FROM workflows WHERE id = $1 FOR NO KEY UPDATE
