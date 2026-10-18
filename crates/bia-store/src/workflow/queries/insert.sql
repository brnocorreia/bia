-- Creates a Workflow; returns no row when the id is taken.
-- $1: workflow id
INSERT INTO workflows (id) VALUES ($1)
ON CONFLICT (id) DO NOTHING
RETURNING id, active_version, created_at
