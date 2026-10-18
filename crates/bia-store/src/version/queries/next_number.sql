-- Takes the next Version number. The row lock serialises concurrent saves, so numbers
-- have no gaps.
-- $1: workflow id
UPDATE workflows SET last_version = last_version + 1
WHERE id = $1
RETURNING last_version
