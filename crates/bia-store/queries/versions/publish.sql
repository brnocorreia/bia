-- Publishes a Draft; updates no row for a Published or missing Version.
-- $1: workflow id, $2: version number, $3: publish time
UPDATE versions SET state = 'published', published_at = $3
WHERE workflow_id = $1 AND number = $2 AND state = 'draft'
RETURNING workflow_id, number, definition, state, published_at, created_at
