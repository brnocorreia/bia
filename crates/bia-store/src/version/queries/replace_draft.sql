-- Replaces a Draft's Definition; updates no row for a Published or missing Version.
-- $1: workflow id, $2: version number, $3: Definition
UPDATE versions SET definition = $3
WHERE workflow_id = $1 AND number = $2 AND state = 'draft'
RETURNING workflow_id, number, definition, state, published_at, created_at
