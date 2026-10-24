-- Workflows whose Schedules were not built from their current Active Version.
SELECT id
FROM workflows
WHERE active_version IS DISTINCT FROM scheduled_version
ORDER BY id
