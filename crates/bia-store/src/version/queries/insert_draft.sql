-- $1: workflow id, $2: version number, $3: Definition
INSERT INTO versions (workflow_id, number, definition, state)
VALUES ($1, $2, $3, 'draft')
RETURNING workflow_id, number, definition, state, published_at, created_at
