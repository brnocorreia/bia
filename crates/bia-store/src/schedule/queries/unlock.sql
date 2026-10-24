-- $1: the scheduler's advisory lock key
SELECT pg_advisory_unlock($1) AS "unlocked!"
