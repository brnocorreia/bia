-- Takes the next Timeline position. The row lock on the Run orders concurrent appends,
-- e.g. from parallel branches.
-- $1: run id
UPDATE runs SET timeline_len = timeline_len + 1
WHERE id = $1
RETURNING timeline_len
