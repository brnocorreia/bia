-- Shares the Workflow row lock while $2 is its Active Version, so the Version cannot stop
-- being active until the surrounding transaction ends.
-- $1: workflow id, $2: version
SELECT 1 AS "one!"
FROM workflows
WHERE id = $1 AND active_version = $2
FOR SHARE
