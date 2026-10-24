-- $1: the scheduler's advisory lock key
SELECT pg_try_advisory_lock($1) AS "locked!"
